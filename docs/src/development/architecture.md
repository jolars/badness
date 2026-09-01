# Architecture

Badness parses LaTeX into a lossless concrete syntax tree (CST) and puts a
formatter, a linter, and a language server on top of it. The design follows
[rust-analyzer](https://rust-analyzer.github.io/): a generic, error-tolerant,
hand-written parser produces a lossless tree, semantics live in a separate layer
above it, and recomputation is incremental via
[salsa](https://github.com/salsa-rs/salsa).
[arity](https://github.com/jolars/arity), the same kind of tool for R, was the
other influence.

This page is a practical tour of Badness's design. Its goal is to help
contributors understand how the pieces fit together. If you want to build and
test the project, start with [Contributing](contributing.md).

The document follows data through the system. It begins with the workspace and
its inputs, then moves from parsing to formatting, linting, and the language
server. The parser and formatter sections are necessarily the most detailed:
most of Badness's safety properties are established at the boundary between
those two components.

## What it does

At a high level, Badness turns source text into a syntax tree, then uses that
tree to produce diagnostics, formatted text, and editor features. It does not
typeset documents or run TeX. With the exception of a few language-server
features, it does not inspect the machine on which it runs either.

The parser is the foundation of the system. It is a hand-written,
recursive-descent parser over a flat token stream, and it builds a lossless
concrete syntax tree (CST) for the document. The formatter, linter, and language
server all work from this shared representation.

Here's the pipeline from text to tree:

```
text → lexer → token stream → parser → event stream → tree_builder → GreenNode
```

Like rust-analyzer's parser, ours does not build the tree directly. It emits a
small stream of events (`Start`, `Tok(idx)`, and `Finish`), which a separate
tree builder feeds into rowan's `GreenNodeBuilder`. Events refer to tokens by
index, while diagnostics travel on a side channel keyed by byte range;
consequently, the event stream needs no `Error` variant. The only specialized
event is `SubTok`, used to recover TeX's one-character script binding from a
coalesced `WORD`. The tree builder also reattaches trivia before producing the
final green tree.

From there, each subsystem has a different view of the same tree. The formatter
lowers it to a `Doc` intermediate representation and prints that representation.
The linter makes one shared traversal and collects diagnostics. The language
server answers requests through salsa queries over the tree.

With the explicit declarations described below as its only additional input, the
tree is a pure function of source text. Ambient configuration, the signature
database, and the filesystem do not influence its shape. This boundary is what
makes deterministic parsing and reliable incremental recomputation possible.

## The crates

Badness is an edition-2024 Cargo workspace with four crates. The root package,
`badness`, contains the CLI, language server, and linter. Two publishable
libraries and an unpublished WebAssembly shim live under `crates/`.

`badness-parser` contains the syntax layer (`syntax` and `ast`), the parser, and
the semantic model. The corresponding BibTeX layers live here too, alongside the
generated signature artifacts in `data/` and the `build.rs` script that turns
them into PHF tables.

`badness-formatter` depends on `badness-parser` and contains the layout engine
(`core`, `ir`, `printer`, `style`, `context`, `colspec`, `sentence`, and
`perturb`) as well as the `.bib` formatter.

`badness-wasm` is a `publish = false` wasm-bindgen shim over the two library
crates. It powers the [playground](../playground/index.html) and is built with
`wasm-pack` through `task playground:wasm`.

Both library crates target `wasm32-unknown-unknown`. As a result, code in those
crates cannot depend on the filesystem, threads, or child processes. The
formatter is also embedded by the [dprint
plugin](https://github.com/jolars/dprint-plugin-badness), and a CI job checks
that this target continues to build. Because the plugin runs in a filesystem
sandbox, it uses an empty runtime signature database; the CLI, by contrast, can
include signatures scanned from neighboring `.sty` and `.cls` files. This is the
one intentional difference from `badness format`.

The root crate owns `linter/`, `lsp/`, `project/`, and `text/`, together with
`incremental.rs` (salsa), `config.rs`, `cli.rs`, `completion.rs`, and
`file_discovery.rs`. It re-exports the member crates at their old module paths
through small shim modules. For example, `src/parser.rs` is just
`pub use badness_parser::parser::*;`, which lets existing callers continue to
use `crate::parser::…`. Two modules are genuine bridges rather than shims:
`src/formatter.rs` holds the `check` batch driver and the disk-backed
`format_file_with_packages` entries, and `src/semantic.rs` holds `load`.

## The BibTeX side

BibTeX is not implemented as a mode of the LaTeX parser. Instead, `.bib` files
have a parallel pipeline in `bib/`. It uses the same basic architecture—a
lossless rowan CST built from a flat event stream—but defines its own grammar,
`SyntaxKind`, `BibLang` marker, lexer, parser, tree builder, typed AST,
formatter, linter, semantic layer, completion, and outline support. Unless a
section says otherwise, the invariants in this document apply equally to both
pipelines.

### `%` comments in `.bib`

There are two plausible ways to interpret `%` in a bibliography, and the major
BibTeX implementations disagree. Classic `bibtex` 0.99d has no comment syntax
and rejects `%` inside an entry. Biber's btparse reader, on the other hand,
treats it as a comment that ends at the next newline and then resumes parsing.
Badness follows biber, consistently with the rest of its BibLaTeX-oriented
support (`bib_fields.json`, for example, tracks `blx-dm.def`). We verified the
difference by compiling examples with both tools.

The difficulty is that the meaning of `%` depends on context. It begins a
comment between a value and the following comma, but remains ordinary text
inside a braced or quoted value: `title = {50% off}` keeps the percent sign. The
lexer therefore stays context-free and always emits a bare `PERCENT` token. The
grammar decides whether that token begins a comment. At positions where it skips
trivia inside an entry—before a field name, `=`, `#`, `,`, or the closing
delimiter—it wraps `%` through the end of the line in a `COMMENT` node. Braced
groups, quoted strings, `@comment` bodies, and top-level junk never take that
path, so `%` remains an ordinary token there. This is the same division of
responsibility used by the LaTeX parser, where the grammar rather than the lexer
recognizes brace structure.

texlab's bib parser models no comment at all, so this is a recorded deliberate
deviation in `bib_parse_compat_allowlist.toml`, not a gauge regression.

A `%` inside a value exposes an awkward boundary between the two languages.
BibTeX passes it through as an ordinary character, but LaTeX later interprets it
as a comment while typesetting the value. Line breaks in such a value are
therefore significant. `lower_value_reflowed` refuses to reflow any value with
an unescaped `%` and emits it byte for byte. A CST oracle cannot detect this
mistake: joining the lines is syntactically lossless, yet changes the typeset
result.

The formatter always re-emits comments. A comment sharing a line with the
previous field stays on that line, just as a trailing LaTeX comment is never
relocated. Other comments bind forward to the field they precede and appear on
their own line above it. Binding to a field rather than a byte offset keeps the
comment attached when fields are sorted canonically. A comment after the final
field appears above the closing delimiter. If an `@string`, `@preamble`, or
field-less entry has no suitable line on which to place a comment, the formatter
preserves the whole block verbatim instead of risking data loss. These rules
inspect only whether a comment is on its own line, a property the formatter
itself preserves, so a second formatting pass makes the same decision.

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
`[format]` (`line-width`, `indent-width`, `item-indent`, `wrap`, `math-wrap`,
`lang`, `no-break-abbreviations`), `[lint]` (`select`, `ignore`), `[build]`
(`aux-dir`), and the [declaration](#declarations) maps `[commands.<name>]` and
`[environments.<name>]`. Excludes follow Ruff: `exclude` replaces the built-in
default, `extend-exclude` adds to it. `wrap` is an `Option` so the LSP can tell
"unset" from "set" when merging editor settings over project config, not because
the fallback depends on the file.

TEXMF discovery is deliberately not a section here. Where a TeX installation
lives is machine state rather than project data, so it arrives through editor
settings.

The language server caches resolved configuration per document directory, but
does not make cache correctness depend on `workspace/didChangeWatchedFiles`.
Each entry records the existence, modification time, and length of the
`badness.toml` candidates examined by the ancestor walk and of any resolved
fallback file. Normal document activity validates that fingerprint before using
the entry. This catches edits, deletion, and creation of a nearer project config
for clients such as Neovim that cannot dynamically register file watchers;
watcher notifications remain the eager invalidation path.

### Declarations

Most config only affects behavior after parsing. Environment declarations are
the exception: they feed the parser directly. Command declarations remain in the
semantic layer.

`[commands.<name>]` lets a project classify a custom command as a reference or
citation family without expanding its definition. The `like` target determines
whether a reference accepts one key (`eqref`) or a comma-separated list
(`cref`), and whether a citation has `nocite`'s wildcard behavior:

```toml
[commands.eqrefs]
like = "cref"

[commands.mycite]
like = "parencite"
```

This first command-declaration vocabulary is deliberately semantic-only. It does
not declare arity, attach arguments, or lend formatter layout behavior. The
alias is consumed by label/citation analysis and completion; the parser sees the
same token stream and produces the same tree with or without it.

`[environments.<name>]` lets a project describe constructs that source text
alone cannot reliably reveal (issue #109). Typical examples are alias delimiters
like `\bea`/`\eea`, environment behavior that should match a built-in, or
verbatim-like environments the definition scan cannot infer.

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

This is a deliberate widening of parser purity, but with strict boundaries. The
parser receives a `ResolvedDeclarations` value, not a full `SignatureDb`, so it
can only see explicit, hand-authored declarations. That keeps parser behavior
independent from ambient package scope and scanned runtime data.

Implementation-wise, the environment subset is seeded into `ParseCtx` on the
first pass, while semantic-model construction reads the command subset. The
shared types live in `badness-parser` so every consumer can use the same model.
In incremental mode, both are carried through one high-durability salsa input
(`incremental::DeclarationsInput`), so changing `badness.toml` invalidates the
dependent parse and semantic results, while normal text edits do not.

Each subset then reaches its readers through its own firewall query
(`parse_declarations`, `semantic_declarations`), which projects out one half and
backdates when that half is unchanged. Both halves sharing a cell is what makes
this necessary: without the split, renaming a command alias would invalidate
every parse in the project, and `parsed_document` is `no_eq`, so it could not
backdate its way out — every reparse base would go too, since a base records the
declarations it was parsed under. A command alias provably cannot change a tree,
so that cost buys nothing. The split makes the cost of an edit proportional to
what it actually changed.

In the LSP, declarations are republished in the request dispatcher (not ad hoc
inside handlers). This avoids stale cross-workspace state when the active file
moves between roots with different config.

The key safety property is simple: **a declaration names a spelling; it does not
force pairing**. Shape gates still decide whether a match is structurally valid.
So a wrong declaration degrades to ordinary syntax instead of corrupting the
tree.

`like` is the main mechanism. Environment declarations copy a curated built-in
entry, resolved against `builtin()`; command declarations copy only a curated
ref/cite command's semantic family, resolved against the family tables
(`ref_command`, `cite_command`). Both are closed tables: spelling alone does not
establish key semantics. The command side deliberately does not consult
`builtin()`: `signatures.json` carries layout data and has no entry for most of
the families, `\cpageref` — the only list-valued page reference — included.
Neither side resolves against CWL or scanned definitions. Unknown `like` targets
are config errors, and a name either curated source already knows may not be
redeclared, since that would reclassify a command the project never meant to
touch.

`like` also stays category-local. Cross-category relationships (for example,
command spellings that stand in for environment delimiters) use explicit keys
such as `begin`/`end`. Declarations do not currently expose command or
environment argspec.

The schema is keyed by **category, then name**. This keeps merging predictable
and avoids category-wide switches that could collide with real construct names.
Keyed tables are used instead of arrays so layered config can merge by name.

Validation happens at config load time, so broken declarations fail loudly
instead of being silently ignored. Rejected forms include empty entries, unknown
`like` targets, conflicting or duplicate spellings, invalid control-word
spellings, and delimiter declarations that violate environment constraints.

`begin`-only and `end`-only declarations are allowed (issue #117), because
literal `\begin{X}` and `\end{X}` forms can still provide the missing side.

Two validation checks are especially important: disallowing empty entries
(prevents silent no-ops) and disallowing obvious collisions with curated command
spellings (prevents accidental global remapping). These are guardrails, not the
primary safety mechanism; shape gates remain the ultimate protection.

Declared entries override scanned and built-in tiers. That is intentional: a
declaration is an explicit correction from the project author.

## Syntax and semantics

Badness deliberately separates syntax from semantics. The syntax layer is a
generic CST and, by default, knows nothing about what a command means. The
semantic layer enriches that tree with a signature database assembled from
curated built-ins, CWL-derived data, and definitions scanned from source. This
layer describes properties such as arity, verbatim behavior, sectioning, and
argument content kinds.

This is not an absolute wall: a small number of semantic facts may influence
parsing when they satisfy both of the following conditions:

1. The source is curated or explicitly declared.
2. A wrong fact can be falsified from text shape and demoted by a gate.

Routing and pairing facts meet this test because the source can disprove them.
Generic arity does not. An incorrect arity can produce a different attachment
while remaining byte-for-byte lossless, so the usual syntax oracles cannot
detect the mistake. For generic LaTeX, arity therefore belongs in the semantic
layer.

`ContentKind::Keyval` is the most sensitive semantic claim because it can affect
typeset output. It is curated and validated conservatively, since it licenses
splits at glued commas in key-value contexts.

Environment signatures separately use the curated `labelKey` flag when a
top-level `label` entry in the first optional argument defines a LaTeX label.
The semantic model accepts only flat literal bare or braced values, applies
repeated entries in order, and treats a later dynamic value as unknown rather
than retaining an earlier literal. The initial built-ins are `frame` and
`lstlisting`; declarations may inherit the fact through `like`, while CWL and
source-scanned signatures cannot grant it. This is not inferred from
`ContentKind::Keyval`, because many key-value processors give `label` unrelated
meanings.

The linter's `label-before-caption` rule uses the independent curated
`captionContainer` flag for non-float environments whose statement-level
`\captionof` conventionally owns a preceding label. `minipage` is the initial
member. Ordinary block environments are not inferred to be caption containers,
and plain `\caption` remains float-scoped; this keeps the unsafe move fix on the
silent side when the intended counter is ambiguous.

## The parser

The parser is hand-written recursive descent over a flat token stream. It treats
its input as generic TeX surface syntax and always produces a lossless tree.

Resolving macros and catcodes in full generality means running a TeX engine, and
we do not do that. Anything we cannot resolve statically degrades to a generic
node, with a diagnostic where one is useful, never to a crash or to corrupted
output.

### Sanctioned lexer modes

Badness does recognize a bounded, gradually growing set of patterns from static
source shape. Recognition is deliberately conservative: when the evidence is
insufficient, the parser leaves the construct generic. The supported patterns
fall into the following categories:

- **Letter modes.** `\makeatletter` makes `@` a letter; `\ExplSyntaxOn` and the
  `\ProvidesExpl*` declarations open expl3, where `_` and `:` are letters. The
  two flags are independent and compose. In a `.dtx` a file-level signal (a
  `%<@@=…>` guard or a `\ProvidesExpl*` anywhere) puts every `macrocode` body
  under expl3 catcodes.
- **Verbatim.** `\verb`, verbatim-like environments, and verbatim-argument
  commands capture their body as a single token. Built-ins are curated;
  user-defined ones are found by a bounded two-pass definition scan that
  fingerprints catcode-othering signals and recognizes definer identities such
  as `\lstnewenvironment`. A curated command may instead mark one positional
  braced argument as verbatim—`\href` uses this for its URL while leaving the
  visible-text argument parsed. The capture forms only when the marked balanced
  group is present; local definitions suppress a colliding built-in mode.
- **Delimiter isolation.** The token after `\left` or `\right` is emitted on its
  own, so the parser can build the `LEFT_RIGHT` pair.
- **Math environments.** An environment the curated table flags `math` has its
  body parsed in math mode and wrapped in a `MATH` node, exactly as `\[…\]`.
  This is a grammar decision needing no lexer math state, and it reads the
  curated flag only, never the bulk or user tiers. Math parsing and alignment
  layout are separate classifications: for example, `gathered` is math-only,
  while `aligned` also carries the `align` flag. The `empheq` wrapper is
  math-only because its required keyval argument selects among AMS equation
  types; the formatter derives grid layout from the resulting body's `&` and
  `\\` structure instead of assigning one layout to every selection.
- **Definition bodies.** Inside the argument groups of the curated definer set
  (`\newcommand` and `\newenvironment` families, xparse, the LaTeX2e hooks),
  `\begin` and `\end` parse as plain commands, because TeX does not require them
  to balance within one group. An unbraced control-symbol name after a command
  definer is likewise consumed as definition data, so declarations such as
  `\DeclareRobustCommand\[` cannot open live display math.
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
  open math. The escaped form also stays data when it occupies a whole alignment
  cell (`` `\X& ``), a local shape that covers templates such as `\char#`
  without inferring macro expansion.
- **Signatures.** `\newcommand` and xparse signatures are extracted into the
  semantic database, never executed.
- **Environment aliases.** A command whose replacement body is exactly
  `\begin{X}` (or `\end{X}`) stands in for that delimiter, so `\bea … \eea`
  pairs as an `ENVIRONMENT` of `X`. See below.
- **Picture-body statements.** In a curated `statementBody` environment body
  (the TikZ/pgf picture family, routed by `ParseCtx::is_statement_environment`
  from curated built-ins plus declarations, the math-routing template), each run
  up to a top-level `;`-carrying `WORD` wraps in a `STATEMENT` node —
  retrospectively, by the same `precede` splice that builds `PARAGRAPH`, so
  there is no gate and no scan. A run that never reaches a `;` stays plain
  paragraph content; a genuine `\begin` is a statement boundary. Only statement
  *extent* is modeled — no `at`/coordinate/path grammar — because extent is what
  statement *boundaries* and the continuation hang need; interior statement
  layout is the semantic layer's job (§ *Statement bodies*).

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

The `\begin` gate runs on the shared batch driver as `EnvGate`. Unlike the
positive pairing gates, it is a *demotion* gate, so its answers have the
opposite sense: finding an escaping `}` demotes the environment, while finding
none keeps it. Reaching end of file does not count as an escape, which preserves
the useful unclosed-environment diagnostic when an author forgets `\end`.

This inversion has two practical consequences. A stray `}` closes the scan
instead of refuting it, even though positive gates treat the same event as a
reason to decline. Math delimiters are not anchors either. A positive gate can
safely decline when it encounters one, but doing so here would retain an
environment that the scan cannot justify. Finally, the enclosing `group_depth`
and the `.dtx` documentation-margin exemption belong to the parser's walk state,
not the scan state. They are checked separately for each opener rather than
stored in the batch.

The two math gates, `DollarGate` and `DelimMathGate`, use the same driver for
consistency rather than speed. They are *single-entry* gates: a batch settles
only its seed and opens no nested entry. This follows naturally from the
grammar. Once a reachable delimiter claims its closer, it also consumes every
potential opener before that closer, leaving no neighboring opener in the same
frame to settle.

Their policies differ from the pairing gates in four ways. An unbalanced `}`
always causes refusal, matching the parser walk they guard. A different kind of
math delimiter is ordinary content (and, for `DollarGate`, another `$` may be
the closer). Environments are counted at every brace depth because math parsing
continues to recognize them inside groups. The closing delimiter itself does not
require balanced environments, since it ends the math body wherever it appears.
`DollarGate` is also the only gate that is not memoized: after a `$$` is
demoted, parsing resumes at its second `$` and asks a genuinely different
question at the same token index and walk state.

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

The distinctive feature of this family is that both anchors are depth-**blind**:
a `\begin`/`\end` refuses rather than counts (an optional never legitimately
spans an environment, so either half means a runaway `[`), and it and the
paragraph break fire at any brace depth. Both follow from the walk they guard:
`optional` bails wherever the cursor stands, and a gate stricter *or* looser
than its parse is a bug.

The in-math gate adds two rules of its own. First, it interprets `$` according
to the enclosing math's *flavor*, which belongs to the walk state and therefore
forms part of the batch's memoization key. Inside `\[…\]` a `$` opens a genuine
nested inline region, so a balanced `$…$` in the bracket is **transparent** —
the entries' own openers and closers stop counting until the matching `$`, and
everything else reads on — while inside `$…$` TeX cannot nest one, so the first
`$` at the bracket's own level is that math's closer and refuses. And the gate
is stricter than the `optional` bail in one preserved respect: its
`\begin`/`\end` anchor carries no `in_macro_code` filter, which can only decline
to attach. All three bracket gates ignore chunk-unmatched braces, matching the
walk they guard: `optional` treats such braces as ordinary macrocode tokens but
still bails at a structural `R_BRACE`.

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

Badness can infer environment aliases from definitions in the current file. For
example, it can recognize `\bea ... \eea` as shorthand for
`\begin{eqnarray} ... \end{eqnarray}`. Projects may also provide aliases
explicitly through [declarations](#declarations).

Inference remains deliberately local and conservative. The parser does not
import aliases from neighboring package files, and the target environment's
behavior must come from the curated built-ins. An alias for only one delimiter
is still useful: an alias opener may pair with a literal closer, and a literal
opener may pair with an alias closer.

Internally, alias and literal closers have separate indexes but share a target
lookup. The parser ignores potential alias openers while processing definitions
such as `\def` and `\let`, preventing the definitions themselves from pairing
with one another. Actual pairing uses a positive shape gate: if the scan cannot
find a reachable closer, the opener falls back to an ordinary command. As with
the other gates, the shared batch driver avoids a separate, potentially
quadratic scan for every opener.

Downstream behavior is resolved from the parsed node, not raw spelling. That
keeps `\begin{bea}` distinct from a command alias `\bea` unless the node itself
was parsed as an alias delimiter.

Alias openers are also recognized in math parsing paths where relevant (for
example `split`-style environments), so literal and alias spellings converge to
the same environment node shape.

Known conservative gaps are accepted (for example complex `\let` chains and
argument-taking aliases) in exchange for parse safety.

### The conditional gate

When it can locate a complete conditional, the parser groups
`\if … \else/\or … \fi` into a `CONDITIONAL` node with positional branches. This
gives the formatter and linter a stable extent for the construct. The node does
not try to identify an exact boundary between the test and its body: TeX's
conditional tests are scanner-driven, and static analysis cannot locate that
boundary reliably enough to put it in the syntax tree.

Recognition uses a curated opener model from `parser::conditional` (shared with
the linter index), including exclusions for `if*` macro families and declaration
operand slots where `\if...` text is not live control flow.

The gate requires a reachable `\fi` at the opener's own recognized nesting
levels (brace/environment/math), with `macrocode` frame boundaries respected.
This prevents the scan from promising closers the structural walk cannot
actually consume.

The located closer bounds the parser walk, but nested openers may be demoted
when the parser applies their gates again. In that case the walk can finish
earlier than the initial scan predicted. For this reason,
`ast::Conditional::closer` is intentionally fallible.

For performance, conditional decisions run through the shared batch gate driver
(`Parser::gate_batch`) instead of per-opener scans. Policy differences remain
explicit per gate.

Conditionals differ from environment pairing in a few important respects:

- EOF without closer demotes conditionals.
- No `.dtx` doc-margin exemption is applied.
- Paragraph breaks anchor conditionals at their own level.
- Conditionals are not recognized inside expl3-owned regions.

### Recursive descent, with Pratt local to math

Hand-written recursive descent is the spine. Precedence climbing is used only
for sub- and superscript binding and for `\left…\right` matching; the text-level
parser has no precedence.

Ordinary input characters, including catcode-12 arithmetic operators, remain
coalesced in `WORD` runs, so unscripted `a+2*1` is one CST token. A semantic
view exposes one virtual math atom per Unicode scalar without changing that
tree. Only structural script binding refines a run with byte-range sub-tokens:
when a script follows, the final input character is isolated as its base, so
`a,b^2` scripts only `b`. When an unbraced script argument starts with a `WORD`
run, it likewise consumes one input character; any remainder returns to the
enclosing math list, so `x^23_i` parses as `x^2` followed by `3_i`. These are
TeX token boundaries, not arithmetic precedence. There is no
arithmetic-precedence expression tree.

The virtual-atom classifier is static semantic data. Its generated baseline is a
normalized extract of unicode-math v0.8r, pinned by commit and regenerated by
`task math-symbols:sync`; its LPPL notice and license ship beside the data.
Curated overrides add kernel aliases and the few cases where Badness must differ
from the baseline. It models `Ord`, `Op`, `Bin`, `Rel`, `Open`, `Close`,
`Punct`, `Fence`, and `Inner`, while a separate delimiter role records whether
an atom is actually pairable. This distinction keeps spacing classes such as
`\sqrt`'s `Open` and `!`'s `Close` out of bracket accounting. Commands and
characters use the same lookup, exact source spans survive multibyte characters,
and unknown commands conservatively fall back to `Ord`.

### Argument grouping and bracket policy

The CST greedily attaches trailing `{…}` and `[…]` groups as argument nodes,
texlab-style. Arity is unknown at parse time; the semantic layer refines it.

One positional refinement is admitted during parsing: curated built-in slots may
declare an `ArgumentDomain` of `Math` or `Text`, independently of formatter
`ContentKind`. A shared matcher aligns groups with slots while skipping omitted
optionals. A matched `Math` group uses the ordinary math-element parser, while
`Text`, `Unknown`, unmatched, and over-attached groups use generic parsing.
Attachment itself remains greedy.

The same curated slot data may mark a braced argument `verbatim`. The lexer then
captures that balanced group as one `VERB` token, and a companion slot matcher
advances past the raw token so later parsed groups keep their positional domains
and content policies. This is a bounded catcode claim, not formatter opacity:
`ContentKind::Opaque` preserves whitespace in an already parsed group, whereas a
verbatim slot prevents characters such as `%` from becoming syntax at all.
Mechanical CWL signatures and scanned definitions cannot establish this mode.

The load-bearing claim is independence from mutable signature data. Positional
domain parsing reads only the hand-curated built-in tier—never package scopes,
scanned definitions, declarations, or CWL signatures. The latter sources assign
`Unknown` to every slot. For generic LaTeX that forces greed: `\foo{a}{b}` is
either a two-argument call or a zero-argument command followed by two groups,
and nothing in the text says which.

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

expl3 is the one systematic counterexample, and the one place attachment is
arity-directed. The argspec suffix rides in the `CONTROL_WORD` token itself,
since in-region `:` and `_` are letters, so arity-directed attachment there is
exactly as text-pure as greed. Greed is not neutral in that dialect, it is a
systematically wrong guess: every single-token slot breaks the run, so under
greed `\tl_set:Nn \l_a {x}` attached `{x}` to the definee, and the semantic
layer's peel-back queue existed only to undo that after the fact. In-region
colon-suffixed heads therefore attach by their argspec (`grammar/expl3.rs`): a
pure token-level scan consumes the head's slots — a control-sequence argument
keeps a bare `COMMAND` node of its own, a relation character or `#`-parameter
bumps as tokens, groups and branches attach as ordinary `GROUP`s — and the walk
replays exactly the scanned plan, so the gate mirrors the walk by construction.
`w`/`D`/colonless heads and the `\::n` expansion drivers stay greedy, and the
scan aborts to greed with no diagnostic wherever it cannot mirror the walk: an
in-math head (an `N` slot would swallow the enclosing math's closer), a docstrip
guard or doc margin mid-unit, a candidate the walk would make a node of, an
unreachable closer, a group that crosses an expl3 lexer-mode toggle, or a
paragraph separator. A blank-line gap inside a brace group instead commits the
consumed prefix, the sanctioned partial commit. The trigger keys on token shape
alone — a colon-carrying control word can only have lexed inside a region —
which also covers the implicit `.dtx` regions the toggle index cannot see, and
the formatter's positional toggle gate stays the formatter's alone.

The scan resolves its group slots through a shared matching-brace table rather
than a rescan per slot, for the reason the shape gates run in batches: nested
call sites ask about spans their enclosing ones already walked, so a per-slot
rescan is quadratic in the nesting depth. One stack pass settles every pair in
the `macrocode` frame, keyed on the two facts that decide pairing — the
chunk-plain brace set and the frame itself. Bounds that move without changing
pairing (an alias closer) filter the answer at query time instead of
invalidating the table.

Mis-attachment is unusually hard to detect because it is invisible at the byte
level: an incorrect tree can still be lossless and format idempotently. To
validate this design, an independent oracle compared grammar attachment with
`semantic::expl3` consumption across the gate corpora. It covered 67,000
statement-leading heads in 265 files and found no unexplained disagreement; the
remaining differences were cases where greedy parsing had harmlessly attached
trailing material to an already consumed argument. Corpus fixtures now preserve
that coverage. Expl3 regions are allowlisted in the texlab gauge because texlab
has no argspec model.

`semantic::expl3` still resolves statement extent and handles heads whose shape
cannot be derived. Its consumption is independent of CST shape, so it also works
for scans that abort and fall back to greedy attachment. Formatter code that
once reconciled those two interpretations can now read the attached nodes
directly.

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

The losslessness property suite complements the curated corpus and texlab
differential oracle. It checks arbitrary valid UTF-8 and recursively generated,
syntax-heavy malformed input through both the LaTeX and BibTeX parsers. LaTeX
cases run as documents, packages, `.dtx` sources, and with fixed declarations.
The sole assertion is byte-for-byte reconstruction; malformed input is not
expected to parse without diagnostics. Ordinary tests run 256 cases per
property, while `task parser-properties` and the scheduled CI job run 4,096.

### Incrementality

Salsa provides the first level of incrementality across files and queries.
Intra-file reparsing is a separate optimization layered on top, described in
[Intra-file reparse](#intra-file-reparse) below.

Green nodes are stored in salsa, never red ones, because red trees are not
`Send`, `Eq`, or `salsa::Update`. `incremental.rs` stores `rowan::GreenNode`
under `no_eq, unsafe(non_salsa_values)`, sound because the tree is a pure
function of the text, and materializes red cursors on demand.

`SourceFile.text` is an `Arc<str>`, not a `String`, and every setter takes
`impl Into<Arc<str>>`. A keystroke moves a document's text through several hands
— the live buffer, the worker job, the salsa cell, every in-flight read job —
and all but the first only read it, so each of those hand-offs is a refcount
bump. It also gives the two hot guards a pointer test: `upsert_file` skips the
salsa write when the text is unchanged (salsa's setter does no equality check of
its own, and writing bumps the revision unconditionally), and every read job
asks whether the snapshot still holds the buffer it captured. Both go through
`Arc::ptr_eq` / a fat-pointer comparison **in front of** the content compare,
never instead of it: the language server hands back the same allocation it
already wrote, while a file re-read from disk is a fresh allocation that may
still be equal. See `IncrementalDatabase::text_is_current`, and [the language
server](#the-language-server) for the buffer at the other end.

Salsa's default input durability is `LOW`. `SourceFile.path` is built at
`Durability::HIGH` because it is set once and never mutated; `text` keeps `LOW`,
since a keystroke rewrites it. The project's [declarations](#declarations) are
the first genuinely config-shaped input, and are likewise built *and written* at
`HIGH`. Any future input promoted from config or package metadata must be
constructed at `HIGH` or `MEDIUM`, or every keystroke's global revision bump
will invalidate it.

Project membership is an explicit `ProjectFiles` singleton input at `MEDIUM`
durability. The single writer synchronizes it whenever a normalized path enters
or leaves the tracked-file map; text edits do not touch it. The keyless
`workspace_project` query derives a canonical `Project` as plain `Eq` data, and
the include graph, package graph, label and citation resolvers, package-option
model, and per-file signature scopes are keyless queries over that value. This
shape is deliberate: Salsa retains interned values for the database lifetime, so
an interned `Project` retained every historical membership and all memos keyed
by it as the language server discovered files. A tracked value instead backdates
when membership is equal and replaces its one memo when membership changes. Read
jobs need no parallel membership vector—their database snapshot already contains
the matching input revision.

The query-execution log is also dormant by default. `clear_query_log` opens an
observation window for incremental tests; production language-server sessions
never enable recording and therefore cannot retain one log entry per query
execution.

### Intra-file reparse

A keystroke used to re-parse the whole file. On a small `.tex` that is fine; on
a 730 KB thesis it was 27 ms, which was 97% of the keystroke. `parser::reparse`
splices the edit into the previous green tree instead: the same keystroke typed
into prose now costs 0.71 ms end to end, and a line typed inside an `lstlisting`
\~0.85 ms. Timed on its own, the reparse those keystrokes pay for is \~37 µs and
\~40 µs against a \~26 ms full parse — roughly 700x, and the ratio grows with
the file, since both leaf tiers are `O(depth)` where a parse is `O(file)`. It
arrives in phases, tracked in `TODO.md` § Incremental reparse — the token and
protected-body tiers are live, delimiter-bearing math fragments cover local
shape changes, the first conservative region slice handles multi-token edits in
inert top-level prose, and an edit no tier claims still costs a full parse.

Those numbers are held by `benches/reparse.rs` (`task bench:gate`), where every
case declares the tier it must reach as well as the speed it claims: a floor
alone would still pass after a case silently fell back to a full parse, because
declining is always sound and fails nothing else. With the parse this cheap the
keystroke's remaining cost moved to the write phase, which was mostly rebuilding
the line table; patching it instead took that keystroke to 91 µs end to end (see
[The live buffer](#the-live-buffer)). `task bench:keystroke-gate` watches both.

**The invariant.** A successful reparse yields a green tree *and* a
`SyntaxError` vector byte-identical to a full parse of the edited text. Nothing
weaker is admissible, because the tree feeds the formatter — which writes the
user's file — and the linter, whose fixes rewrite content. Every guard failure
returns `None` and the caller full-parses: never an error, never a best-effort
tree. That refusal-first contract is what makes the design extensible. A
construct the guards do not understand costs speed and nothing else, so a new
guard is always a safe change and an oracle failure is always fixed by *adding a
bail*, never by relaxing the assert.

**The shape, and what it does not require.** The tiers sit strictly on top of
`parse` and `lex`. The token tier relexes one leaf in isolation, proves the
relex is a single token of the same kind joining its neighbours the same way,
and splices with rowan's `SyntaxToken::replace_with` — every green node off the
leaf-to-root path is shared, so the cost is `O(depth)`, not `O(file)`. The
protected-body tier makes the same splice from a different proof (below). The
math tier reparses the outermost enclosing delimiter-bearing math node and
splices that node. The region tier re-runs the *ordinary* parser over a
substring and splices the resulting children under `ROOT`, using neighbour-sized
boundary parses purely as proofs that the substring is decoupled from its
context, then discarding them.

**What the token tier has to prove, and how.** A parse is a function of exactly
two things: the token vector and the `ParseCtx`. Fix both and the grammar is
deterministic — the shape gates, the prescan indices, the trivia binding, and
the attachment walk all read tokens, never source offsets. So changing one
leaf's text reproduces a full parse when three things hold. The token *kind*
sequence is unchanged: the new text must relex, alone, to a single token of the
leaf's own kind, and two join probes must show it still separates from its
neighbours (`\foo` beside `1ab` is two tokens only because the word starts with
a non-letter, and editing it to `aab` merges the pair). The definition scan
cannot have moved: it walks only `COMMAND` nodes whose head names a definition
family, so a leaf under none of them changes nothing it found. And no decision
that reads a token's *text* can flip.

That third one is the interesting one, because it has no compile-time link to
the code it describes. It is held by a test that scans the grammar sources for
every text comparison and fails on one nobody classified — 42 sites, each
carrying a verdict, of which 35 are kind-gated to a control sequence and so can
never see a spliced leaf at all. The remaining handful are the real reads: the
`;` that ends a picture-body statement, the lone `*` of a starred variant, the
math script slicing, the expl3 argument slots, and the environment-name
assembly. Each is neutralized either by a text guard or by a position ban. Math
`WORD`s need one extra proof: the CST may hold several adjacent `WORD` leaves
cut from one lexer token by per-scalar script binding. The tier reconstructs
that coalesced word, relexes it as one `WORD`, probes only its outer neighbours,
and checks the edited leaf's structural role. A direct `SCRIPTED`, `SUBSCRIPT`,
or `SUPERSCRIPT` word must remain exactly one Unicode scalar; an unscripted
prefix or remainder may change length. A moved boundary declines to the math
tier.

Refusals are free, so they are generous. Line terminators, environment names,
definition bodies, and a join probe against an oversized neighbour are refused.
`.dtx` is not refused wholesale: a fragment must expose enough of each docstrip
state bit for an isolated relex to disagree, and the source-scanning survey pins
one counterexample per bit.

**What the protected-body tier has to prove instead.** An edit inside an
`lstlisting`, a `\verb`, or a `\url` is the same one-leaf splice, but the token
tier's proof is unavailable: a raw capture is a kind the lexer only emits once
it has seen an opener, so the body lexed on its own comes back as ordinary
prose. Rather than restate the catcode rules — a second copy of the lexer, to be
kept in step forever — this tier relexes the leaf's **whole enclosing node with
its delimiters**, which puts the isolated lexer into the capturing mode for
free. This proof has four parts. First, *faithfulness*: the unedited fragment
must relex to its original tokens. This demonstrates that the bytes do not
depend on the state the file arrived in, and is what rules out a short-verb
span, an `@`-bearing name under `\makeatletter`, and a name that only lexes
whole inside an expl3 region, without enumerating any of them. *Locality*: a raw
capture's bytes never reach the lexer's state updates, so it leaves the fragment
in the state it entered — a claim about lexer code, and therefore a lexer test,
with a counterexample beside it (a body that *breaks* its capture does move
later lexing). *Termination*: a `VERB` carries its closer in its own text, but a
`VERBATIM_BODY`'s `\end{name}` is a sibling, and an unterminated body runs to
EOF — so the tier requires that `\end` to be inside the fragment, or the
isolated scan would stop where the file's does not. *The sequence check*: the
edited fragment must relex to the same tokens with only the leaf's text changed,
which is what catches an `\end{verbatim}` typed into a body or a brace that
unbalances a `\url`.

Newlines are allowed here, unlike on the token tier. That is the point — inside
a raw body a line break restructures nothing, because the grammar sees one
opaque token either way, and pressing Enter in a listing is the workload.

For `.dtx`, the fragment relex receives the base parse's full-file
`implicit_expl` bit. It still has to reproduce the concrete margin, macrocode,
and lexer-mode tokens around the capture; an edit that changes the full-file
signal is refused before relexing. Faithfulness is evidence about the fragment,
not permission to infer missing file state.

**The math tier.** A shape-changing edit cannot splice `SCRIPTED` or `MATH`
alone: neither carries the delimiter that establishes math mode. The tier takes
the outermost enclosing `INLINE_MATH`, `DISPLAY_MATH`, or math `ENVIRONMENT`,
including all enclosing math gates an inner edit could invalidate. An isolated
faithfulness parse under the base `ParseCtx` must first reproduce the old node.
The edit may touch only state-neutral math surface syntax; control sequences,
comments, environment names, definition-sensitive positions, `@`/`:` mode
ambiguity, and `.dtx` decline. The edited parse must yield one same-kind node
spanning every fragment byte, and a one-token right-boundary probe proves local
recovery cannot consume an unchanged suffix. Bases whose diagnostics are not in
source order also decline, because a local splice cannot reproduce a global
recovery-stack reorder.

The replacement parse supplies diagnostics inside the fragment; prefix
diagnostics are retained and suffix diagnostics shifted. The direct benchmark
pins partition-preserving edits to `Token` and an `x^23_i` boundary move to
`Math`. On the 730 KB benchmark document the measured math splice is about 10 µs
against a 26 ms full parse, over 2,500x faster.

So none of the parser's left-to-right state is checkpointed: not the lexer's
(`at_letter`, `expl_syntax`, `short_verbs`, `macrocode`, brace depth), not the
grammar's prescan indices, not the gate memo's token-keyed verdicts. This is
worth stating because a first reading of the parser suggests the opposite —
those look like the obstacles, and they would be for a parser that resumed
mid-stream. They return only at the region tier, where a shape gate's verdict
for a node *before* the edit can flip because a closer *after* it appeared or
vanished, which is why that tier is last and why it wants the precomputed closer
map rather than per-opener scans.

**The region tier.** Its two conservative slices reparse one top-level prose
`PARAGRAPH` when an edit spans multiple direct prose leaves, and the two
paragraphs around a blank-line seam when that seam is deleted or replaced. A
faithfulness parse must first reproduce the old fragment under the base's exact
`ParseCtx` and full-file `.dtx` implicit-expl signal. That admits unchanged
commands inside a paragraph without assuming the fragment's entry state: edits
themselves may touch only direct prose/trivia leaves and may insert no
structural or catcode-sensitive spelling, so those commands and their state
transitions remain unchanged. The one-paragraph case uses rowan's node splice;
the seam case rebuilds `ROOT` from shared green children, allowing two paragraph
nodes to become one. Diagnostics outside the fragment are shifted (and fragment
diagnostics replaced), and the common oracle checks both results. Seam splicing
initially refuses `.dtx`, whose column-sensitive doc layer needs its own proof.
Single-leaf edits stay with the cheaper tiers. The direct-reparse benchmark pins
both paths to `ReparseTier::Region` and gives each its own calibrated speedup
floor, so a future guard change cannot silently turn either measurement into a
full parse or another tier.

Unrestricted regions would require three further proofs: *gate isolation*, every
construct whose forward verdict could flip outside the fragment must be
accounted for; *boundary-parse verification*, unchanged neighbours must prove
the fragment is decoupled from its context; and *concatenation*, token-inclusive
seams, replacement diagnostics, and untouched siblings must reproduce the full
result. Blank lines alone reset neither every lexer mode nor every forward gate,
so they are a candidate partition rather than a proof. The precomputed closer
map tracked under Parser is the natural dependency for making the gate proof
cheap enough to use in a refusal-first tier. That widening is deliberately
deferred until a measured workload justifies the new parser infrastructure; it
is not required for the current conservative region tier to be complete.

**The salsa side channel.** `parsed_document` needs the previous text, tree,
errors, and the edits since — none of which are salsa inputs, and none of which
may become any. A base that invalidated on write would defeat the purpose; one
that did not would lie to the dependency graph. Instead they live beside salsa,
reached through default `IncrementalDb` methods (`reparse_prev`,
`reparse_stage_edits`, `reparse_pending_edits`, `reparse_store`,
`reparse_evict`), so a database without a cache simply always full-parses.
Reading mutable state from inside a tracked query is sound *only* because of the
invariant above: the query returns what `parse(text)` would whatever the cache
holds, so a cold, stale, or evicted cache costs a parse.

Three details are essential to this arrangement. The store happens **last**,
after every fallible step, so a panic or salsa cancellation cannot leave a base
whose text and tree disagree. The chain is drained by **consumed prefix count**
rather than cleared, because a stage can land between the peek and the store —
and it is drained **unconditionally**, even when it went unused, since a chain
kept back because it failed to verify describes a transform out of a text the
base no longer holds and would poison every later parse. And eviction has **two
classes**: an entry is *hot* once it has shown it benefits, and cold entries go
first, because a `package_graph` or `scope_signatures` sweep parses every
workspace member and stores a base it can never hit — under a plain LRU one
project-wide query would cost every open buffer its base.

There is deliberately **no whole-text `diff_edit`** in the query. The language
server knows the range it spliced and hands it over; re-deriving it costs more
than the reparse it feeds. A text that changed by a route carrying no edits — a
disk reload, a whole-buffer replace — simply full-parses, and both are shapes a
cost guard would decline anyway.

**Where the chain comes from.** `apply_content_changes` (`lsp.rs`) already
resolves each `didChange` range to byte offsets to splice the live buffer, so it
returns that as an `Edit` chain — the clamped offsets it actually used, each
edit against the text its predecessors produced, `None` for a range-less
whole-buffer replacement. `WorkerJob::Edit` carries it to the worker, which
stages it against the `SourceFile` returned by `upsert_file`, on the line after.
Every other `upsert_file` site — `didOpen`, the push-mode re-lint sweep, sibling
seeding, a watched-file re-read — stages `None`, so the pairing needs no
exceptions.

The ordering matters in two places. First, staging follows the write because
`upsert_file`'s `&mut db` is what proves no analyze is reading: a chain staged
ahead of the text it describes could be peeked by an in-flight
`parsed_document`, which would fail to verify it, perform a full parse, and
drain it. Second, the chain is staged even when `upsert_file` skips its write,
because it is anchored at the *base*, not at the db text — a buffer that
round-trips back to what salsa holds still took a transform to get there.

**How it is held.** A `#[cfg(debug_assertions)]` oracle compares every
successful reparse against a full parse, and every tier returns through a single
`finish` so it cannot skip that or the `O(1)` check that the tree spans exactly
its text. The latter runs in *every* build and falls back rather than panicking,
because the release binary is precisely the one the debug oracle is absent from
and also the one whose formatter writes the file. On top sits a seeded harness
(`crates/badness-parser/tests/incremental_reparse.rs`) over hand-written hazard
snippets — one per sanctioned lexer mode — and the parser corpus. Both oracles
carry should-panic self-tests, since a net nobody has watched catch something is
not evidence that it can.

Breadth comes from the **corpus sweep**
(`crates/badness-parser/tests/reparse_corpus_sweep.rs`,
`task reparse-corpora:check`): the same generator and the same checker, shared
as `tests/support/reparse_harness.rs`, run over the pinned gate corpora — \~6.3k
files against the fast suite's \~90 — with each file parsed under the
`LexConfig` its extension would get. It asserts the invariant and a per-driver
splice-rate **floor**, and records the exact tallies in
`tests/reparse_baselines/` as a two-sided ratchet in the shape of
`tests/gate_baselines/`. The floor and the record answer different questions:
every invariant assertion is vacuously true on a refusal, so a guard that
narrowed a tier to nothing would leave the sweep green while testing nothing
(panache's window cutoff cost its fuzzer two thirds of its coverage with every
assertion still passing), while the recorded tier columns catch the movement no
floor can see — a workload changing *tier*, which keeps every rate identical
because declining is always sound.

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

The formatter alone decides how a document should be laid out. It first lowers
the CST into a Wadler/Prettier-style `Doc` intermediate representation. A
separate printer then chooses between flat and broken forms according to the
available width. Keeping these steps separate lets lowering describe the
possible layouts without committing to a particular line break too early.

### It is whitespace-only

The layout engine may change trivia—whitespace, newlines, comments, and `.dtx`
margins and guards—but it never inserts, removes, or rewrites a non-trivia
token. In the usual case, lowering replaces each maximal run of whitespace and
newline trivia with a break primitive, leaving the printer to choose the line
break and indentation.

Meaning-preserving content rewrites therefore do not live here. Stripping
redundant braces around a single-token script (`x^{2}` → `x^2`) and rewriting
`$$…$$` → `\[…\]` are linter autofixes. This mirrors the fix-then-format rule:
just as the formatter never runs inside `--fix`, content rewrites never run
inside `format`. The payoff is a guarantee by construction, checked by the
non-trivia-content oracle, instead of a meaning-preservation argument defended
one fixture at a time.

The formatter may still change CST *shape*. Math operators are virtual atoms
inside a coalesced `WORD`, so inserting insignificant math whitespace makes the
output re-lex into separate leaves. The oracle compares the concatenated text of
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

The command-only-line rule is the only such exception inside the default
`Reflow` mode. Curated block commands carry a positive `CommandSig::block`
property and are laid out as block-level statements without consulting trivia,
so what the rule still decides is the authored break around a command whose
block-ness no signature tier can know — an un-signatured or scanned-definition
`\mymacro` on its own line — plus block commands glued to adjacent content.
Retiring that would glue every such authored line into the paragraph fill: a
policy change, not a fix. So the residue is sanctioned as Tier 2 on the argument
written at `line_is_command_only`: the rule is preservation-only, hardening gaps
that already hold a newline and never writing or moving a break, so a kept break
re-reads to itself in place, and a fill break it hardens on the next pass — a
width wrap that stranded a command alone on a printed line — coincides with the
break the first-fit fill chose, which refills identically around a hard stop.
The cost is by design: `--checks trivia-strict` still reports these shapes,
because preserving the authored break *is* the information the rule reads. One
scope limit keeps the residue honest: it does not fire inside a signature-proven
prose *argument* body (`ReflowKind::ProseArg`), where width alone owns the
layout — preserving a command-only line there mints a forced break only pass 2
can see, and that bit leaks upward through every `contains_forced_break` reader,
flipping the enclosing group between its inline and block forms across passes.

The last Tier-1 reader — the `Opaque`-group `spans_multiple_lines` choice, with
`lower_optional`'s fallbacks to the same — is retired. Under `Reflow` a brace
group is width-driven (`lower_opaque_group`): flat when it fits, byte-identical
to the generic inline path except that a lone-newline run renders as one space,
and first-fit wrapped at its authored gaps otherwise. Break opportunities are
exactly the perturbation-eligible gaps, so strict invariance holds by
construction; a glued junction never gains a break, and delimiter padding rides
the flat rendering and vanishes broken — exchanged for the delimiter's own
newline, never deleted, since an opaque argument's space tokens are typeset. An
authored newline immediately after a `LINE_BREAK` node is the one Tier-2 hard
boundary in a structurally plain, command-only text group: it remains a newline,
while inline and macro-like groups retain their ordinary fill and a same-line or
glued successor remains untouched. The preservation is a fixed point; a break
after every `LINE_BREAK` and the block-form delimiter framing are re-emitted, so
the same structural gate selects the same layout on the next pass. The narrow
shape avoids claiming that `\\` in opaque macro code is semantic. Virtual `.dtx`
documentation streams are excluded because a forced child break can escape
through their rebuilt `% ` margins and perturb structural framing on the next
pass. An edge gap joins that vanish-when-broken protocol only when its flat
spelling is a single space, the one spelling a break reproduces; any other
spelling rides verbatim and never breaks. An *interior* blank line, a direct
comment, a token embedding a newline, or a child carrying a forced break sends
the group to the indented block form instead — preserved predicates and content
only. An *edge* blank does not: the block form trims edge blanks away, so
declining on one would key on a predicate the emitter destroys, and it erases to
padding instead, matching the deletion the block form already performed. The
optional-argument lowering makes the mirrored promise: a
`segment_delimited_body` decline takes the block form unconditionally, and a
dropped trailing separator re-emits the authored whitespace it replaced. What
remains of `spans_multiple_lines` is the delimited-group residue behind the
non-`Reflow` modes and the doc-margined corner, sanctioned Tier 2 on the
fixed-point argument written at the predicate: the block form always ends with a
newline before its closer, so its output re-reads multi-line and re-blocks
byte-stably, and the inline path emits no newline, so single-line re-reads
single-line.

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
setting carries over to math for free. Under `Break`, the operator layout is
precedence-aware where TeX's `Bin` and `Rel` classes are too coarse: additive
operators such as `\pm` remain continuation points, `\cdot` stays with its
multiplicative term, and a top-level `\mid` keeps the following condition's
relations out of the equation-chain alignment. This may leave a cohesive term a
few columns over width rather than strand a short operator fragment.

### Statement bodies

Not every environment body is prose. A TikZ or pgfplots picture holds a sequence
of `;`-terminated path statements, and a greedy prose fill actively damages it:
it runs `\draw (0,0) -- (1,1);` and `\node at (0,0) {A};` onto one line, and at
a narrow enough width it splits a `\foreach` header away from its loop variables
(issue #114).

The curated `statementBody` flag in `data/signatures.json` names that family —
`tikzpicture`, `pgfpicture`, `scope`, `pgfonlayer`, and the pgfplots axis
environments — and a paragraph inside one is lowered under
`ReflowKind::Statement` instead of `ReflowKind::Prose`.

Statement boundaries are **structural**. The parser wraps each run of a
statement body up to a top-level `;` in a `STATEMENT` node (§ *Sanctioned lexer
modes*, the picture-body statement entry), and under `WrapMode::Reflow` the
formatter derives the layout from that node (`lower_statement`): one statement
per line — two statements on one authored line split, one authored across lines
joins when it fits — and **every continuation line hangs one indent step under
its head**, so a wrapped `\node[…] at (2,3)` / `{…};` reads as a continuation
rather than a sibling. The statement's interior reflows under
`ReflowKind::StatementInterior` (a lone newline is a plain atom boundary the
width fill re-decides; a comment still rides and ends its line; a `{label}`
block hangs as its own segment with a glued `;` riding its last line), and the
whole lowering is Tier 1: the hang is emitted, never read, and the node
re-derives from its `;` however the emitted layout breaks, so the hanging indent
is idempotent by structure — the property whose absence had deferred it (the
expl3 call unit is the same move made from the semantic side). A bound leading
documentation comment and a maximal leading run of comment-terminated
command-only macro invocations stay outside the hang at body indentation. Both
gates read comment presence rather than authored newline shape, and their forced
comment breaks reproduce the same prefix on the next pass; after non-command
statement content begins, a post-comment tail remains a hanging continuation. A
**glued** statement boundary (`…;\draw` with no gap) is never split; the
statement rides the previous line, the glued-divider principle. Content no `;`
terminates — a `\tikzset` line, a lone `\foreach` header — keeps the
authored-line rule: its own logical line, flush width wraps, the Tier-2
fixed-point argument unchanged. Every non-`Reflow` path splices the wrappers out
(`flatten_statements`) and behaves byte-identically to the pre-statement layout.

Breaks *inside* a statement come from the **TikZ unit model**
(`semantic::tikz::statement_glue`) — the vocabulary the extent node cannot
carry. It remains in the semantic layer because `(a)` as a coordinate versus a
node-name reference versus prose has no text-shape demotion, so a wrong reading
could not be gated in the grammar, while here it degrades to a worse break
choice, never a wrong tree (the same staging expl3 went through before its
attachment migration). The model is a glue map, not a grammar: for each authored
gap between a statement's top-level elements, one verdict — unit-internal (a
single space, never a break) or neutral. Its curated rules, each backed by a
survey of \~6000 statements across pgf's own manual sources and a user corpus: a
path operator binds forward (breaks land *before* operators, the \~3:1 idiom),
`at` binds both sides (split from its coordinate 5 times in 3103 continuation
lines), a coordinate binds its operation and an operation its argument
(`(6,6) circle (3)` never splits), a loose `[…]` options run glues except after
a comma (the keyval entry convention: `edge [loop above]` never splits an option
mid-phrase, while a long keyval run still breaks per entry), and a comment
suppresses every rule. Everything unrecognized — library verbs, axis prose—is
neutral and retains the ordinary layout. The wrap policy over the resulting
units is a plain greedy fill (the user-corpus lean; Tantau mixes styles).

One more claim rides the `statementBody` flag: whitespace *between* a picture
body's statements is insignificant to the package that consumes them, so a
statement always opens its own line **even at a seam the author glued**
(`…;\draw`). That is the one sanctioned breach of the glued-divider principle,
licensed the way `ContentKind::Keyval` licenses the glued comma split — a
curated whitespace-safety claim, held to the same standard and proven by a real
compile (`tests/typeset/statement_seams.tex`, `task typeset:check`). Glued seams
are unattested in the surveyed corpora, so in practice the license buys
uniformity: one statement per line, however the author spelled it.

Three things keep the flag narrow. It is **curated only**: a statement
terminator is package grammar, not a TeX-surface fact, so neither the CWL
codegen nor the runtime definition scan can set it. It is **distinct from
`code`**, which is the `.dtx` documentation layer's `macrocode` — a fact about
re-lexing under the package regime, not about layout; conflating the two would
hand a future `.dtx` consumer a `tikzpicture`. And it is read from the
**nearest** environment ancestor, never from any of them, so an `itemize` or a
`tabular` inside a `\node`'s label still reflows as the prose it is.

The same picture family is curated a second time in the linter
(`linter::rules::is_pgf_picture_environment`), which keeps `dash-length` off
coordinate arithmetic. Merging the two requires the effective signature scope to
reach `RuleContext`; its current lazy `user_definitions` database contains only
definitions scanned from the file, not built-ins, project declarations, or
loaded-package signatures.

### Reflow is safe by construction

Reflow safety cannot be inferred from a file extension. A `.sty` file may
contain ordinary prose that is safe to reflow, while a `.tex` or `.dtx` file may
contain structures whose whitespace is significant. Older versions selected
`Reflow` for `.tex` and `Preserve` for `.sty`, `.cls`, and `.dtx`; that merely
hid unsafe paths and still allowed an explicit `--wrap reflow` to corrupt a
document.

The safety is now structural, and every gate is independent of the wrap mode, so
an explicit `--wrap reflow` is exactly as safe as any other mode. A fully
margined, line-owning documentation environment is lowered as virtual LaTeX: its
`DOC_MARGIN` tokens remain in the CST, the formatter omits them while laying out
the environment, then applies `% ` to each generated content line and `%` to an
empty line. Such an environment composes as a self-margin-owning block inside a
documentation paragraph, so prose before and after it continues through the
ordinary margin-aware reflow without acquiring a second prefix. Alignment grids
consume that virtual stream recursively before measuring cells: physical margins
and their padding never enter a cell, nested continuation newlines collapse in
virtual coordinates, and the prefix-aware printer accounts for the documentation
margin only after the source columns are laid out. Guards, `macrocode`,
protected bodies, mixed-margin regions, and nodes that do not own their closing
line refuse this path. Other relayout arms refuse a node whose subtree carries a
`.dtx` margin or guard, because reflowing one can drop the `%` margin and on an
unmargined line a `^^A` doc comment re-lexes as content. A residual
margin-escape detector backs that up: when a probe-gated reflow would commit
content outside the margin, the paragraph re-lowers on the byte-faithful
preserve path. Never re-introduce a file-kind wrap default to paper over a
layout bug; fix the gate.

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

Commands are otherwise opaque to math lowering. A resolved signature containing
a positional `Math` domain opens only those matched brace or bracket groups to
recursive math formatting; the control word, trivia, `Text` and `Unknown` slots,
unmatched groups, and groups beyond declared arity remain byte-for-byte as
authored. Formatter signature precedence still applies, so a scanned
redefinition shadows the curated built-in with `Unknown` domains and restores
whole-command preservation.

The license for normalizing math whitespace is correspondingly narrow. It covers
ordinary catcode-10 whitespace delivered directly to a math list, where TeX
discards it. Math ancestry alone grants no such license: whitespace inside text
islands or arbitrary macro argument token lists can be preserved, inspected, or
replayed in a non-math mode. Code, keys, comments, and explicit spacing commands
are likewise outside the license. The formatter therefore crosses a command
boundary only for a signature-proven `Math` slot; every other argument retains
its authored whitespace. `tests/typeset/math_whitespace.tex` exercises both a
macro that preserves argument spaces and one that branches on them.

Math lowering consumes the shared virtual-atom view rather than parser token
boundaries. In direct math content and signature-proven `Math` slots, its policy
places one space around `Bin` and `Rel` atoms, preserves compound relation
spellings such as `<=`, `:=`, and `::=`, and treats a binary atom without a left
operand as unary. Subscript and superscript content instead keeps punctuation
operators compact throughout its nested math subtree (`i=1` and `n+1`) while
retaining spaces around control-word operators (`x \in A` and `x \leq y`).
Authored gaps at delimiter edges collapse away, so function application reads
`\Gamma(x)`. Other classes are operands for this stage; in particular,
unicode-math classifies ordinary `/` as `Ord`. The formatter preserves a fully
glued slash (`a/b`) but symmetrizes a gap on either side (`a/ b` or `a /b`) to
`a / b` in both policies. Delimiter depth and unary-after-opener detection use
the classifier's separate delimiter role, not parallel string tables. The
ellipsis lint consumes the same facts but applies its own policy.

### Conditionals

When the parser can pair a conditional, formatter layout is all-or-nothing:

- flat if the full construct fits;
- fully broken at dividers if it does not.

That keeps conditional formatting coherent and avoids newline-sensitive
behavior. The flat vs broken choice is computed from content, not authored
single-newline spelling.

There is one important safety carve-out: if any divider is glued in source
(`\ifmmode y\else z\fi`), we preserve authored bytes. Otherwise, splitting a
glued divider can change TeX-visible spacing even though CST trivia checks stay
green.

Conditional relayout runs only in wrap modes that already own prose layout.
`WrapMode::Preserve` keeps authored line breaks byte-faithfully.

Branch internals are lowered using the nearest non-conditional ancestor context
(paragraph-like contexts reflow; group-like contexts preserve). This avoids
oscillation in package-code patterns that depend on authored line structure.

Also note: a `DOC_COMMENT` may be reparented inside `CONDITIONAL`; lowering must
carry it through explicitly.

There is no body indent model because parser structure does not separate `\if`
test and body boundaries with enough certainty (see § *The conditional gate*).

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
statement breaks that way regardless of width. Since arity-directed attachment
landed, this is a decision the *tree* already answers: a recognized conditional
owns its branches as the head's trailing groups, whatever sat between —
`\tl_if_empty:nTF {#1} {T} {F}` and `\int_compare:nNnTF {a} = {1} {T} {F}` are
one shape — so the explosion reads the head node's own children, and the
unit-scoped rescan that re-split greedy sibling scatter is gone (the migration
oracle measured zero recognition disagreements, so a head the node cannot
resolve has no unit either). The statement-leading/trailing distinction survives
as pure layout policy: leading, the explosion is unconditional; trailing, it is
width-conditional.

### Line endings

The printer always builds output with `\n` and is the sole authority on where
breaks go. `FormatStyle::line_ending` decides only how those breaks are spelled,
as a pass over the finished text: `auto` (the default) follows the source, `lf`
and `crlf` are unconditional, and `native` follows the platform. `auto` is the
default so a CRLF repository does not get a whole-file diff the first time it is
formatted.

At the lexer boundary, CRLF is one physical end-of-line unit. This remains true
when a preceding backslash swallows the line ending into a `CONTROL_SYMBOL`: the
token spans both `\r` and `\n`, just as its LF counterpart spans the `\n`.
Keeping that unit atomic gives LF and CRLF the same token-kind and CST shape
while the lossless tree still preserves their original bytes.

This is the one carve-out in the protected-regions rule. A `verbatim` body is
emitted from source token text, so without a document-wide conversion a CRLF
document came out CRLF inside the protected region and LF everywhere else. Only
the `\r\n` and `\n` pair is touched; every other byte of the region is still
untouched.

### Comment directives

`badness_parser::directives` resolves suppression directives into sorted,
non-overlapping byte ranges (one list per axis). Formatter and linter both use
that shared resolution path.

The parser crate owns this logic because it is pure tree analysis needed by both
consumers (formatter in wasm-clean crate, linter in root crate).

Design rules:

1. **Verb defines scope.** `% badness-format ...`, `% badness-lint ...`, and
   `% badness ...` share one grammar, with `skip` / `off` / `on` / `skip-file`
   verbs.
2. **Legacy spellings still work.** `% badness-ignore ...` is deprecated but
   intentionally still supported.
3. **`.bib` uses a different carrier.** Directives are read from `@comment{...}`
   entries because BibTeX has no `%` line-comment token between entries.

The resolver also retains every recognized directive with its carrier range and
an outcome: honored, dangling `skip`, unmatched `on`, unclosed `off`, or
unsupported. This is the single source of truth for `inert-suppression`; lint
rules do not re-parse comments or repeat the CST attachment walk. Directive-like
text on a `.dtx` `DOC_MARGIN` line is retained as unsupported without creating a
suppression range, as is the format-only axis in a BibTeX `@comment` carrier.

Suppression matching is by **containment**, not overlap. This avoids accidental
"suppress the whole document" behavior when a region starts inside an ancestor
node.

Region anchoring follows `skip_target` semantics and is clamped to the previous
directive boundary. This keeps adjacent `off`/`on`/`off` sequences from merging
incorrectly.

Suppressed nodes are emitted as verbatim source for preservation. Indentation at
the first line may be normalized by placement, but interior bytes remain intact.

## The linter

The linter reads the same lossless CST as the formatter. Like the formatter, it
is a pure function of the input and data shipped with Badness; it does not
depend on ambient machine state. The user-facing catalog of built-in rules lives
in the reference section ([Linter Rules](../reference/linter-rules.md), [BibTeX
Linter Rules](../reference/bib-linter-rules.md)), generated from each rule's own
description and examples.

### Rules and dispatch

Every lint implements `Rule`, which is `Send + Sync` so the registry can be
shared across the LSP's read pool. A rule declares a stable kebab-case `id`, a
`default_severity`, whether it is enabled by default, the description and worked
examples that generate the rule reference, and whether it can ever emit a fix.

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
also precomputes two shared side indexes, one of effective mode and one of
`\if…\else…\fi` branch paths. The mode index partitions token ranges into
`Math`, `Text`, and `Unknown`: document content begins as text, explicit `MATH`
bodies override it, and command or environment arguments override their ambient
mode with the matched curated positional domain. Unknown commands, unmatched or
over-attached groups, and uncurated slots are `Unknown`; direct groups inherit.
Nested explicit math may in turn override a text island. Math-only rules require
`Math`, text-only rules require `Text`, and rules whose fix differs by mode skip
`Unknown`.

The registry compiles the rule list into a dispatch table indexed by
`SyntaxKind`, so node dispatch is a slice index, and it is cached across files
and shared by reference across the CLI's rayon lint phase. Configuration narrows
the active set as a post-filter, so the shared driver stays config-unaware. With
no `select`, resolution starts from rules whose `default_enabled` value is true;
an explicit `select` may choose any current rule, including an opt-in one.

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
re-glue the argument, so `x^{2}-3` stays braced. It also retains braces around
standard named math operators such as `\max`: those commands expand through
`\mathop`, which TeX cannot consume as an unbraced script field.

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

The language server has a slightly different boundary from the formatter. The
formatter is hermetic, but navigation necessarily depends on the user's local
project and TeX installation. The LSP may therefore consult read-only indexes
and metadata for editor features. That information must never flow back into
formatting or change the syntax tree.

The LSP is built on `lsp-server` and `lsp-types`, rust-analyzer's stack. Salsa
cancellation is a synchronous unwind that composes with `lsp-server`'s sync main
loop plus threadpool.

### The live buffer

An open document is a `text::TextBuffer`: the text as an `Arc<str>`, the
position encoding negotiated at `initialize`, and the `LineTable` over them,
built on first use behind a `OnceLock`. The main loop holds it as an
`Arc<TextBuffer>` and so does every buffer-carrying `WorkerJob`, which is what
makes a keystroke's fan-out cheap in both directions: capturing the buffer for a
job is a refcount bump rather than a copy of the document, and the table is
built once per document version rather than once per request, on whichever
thread asks first. The handlers that index the *cursor* buffer take
`&TextBuffer` and call `line_index()`; the ones that walk *other* project
members still build their own index per member, since those texts come off the
salsa snapshot and have no buffer.

The *table* and the *queries* are separate types, and the split is what makes
the table patchable. `LineTable` is the value — a line-start offset per line,
plus a flag per line for "holds a non-ASCII byte" — and `LineIndex<'a>` is the
short-lived pairing of a text with a table, borrowing one where a buffer
maintains it and scanning otherwise. So a query reads the text: a UTF-16 column
walks the one line concerned, and the flag is what keeps an ASCII line a plain
byte distance. Precomputing every wide character instead, which is the shape
this had, cost more to build than every conversion it ever answered, and it is
the shape that *cannot* be patched — a table keyed by line number has to be
rekeyed wholesale when the line count moves. The one hazard the split adds is
`LineIndex::with_table`, the single place a text and a table are paired: given a
table built for other bytes it answers wrong positions rather than panicking.

The buffer is immutable: an edit yields a new one rather than mutating in place.
That is not a cost, because an `Arc<str>` cannot be spliced in place anyway, and
it is what lets a job that captured the previous version keep reading a
consistent text and index with no lock. It also means the pointer identity is
meaningful, which is what the salsa-side staleness guards trade on (see
[Incrementality](#incrementality)).

The line table is **patched, not rebuilt**, across an edit. It was rebuilt for a
long time, and defensibly: the rescan was dwarfed by the full reparse every
keystroke paid, so splicing it would have been optimizing the wrong row. Once
both leaf tiers landed and the parse fell to \~37 µs, the rebuild *was* the row
— \~580 µs of a \~640 µs keystroke on the thesis, 52 copies of the document
where the two linear passes a splice needs would be 2-3.

`LineTable::patch` splices it instead. Line starts fall into three groups: those
before the edit are untouched, those after it keep their verdict and shift by
the byte delta, and those *at* its boundaries are re-derived from the edited
text. That third group is the whole subtlety, and it is why the patch cannot be
copied from fatou's. Badness treats a bare `\r` as a line break, so whether a
byte ends a line depends on the byte *after* it too — meaning an edit can split
or join a `\r\n` without touching either of its bytes. Inserting `x` into
`"a\r\nb"` at offset 2 gives `"a\rx\nb"`, which has a line the pre-edit table
did not. With `\n` alone the predicate reads one byte, a start at the edit
cannot flip, and the new breaks can be read straight out of the insert; here
both boundary positions have to be re-read out of the result.

Reuse is *structural* rather than cached. The table lives in the buffer and the
buffer is what an edit derives, so the pair travels together: nothing validates
a table against the text it describes, and one patch serves the write phase and
every read job off the same edit. Panache, whose index lives in a salsa memo
that every keystroke invalidates, needs a side cache keyed by document and an
`Arc::ptr_eq` to know whether an entry is still true — and because that cache is
main-thread-only, its readers still rebuild once per revision. A buffer with no
table yet stays without one, so a document nobody asks a positional question
about never pays; on the keystroke path there is always one, because
`apply_content_changes` resolves the change's range through `line_index()`
before splicing.

The write phase now costs **2.5 copies** of the document — 28 µs on the thesis
against 575 µs, with the keystroke at 91 µs end to end. Two of those copies are
the text rebuild an `Arc<str>` cannot avoid; the rest is cloning the table and
shifting its tail. A `debug_assert` rescans after every patch, which makes every
test in the suite that edits a buffer an oracle for it, and is also why
`task bench:keystroke-gate` — the row that watches all of this — must never be
run in a debug build.

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

**Bibliography resource lookup** (`project::bibliography`) keeps static command
extraction pure, then resolves missing literal `.bib` paths at the filesystem
boundary. A project-local file wins; plain `BIBINPUTS`/`TEXBIB` entries provide
a no-subprocess fallback, and `kpsewhich --progname=bibtex --format=bib` handles
the full Kpathsea grammar when available. The CLI loads the result only as a
citation dependency, while the language server publishes the written-to-actual
path alias as an explicit salsa input. Parser shape and citation queries
therefore remain independent of ambient environment state, and navigation
retains the real file location.

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
we measure against, not one we match. The CST also cannot prove that changing a
space leaves TeX's output alone: a key-value license can materialize a space
token, while an over-broad math license can rewrite one captured by a macro.
`task typeset:check` therefore compiles fixtures before and after formatting and
diffs the typeset output.

## Technology choices

The main dependencies follow directly from the architecture. Rowan provides the
lossless CST, while salsa manages incremental queries. Token text uses
[smol_str](https://docs.rs/smol_str), and [insta](https://insta.rs/) supplies
snapshot testing. Diagnostics are rendered with
[annotate-snippets](https://docs.rs/annotate-snippets), and the CLI is built
with [`clap`](https://docs.rs/clap). The root `build.rs` uses the clap model to
generate manual pages, shell completions, and Markdown documentation.

## Non-goals

Badness is not a TeX interpreter. It does not expand macros, execute primitives,
or implement `\def` semantics. It may extract common `\newcommand`,
`\newenvironment`, and xparse *signatures* into the semantic database, but it
never executes those definitions.

For the same reason, Badness does not attempt general `\catcode` evaluation. It
supports only the bounded, statically recognizable patterns listed under
[sanctioned lexer modes](#sanctioned-lexer-modes).

Badness does not typeset documents. It never runs `latexmk`, `pdflatex`, or any
other TeX engine, and it does not parse `.synctex.gz` files. Forward search is a
narrow exception in the language server: in response to an explicit user action,
it launches a viewer. That process is not a build step, and none of the
information it touches flows back into the formatter or linter.

The formatter never reads the environment. Its output is a function of the input
plus shipped data, and it resolves local `.sty` and `.cls` files sitting next to
the document rather than the installed TEXMF tree, so output cannot depend on
what happens to be installed.
