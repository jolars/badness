# Architecture

Badness parses LaTeX into a lossless concrete syntax tree (CST) and puts a
formatter, a linter, and a language server on top of it. The design follows
[rust-analyzer](https://rust-analyzer.github.io/): a generic, error-tolerant,
hand-written parser produces a lossless tree, semantics live in a separate layer
above it, and recomputation is incremental via
[salsa](https://github.com/salsa-rs/salsa).
[arity](https://github.com/jolars/arity), the same kind of tool for R, was the
other influence.

This page is the whole design in one place. It is deliberately a tour rather
than a specification: where a decision has a long provenance of worked examples
and issue references, that detail lives in `.claude/rules/`, which is written
for contributors working inside a specific subsystem. If you want to build and
test the project, start with [Contributing](contributing.md).

## What it does

Badness turns source text into a syntax tree, and the tree into diagnostics and
formatted text. It does not typeset, it does not run TeX, and, outside the
language server, it does not look at the machine it runs on.

The pipeline is:

```
text → lexer → token stream → parser → event stream → tree_builder → GreenNode
```

The parser emits events (`Start`, `Tok(idx)`, `Finish`) instead of building a
tree directly. Tokens are referred to by index and diagnostics travel on a side
channel keyed by byte range, so there is no `Error` event. One extra event,
`SubTok`, attaches a `WORD` sub-slice for the math operator split. The tree
builder re-attaches trivia and feeds rowan's `GreenNodeBuilder`.

Everything downstream reads that tree. The formatter lowers it to a `Doc` IR and
prints the IR, the linter walks it once and collects diagnostics, and the
language server answers requests from salsa queries over it.

The tree is a pure function of the file's text. Config, the signature database,
and the filesystem take no part in producing it. Determinism, error tolerance,
and incremental recomputation all rest on that.

## The crates

Badness is a four-crate Cargo workspace on edition 2024. The root package is the
CLI, LSP, and linter crate `badness`; two publishable library crates and one
unpublished wasm shim live under `crates/`.

`badness-parser` holds the syntax layer (`syntax`, `ast`), the parser, the
semantic layer, the BibTeX parsing and semantic layers, the `data/` signature
artifacts, and the `build.rs` that bakes them into phf tables.

`badness-formatter` holds the layout engine (`core`, `ir`, `printer`, `style`,
`context`, `colspec`, `sentence`, `perturb`) and the `.bib` formatter. It
depends on `badness-parser`.

`badness-wasm` is a `publish = false` wasm-bindgen shim over the two library
crates. It powers the [playground](../playground/index.html) and is built with
`wasm-pack` through `task playground:wasm`.

Both library crates build for `wasm32-unknown-unknown`, so nothing in them may
touch the filesystem, threads, or processes. The formatter is embedded by the
[dprint plugin](https://github.com/jolars/dprint-plugin-badness), and a CI job
guards the target. The plugin is sandboxed with no filesystem, so it passes an
empty signature database where the CLI folds in signatures scanned from sibling
`.sty` and `.cls` files. That is the one sanctioned divergence from
`badness format`.

The root crate keeps `linter/`, `lsp/`, `project/`, `text/`, plus
`incremental.rs` (salsa), `config.rs`, `cli.rs`, `completion.rs`, and
`file_discovery.rs`. It re-exports the member crates at their old module paths
through shim modules, so `src/parser.rs` is one
`pub use badness_parser::parser::*;` line and callers keep writing
`crate::parser::…`. Two modules are real bridges rather than shims:
`src/formatter.rs` holds the `check` batch driver and the disk-backed
`format_file_with_packages` entries, and `src/semantic.rs` holds `load`.

## The BibTeX side

`.bib` files get their own pipeline in `bib/`, a sibling of `parser/` rather
than a mode of it. It is built on the same lossless rowan CST and the same flat
event stream, but has its own grammar, `SyntaxKind`, `BibLang` marker, lexer,
parser, tree builder, typed AST, formatter, linter, semantic layer, completion,
and outline. The invariants below apply to it unchanged.

### `%` comments in `.bib`

BibTeX's two readers disagree about `%`, so we had to pick one. Classic `bibtex`
(0.99d) has no comment syntax at all and rejects a `%` inside an entry;
**biber**'s reader (btparse) ends a comment at the newline and resumes parsing.
Badness follows biber, as the rest of the bib layer does (`bib_fields.json`
tracks biblatex's `blx-dm.def`) — verified by compiling both readings.

The context-dependence is the interesting part: `%` is a comment between a value
and the following `,`, but ordinary text inside a braced or quoted value
(`title = {50% off}` keeps the `%`). So the **lexer stays context-free** — `%`
is a bare `PERCENT` token wherever it appears — and the **grammar** decides,
wrapping `%` … end-of-line in a `COMMENT` node at exactly the positions where it
skips trivia inside an entry (before a field name, `=`, `#`, `,`, the closer). A
`BRACE_GROUP`, a `QUOTED` string, an `@comment` body, and top-level junk never
call that skip, so a `%` there stays an ordinary token. This mirrors the LaTeX
side's split, where brace *structure* is likewise the grammar's job, not the
lexer's.

texlab's bib parser models no comment at all, so this is a recorded deliberate
deviation in `bib_parse_compat_allowlist.toml`, not a gauge regression.

A `%` *inside* a value is where the two languages collide: BibTeX passes it
through as an ordinary character, and the LaTeX that finally typesets the value
reads it as a comment. So the value's line breaks are content, and value reflow
refuses any value carrying an unescaped `%` (guard 5 in `lower_value_reflowed`)
and emits it byte-exact. No CST oracle can catch this — joining two lines there
is byte-legal and typeset-wrong.

The formatter re-emits every comment: one that **shares a line** with the field
before it rides that field's line (the bib analog of the LaTeX rule that a
trailing comment is never relocated), and every other one binds **forward** to
the field it precedes (decision #9) and prints on its own line above it. Binding
to a *field* rather than an offset is what keeps a comment attached through the
canonical field sort. A comment past the last field prints above the closing
delimiter; a `@string`/`@preamble`/field-less entry carrying one has no line to
put it on, so that whole block is emitted verbatim rather than losing it. Both
rules read only comment own-line-ness, which the formatter preserves, so the
placement is a fixed point.

## Inputs and configuration

The CLI processes `.tex`, `.sty`, `.cls`, `.dtx`, `.ins`, and `.bib`.
Directories are walked with [`ignore`](https://docs.rs/ignore), honoring
`.gitignore` and `badness.toml` excludes.

The lexer's `LatexFlavor` picks the starting catcode regime. `Package` (`.sty`,
`.cls`, `.dtx`) begins with `@` already a letter, as if under `\makeatletter`;
`Document` does not. `.dtx` docstrip surface syntax is parsed.

Wrap mode is not a property of the file kind. Every kind defaults to
`WrapMode::Reflow`, and content that cannot be safely reflowed is refused
structurally in every mode; see [reflow
safety](#reflow-is-safe-by-construction).

`badness.toml` is found by walking ancestors from each input. The CLI is its
only consumer; the library API takes a resolved `FormatStyle`. Sections are
`[format]` (`line-width`, `indent-width`, `wrap`, `math-wrap`, `lang`,
`no-break-abbreviations`), `[lint]` (`select`, `ignore`), `[build]` (`aux-dir`),
and the [declaration](#declarations) maps `[environments.<name>]` and
`[commands.<name>]`. Excludes follow Ruff: `exclude` replaces the built-in
default, `extend-exclude` adds to it. `wrap` is an `Option` so the LSP can tell
"unset" from "set" when merging editor settings over project config, not because
the fallback depends on the file.

TEXMF discovery is deliberately not a section here. Where a TeX installation
lives is machine state rather than project data, so it arrives through editor
settings.

### Declarations

*Landing in stages (TODO.md § Declarations): `[environments.…]` is honored
everywhere a document is parsed — `format`, `--check`, `lint` (report and
`--fix`), `parse`, and the language server. `[commands.…]` is not implemented
yet.*

Two sections are different in kind from the rest, because they reach the
*parser*: `[environments.<name>]` and `[commands.<name>]` let a project name
constructs badness cannot see. A `\bea`/`\eea` delimiter pair, an environment
that behaves like `align`, a verbatim environment defined by machinery no scan
can follow — all of these are facts about the document that the text alone does
not carry (issue #109).

```toml
# \begin{myenv} … \end{myenv}, with no built-in counterpart
[environments.myenv]
like = "align"

# extra delimiter spellings for an environment badness already knows
[environments.eqnarray]
begin = ['\bea']
end = ['\eea']

# both: a declared environment reached only through commands
[environments.mytheorem]
like = "theorem"
begin = ['\startmyenv']
end = ['\endmyenv']
```

Literal strings (`'\bea'`) avoid TOML's escaping; a leading `\` is optional, and
a control-word name can never contain one, so there is nothing to disambiguate.

This is a deliberate widening of the database independence claimed under
[argument grouping](#argument-grouping-and-bracket-policy), and it is worth
being precise about what it does and does not admit. What reaches the parser is
a `ResolvedDeclarations` value: a closed, hand-authored vocabulary, seeded into
the existing `ParseCtx` for the *first* pass, so a declaring project pays no
extra parse and the second pass still asks only whether the file's own
definition scan found something new. That the parameter is a newtype rather than
a bare `SignatureDb` is deliberate — a value of that type can only have come
from a declaration block, so the entry point cannot be handed a document's
merged scope, and the database independence above is kept by construction rather
than by review. It lives in `badness-parser` so the dprint plugin and a future
`% badness-env` comment directive can feed the same type. In the editor it is
carried on a **singleton salsa input** at `Durability::HIGH`
(`incremental::DeclarationsInput`), read by `parsed_document` and by
`scope_signatures`, so editing `badness.toml` reparses the world and a keystroke
does not. One cell per database rather than one per file: a declaration block is
a property of the project, and threading it as a query parameter would push it
through every caller of `parsed_tree_root`. The language server resolves config
per anchor directory, so a session holding two workspaces with *different*
blocks rewrites the cell as the active document crosses between them — the
accepted cost, and the reason the main loop mirrors the last value it published
and writes only on a change (`GlobalState::analysis_settings`). Declaring
nothing, the overwhelming default, never writes it after construction. The
read-pool jobs that bypass the cache — a format or diagnostic that races an edit
and falls back to reparsing its captured buffer — take the declarations off the
snapshot (`Analysis::declarations`), so a fallback differs from the cached path
only by the package tiers it could not reach. The ambient `SignatureDb` still
never reaches the parser, and neither do package scopes or the CWL tier — the
thing the original invariant protects against is data that shifts as files are
scanned, loaded, or added, and a declaration block is none of those.

The safety property that makes config admissible at all is that **a declaration
names a spelling, never a pairing**. Every shape gate runs unchanged: a declared
`\bea` whose `\eea` is unreachable at the same brace, environment, and math
level demotes to a plain command exactly as an inferred one does. Config can
therefore widen what is *recognized*, and can never force a tree the text does
not support, which is what keeps a typo'd declaration a no-op rather than a
corruption.

`like` is the workhorse, and it means one thing: copy the curated built-in entry
of the same kind. It resolves against `builtin()` alone — never CWL, never
scanned definitions — for the same reason the alias arm of
`Signatures::environment_at` does, that a declaration supplies a spelling and
behavior always comes from curated data. An unknown target is a config error
rather than a silent no-op, because a mistyped `like = "algin"` is otherwise
invisible. It never crosses categories: the command-standing-in-for-a-delimiter
relation is genuinely cross-category and gets an explicit key on the environment
side (`begin`/`end`), rather than a tagged `like` that would turn one word into
a query language. Where `like` runs out — a construct resembling nothing built
in — arity is spelled in xparse argspec (`args = "o m m"`), reusing a standard
LaTeX users already know and a parser the semantic layer already has.
`ContentKind` has no argspec spelling, which is the known limit and the reason
`like` stays primary.

The shape generalizes by keying on **category, then name**. Everything a user
might plausibly declare is name-keyed in one of two maps (an environment's
behavior and delimiters; a command's arity, verbatim-ness, sectioning level, or
ref/cite family membership), with a third map keyed by character if the
shortverb case is ever taken. Two constraints keep that stable: a name map holds
only entries, never scalar knobs, since a category-wide switch would collide
with a construct of that name; and entries are keyed tables rather than an
`[[environments]]` array, since only keyed tables merge per name once config
layers or per-file overrides appear.

Resolution is where the rules are enforced, and it runs when the config loads,
so a broken declaration is an error rather than a block that parses fine and
does nothing to the document. It projects the entries into a `SignatureDb` — the
struct already holding exactly these three maps — so the declared tier folds
into a document's scope with the same merge the scanned tier uses. Rejected,
each naming the key the user wrote: an unknown `like` target; `begin` without
`end` or the mirror; delimiters for a verbatim or argument-taking environment;
delimiters for an environment whose behavior is unknown; a spelling two entries
both claim; and a spelling that could never lex as a single control word. An
entry declaring behavior alone carries none of those restrictions, which is what
makes `like = "lstlisting"` the way to name a verbatim environment no definition
scan can find.

Declared entries beat scanned definitions and the built-in tiers alike — a
declaration is the user explicitly correcting an inference. They are merged into
a document's signature scope last, as the top tier, and carry a provenance mark
while they are there. The mark exists for one lookup: the alias arm of
`Signatures::environment_at` resolves its target against *curated* data only, so
it has to tell a declared `myenv` (curated) from a scanned `\newenvironment` of
the same name (not curated, and still unable to lend an alias its behavior) —
and once both sit in one scope, nothing else distinguishes them.

## Two layers

The syntactic layer is the generic CST. It knows nothing about what a command
means.

The semantic layer is a signature database: a curated built-in table, a bulk
CWL-derived tier, and `\newcommand`/`\newenvironment` scanning. It assigns
arity, verbatim-ness, sectioning, and per-argument content kinds.

Meaning never leaks downward. The parser may read static lexical facts, never
signature data that package scopes, scanned definitions, or the CWL tier can
change. The one exception is explicit: a project may [declare](#declarations)
constructs the parser cannot see, and those declarations name spellings rather
than direct grouping.

One content kind is worth naming here, because it is the only place where a
signature claim can change typeset output. `ContentKind::Keyval` asserts that a
keyval-family processor strips spaces around entries, which is what licenses the
formatter to break a `[…]` at a comma the author glued. Compiling both spellings
shows the claim is real for `\usepackage`, `\includegraphics`, tikz, and
`lstlisting`, and false for every textual optional such as `\item`, `\caption`,
`\cite`, or a `\newcommand` default. It is held to the same curated standard as
the math-environment routing.

## The parser

The parser is hand-written recursive descent over a flat token stream. It treats
its input as generic TeX surface syntax and always produces a lossless tree.

Resolving macros and catcodes in full generality means running a TeX engine, and
we do not do that. Anything we cannot resolve statically degrades to a generic
node, with a diagnostic where one is useful, never to a crash or to corrupted
output.

### Sanctioned lexer modes

What we do handle is a bounded, growing set of patterns recognizable from static
shape alone. They are deliberately conservative: when in doubt, a construct
stays generic. The catalog:

- **Letter modes.** `\makeatletter` makes `@` a letter; `\ExplSyntaxOn` and the
  `\ProvidesExpl*` declarations open expl3, where `_` and `:` are letters. The
  two flags are independent and compose. In a `.dtx` a file-level signal (a
  `%<@@=…>` guard or a `\ProvidesExpl*` anywhere) puts every `macrocode` body
  under expl3 catcodes.
- **Verbatim.** `\verb`, verbatim-like environments, and verbatim-argument
  commands capture their body as a single token. Built-ins are curated;
  user-defined ones are found by a bounded two-pass definition scan that
  fingerprints catcode-othering signals and recognizes definer identities such
  as `\lstnewenvironment`.
- **Delimiter isolation.** The token after `\left` or `\right` is emitted on its
  own, so the parser can build the `LEFT_RIGHT` pair.
- **Math environments.** An environment the curated table flags `math` has its
  body parsed in math mode and wrapped in a `MATH` node, exactly as `\[…\]`.
  This is a grammar decision needing no lexer math state, and it reads the
  curated flag only, never the bulk or user tiers.
- **Definition bodies.** Inside the argument groups of the curated definer set
  (`\newcommand` and `\newenvironment` families, xparse, the LaTeX2e hooks),
  `\begin` and `\end` parse as plain commands, because TeX does not require them
  to balance within one group.
- **Macrocode chunks.** A frame-lexed `macrocode` body is macro code terminated
  only by the literal frame line, a line-oriented docstrip fact. Unmatched
  braces inside a chunk are plain tokens, since a `\def` regularly opens `{` in
  one chunk and closes it several chunks later.
- **Short verbs.** `\MakeShortVerb{\|}` toggles a character's short-verb
  catcode, so `|…|` on one line captures as an opaque `VERB`. Curated doc
  classes and `.dtx` mode enable `|` from the start.
- **Docstrip guards and `^^A` doc comments.** A line-leading `%<…>` lexes as a
  `GUARD` trivia leaf; on a doc-margin line the literal `^^A` comments to end of
  line, matching ltxdoc's catcode 14.
- **expl3 regions.** In-region, token lists pass `\begin` and `\end` around as
  data, so they parse as plain commands and an orphan `\]` is data with no
  diagnostic.
- **Char constants.** After a numeric-context primitive from a closed curated
  set, a backtick opens TeX's char-constant notation, so ``\char`$`` can never
  open math.
- **Signatures.** `\newcommand` and xparse signatures are extracted into the
  semantic database, never executed.
- **Environment aliases.** A command whose replacement body is exactly
  `\begin{X}` (or `\end{X}`) stands in for that delimiter, so `\bea … \eea`
  pairs as an `ENVIRONMENT` of `X`. See below.

Four shape gates round this out. A `$`, `\[`, or `\(` opens math only when a
matching closer is reachable before an unbalanced `}`, a paragraph break, or
EOF, because macro code passes the delimiters around as data at least as often
as prose uses them. Environment pairing is gated on brace structure rather than
a command set: an environment can never outlive the brace group its `\begin`
opened in, since braces are catcode structure while `\begin` and `\end` are only
macros. A conditional pairs only when its `\fi` is reachable, as below. And an
environment alias pairs only when its closer is positively located. All four
degrade to a plain token with no diagnostic, because parser diagnostics gate the
formatter and so must be high precision.

The `\begin` gate runs on the shared batch driver as `EnvGate`, and it is the
first *demotion* gate there, so its policy reads inverted: the located "closer"
is the escaping `}`, `Some` demotes the environment and `None` keeps it, and
running out of file is not an escape — that is what keeps the
unclosed-environment diagnostic firing on a forgotten `\end`. Two consequences
follow. A stray `}` closes rather than refutes, the same token event the
positive gates read as a refusal. And a math delimiter is not an anchor at all:
for a positive gate, declining behind one is the conservative direction, while
here it would *keep* an environment the scan cannot vouch for. The gate's two
per-opener pre-checks — the enclosing `group_depth`, and the `.dtx` doc-margin
exemption — are walk state rather than scan state, so they are applied per query
and never stored in a batch.

The two **math** gates (`DollarGate`, `DelimMathGate`) run on the same driver,
and for the uniformity rather than for speed: they are *single-entry*, opening
no nested entry, so a batch settles its seed and nothing else. That is not a
limitation but the shape of the problem — a delimiter whose closer is reachable
swallows every opener up to it, so there is never a same-frame neighbor left to
settle. Four policies invert with them. A `}` refuses unconditionally, where the
pairing gates refuse only when a group actually encloses the opener, because the
parse they guard bails at any unbalanced `}`. A foreign math delimiter is
ordinary content — for the `$` gate it *is* the closer. Environments count at
every brace depth, since a math body descends into a group and keeps parsing
environments there. And the closer needs no environment balance, since a
delimiter ends the body wherever it sits. The `$` gate is also the one gate that
runs *unmemoized*: a demoted `$$` re-enters on its second `$`, asking a
different question about the same token index under the same walk state, which a
slot keyed on the walk state alone would answer from the first query's verdict.

The `\left…\right` gate (`LeftRightGate`) is the last to join, and the only one
whose entries **stack** rather than count. Every other gate models its nesting
as two independent counters — how many nested openers and how many environments
stand between an entry and the token at hand — because that is all its
per-opener scan ever knew. A `\left` pairs by count wherever it sits, so its
scan reads one LIFO stack of `{`, `\begin`, and `\left` frames alike, and the
difference is visible: a frame **mismatch** (an `\end` or a `\right` that
reaches a frame of the wrong kind) is seen by every outer `\left` too, since the
innermost frame is common to all of them, so it refuses the whole scan rather
than one level of it — while the *absence* of frames that the blank-line anchor
tests is seen only by the innermost `\left`, so a nested pair **shields** the
ones around it from a paragraph break. Both readings are `Nesting::Interleaved`
in the driver.

Its math anchor inverts too. A conditional lives in text, so what defeats it is
math *starting*; a `\left` already lives inside a math body, so what defeats it
is that body *ending* — `$`, `\]`, `\)`, exactly the recovery anchors of the
`left_right` walk it guards, while a `\[` in the way is ordinary content. And it
is the gate whose opener and closer recognition **ignores `in_macro_code` on
purpose** where the driver's own `\begin`/`\end` counting does not:
`\left`/`\right` are catcode-neutral math structure that pairs by count no
matter what, and a `\def` body or a `macrocode` chunk is exactly where package
math like `$\left#2\right#4$` lives (issue #95). On the driver that is two
predicates in a policy; as a hand-written scan it was a comment nothing
enforced.

The **bracket family** closes the migration: three gates (`TextBracketGate`,
`MathBracketGate`, `MacrocodeBracketGate`) asking whether a `[`'s `]` is
reachable before the token that would make the `optional` walk bail, in text, in
math, and inside a `macrocode` chunk. Their nesting turned out to need no new
model. A per-opener bracket scan counts the `]`s *owed* to the command-abutting
`[`s it passes — such a `[` is itself argument-shaped and will claim the next
`]` when parsed, so that `]` cannot also satisfy the outer one (issue #55) — and
that claim countdown **is** the driver's nested-opener stack once an opener is
defined as a command-abutting `[`, since closer matching is LIFO either way.

What is new is that both of the family's anchors are depth-**blind**: a
`\begin`/`\end` refuses rather than counts (an optional never legitimately spans
an environment, so either half means a runaway `[`), and it and the paragraph
break fire at any brace depth. Both follow from the walk they guard: `optional`
bails wherever the cursor stands, and a gate stricter *or* looser than its parse
is a bug.

Two things are the in-math gate's own. A `$` there is read from the enclosing
math's *flavor*, which is walk state and so rides the batch's memo key: inside
`\[…\]` a `$` opens a genuine nested inline region, so a balanced `$…$` in the
bracket is **transparent** — the entries' own openers and closers stop counting
until the matching `$`, and everything else reads on — while inside `$…$` TeX
cannot nest one, so the first `$` at the bracket's own level is that math's
closer and refuses. And the gate is stricter than the `optional` bail in two
preserved respects: its `\begin`/`\end` anchor carries no `in_macro_code`
filter, and a chunk-unmatched brace is group structure to it rather than a plain
token. Both only ever decline to attach. The second is arguably the *faithful*
reading — `optional` itself bails at any `R_BRACE` without consulting
`plain_braces`, so its two siblings, which do consult it, are the loose ones —
but unifying either way moves verdicts and is its own commit.

The `macrocode` gate keeps one divergence of its own: it is the one bracket gate
the batch cannot make linear, single-entry by policy, so a chunk of `\cmd[`
openers whose only `]` sits past the frame still scans to the frame per opener.

Its *other* divergence became the driver's rule for every gate. A docstrip guard
line **breaks** the paragraph run rather than floating through it: docstrip
deletes a guard-only line outright when it strips the file, so `%<*dtx>` between
two lines does not part them (issue #71) — the guard breaks the newline run
without being a newline, which is exactly
`TriviaScan::saw_blank_line_outside_guards`. A `.dtx` doc margin still floats,
so a margin-only line is still the blank line of the documentation layer. Only
the `macrocode` gate read guards that way at first, because only its pre-batch
scan happened to skip whitespace alone; the other seven inherited the float from
the driver's trivia arm and were the ones diverging from the considered model.
`rotating.dtx` pinned the reading (the date optional of its `\ProvidesPackage`
runs over three guard lines inside one chunk), and unifying paid immediately in
the other direction: `trace.dtx`'s second `% \iffalse … % \fi` header spans four
guard lines, so the float made its `\iffalse` a plain command and the formatter
reflowed the guards into prose — collapsing `%<driver>` off column 0, a
non-trivia content change the two-sided corpus ratchet had recorded as a known
failure.

### Environment aliases

`\newcommand{\bea}{\begin{eqnarray}}` plus `\newcommand{\eea}{\end{eqnarray}}`
makes `\bea … \eea` a spelling of `\begin{eqnarray} … \end{eqnarray}`, and
badness learns it by scanning the file's own definitions (issue #109). This is
the *second pass* that already exists for user-defined verbatim commands. An
alias defined in a sibling `.sty` deliberately does **not** pair — package scope
reaches the formatter, never the parse — so inference stays a pure function of
that file's text. The complex cases inference cannot reach are covered by an
explicit [declaration](#declarations) instead, which lands in the same
`ParseCtx` maps and is indistinguishable downstream; the rest of this section
describes the *inferred* path.

Admission is narrow, because a wrong pairing rewrites layout. The target must be
a **curated built-in** environment, so an alias declares a *spelling* and never
a *semantic* — every behavior flag still comes from curated data, exactly as
`is_math_environment` requires. It must be **non-verbatim**, since
`\newcommand{\bv}{\begin{verbatim}}` does not work in TeX at all (the body is
tokenized before the macro expands). It must take **no arguments**, and so must
the alias, since the head consumes none and attaching them from the target's
signature would be arity-directed grouping from scanned data. And **both
halves** must be defined in the file, since a lone opener can never pair anyway.

Two details carry most of the risk. First, the opener index must exclude every
*name being bound*, which is a **slot countdown** and not a test of the single
word after the keyword (`lexer::definition_name_slots`). `command()` sets
`in_def_body` after a `\def` head only when the definee is a control symbol, so
in `\def\bea{\begin{eqnarray}}` the definee reaches the dispatch as an ordinary
command at brace depth 0 — and unfiltered, the two *definition lines* pair with
each other. `\let\oldbea\bea` is the same failure one slot over: `\let` binds
two names, and left live the *source* operand pairs with the next stray `\eea`
and swallows the prose between them. (The conditional recognizer subtracts the
same `("let", 2)` slots for the same reason.) The braced `\newcommand{\bea}{…}`
form is covered by `in_def_body` instead. Second, the gate is **positive**,
modelled on `conditional_closer` rather than on the `\begin` gate: an alias
opener has no `{name}` corroborating it and no unclosed-environment diagnostic
worth preserving, so it must be refused unless its closer is located, and the
walk is then bounded by that index. Unlike the conditional gate there is
deliberately no paragraph-break anchor — an `itemize` alias legitimately spans
blank lines, and reading one would key layout on a trivia predicate the
formatter does not preserve.

The scan is bounded twice over, because it is otherwise quadratic in a shape
real packages have: a `.sty` that defines `\bc`/`\ec` for its users and calls
`\bc` from macro bodies has openers that never pair, and each walks to EOF.
Every `Some` verdict names an index in the closer index, so the walk stops at
the *last* closer in the file (none at all, and the gate refuses outright), and
the verdict is memoized for the one opener the caller asks about twice —
`starts_block_env` peeks before `element` dispatches.

The gate runs on the shared batch driver (`AliasGate`), the conditional gate's
second client, so the residual adversarial shape — thousands of openers with a
single closer at EOF, where the last-closer bound spans the whole file and cuts
nothing — is one linear pass rather than a scan per opener. Its two policy
divergences from the conditional gate are the missing paragraph anchor above and
the name match on the closer: nesting counts *any* alias opener and *any* alias
closer, so `\bea \bce \ece \eea` pairs while the crossing `\bea \bce \eea \ece`
refuses outright instead of letting an inner walk run past the outer bound.

The node is the ordinary `ENVIRONMENT > BEGIN … END`, with the delimiters
holding a bare `CONTROL_WORD` instead of `\begin` plus a `NAME_GROUP`, so every
consumer downstream works unchanged. `ast::Begin::name` falls back to that
control word. `name_range()` stays `None`, which is what makes the
name-rewriting consumers (rename, change-environment, the `obsolete-environment`
fix) decline cleanly rather than emit a half-edit.

Behavior is resolved **from the node, never from the name**:
`Signatures::environment_at` reads the alias map only for a delimiter
`Begin::is_alias` recognizes, and the plain name-keyed `Signatures::environment`
never reads it at all. The distinction is the whole point of keeping aliases in
a side map rather than cloning an `EnvironmentSig` under the alias name — a
literal `\begin{bea}` written in a file that also defines `\bea` is an unrelated
environment that happens to spell the same word, and it must stay unknown rather
than inherit `eqnarray`'s math and alignment. By the same token a
`\newenvironment{bea}` and an alias `\bea` coexist, each node resolving to its
own.

Accepted false negatives: `\let` chains, aliases used inside math (`math_atom`
pairs environments ungated, so an alias arm there would be strictly worse), and
argument-taking aliases.

### The conditional gate

`\if…\else…\or…\fi` becomes a `CONDITIONAL` holding a run of
`CONDITIONAL_BRANCH`es with the `\fi` as its last child, mirroring
`ENVIRONMENT > BEGIN … END`. The first branch carries the opener, its test, and
the then-body; every later one opens with its own divider, so a consumer finds
the boundaries positionally and never by matching the name `\else`.

The `\if` *test*'s extent is not statically resolvable — `\ifnum\radius>5` scans
⟨number⟩⟨rel⟩⟨number⟩ by TeX's own scanner, `\ifx` takes two tokens, a
`\newif`-defined `\if@foo` takes none — so there is deliberately no head node,
and with it no body indent. What the node buys is the construct's *extent*,
which is what lets the formatter lay it out all-or-nothing.

Recognition is pair-and-trust over the lowercase `if` prefix, minus two curated
families measured over the gate corpora: the brace-argument `if*` macros
(`\ifthenelse`, `\iftoggle`, the etoolbox test family) and the operand slots of
`\if`/`\ifx`/`\ifcat`/`\ifdefined`/`\newif`/`\let`, where an `\ifX` is a token
being declared or compared rather than live control flow. Subtracting the first
is load-bearing rather than cosmetic: shape alone does not merely fail on an
`\ifnumgreater`, it *mis-pairs*, stealing an enclosing conditional's `\fi`. The
name sets live in `parser::conditional`, along with the small state machine that
turns them into a positional verdict (the operand countdown, the `\ifcsname`
body), and the linter's `ConditionalIndex` drives the same one. So branch paths
and CST nodes can never disagree about what an opener *is*. They can still reach
different verdicts on a token, because each consumer decides for itself which
stream to interpret: the parser visits every token and then drops the openers
inside an expl3 region, while the linter withholds a `\def` body's span
altogether — a `\let` that a definition merely carries must not arm the operand
countdown for the code after it, whereas the parser needs no such rule because
its brace anchor already refuses to pair across the body's group.

The gate itself demands a reachable `\fi`, and demands it at the opener's own
level of **every nesting the parse recognizes** — braces, environments, and math
alike. This is the subtle part. A token scan that counts a `\fi` the recursive
walk will consume inside some other construct promises a pairing the walk cannot
honor, and the walk then runs on looking for a closer that is gone.
`ltboxes.dtx` is the case that taught this: its
`\else\@pboxswtrue $\vcenter \fi\fi\fi … \if@pboxsw \m@th$\fi` puts all three
`\fi`s inside a `$…$`, and a brace-only gate carried the construct over 160
lines and every `macrocode` chunk in between, stranding the cursor past the
chunk terminator for every chunk-bounded scan downstream. So math anchors the
scan, a `macrocode` frame is a hard boundary in both directions, and the walk is
additionally bounded by the closer index the gate located — belt and braces,
since the scan reads tokens while the walk reads structure.

That bound is deliberately one-directional, and it is worth being exact about
which direction. The walk can never run *past* the located `\fi`; it can still
stop *before* it, because the scan counts nested openers by name while the walk
re-gates each one and may demote it — and a demoted opener's `\fi` is then a
closer the walk reaches first. `\ifA \begin{center} \ifB \end{center} \fi \fi`
is the shape: the scan counts `\ifB` and picks the second `\fi`, the walk
demotes `\ifB` and closes at the first, and the leftover `\fi` is a plain
command. The tree is still well formed and still lossless, which is the bar; but
it is why `ast::Conditional::closer` is fallible and why no consumer may assume
the scan's index and the walk's agree.

The cost is one forward scan per *batch*, not per opener. The scan is bounded by
the last `\fi`-flavored word in the file (a file with none refuses without
scanning), and one scan settles every opener it passes in the seed's own brace
frame: nested openers are only counted at brace depth zero, so they share the
seed's frame exactly and `\fi` matching is pure LIFO over a pending stack. The
batch is memoized against the walk state the scan read (`macrocode_end`,
`in_def_body`, whether a group encloses), so a run of top-level openers costs
one linear pass where it used to cost one scan each — the quadratic
thousands-of-openers shape recorded here before the batch now measures in the
tens of milliseconds. One rule in the batch is load-bearing: a refuted entry is
settled, never removed, because the per-opener scan counts nested openers by
name and never un-counts one — a later `\fi` must still be consumed by the
refuted entry's slot, or the outer opener would pair where the per-opener scan
demoted it. Every ordinary anchor still cuts a scan short, so conditional-heavy
real packages (`biblatex.sty`, `latexrelease.sty`, `memoir.cls`) measure the
same as they did before the node existed.

The batch is not the conditional gate's own machinery. It is a **driver**
(`Parser::gate_batch`) that the other shape gates migrate onto one at a time
(`TODO.md`, container stack C2): the driver owns the bookkeeping they all share
— the bound, brace depth under `plain_braces`, environment counting, the
`macrocode` frame, the entry stack with its settled-never-removed rule, the scan
metering, and the walk-state memo — while each gate supplies a `GatePolicy`
naming its own bound, its openers and closers, and whether a blank line anchors
it. The divergences between gates are deliberate, so they stay visible as policy
methods rather than being averaged into the loop. With the bracket family
migrated the driver serves all nine gates, and the policy surface is what the
migration bought: every place two gates read the same token differently is a
named axis with a documented reason, where before it was nine hand
transcriptions in which a fix to one copy did not propagate (issue #95).

Two anchors differ from the environment gate on purpose. Running out of file
demotes here, where the environment gate keeps the node so it can still report
an unclosed environment; a conditional has no diagnostic to preserve. And there
is no `.dtx` doc-margin exemption: that exists so the documentation layer keeps
pairing `\begin{macro}` across the chunks between them, and a conditional has no
such split-across-chunks story. A paragraph break anchors at the construct's own
level, which keeps `CONDITIONAL` a within-paragraph construct — it can never
straddle a `PARAGRAPH` boundary, so no paragraph nests inside one. Conditionals
are not recognized inside expl3 regions, where the formatter owns layout through
the expl3 statement segmentation, nor (yet) in math mode.

### Recursive descent, with Pratt local to math

Hand-written recursive descent is the spine. Precedence climbing is used only
for sub- and superscript binding and for `\left…\right` matching; the text-level
parser has no precedence.

Arithmetic operators are catcode-12 "other" characters, so a faithful lexer
globs them into `WORD` runs and `a+2*1` is one token. Operator-ness is a
math-semantic fact assigned after catcode lexing, which makes it the parser's
job: inside math a `WORD` is split at operator boundaries into flat sibling
atoms, by byte range rather than by re-lexing. Only the trailing operand is the
scriptable base, so `a+2*1^5` binds `^5` to `1`, matching TeX. Operators become
atoms so the formatter can space them and the display breaker can break long
chains. There is no arithmetic-precedence expression tree.

### Argument grouping and bracket policy

The CST greedily attaches trailing `{…}` and `[…]` groups as argument nodes,
texlab-style. Arity is unknown at parse time; the semantic layer refines it.

The load-bearing claim is database independence. Attachment reads the input text
plus compiled-in data, never mutable signature inputs such as package scopes,
scanned definitions, or the CWL tier. Consulting the signature database during
grouping would make the tree a function of something other than the text, and
every signature edit would invalidate every parse. For generic LaTeX that forces
greed: `\foo{a}{b}` is either a two-argument call or a zero-argument command
followed by two groups, and nothing in the text says which.

Project [declarations](#declarations) are the one sanctioned input that is not
the text. They are admissible precisely because they do not touch this: a
declaration names a construct — a delimiter spelling, an environment's behavior
— and never directs attachment, which stays greedy and generic.

Attachment is therefore text-pure, but not uniform. Deviations read static facts
only. Brackets are shape-gated, since `[` and `]` are not real grouping in TeX:
a bracket attaches only when it reads as an argument, which in math means
directly abutting the command with its `]` reachable before the math ends, and
in text mirrors the `$` gate. A lone `*` tight to a command and followed by an
argument folds in as a starred-variant marker instead of breaking the run.

expl3 is the one systematic counterexample. The argspec suffix rides in the
`CONTROL_WORD` token itself, since in-region `:` and `_` are letters, so
arity-directed attachment there would be exactly as text-pure as greed. Greed is
not neutral in that dialect, it is a systematically wrong guess: every
single-token slot breaks the run, so `\tl_set:Nn \l_a {x}` attaches `{x}` to the
definee, and the formatter's peel-back queue exists only to undo that after the
fact. Arity-directed expl3 attachment is the recorded candidate deviation,
deliberately unimplemented until three questions have answers: the mixed-shape
CST every consumer would have to handle, a false-positive blast radius that
moves from layout into the tree, and the divergence ledger against texlab.

### Trivia attachment

Comments bind forward, whitespace floats, and a blank line breaks the bind.
Trivia is never dropped, so the only question is which node owns it.

By default trivia floats at the nearest enclosing node. A contiguous run of
own-line `%` comments immediately preceding a `COMMAND` or `ENVIRONMENT` binds
leading into it as a `DOC_COMMENT` node, with "documentable" decided on node
kind alone so no signature lookup leaks into the parser. A same-line trailing
comment never binds.

This diverges from rust-analyzer's `n_attached_trivias`, which peeks past a
blank line when the next comment is an outer doc comment. That peek keys on the
`///` versus `//` distinction, and LaTeX's single catcode-14 `%` has no
equivalent, so we bind only the maximal blank-line-free suffix. Otherwise a
license header would glue into the following command's doc comment.

### Error recovery

A single syntactic error never fails the whole parse; errors travel alongside
the tree. The recovery anchors are `\end{…}`, `\begin`, a blank line, `}`, `$`,
`&`, and `\\`. The parser always makes progress and never loops on unexpected
input.

### Incrementality

Incrementality is salsa-first. Cross-file and cross-query incrementality is the
v1 story; intra-file reparse that reuses green subtrees is a later optimization,
since a whole-file reparse of a typical `.tex` is sub-millisecond.

Green nodes are stored in salsa, never red ones, because red trees are not
`Send`, `Eq`, or `salsa::Update`. `incremental.rs` stores `rowan::GreenNode`
under `no_eq, unsafe(non_update_types)`, sound because the tree is a pure
function of the text, and materializes red cursors on demand.

Salsa's default input durability is `LOW`. `SourceFile.path` is built at
`Durability::HIGH` because it is set once and never mutated; `text` keeps `LOW`,
since a keystroke rewrites it. The project's [declarations](#declarations) are
the first genuinely config-shaped input, and are likewise built *and written* at
`HIGH`. Any future input promoted from config or package metadata must be
constructed at `HIGH` or `MEDIUM`, or every keystroke's global revision bump
will invalidate it.

### Typed AST wrappers

On top of the untyped rowan CST sits a thin typed layer: rust-analyzer-style
`AstNode` and `AstToken` traits, an identity macro, and one wrapper struct per
node kind. Wrappers are a read-only view, never a re-model of the tree. They
expose structure and never meaning, so no signature lookup lives here, and
because the CST is greedy and generic the accessors are positional and tolerate
over-attachment by construction. `Command::title()` would be a lie, since a
`\section` and a `\newcommand` share the `COMMAND` shape.

The formatter deliberately stays raw for structural work, where the `lower_node`
dispatch and the token-classification loops are ordinary tree walking that
wrappers would only obscure. It adopts wrappers for field access alone.

## The formatter

The formatter is the sole authority on layout. It lowers the CST into a
Wadler/Prettier-style `Doc` IR, and a printer lays that IR out under a
flat-or-break fit model.

### It is whitespace-only

The layout engine changes trivia and nothing else: whitespace, newlines,
comments, and `.dtx` margins and guards. It never inserts, deletes, or rewrites
a non-trivia token. Mechanically, each maximal run of whitespace and newline
trivia is replaced by a break primitive, and the printer computes indentation.

Meaning-preserving content rewrites therefore do not live here. Stripping
redundant braces around a single-token script (`x^{2}` → `x^2`) and rewriting
`$$…$$` → `\[…\]` are linter autofixes. This mirrors the fix-then-format rule:
just as the formatter never runs inside `--fix`, content rewrites never run
inside `format`. The payoff is a guarantee by construction, checked by the
non-trivia-content oracle, instead of a meaning-preservation argument defended
one fixture at a time.

The formatter may still change CST *shape*. The math operator split re-groups a
catcode-12 `WORD`, so inserting insignificant math whitespace makes the output
re-lex into separate atoms. The oracle compares the concatenated text of
non-trivia tokens rather than their boundaries, so it tolerates the re-grouping
while still catching any inserted or deleted non-trivia character.

### Trivia-invariant layout

Whitespace-only says what the formatter may write. Trivia-invariant layout says
what the lowering may read:

> Layout is a function of non-trivia content, config, and only those trivia
> predicates the formatter itself preserves.

A predicate `P` is preserved when `P(fmt(x)) == P(x)`. Reading a preserved
predicate is safe, because the formatter cannot change the answer; reading an
unpreserved one means pass 1's layout silently edits pass 2's input.

Three predicates are preserved and may be read: whether a blank line is present,
whether a comment is present and whether it is own-line or trailing, and whether
a `%` margin or `%<…>` guard sits at column 0. One is not, and must never be
read: whether a gap is a lone newline or a space. The formatter converts freely
in both directions, turning `alpha\nbeta` into `alpha beta` and writing a
newline where a width wrap needs one.

This makes idempotence a theorem rather than an empirical property. Since the
formatter changes only trivia, `fmt(x)` is by construction a trivia-perturbation
of `x`, so layout invariant under trivia perturbation gives
`fmt(fmt(x)) == fmt(x)` for free. The alternative does not scale: every layout
decision that reads the unsafe predicate is an independent latent bug, and the
supply of decisions is unbounded. The whole K&R-versus-Allman family of bugs is
that one pattern, where a soft width break becomes a hard statement boundary on
the reparse and the layout flips with it.

The intended enforcement is to delete the information at the boundary: the
lowering would consume a normalized inter-token gap with no `Newline` variant
rather than raw trivia tokens, so a rule could not key on what it cannot see.
Rules that legitimately preserve authored breaks — the modes *defined* by them
(`WrapMode::Stable`, `Sentence`, `Semantic`, and `ReflowKind::Statement`), the
expl3 fallback statement, the command-only-line rule's residue, and the
delimited-group block residue on `spans_multiple_lines` — would take a widened
gap, and each owes a written fixed-point argument showing that every layout it
can emit re-reads to itself.

The command-only-line residue is the newest of those, and the only one living
inside the default `Reflow`. Curated block commands carry a positive
`CommandSig::block` property and are laid out as block-level statements without
consulting trivia, so what the rule still decides is the authored break around a
command whose block-ness no signature tier can know — an un-signatured or
scanned-definition `\mymacro` on its own line — plus block commands glued to
adjacent content. Retiring that would glue every such authored line into the
paragraph fill: a policy change, not a fix. So the residue is sanctioned as Tier
2 on the argument written at `line_is_command_only`: the rule is
preservation-only, hardening gaps that already hold a newline and never writing
or moving a break, so a kept break re-reads to itself in place, and a fill break
it hardens on the next pass — a width wrap that stranded a command alone on a
printed line — coincides with the break the first-fit fill chose, which refills
identically around a hard stop. The cost is by design: `--checks trivia-strict`
still reports these shapes, because preserving the authored break *is* the
information the rule reads. One scope limit keeps the residue honest: it does
not fire inside a signature-proven prose *argument* body
(`ReflowKind::ProseArg`), where width alone owns the layout — preserving a
command-only line there mints a forced break only pass 2 can see, and that bit
leaks upward through every `contains_forced_break` reader, flipping the
enclosing group between its inline and block forms across passes.

The last Tier-1 reader — the `Opaque`-group `spans_multiple_lines` choice, with
`lower_optional`'s fallbacks to the same — is retired. Under `Reflow` a brace
group is width-driven (`lower_opaque_group`): flat when it fits, byte-identical
to the generic inline path except that a lone-newline run renders as one space,
and first-fit wrapped at its authored gaps otherwise. Break opportunities are
exactly the perturbation-eligible gaps, so strict invariance holds by
construction; a glued junction never gains a break, and delimiter padding rides
the flat rendering and vanishes broken — exchanged for the delimiter's own
newline, never deleted, since an opaque argument's space tokens are typeset. An
edge gap joins that vanish-when-broken protocol only when its flat spelling is a
single space, the one spelling a break reproduces; any other spelling rides
verbatim and never breaks. An *interior* blank line, a direct comment, a token
embedding a newline, or a child carrying a forced break sends the group to the
indented block form instead — preserved predicates and content only. An *edge*
blank does not: the block form trims edge blanks away, so declining on one would
key on a predicate the emitter destroys, and it erases to padding instead,
matching the deletion the block form already performed. The optional-argument
lowering makes the mirrored promise: a `segment_delimited_body` decline takes
the block form unconditionally, and a dropped trailing separator re-emits the
authored whitespace it replaced. What remains of `spans_multiple_lines` is the
delimited-group residue behind the non-`Reflow` modes and the doc-margined
corner, sanctioned Tier 2 on the fixed-point argument written at the predicate:
the block form always ends with a newline before its closer, so its output
re-reads multi-line and re-blocks byte-stably, and the inline path emits no
newline, so single-line re-reads single-line.

The rule is enforced at the boundary rather than by review. Every trivia run the
lowering consumes arrives as a normalized `Gap`
(`Glued | Space { flat } | Blank | Comment`) with **no `Newline` variant**:
inline whitespace and a lone newline are one variant, because a rule cannot key
on what it cannot see. `Gap::flat` is what a one-line rendering writes there — a
single space wherever the run held a newline, blank line included, since that is
the only spelling a break reproduces, and otherwise the authored whitespace
verbatim. So a lone newline and a single authored space are indistinguishable,
while a wider run (`\pgfpoint@oncoil{0    }`) still rides verbatim; that is not
a leak, because every reader of `flat` emits it unchanged and so preserves it.
`Gap::separator` is the split-point rendering the two former prototypes agreed
on — an `Ir::Line` at a gap, an `Ir::SoftLine` at a glued junction — and both
(the conditional divider's `DividerGap`, the `[…]` split point's `KeyBreak`) are
folded into the one vocabulary.

The Tier-2 sites take a `WideGap`, which carries the newline count alongside the
normalized gap: the byte-faithful stream (`classify_trivia`), the
preserve-shaped modes (`lower_prose_stream`, `MathWrap::Preserve`), and the two
reflow drivers (`ReflowKind::Statement`, the expl3 fallback statement, the
command-only-line residue, which reach it through `consume_widened_gap_slice`).
Their names are the warning, and each still owes the written fixed-point
argument; the preservation-only ones have the easy version — a hard line prints
a newline, which re-reads as a newline and is emitted as a hard line again, and
nothing there ever *converts* between the two spellings, which is what a Tier-1
read would do. Everything width-driven takes `consume_gap` and the narrow `Gap`,
so it is not merely disciplined out of the unsafe predicate but structurally
unable to reach it.

The oracle is `formatter::perturb`, which generates TeX-identical trivia
perturbations of each input. It has two forms. `check_trivia_convergence` is
what gates: every variant must format to a fixed point that parses cleanly,
round-trips losslessly, and carries the same non-trivia content — strictly
stronger than idempotence, which only ever exercises the single trivia
configuration `fmt` itself produces. `check_trivia_invariance` is the strict
end-state contract, `fmt(perturbed) == fmt(original)`; it cannot gate a corpus
until the unsafe predicate is unreadable, but it is the only mechanical way to
*find* a decision that reads it, since such a decision is self-consistent on
both spellings and so invisible to convergence and idempotence alike. Its
surveying form is `badness debug format --checks trivia-strict`.

### Paragraph line breaks

Paragraph line breaks are controlled by `WrapMode`, modeled on the sibling
[panache](https://github.com/jolars/panache) formatter and mechanized through
the `Doc` IR rather than a separate line filler. All five modes are implemented.
`Reflow`, the default, width-fills. `Stable` keeps acceptable authored breaks
while optimizing overflow, change, displacement, and raggedness against a soft
target. `Preserve` keeps authored breaks. `Sentence` and `Semantic` split one
sentence per line and ignore width, with `Semantic` additionally ending a line
at every authored newline.

Sentence-boundary detection is a per-language abbreviation profile ported from
panache, resolved from `[format] lang` and `[format.no-break-abbreviations]`.

Display math has its own knob, `MathWrap`, scoped to single-formula display
bodies. Its default resolves against the effective `WrapMode`, so one `wrap`
setting carries over to math for free.

### Statement bodies

Not every environment body is prose. A TikZ or pgfplots picture holds a sequence
of `;`-terminated path statements, and a greedy prose fill actively damages it:
it runs `\draw (0,0) -- (1,1);` and `\node at (0,0) {A};` onto one line, and at
a narrow enough width it splits a `\foreach` header away from its loop variables
(issue #114).

The curated `statementBody` flag in `data/signatures.json` names that family —
`tikzpicture`, `pgfpicture`, `scope`, `pgfonlayer`, and the pgfplots axis
environments — and a paragraph inside one is lowered under
`ReflowKind::Statement` instead of `ReflowKind::Prose`: each authored line stays
its own logical line, and only an over-long one wraps. That is the same posture
a code-like brace-group body (a `\newcommand` definition) already takes, and it
inherits that arm's Tier-2 fixed-point argument unchanged — the wrap continues
flush, so a wrapped tail re-reads on the next pass as a line already at the body
indent.

Three things keep the flag narrow. It is **curated only**: a statement
terminator is package grammar, not a TeX-surface fact, so neither the CWL
codegen nor the runtime definition scan can set it. It is **distinct from
`code`**, which is the `.dtx` documentation layer's `macrocode` — a fact about
re-lexing under the package regime, not about layout; conflating the two would
hand a future `.dtx` consumer a `tikzpicture`. And it is read from the
**nearest** environment ancestor, never from any of them, so an `itemize` or a
`tabular` inside a `\node`'s label still reflows as the prose it is.

What this deliberately does *not* do is indent a statement's continuation, or
the body of a `\foreach`. That needs a node owning the whole statement, and a
TikZ path statement (`at`, `(coord)`, `;`, `{label}`) is package grammar the
generic parser does not model — see TODO.md, *Hanging continuation indent for
wrapped statements*.

The same picture family is curated a second time in the linter
(`linter::rules::is_pgf_picture_environment`), which keeps `dash-length` off
coordinate arithmetic. Merging the two waits on a signature DB reaching
`RuleContext`.

### Reflow is safe by construction

`WrapMode` used to be resolved per file extension, with `.tex` reflowing while
`.sty`, `.cls`, and `.dtx` fell back to `Preserve`. That default is gone.
Whether content is safe to reflow is a property of the content, not of the file
name, and answering it by extension left `--wrap reflow` on a `.dtx` free to
corrupt the document.

The safety is now structural, and every gate is independent of the wrap mode, so
an explicit `--wrap reflow` is exactly as safe as any other mode. Every relayout
arm refuses a node whose subtree carries a `.dtx` margin or guard, because
reflowing one drops the `%` margin and on an unmargined line a `^^A` doc comment
re-lexes as content. A residual margin-escape detector backs that up: when a
probe-gated reflow would commit content outside the margin, the paragraph
re-lowers on the byte-faithful preserve path. Never re-introduce a file-kind
wrap default to paper over a layout bug; fix the gate.

### Optional arguments, tables, and math spacing

An optional argument is a plain Wadler group over its top-level comma-separated
entries: flat when it fits the width, one entry per line when it does not. Width
alone decides. There is deliberately no "expand once the list has more than N
keys" rule and no Black-style magic trailing comma, since content steering
layout conflicts with the sole-authority tenet.

Which commas are break opportunities is the subtle part. A comma the author
already followed by whitespace is free, since flat-to-broken is just a
space-to-newline exchange. A comma glued inside a `WORD` is not: breaking there
materializes a space token TeX will see, so it is emitted only for an argument
the signature database proves is a key-value list.

That proof, not the delimiter, is what selects the segmented layout, so it
extends to a *mandatory* group as well. The keyval-family setters — `\pgfkeys`,
`\tikzset`, `\lstset`, `\setlist` — carry the whole key list in `{…}`, and
without the routing that body fell to the prose reflow, which word-wrapped it
mid-key. It now takes the same shape as the bracket: flat when it fits, one
entry per line when it does not, nested commas sealed inside their child group.
A mandatory group is the ordinary home of typeset text, though, so it reaches
this only through the hand-curated signature tier. The bulk CWL tier still drops
a `%keyvals` mark on a `{…}`: the mark is mechanical rather than validated, and
a wrong claim costs more on a mandatory group than on a bracket.

Table column alignment is layout, so the formatter owns it. The `{lcr}` column
spec is parsed from static argument text only, conservatively bailing to
all-left on anything it does not model, and the grid renderer pads cells left,
center, or right. Routing to the grid is primarily semantic, through the curated
`align` flag, but one arm additionally routes any remaining environment whose
body carries a top-level `&`, since a `&` at catcode 4 is a column tab and the
signature database cannot name a user-defined alignment.

Math operator spacing is a single space around each binary and relation atom,
with unary signs and scripts tight.

### Conditionals

A conditional the parser paired lays out all-or-nothing: flat when the whole
construct fits, and with every divider opening a line when it does not. That is
the only coherent form available, and the reason the construct needed a node at
all. A per-divider rule at the layout layer has no good version of itself. Fired
only across a gap the author already wrote, it *is* the lone-newline read. Fired
unconditionally, it manufactures a space token at the roughly one boundary in
four that authors glue, which TeX contributes to the horizontal list. Fired only
where the author already broke, it breaks one divider and leaves its sibling
glued, decided by nothing but where the author happened to type.

The two forms are handed to the printer as whole candidates rather than as a
single Wadler group of soft lines, and that distinction is load-bearing. A group
saturates its break state from whatever forced breaks its subtree carries, and a
branch *interior* carries one for every physical line the command-only-line rule
keeps — so a group would end up deciding the dividers from the interior's
authored newlines, which is exactly the predicate that must not decide them. The
flat candidate is instead collapsed from content alone, so its width, and with
it the choice between the two, is a function of non-trivia content and the
config. When no flat candidate exists at all — a `%` comment in a branch, a
nested environment — the broken form is unconditional, and both of those are
content facts that layout may read.

One carve-out keeps the rule typeset-safe. A separator renders as a space when
flat and a newline when broken, and TeX makes a space token of either, so
breaking at a divider the author glued (`\ifmmode y\else z\fi`) changes what TeX
sets — a change no CST oracle can see, since whitespace is trivia to them and
content to TeX. A construct with any glued divider therefore keeps its authored
bytes rather than relayout: breaking only the unglued siblings would be the
lopsided form again.

The whole relayout is confined to the modes that lay prose out at all.
`WrapMode::Preserve` promises authored line breaks are untouched, and rejoining
a conditional the author spread over lines is exactly what that forbids, so
there the construct takes the byte-faithful stream. The other three rebuild
every prose line from runs already, so the choice is theirs to make.

A branch *interior* is lowered the way the construct's enclosing context would
lower the same elements, which is what "as anywhere else" has to mean here. It
cannot be read off the branch: the gate keeps a conditional inside one
paragraph, so no `PARAGRAPH` node ever nests in a branch to carry the prose
lowering the way an environment body's does. Instead the lowering looks at the
conditional's nearest non-conditional ancestor. In running text that is a
paragraph, and the branch reflows with it — its words wrap and its inter-word
spacing normalizes, just as they would outside the construct. Inside a `\def`
body it is a group, which emits the byte-faithful stream, so the branch does too
and package code keeps its authored lines. That second case is not a nicety:
`pagesel.sty`'s `\ifx\\#2\\%` has the parser's `LINE_BREAK` node sitting in an
`\ifx` operand slot, and the prose reflow's "a `\\` ends its line" rule
oscillates on it pass over pass.

One further child can hang off a `CONDITIONAL`, and it is easy to lose. An
own-line `%` run before the opener binds forward as a `DOC_COMMENT`, and the
grammar reparents it *inside* the node, as a sibling of the branches. A lowering
that walks only the branches and the closer drops it silently — and the
non-trivia-content oracle cannot object, because a comment is trivia to the CST.
The comment oracle in `assert_format_invariants` exists for exactly this class
of bug, mirroring the one the `.bib` formatter has carried since it started
reordering fields.

There is no body indent, because the parser cannot separate the `\if` test from
the then-body (see § *The conditional gate*). The environment-shaped layout that
much package code is written in is therefore not the target.

### expl3 code formatting

Inside an expl3 region, source spaces and tabs are catcode 9 (ignored) and `~`
is catcode 10. Because inter-token whitespace is provably insignificant there,
the formatter owns the layout of in-region code, indentation and line breaks
alike, regardless of `WrapMode`. This is idempotent by construction: the
inserted whitespace is itself catcode-insignificant, so re-lexing the output
yields the same token sequence.

The target is the LaTeX Project's own house style, `l3styleguide.tex`. Its
mechanical rules are an 80-column target, a two-space indent per level, single
spaces between everything except simple runs of parameter tokens, one
conceptually separate step per line, a canonical brace layout, and no tabs. The
non-layout rules, such as naming prefixes and expandability, are meaning rather
than trivia and belong to a linter.

Two decisions carry most of the weight. Statement boundaries are **structural**
rather than newline-keyed: a call unit is a head command whose argspec suffix
gives derivable arity, plus the elements its slots consume, so the formatter
owns one-call-per-line and a width wrap re-derives the same unit on the next
pass. Whatever the scan cannot resolve degrades to a per-statement fallback that
is the authored physical line, which is the old newline rule demoted to a
residue and carrying its own fixed-point argument. And layout ownership is
positionally gated: the lexer and the formatter share the toggle-*name* set so a
new spelling is recognized in both, but only the formatter additionally requires
the toggle to be a top-level statement. A toggle spelling TeX never executes is
a false positive of the static model, and mis-owning its layout rewrites real
space tokens even though the byte-level oracles stay green. The lexer keeps the
naive name-only model on purpose, because mis-lexing a name only splits CST
tokens, which is lossless and cosmetic.

Conditionals are the one construct with a layout of its own. The guide's worked
example puts each `T`/`F` branch on its own line one indent step under the call,
even though joining them would fit the line, so a conditional that starts a
statement breaks that way regardless of width. Which is a decision about the
*call*, not about the tree: greedy attachment hangs the branch groups off the
head in `\tl_if_empty:nTF {#1} {T} {F}`, but a single-token slot breaks
attachment and hands them to a sibling instead, and
`\int_compare:nNnTF {a} = {1} {T} {F}` leaves them at the stream level once the
relation intervenes. Keying on the head node's own children would format those
differently for no reason an author could see, so the branches come from the
resolved call unit — the same argspec scan that decides statement boundaries,
which already had to find them.

The rescan is confined to statement-leading position. Anywhere else the
segmentation has already established that the conditional is an argument being
passed as a token rather than a call, and resolving a unit headed there would
claim the enclosing call's arguments as branches.

### Line endings

The printer always builds output with `\n` and is the sole authority on where
breaks go. `FormatStyle::line_ending` decides only how those breaks are spelled,
as a pass over the finished text: `auto` (the default) follows the source, `lf`
and `crlf` are unconditional, and `native` follows the platform. `auto` is the
default so a CRLF repository does not get a whole-file diff the first time it is
formatted.

This is the one carve-out in the protected-regions rule. A `verbatim` body is
emitted from source token text, so without a document-wide conversion a CRLF
document came out CRLF inside the protected region and LF everywhere else. Only
the `\r\n` and `\n` pair is touched; every other byte of the region is still
untouched.

### Comment directives

`badness_parser::directives` resolves the `% badness-format` family (`skip`,
`off`/`on`, `skip-file`) and the combined `% badness` family into sorted,
non-overlapping byte ranges, one list per axis. The formatter takes the format
list on `LowerCtx::suppressed` — the same shape as `expl3_regions`, owned by
`format_root` for the lowering — and the linter folds the lint list into
`SuppressionMap` as ranges where every rule is off.

It lives in the parser crate because neither consumer can reach the other: the
formatter is wasm-clean and is what the dprint plugin embeds, while the linter
lives in the root crate. Resolving a directive is a pure function of the tree,
so it sits below both, and the plugin gets suppression for free rather than
becoming a second sanctioned divergence from `badness format`.

Three decisions carry the weight.

**The verb carries the scope.** Rule-selective lint suppression needs a selector
slot; layout suppression has nothing to select. A grammar putting the selector
in second position would therefore be lint-shaped by construction, with the
format axis forced to leave a slot nothing could fill. Splitting on the verb
instead (`skip` / `off` / `on` / `skip-file`, the vocabulary tex-fmt, ruff, and
black already established) lets all three axes share one grammar, keeps the
selector optional where it means something and absent where it does not, and
keeps every form reading as an imperative — `skip-file` is "skip this file",
where a `-file` suffix on the subsystem name would read as a noun phrase.

The retired `% badness-ignore <rule>` and `% badness-ignore-file` spellings map
onto `% badness-lint skip <rule>` and `% badness-lint skip-file <rule>`, and
resolve through the same path. They are undocumented but permanent: a directive
spelling is user-facing API in the same sense a rule id is.
`Directive::deprecated` marks them for a future lint rule (recorded in
`TODO.md`). What the lint axis gains in the move is the **region** scope, which
the retired family never had.

The `.bib` side shares the grammar and differs only in carrier: BibTeX has no
line-comment token between entries, so a directive rides an `@comment{…}` entry
(`bib/linter/suppression.rs`). The format axis parses there and deliberately
does nothing — the bib formatter is a canonical re-emitter rather than a
trivia-only pass, so byte-exact reproduction is a different mechanism.

**Suppression is containment, not overlap.** An `off`/`on` region is delimited
by comments the author placed, not by CST boundaries, so it can begin halfway
through a construct — and every construct it begins inside is an *ancestor* of
the content it means to cover. Testing overlap would therefore suppress the
outermost such ancestor: one directive anywhere in a document body suppresses
the whole `document` environment, and with it the file. Containment picks the
outermost node fitting *within* the region, so a straddling ancestor keeps
descending and only the enclosed blocks are reproduced.

**A region anchors where a `skip` would target, clamped to the previous
directive.** An own-line `%` binds *forward* into the following construct's
`DOC_COMMENT` (see [Trivia attachment](#trivia-attachment)), so a region
starting at the byte after the `off` comment begins *inside* the construct it
means to cover, which then fails the containment test. Anchoring through
`skip_target` instead fixes that and picks up a preceding comment run bound into
the same `DOC_COMMENT`. The clamp is what keeps consecutive directives apart:
`on` / `off` / block puts both directives in one comment run, so the reopening
`off` resolves to a construct starting at the `on` and would otherwise swallow
the closer of the region before it, fusing the two.

Suppressed content is emitted as `Ir::verbatim` of the node's source text.
`Writer::write_verbatim` flushes the pending indent on the first line and leaves
every later line at its authored column, which is the same deliberate asymmetry
protected regions already carry: the block lands where the formatter puts it,
its interior is untouched. Trivia *inside* a region is emitted per token rather
than collapsed into a `Gap`, or the seams between two suppressed blocks would
quietly normalize while the blocks themselves stayed exact.

A suppressed region is preservation-only, so it upholds the
[trivia-perturbation](#trivia-invariant-layout) oracle by construction: every
perturbed variant is reproduced verbatim and is therefore its own fixed point.
The document-level trailing-edge normalization and the `line_ending` post-pass
still run over the result — the same carve-out protected regions live under.

## The linter

The linter reports diagnostics over the same lossless CST the formatter
consumes, and like the formatter it is a pure function of the input plus shipped
data. The user-facing catalog of shipped rules lives in the reference section
([Linter Rules](../reference/linter-rules.md), [BibTeX Linter
Rules](../reference/bib-linter-rules.md)), generated from each rule's own
description and examples.

### Rules and dispatch

Every lint implements `Rule`, which is `Send + Sync` so the registry can be
shared across the LSP's read pool. A rule declares a stable kebab-case `id`, a
`default_severity`, the description and worked examples that generate the rule
reference, and whether it can ever emit a fix.

No rule walks the tree on its own. Each participates in the driver's single
shared traversal one of three ways. Node-shape rules name the `SyntaxKind`s they
care about and get called once per matching element. Whole-file rules run once
after the walk, which suits rules driven by the semantic model or by cross-file
resolution. Streaming rules return a visitor fed every element in document
order, for findings that depend on the sequence, such as a running toggle or the
previous heading's level.

Each rule reads a `RuleContext` assembled once per file. Besides the syntax root
and the semantic model it carries the cross-file resolution a project view
provides (labels, cite keys, and package options), each `None` when there is no
project view, which makes the corresponding rules inert rather than wrong. It
also precomputes two shared side indexes, one of math byte ranges and one of
`\if…\else…\fi` branch paths, so the many rules that need them share one
membership test instead of each climbing the ancestor chain per token.

The registry compiles the rule list into a dispatch table indexed by
`SyntaxKind`, so node dispatch is a slice index, and it is cached across files
and shared by reference across the CLI's rayon lint phase. Configuration narrows
the active set as a post-filter, so the shared driver stays config-unaware.

### Autofixes

A diagnostic may carry a `Fix`: one or more edits applied atomically, so a
paired insertion can never half-apply. Each edit names its target file, so a fix
may reach across files, and atomicity then spans files.

A fix decides what to rewrite, never how to lay it out. It owes correctness, so
the result still parses and is still lossless, but not line width. When a fix
cannot meet that bar for some shape, make it correct by construction or withhold
it for that shape while still reporting the finding. Because a fix owes
correctness as a raw edit, with no formatter spacing to lean on, such a rule can
be strictly more conservative than a layout pass would be:
`redundant-script-braces` withholds the strip when a following character would
re-glue the argument, so `x^{2}-3` stays braced.

Each fix declares an applicability. `Safe` fixes preserve meaning and are
applied by `lint --fix`; `Unsafe` ones, those that could change typeset output,
require `--unsafe-fixes` or an explicit editor code action. The apply engine is
a pure function over source, fixes, and that flag, shared by the CLI and the LSP
code-action path. It drops any malformed or overlapping fix so the output stays
well-formed, and `lint --fix` runs it to a fixpoint, re-linting between rounds.

Findings are suppressed inline with `% badness-lint skip <rule>: <reason>`,
covering the next meaningful sibling; `off`/`on` covers a region and `skip-file`
the whole file, and omitting the `<rule>` covers every rule. See [Comment
directives](#comment-directives) for the shared grammar.

## The language server

The formatter is hermetic: its output is a function of the input plus shipped
data. The language server is allowed more latitude, because navigation is
inherently about the local environment. A runtime query of the TeX distribution
feeding the *formatter* stays a non-goal; a read-only index or metadata feeding
LSP navigation is sanctioned.

The LSP is built on `lsp-server` and `lsp-types`, rust-analyzer's stack, rather
than tower-lsp. Salsa cancellation is a synchronous unwind that composes with
`lsp-server`'s sync main loop plus threadpool and fights tower-lsp's async
`&self` model.

Environment awareness has four sources, all reading static facts only, with no
macro meaning and no typesetting.

**Shipped CTAN metadata**, generated from the pinned tlpdb, maps a package stem
to a description and catalogue id, and drives package hover and completion
detail. It has the same read-only posture as the name lists and CWL.

**A read-only TEXMF file index** (`project::texmf`) indexes the installed
`.sty`, `.cls`, and `.dtx` files, delegating root discovery to
`kpsewhich -var-value` since reimplementing kpathsea is out of scope. It is
cached to the OS cache directory keyed by a distro fingerprint, and it powers
document links, go-to-definition, and installed-set completion. It is gated by
editor settings, and it is never wired into signature resolution.

**The compile's `.aux` artifacts** (`project::aux`) are read by a dedicated
line-oriented scanner, never the LaTeX parser, since aux files are written under
`\makeatletter`. It extracts label numbers and toc entries, following `\@input`
chains, with freshness keyed by mtime and length so a recompile is picked up
without a watcher. This powers label hover and document-symbol number
enrichment. A test guards that the formatter never reads the aux file.

Citation completion returns the entire bibliography namespace rather than
prefix-filtering server-side, with each item carrying a `filterText` of key,
title, and authors so the client matches on any of those fields. That is
deliberately editor-agnostic: `filterText` is LSP-standard, so every compliant
client filters against it with no client-specific code.

## Tenets

1. Layout is decided solely by the formatter's rules and the layout engine. The
   formatter is the sole authority on layout, so push back against hard-coded
   special cases.
2. Autofixes are textual edits that never invoke the formatter. A fix decides
   what to rewrite, never how to lay it out, and owes correctness but not line
   width. The pipeline is fix-then-format, and the mirror holds: content
   rewrites never run inside `format`.
3. Parser and CST work must keep the salsa reparse path viable.
4. Parsing is the parser's job. Never paper over a parser mistake in the
   formatter, and never let parsing logic creep into the formatter.
5. Losslessness is the parser's job. The formatter may assume a lossless CST.

## Invariants

These are held by construction and enforced as test oracles. Breaking one is a
bug, not a trade-off.

- **Losslessness**: `reconstruct(text) == text`, byte for byte.
- **Idempotence**: `fmt(fmt(x)) == fmt(x)`.
- **The formatter is whitespace-only.** It changes trivia and nothing else, and
  never inserts, deletes, or rewrites a non-trivia token.
- **Protected regions** (`verbatim`, `lstlisting`, `\verb`, comments) are never
  altered, with the single line-terminator carve-out described above.
- **Reflow safety is structural**, never config-derived, so no wrap mode can
  corrupt a `.dtx`.
- **Trivia-invariant layout**: layout may read only those trivia predicates the
  formatter itself preserves. This one is still being rolled out.

There is deliberately no parse-stability invariant. The formatter may change CST
shape, and the whitespace-only invariant pins the non-trivia content the tree
carries, which is the part that matters. Running the formatter over a corpus is
a good way to find parser modeling gaps, so this freedom is useful rather than
merely tolerated.

Two oracles sit outside the fast test suite. We run
[texlab](https://github.com/latex-lsp/texlab)'s parser as a differential parse
oracle over a corpus, skeletonizing both trees and comparing; it is a reference
we measure against, not one we match. And because the CST cannot see the one
risk `ContentKind::Keyval` takes, where a space token is trivia to the CST and
content to TeX, `task typeset:check` compiles fixtures before and after
formatting and diffs the typeset output.

## Technology choices

rowan for the CST, salsa for incremental queries,
[smol_str](https://docs.rs/smol_str) for token text, [insta](https://insta.rs/)
for snapshot tests, [annotate-snippets](https://docs.rs/annotate-snippets) for
diagnostic rendering, and [`clap`](https://docs.rs/clap) for the CLI, with
`build.rs` generating man pages, completions, and markdown.

## Non-goals

No macro expansion, no TeX evaluator, no execution of primitives or `\def`
semantics. Common `\newcommand`, `\newenvironment`, and xparse *signatures* may
feed the semantic database, but they are extracted, never executed.

No general `\catcode` handling beyond the bounded patterns listed under
[sanctioned lexer modes](#sanctioned-lexer-modes).

No typesetting. Badness never runs `latexmk`, `pdflatex`, or any other engine,
and it never parses a `.synctex.gz`. Forward search *launches* a viewer, which
is an outbound side effect but not a build: it is an explicit user action, and
nothing it touches feeds back into the formatter or linter.

The formatter never reads the environment. Its output is a function of the input
plus shipped data, and it resolves local `.sty` and `.cls` files sitting next to
the document rather than the installed TEXMF tree, so output cannot depend on
what happens to be installed.
