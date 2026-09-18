# Architecture

Badness parses LaTeX into a lossless concrete syntax tree (CST). Its formatter,
linter, and language server work from that tree, while a separate semantic layer
interprets commands and environments. This separation lets the parser preserve
source it cannot fully understand—a necessity for a language whose syntax can
change as a document runs.

The design follows [rust-analyzer](https://rust-analyzer.github.io/), with a
hand-written, error-tolerant parser, a
[rowan](https://github.com/rust-analyzer/rowan) tree, and
[salsa](https://github.com/salsa-rs/salsa) queries for incremental
recomputation. [arity](https://github.com/jolars/arity), a similar tool for R,
was another influence. Build and test instructions live in
[Contributing](contributing.md).

## From source to editor features

The parser reads a flat token stream and emits events for a separate tree
builder:

```text
text → lexer → token stream → parser → event stream → tree_builder → GreenNode
```

`Start`, `Tok(idx)`, and `Finish` events describe the tree without constructing
it during parsing. Diagnostics travel separately, keyed by byte range. The tree
builder feeds the events into rowan and attaches trivia, retaining every byte of
the source. A specialized `SubTok` event lets math parsing split a lexer token
when TeX binds a script to a single character.

The formatter lowers the tree to a `Doc` intermediate representation and prints
it. The linter collects diagnostics through a shared traversal. The language
server uses salsa queries to combine syntax and semantic information across
files.

A tree depends only on source text and explicit project declarations. Package
scope, the runtime signature database, and the filesystem cannot change its
shape. Parsing can therefore be repeated or cached without depending on the
machine that happens to run it. Badness does not execute macros or typeset
documents.

## The crates

The Cargo workspace contains four crates. The root crate, `badness`, owns the
CLI, linter, language server, configuration, and project discovery. It handles
filesystem access and passes resolved inputs to the libraries.

`badness-parser` contains the LaTeX and BibTeX syntax trees, parsers, typed AST
wrappers, and semantic models. Its `data/` directory holds signature data, which
`build.rs` turns into PHF tables. `badness-formatter` depends on that crate and
provides the layout engine and both formatters. `badness-wasm` is an unpublished
wasm-bindgen wrapper that powers the [playground](../playground/index.html).

Both library crates build for `wasm32-unknown-unknown`, so their runtime code
cannot use the filesystem, threads, or child processes. The formatter also runs
in the [dprint plugin](https://github.com/jolars/dprint-plugin-badness). The
plugin uses an empty runtime signature database, whereas the CLI can supply
signatures scanned from neighboring `.sty` and `.cls` files. This accounts for
an intentional difference between the two formatting entry points.

The root crate re-exports the libraries through modules such as `src/parser.rs`
to preserve existing import paths. Its `formatter` and `semantic` modules also
provide the disk-backed entry points that cannot live in the libraries.

## The BibTeX side

BibTeX has a separate pipeline in `bib/`, with its own grammar, lexer, syntax
kinds, typed AST, and semantic model. It follows the same lossless event-stream
architecture as LaTeX and supports formatting, linting, completion, and document
outlines.

### `%` comments in `.bib`

Badness follows biber's interpretation of `%` inside an entry: it begins a line
comment between fields or value components, but remains ordinary text inside a
braced or quoted value. Thus `title = {50% off}` retains its percent sign.
Classic BibTeX rejects comments inside entries, and texlab does not model them;
the latter difference is recorded in `bib_parse_compat_allowlist.toml`.

The lexer always emits a bare `PERCENT` token. The grammar knows whether it is
reading a value or skipping trivia, so it decides whether to wrap the rest of
the line in a `COMMENT` node. Braced groups, quoted strings, `@comment` bodies,
and top-level junk retain `%` as ordinary text.

A percent sign inside a value also matters to formatting. BibTeX passes it to
LaTeX, where it becomes a comment and makes the following line break
significant. The formatter therefore preserves values containing an unescaped
`%` byte for byte. Comparing syntax trees would not catch an unsafe line join
here: the bibliography would still parse, but typeset differently.

Comments between fields stay attached to their surroundings when fields are
sorted. A trailing comment remains on its field's line, while an own-line
comment moves with the following field. Comments after the final field appear
above the closing delimiter. If an entry has nowhere suitable to place a
comment, as can happen with `@string`, `@preamble`, or an entry without fields,
the formatter preserves the entire block.

## Inputs and configuration

The CLI discovers `.tex`, `.sty`, `.cls`, `.dtx`, `.ins`, and `.bib` files
through [`ignore`](https://docs.rs/ignore), respecting `.gitignore` and project
excludes. It finds `badness.toml` by walking each input's ancestors. Filesystem
discovery and configuration resolution stay in the root crate; the libraries
receive values such as `FormatStyle` and resolved declarations.

The file kind determines the lexer's starting mode. Package sources (`.sty`,
`.cls`, and `.dtx`) begin with `@` treated as a letter, as though
`\makeatletter` had already run. Document sources do not. Every file kind uses
`WrapMode::Reflow` by default because [reflow
safety](#reflow-is-safe-by-construction) depends on the source's structure, not
its extension.

Project configuration controls formatting, lint selection, build-artifact
locations, declarations, and excludes. Machine settings, such as the TeX
installation and PDF viewer, belong in editor configuration. The language server
merges editor settings with project settings and distinguishes an unset option
from an explicit value.

The server caches resolved configuration by document directory. Before reusing
an entry, it checks the existence, modification time, and length of the config
files examined by the ancestor walk, including any fallback. This detects a
changed or deleted config and a newly created nearer one even when the editor
cannot register file watchers. Watcher notifications provide earlier
invalidation when available.

### Declarations

Declarations let a project describe constructs whose meaning cannot be inferred
reliably from source. Command declarations assign reference or citation
semantics:

```toml
[commands.eqrefs]
like = "cref"

[commands.mycite]
like = "parencite"
```

Here, `eqrefs` takes `cref`'s comma-separated reference keys, and `mycite` takes
`parencite`'s citation behavior. These declarations affect analysis and
completion. They do not declare arity, change argument attachment, or select
formatter layouts.

Environment declarations can assign built-in behavior or introduce delimiter
spellings:

```toml
[environments.myenv]
like = "align"

[environments.eqnarray]
begin = ['\bea']
end = ['\eea']

[environments.mytheorem]
like = "theorem"
begin = ['\startmyenv']
end = ['\endmyenv']
```

An alias may supply just one delimiter because a literal `\begin{X}` or
`\end{X}` can supply the other. TOML literal strings avoid escaping the
backslash, which is optional in these spellings.

Environment declarations enter the parser through `ParseCtx`. The parser sees
only the resolved declarations, never a full `SignatureDb`, and still checks
whether delimiters can pair structurally. A declaration supplies a spelling; it
cannot force an impossible pair into the tree.

The `like` targets come from closed, curated tables. Environment declarations
copy a built-in environment signature; command declarations copy a reference or
citation family. Neither resolves against CWL data or scanned definitions, and
neither exposes an argument specification. Config loading rejects unknown
targets, empty entries, invalid spellings, and collisions. Tables are keyed by
category and name so layered configuration can merge individual declarations.
Once validated, declared entries take precedence over scanned and built-in
entries.

Both declaration categories share a high-durability salsa input, but reach their
readers through separate queries: `parse_declarations` and
`semantic_declarations`. Each query retains its previous result when its subset
is unchanged. Renaming a citation alias therefore updates semantics without
invalidating parses or their reparse caches. The LSP publishes declarations in
the request dispatcher so switching between workspace roots cannot leave a
handler using another project's configuration.

## Syntax and semantics

The syntax tree records what the source contains. The semantic layer interprets
it using curated built-ins, CWL-derived signatures, and definitions scanned from
source. Keeping those responsibilities separate is especially useful for generic
LaTeX arguments: `\foo{a}{b}` might be a two-argument call or a command followed
by two independent groups. The parser preserves the groups; semantics can
interpret them when a signature is available.

Some curated or declared facts do influence parsing, but only when the source
can disprove them. A delimiter alias, for example, must pass a shape gate before
it can open an environment. If no valid closer is reachable, it remains an
ordinary command. Generic arity cannot be checked this way: an incorrect arity
can produce a lossless tree with the wrong attachment. It therefore stays out of
the parser.

Semantic lookup can depend on the enclosing environment.
`SignatureDb::command_at` searches the nearest environment with a local entry in
`environmentCommands`, then falls back to the global signature. In exam's
`parts` environment, for example, `\part` accepts an optional points argument
and has no sectioning role. The linter, outline, and label context share this
interpretation, including in question files without a class declaration.
Environment headers and closers retain their surrounding meaning. These local
entries do not change parser grouping or formatter layout.

Facts that authorize a rewrite need stronger evidence than facts used for
completion. `ContentKind::Keyval` permits breaks after commas that had no
following whitespace, so a wrong classification can change typeset output.
Likewise, the curated `labelKey` flag says that an environment's first optional
argument can define a label. This cannot be inferred merely from key-value
syntax: different processors can give a key named `label` different meanings.
The semantic model accepts flat literal values, processes repeated entries in
order, and treats a later dynamic value as unknown. A separate
`captionContainer` flag identifies non-float environments in which `\captionof`
can own a preceding label. Keeping these claims separate prevents one layout or
lint classification from silently granting another.

## The parser

The parser uses recursive descent over a flat token stream. It always produces a
lossless tree, including for malformed source. When it cannot establish a
construct's structure statically, it leaves that construct generic rather than
trying to execute TeX.

### Sanctioned lexer modes

TeX lets source change how later characters are read. Badness recognizes a
bounded set of common patterns where the source gives enough evidence to select
a lexer mode. These modes cover package code, expl3, verbatim material, and
literate `.dtx` sources without attempting general `\catcode` evaluation.

`\makeatletter` makes `@` a letter. `\ExplSyntaxOn` and `\ProvidesExpl*`
declarations enable expl3, where `_` and `:` are letters. The flags are
independent and can be active together. In `.dtx` files, a `%<@@=…>` guard or
`\ProvidesExpl*` declaration anywhere in the file enables expl3 catcodes in
every `macrocode` body.

Verbatim commands and environments capture their bodies as single tokens.
Curated signatures describe built-ins, while a bounded two-pass definition scan
recognizes user definitions from catcode-changing patterns and known definers
such as `\lstnewenvironment`. A command can also have one positional verbatim
argument: `\href` captures its URL but leaves its visible text parsed. Such a
capture requires the expected balanced group, and a local redefinition
suppresses a colliding built-in mode. Short-verb declarations such as
`\MakeShortVerb{\|}` allow `|…|` on one line to form an opaque token; `.dtx`
mode and curated documentation classes enable `|` initially.

Definition bodies need different treatment from running document text. Inside
curated command and environment definers, `\begin` and `\end` remain ordinary
commands because a replacement body need not balance them. A control-symbol name
in a definition, such as `\DeclareRobustCommand\[`, is definition data and
cannot open display math. Expl3 regions likewise pass environment delimiters
around as data, so the parser leaves them as commands and accepts an orphan `\]`
without a diagnostic.

A `.dtx` `macrocode` body ends only at its literal frame line. Unmatched braces
inside a chunk remain plain tokens because a definition can open a brace in one
chunk and close it in another. The lexer also recognizes line-leading `%<…>`
guards and `^^A` comments on documentation-margin lines. These distinctions
matter later when the formatter reconstructs the documentation's `%` margins.

Other bounded rules prevent ordinary TeX data from becoming structure. The lexer
isolates the delimiter after `\left` or `\right`. In numeric contexts, it
recognizes backtick character constants, so ``\char`$`` cannot open math. It
also recognizes escaped character constants that occupy a whole alignment cell.

### Shape gates

Before opening some constructs, the parser scans ahead to check that its normal
walk can consume them. These checks are called shape gates. They keep delimiters
used as macro data from swallowing unrelated source. A failed gate usually
leaves ordinary syntax without a diagnostic: parser diagnostics can prevent
formatting, so routine macro patterns should not trigger them.

For `$`, `\[`, and `\(`, a gate requires a reachable closer before an unbalanced
brace, paragraph break, or end of file. Environment pairing asks a different
question: would the environment escape the brace group in which it began? An
escaping `}` demotes the opener, but reaching EOF does not. This preserves the
useful diagnostic for a document with a missing `\end`.

Gates share `Parser::gate_batch`, which can settle several openers during one
scan instead of repeatedly scanning nested source. Each policy must match the
walk it guards, including recovery anchors, brace handling, and `.dtx` frames.
State that can change an answer, such as the enclosing math flavor, belongs in
the memoization key. `DollarGate` is not memoized because demoting `$$` resumes
parsing at its second dollar sign, where the same token position can pose a
different question.

Most pairing gates count nested openers and environments independently.
`LeftRightGate` instead uses one stack of brace, environment, and `\left`
frames, because their order determines whether a `\right` can match. A frame
mismatch invalidates outer pairs as well. Its anchors are the delimiters that
end the surrounding math body; `\left` and `\right` themselves remain
recognizable in definition and macrocode bodies, where package math commonly
uses them.

Bracket gates reflect the fact that `[` and `]` are ordinary TeX characters.
They attach an optional argument only when its closer is reachable along the
optional-argument parse path. A command-adjacent nested `[` can claim a closer,
which then cannot close the outer argument. Environment delimiters and paragraph
breaks normally stop the scan at any brace depth. The narrow long-text exception
allows a paragraph break when both ends have the tight shape `\cmd[…]{…}`: the
bracket directly follows the command, and its closer directly precedes a
mandatory group. Signature arity does not supply this evidence.

In math, bracket gates also account for the enclosing delimiter. Within display
math, a balanced `$…$` can be nested inside an optional argument. Within inline
`$…$`, a dollar sign at the bracket's own level ends the surrounding math and
prevents attachment. Macrocode brackets ignore braces already classified as
unmatched chunk data, but stop at structural closing braces and respect the
chunk's frame.

All gates use the same interpretation of docstrip trivia. A guard-only line
interrupts a run of newlines because docstrip removes that line entirely. A
documentation margin, by contrast, leaves the line in the documentation stream,
so a margin-only line can still form a paragraph break.

### Environment aliases

A definition whose entire replacement body is `\begin{X}` or `\end{X}` can
supply an environment alias. This lets `\bea … \eea` form the same environment
shape as `\begin{eqnarray} … \end{eqnarray}`. An alias for one delimiter may
pair with a literal spelling of the other. Projects can also provide aliases
through [declarations](#declarations).

Inference is limited to the current file and curated target environments.
Definitions themselves cannot open alias environments, and a positive gate must
find a reachable closer before an alias can pair. More complicated definitions,
such as argument-taking aliases or unresolved `\let` chains, remain generic.

Consumers resolve an alias from the parsed node. Looking only at raw spelling
would confuse a command alias `\bea` with the unrelated environment name in
`\begin{bea}`. Literal and alias delimiters share a target lookup, including in
math parsing, while retaining separate closer indexes.

### The conditional gate

A complete `\if … \else/\or … \fi` becomes a `CONDITIONAL` node with positional
branches. Recognition uses a curated opener model shared with the linter,
excluding `if*` macro families and definition operands that are not live control
flow. The gate requires a reachable `\fi` at the appropriate brace, environment,
and math nesting levels. It respects macrocode frames, stops at paragraph breaks
at the conditional's own level, and does not recognize conditionals in expl3
regions.

The node establishes the conditional's extent, but does not claim to separate
its test from its body. TeX's scanners determine that boundary, and a static
parser cannot identify it reliably. Even `ast::Conditional::closer` is fallible:
a nested opener can be demoted when the walk applies its gate, allowing the walk
to finish before the outer scan's predicted closer.

### Recursive descent, with Pratt local to math

Text parsing has no precedence rules. Math uses local precedence handling for
script binding and `\left…\right` structure, but does not build an arithmetic
expression tree. Ordinary characters, including arithmetic operators, stay in
coalesced `WORD` tokens until a script requires a finer boundary.

For example, `a,b^2` attaches the superscript to `b`. An unbraced script
argument also consumes just one input character, so `x^23_i` becomes `x^2`
followed by `3_i`. The parser emits byte-range sub-tokens for these boundaries.
Elsewhere, a semantic view presents one virtual math atom per Unicode scalar
without changing the CST.

The atom classifier combines generated unicode-math data with curated overrides.
It supplies TeX spacing classes and a separate delimiter role. That separation
matters because an atom's spacing class does not prove it can pair: `\sqrt`'s
`Open` class, for example, must not enter bracket accounting. Commands and
characters share the lookup, and unknown commands default conservatively to
`Ord`.

Curated environment signatures can select math parsing for an entire body. Math
parsing and alignment layout remain separate claims: `gathered` is math, while
`aligned` is both math and an alignment. A wrapper such as `empheq` can
therefore parse as math while the formatter derives its grid layout from the
body's `&` and `\\` structure.

### Argument grouping and bracket policy

The parser greedily attaches trailing brace and bracket groups, subject to the
bracket gates. The semantic layer later interprets that attachment using the
available arity. A lone `*` directly after a command and before an argument can
join the command as a starred-variant marker.

Curated built-in slots may refine how a group's contents are parsed through
`ArgumentDomain::Math` or `Text`. A positional matcher skips omitted optionals
and aligns the attached groups with those slots. A matched math group uses the
math parser; unknown, unmatched, and excess groups use generic parsing.
Attachment remains greedy, and CWL data, scanned definitions, and project
declarations cannot supply these domains.

Verbatim slots require a related distinction. A verbatim argument is captured by
the lexer, preventing characters such as `%` from becoming syntax.
`ContentKind::Opaque`, by contrast, tells the formatter to preserve whitespace
in an already parsed group. A companion slot matcher accounts for captured
arguments so later groups retain their proper positions.

Expl3 provides an argument specification in the command spelling itself. The
parser can therefore attach `\tl_set:Nn \l_a {x}` according to `Nn`, keeping
`\l_a` and `{x}` as arguments of the same call. Greedy attachment would instead
attach `{x}` to `\l_a`. A token scan in `grammar/expl3.rs` plans the attachment,
and the walk replays that plan. Control-sequence arguments keep their own bare
`COMMAND` nodes; groups remain ordinary `GROUP` nodes.

Underivable heads, including `w` and `D` forms, colonless names, and `\::n`
expansion drivers, retain greedy grouping. The scan also declines when math,
lexer-mode changes, docstrip boundaries, or unreachable closers prevent it from
matching the walk. A blank line inside a brace group allows the consumed prefix
to be committed. Matching braces are indexed once per frame so nested calls do
not repeatedly scan the same groups.

Attachment mistakes can survive both losslessness and formatter idempotence
checks. An independent oracle therefore compares grammar attachment with
`semantic::expl3` argument consumption. The semantic model also handles calls
that fall back to greedy parsing. Texlab has no argspec model, so expl3 regions
are allowlisted in the differential gauge.

### Trivia attachment

Trivia normally belongs to the nearest enclosing node. A contiguous run of
own-line `%` comments immediately before a command or environment instead binds
to it as a `DOC_COMMENT`. A trailing comment stays where it is, and a blank line
breaks the forward attachment.

LaTeX has no separate documentation-comment marker like Rust's `///`. Stopping
at blank lines keeps a license header from becoming documentation for the first
command. Node kind alone determines which constructs can receive comments;
attachment does not consult signatures.

### Error recovery

An error produces a diagnostic alongside the tree. The parser recovers at
structural anchors such as `\begin`, `\end{…}`, a blank line, `}`, `$`, `&`, and
`\\`, always consuming input rather than looping on an unexpected token.

Losslessness tests reconstruct arbitrary UTF-8 and generated malformed syntax
through both parsers. LaTeX cases cover documents, packages, `.dtx` sources, and
explicit declarations. These tests require byte-for-byte reconstruction, not an
absence of diagnostics. Curated snapshots and differential comparisons with
texlab check the structure that reconstruction alone cannot establish.

### Incrementality

Salsa tracks dependencies across files and queries. Its database stores rowan
`GreenNode`s and creates red cursors on demand, because red trees cannot be
shared across threads. `parsed_document` stores green trees under
`no_eq, unsafe(non_salsa_values)`; parsing remains a pure function of source and
declarations.

Source text is an `Arc<str>`, shared by the live buffer, worker jobs, salsa, and
read snapshots. Passing a document between them increments a reference count.
`upsert_file` avoids a salsa write when text is unchanged, and `text_is_current`
checks whether a read job still describes the current buffer. Both compare
pointers first, then contents: a disk reread may have equal text in a new
allocation.

Inputs have durability levels that reflect how often they change. Text has `LOW`
durability, paths and declarations have `HIGH`, and the `ProjectFiles`
membership input has `MEDIUM`. Typing can then invalidate text-dependent queries
without making stable project data appear changed.

`workspace_project` derives an equality-comparable project value from
`ProjectFiles`. Keyless queries use it for include and package graphs, label and
citation resolution, package options, and signature scopes. Membership belongs
in an input so each database snapshot contains the right file set. Keeping one
current project value also avoids retaining interned copies of every historical
membership and their dependent query results.

### Intra-file reparse

Within a changed file, `parser::reparse` can reuse most of the previous green
tree. It tries progressively broader edits: replacing one ordinary token,
replacing a protected body token, reparsing a math node, or reparsing a limited
prose region. Each tier uses the ordinary lexer or parser and must establish
that the edit cannot change structure outside its replacement. If none can do
so, the caller parses the whole file.

A successful reparse must produce exactly the same green tree and `SyntaxError`
vector as a full parse of the edited text. Every failed proof returns `None`.
This lets guards remain conservative: an unfamiliar construct costs a full parse
without weakening correctness. The implementation does not resume from lexer
checkpoints or preserve grammar restart state.

#### Token and protected-body splices

The token tier relexes a leaf and checks that it remains one token of the same
kind. Probes on either side check that it still separates from its neighbors. In
`\foo1ab`, changing `1ab` to `aab` would merge the two tokens into `\fooaab`, so
that edit cannot use a leaf splice. The tier also rules out changes to the
definition scan and to any grammar decision that reads the token's text.

A source-scanning test tracks those text reads in the grammar and lexer
predicates. Each read must be unreachable from a spliced leaf or protected by a
guard. This covers facts such as a statement's terminating semicolon, a starred
command marker, and an environment name. Newline edits and definition-sensitive
positions decline, as do oversized boundary probes.

Math leaves need an additional check because script parsing can split one lexer
`WORD` into several CST leaves. The tier reconstructs the coalesced word and
checks its outer boundaries. A leaf that supplies a script or its base must
remain one Unicode scalar; an unscripted prefix or remainder may change length.
An edit that moves this partition goes to the math tier.

Protected bodies cannot be relexed alone: the lexer needs the opener to enter
verbatim mode. That tier therefore relexes the whole enclosing node, including
its delimiters. The unedited fragment must reproduce its original tokens, and
the edited fragment must reproduce the same sequence with only the body token
changed. The closer must be present inside the fragment, ruling out an
unterminated capture. This catches an inserted `\end{verbatim}` or an unbalanced
brace in a captured URL without duplicating the lexer's capture rules.

The locality argument also depends on raw body bytes bypassing lexer state
updates. Lexer tests check that property and the counterexample where breaking a
capture changes later lexing. Newlines are safe within an intact capture, so
pressing Enter in a listing can still use a leaf splice. The replacement shares
every green node outside the path from the leaf to the root.

Fragment lexing uses the base parse's `ParseCtx` and full-file `.dtx` facts,
including implicit expl3 mode. An edit that changes a full-file signal declines.
Reproducing the old fragment does not authorize inventing missing entry state.

#### Math and prose regions

A math edit that changes shape reparses the outermost enclosing `INLINE_MATH`,
`DISPLAY_MATH`, or math environment. Its delimiters establish math mode and
include the enclosing gates that the edit could invalidate. An isolated parse
must first reproduce the old node. The edited parse must then yield one node of
the same kind spanning the whole fragment, and a right-boundary probe checks
that recovery cannot consume the unchanged suffix.

This tier admits state-neutral math syntax. Control sequences, comments,
environment names, definition-sensitive positions, lexer-mode ambiguity, and
`.dtx` source decline. Fragment diagnostics are replaced, prefix diagnostics are
retained, and suffix diagnostics shift with the edit. A base whose diagnostics
are out of source order declines because a local splice cannot reproduce a
global recovery-stack reorder.

The region tier covers two prose shapes: an edit across several direct leaves of
one top-level paragraph, and an edit to the blank-line seam between two
paragraphs. It first reparses the old fragment under the base's exact context.
Edits may touch only direct prose or trivia and insert no structural or
catcode-sensitive spelling. Unchanged commands can remain within the fragment
because their state transitions are unchanged.

A single paragraph uses a node splice. Removing a seam may merge two paragraphs,
so that case rebuilds `ROOT` from shared green children. Both paths replace
fragment diagnostics and shift those after it. Seam splicing declines `.dtx`
until its column-sensitive documentation layer can be accounted for.

Blank lines alone do not prove that arbitrary regions are independent. A wider
region tier would also need to account for forward-looking gates outside the
fragment, verify boundaries against unchanged neighbors, and show that the
replacement's tokens and diagnostics compose with untouched siblings. The
current restrictions provide that locality without additional parser state.

#### The reparse cache and edit chain

The previous text, tree, diagnostics, and pending edits live beside salsa,
accessed through the `IncrementalDb` reparse methods. They cannot be ordinary
inputs: invalidating the previous parse on every write would discard the value
needed for reuse. Reading this cache inside `parsed_document` is sound because
it cannot change the query's result. A missing, stale, or evicted entry merely
causes a full parse.

The cache stores a new base only after every fallible step has completed, so a
panic or cancellation cannot leave mismatched text and tree. It drains the
consumed prefix of the edit chain rather than clearing the whole chain, because
another edit may arrive during parsing. That prefix is drained even if reuse
fails: once the base changes, those edits no longer describe a transformation
from it. Eviction favors entries that have already benefited from reparsing,
preventing a project-wide query from displacing every open buffer with files
parsed only once.

The language server supplies edits directly. `apply_content_changes` returns the
clamped byte offsets it used, with each edit relative to the text produced by
its predecessors. A whole-buffer replacement returns `None`. Disk reloads and
other writes without an edit chain also force a full parse; the query does not
spend time rediscovering edits by diffing whole texts.

Every `upsert_file` is followed by `reparse_stage_edits`, passing the available
chain or `None`. Staging must follow the write so an in-flight query cannot see
edits for text that has not arrived. It must also happen when the write is
skipped: a buffer can return to the text salsa already holds while still having
an edit chain relative to the older reparse base.

#### Reparse validation

Every tier returns through a shared `finish` function. In debug builds, it
compares the result with a full parse. In every build, it checks that the tree
spans exactly the edited text and falls back if the length differs. Seeded
hazard cases and corpus edits check both the tree and diagnostics; deliberate
failures test that the oracles detect divergence.

`task reparse-corpora:check` runs the shared harness in
`tests/support/reparse_harness.rs` across the pinned corpora, using each file's
normal lexer configuration. It checks a minimum splice rate and compares exact
per-tier tallies against `tests/reparse_baselines/`. Both are needed: a tier
that refuses every edit can pass every equivalence assertion, and a move between
tiers can leave the overall splice rate unchanged.

The direct benchmark in `benches/reparse.rs` declares the required tier and a
speedup floor for each case, comparing it with a full parse in a release build.
`task bench:gate` runs these checks alongside the end-to-end keystroke
benchmark, which also measures buffer updates. Together they test correctness,
useful coverage, and whether reuse actually saves work.

### Typed AST wrappers

Thin `AstNode` and `AstToken` wrappers provide typed, read-only access to the
rowan tree. Their accessors describe positions and tolerate greedy attachment.
They do not consult signatures: a `Command::title()` accessor would be
misleading because `\section` and `\newcommand` share the same syntax kind.

The formatter uses wrappers for field access and raw nodes for structural
dispatch. Meaning remains in the semantic layer, where callers have the context
to interpret an attached group.

## The formatter

The formatter lowers the CST into a Wadler/Prettier-style `Doc` intermediate
representation. A separate printer chooses flat or broken forms according to the
available width. Lowering can thus describe the possible layouts without
committing to a line break before the printer knows the current column.

### Whitespace and content

The formatter changes trivia while preserving non-trivia content. It normally
replaces whitespace runs with break primitives, then lets the printer choose
spaces, line breaks, and indentation. Comments and protected regions retain
their contents, apart from configured line-ending normalization.

Content rewrites belong to linter fixes. Removing script braces from `x^{2}` or
replacing `$$…$$` with `\[…\]` requires a separate meaning-preservation
argument. Keeping such edits out of formatting makes its content guarantee
checkable independently of individual layout rules. Fix application likewise
does not invoke the formatter.

Formatting can change token boundaries and CST shape. Inserting insignificant
math whitespace, for example, splits a coalesced `WORD` into several tokens. The
content oracle therefore compares concatenated non-trivia text rather than token
boundaries. Parse stability is not a formatter invariant.

### Trivia-invariant layout

Idempotence requires the second formatting pass to make the same layout choices
as the first. That becomes difficult if a rule reads a distinction the formatter
can erase or create. A lone newline is the usual example: reflow can turn
`alpha\nbeta` into `alpha beta`, or insert a newline when a line exceeds the
width. Treating that newline as evidence of a structural boundary lets the first
pass change the second pass's decision.

Ordinary layout rules therefore read only trivia properties their output
preserves: blank-line presence, comment presence and own-line status, and `.dtx`
margins or guards at column zero. If a predicate satisfies `P(fmt(x)) == P(x)`,
using it cannot by itself make the next pass choose a different layout.

The `Gap` type enforces this distinction at the lowering boundary. Its variants
are `Glued`, `Space { flat }`, `Blank`, and `Comment`; there is no `Newline`
variant. A lone newline and a single space have the same flat rendering. Wider
authored whitespace can remain in `flat` only because its readers reproduce it
unchanged. Width calculations and break selection use this normalized view.

Some policies intentionally preserve authored lines. They use `WideGap`, which
also exposes newline counts, and need a fixed-point argument for every layout
they can emit. These Tier 2 cases include preservation modes, unresolved
statement layouts, and a few narrow rules within reflow. A preservation rule has
a straightforward argument when it re-emits a newline in the same place: the
next pass sees the same break and preserves it again.

The command-only-line rule is one such case. Curated block commands have a
positive signature property, but an unknown `\mymacro` may still occupy its own
authored line. The rule preserves breaks around that line without moving them.
If width wrapping strands a command on its own line, preserving that break on
the next pass agrees with the original fill. The rule is excluded from
signature-proven prose arguments, where a newly forced child break could instead
change the enclosing group's layout.

Opaque groups under `Reflow` are otherwise width-driven: they stay flat when
they fit and wrap at existing gaps when they do not. A glued junction cannot
acquire a break. Delimiter padding can become a newline only when its flat
spelling is the single space that newline reproduces. Interior blank lines,
comments, embedded newlines, and forced child breaks select a block form; edge
blank lines cannot select it because that form trims them away.

A narrow exception preserves a newline after `\\` in a structurally plain,
command-only text group. Its block framing and row breaks recur on the next
pass, giving the rule a fixed point. Macro-like groups and virtual `.dtx`
documentation streams do not use it. Outside ordinary reflow, delimited-group
rules may also retain the single-line versus multiline distinction when each
emitted form preserves that distinction.

`formatter::perturb` tests these properties by varying trivia without changing
TeX content. `check_trivia_convergence` requires every variant to reach a fixed
point, parse cleanly, reconstruct losslessly, and retain its non-trivia content.
`check_trivia_invariance` asks the stronger question of whether every variant
formats identically. The latter also reports intentional line-preserving cases,
so `badness debug format --checks trivia-strict` serves as a survey rather than
a universal pass condition.

### Paragraph line breaks

`WrapMode` determines how the printer handles paragraph breaks. The default,
`Reflow`, fills lines to the configured width. `Stable` keeps acceptable
authored breaks while balancing overflow, changes, displacement, and raggedness.
`Preserve` keeps authored breaks. `Sentence` places one sentence on each line,
while `Semantic` additionally ends a line at each authored newline. The last two
modes ignore width. Sentence detection uses language-specific abbreviation
profiles resolved from the format configuration.

Display math has a separate `MathWrap` setting for single-formula bodies, whose
default follows the effective paragraph mode. Its breaking policy keeps
multiplicative terms together and allows continuations at additive operators. A
top-level `\mid` separates the following condition from equation-chain
alignment. These choices can leave a cohesive term slightly over width instead
of separating a short operator fragment from it.

### Statement bodies

A TikZ picture contains statements rather than prose. The curated
`statementBody` flag identifies this behavior in the TikZ and pgfplots families.
The parser wraps runs ending at a top-level semicolon in `STATEMENT` nodes,
supplying the extent the formatter needs without parsing the full path language.

Under `Reflow`, each statement starts a line, and its continuation lines hang
one indentation step beneath the head. The semicolon determines the statement on
every pass, so width wrapping cannot change that boundary. Leading comments and
comment-terminated command-only prefixes stay at body indentation. Content
without a terminating semicolon retains its authored-line policy. Other wrap
modes flatten the statement wrappers before laying out the body.

Breaks within a statement use `semantic::tikz::statement_glue`. It marks gaps
that belong inside a unit, keeping a path operator with its argument, `at` with
its surrounding operands, and a coordinate with its operation. Option lists can
break after commas but retain phrases such as `loop above`. Comments stop these
rules, and unrecognized syntax uses ordinary layout. This model belongs in
semantics because a coordinate-like spelling can also be a node name or prose;
interpreting it need not change the syntax tree.

The flag also asserts that whitespace between statements is insignificant. The
formatter can therefore split even a glued `…;\draw` boundary. This is a
specific permission to introduce whitespace, checked by
`tests/typeset/statement_seams.tex`. Only curated signatures and declarations
that inherit them can supply the claim. The nearest environment determines the
body policy, so an `itemize` nested inside a node label retains its own layout.
`statementBody` remains separate from `code`, which identifies `.dtx` macrocode
and its lexer regime.

### Reflow is safe by construction

A file extension cannot establish whether reflow is safe. Package files can
contain ordinary prose, and document files can contain macro code whose spaces
matter. Each formatting path instead checks the structure it is about to lay
out, independently of the selected wrap mode.

Fully margined documentation environments in `.dtx` can be formatted as virtual
LaTeX. Their `DOC_MARGIN` tokens remain in the CST, but lowering omits them
while laying out content and restores `% ` on generated content lines and `%` on
empty lines. Such a block owns its margins, so surrounding documentation prose
does not add a second prefix. Alignment grids measure the virtual content before
the printer accounts for the margin.

This path requires the environment to own its closing line. Guards, macrocode,
protected bodies, and mixed margins prevent entry. Other layout paths also
refuse subtrees whose margins or guards they cannot preserve. Dropping a margin
could turn a `^^A` documentation comment into source content. A final detector
checks whether reflow has allowed content to escape its margin and, if so,
re-lowers the paragraph with byte-faithful preservation. The literal framing
lines around macrocode remain intact.

### Optional arguments, tables, and math spacing

An optional argument can use a grouped layout over its top-level comma-separated
entries: flat when it fits, one entry per line otherwise. Width selects the
form; the number of keys or a trailing comma does not force expansion.

A comma followed by authored whitespace supplies a break opportunity. Breaking
after a glued comma introduces a TeX space token, so it requires a signature
that identifies the argument as a key-value list. The same permission lets
mandatory groups in commands such as `\pgfkeys`, `\tikzset`, and `\lstset` use
the segmented layout. Nested groups protect their internal commas. Because
mandatory groups often hold typeset text, this classification must come from
curated signatures; mechanical CWL `%keyvals` marks cannot establish it for a
brace group.

Table layout is also formatter-owned. The renderer reads static column
specifications such as `{lcr}` and aligns cells accordingly, falling back to
left alignment for specifications it cannot model. The curated `align` flag
normally selects grid layout, but a top-level `&` can establish an alignment in
an otherwise unknown environment.

Math whitespace has a different safety argument: TeX discards ordinary
catcode-10 whitespace delivered directly to a math list. That does not make all
whitespace beneath a math node insignificant. A macro can inspect spaces in its
arguments or replay them as text. The formatter therefore enters a command's
arguments only at signature-proven `Math` slots, leaving text, unknown,
unmatched, and excess arguments unchanged. A scanned redefinition shadows a
built-in with unknown domains and restores preservation. Typeset fixtures test
macros that preserve argument spaces or branch on them.

Within direct math content and proven math slots, lowering uses the shared
virtual-atom view. It spaces binary and relation operators, retains compound
relations such as `:=`, and treats a binary atom without a left operand as
unary. Scripts keep punctuation operators compact, as in `i=1`, while retaining
spaces around control-word operators. Delimiter-edge gaps disappear, yielding
`\Gamma(x)`. A fully glued slash stays glued; a gap on either side becomes
symmetric, as in `a / b`. The atom classifier's delimiter role supplies nesting
information shared with the linter.

### Conditionals

A paired conditional stays flat if the whole construct fits and breaks at all
dividers otherwise. If any divider is glued to adjacent content, as in
`\ifmmode y\else z\fi`, the formatter preserves the authored bytes because
adding a space can change TeX's output. `WrapMode::Preserve` also retains the
original line breaks.

Branch contents use the nearest non-conditional ancestor's policy: prose
contexts reflow, while group-like contexts preserve. Documentation comments
remain part of the lowered content even when parsing attaches them inside the
conditional. There is no separate body indentation because the CST does not
establish where the conditional's test ends and its body begins.

### expl3 code formatting

Within expl3, spaces and tabs are ignored characters and `~` supplies a space
token. The formatter can therefore control inter-token whitespace independently
of `WrapMode`. Its layout follows the LaTeX Project's style guide: separate
steps on separate lines, consistent spacing and brace placement, and indentation
that reveals the call structure. Naming and expandability rules belong to the
linter.

A derivable argspec identifies a call and the elements its slots consume. That
unit supplies a structural statement boundary that survives width wrapping. When
the argument scan cannot resolve a call, the formatter falls back to its
authored physical line and preserves that boundary on subsequent passes.

The lexer and formatter share toggle-name recognition, but the formatter also
requires a toggle to be a top-level statement before taking control of layout. A
toggle mentioned as data may never execute. The lexer's broader recognition can
still produce a lossless tree, whereas formatting that region as expl3 could
remove meaningful spaces.

Expl3 conditionals expose their `T` and `F` branches as attached groups. A
conditional at the start of a statement places each branch on its own line, one
indentation step beneath the call. A trailing conditional expands only when
width requires it. Both decisions read the same attached structure, including
calls with intervening single-token arguments such as
`\int_compare:nNnTF {a} = {1} {T} {F}`.

### Line endings

The printer builds output with `\n`. A final pass applies
`FormatStyle::line_ending`: `auto` follows the source, `lf` and `crlf` select a
fixed spelling, and `native` follows the platform. Keeping this separate from
layout prevents line-ending preferences from influencing break placement.

The lexer treats CRLF as one physical line ending, including when a backslash
captures it in a `CONTROL_SYMBOL`. LF and CRLF thus produce the same token-kind
structure while their trees retain the original bytes.

Line-ending normalization also applies to protected regions. Otherwise a CRLF
source could retain CRLF inside verbatim bodies while the printer emitted LF
everywhere else. Only line terminators change; the rest of each protected region
is preserved.

### Comment directives

`badness_parser::directives` resolves suppression comments into sorted,
non-overlapping byte ranges for formatting and linting. The shared resolver
lives in the parser crate because it is pure tree analysis needed by both
consumers.

`% badness-format`, `% badness-lint`, and `% badness` select formatting,
linting, or both. They share the verbs `skip`, `off`, `on`, and `skip-file`. The
retired `% badness-ignore` spelling remains supported. BibTeX carries lint
directives in `@comment{...}` entries because `%` does not form a line comment
between entries.

The resolver records each directive's range and outcome, including dangling
`skip`, unmatched `on`, unclosed `off`, and unsupported forms. The
`inert-suppression` rule reads these results instead of parsing comments again.
Directive-like text on `.dtx` documentation margins, and format directives in
BibTeX carriers, are recorded as unsupported without suppressing anything.

Suppression requires containment in the resolved range. Mere overlap could
otherwise suppress an ancestor containing most of the document. Anchors follow
`skip_target` and are clamped at the previous directive boundary, keeping
adjacent regions distinct. The formatter emits suppressed nodes from source;
placement may adjust first-line indentation, but their interior bytes remain
intact.

## The linter

The linter reads the shared CST and semantic model without consulting ambient
machine state. Each rule supplies the description and examples used to generate
the [LaTeX](../reference/linter-rules.md) and
[BibTeX](../reference/bib-linter-rules.md) rule references.

### Rules and dispatch

A `Rule` declares a stable kebab-case identifier, severity, whether it is
enabled by default, and whether it can emit a fix. Rules are `Send + Sync`, so
the same registry can serve parallel CLI work and the language server's read
pool.

Rules share one traversal. A node rule subscribes to syntax kinds and runs when
the driver encounters matching elements. A whole-file rule runs after the walk,
using semantic or project information. A streaming rule receives elements in
document order, which suits checks that track a toggle or the preceding heading.
The registry compiles node subscriptions into a dispatch table indexed by
`SyntaxKind` and reuses it across files. Rule selection and ignores are applied
as a post-filter, keeping configuration out of the driver.

`RuleContext` assembles the file's tree, semantic model, and available project
resolution. Missing cross-file information is represented by `None`, leaving the
corresponding rules inactive. It also shares indexes for conditional branch
paths and effective text or math mode, so rules do not derive them repeatedly.

The mode index partitions token ranges into `Math`, `Text`, and `Unknown`.
Explicit math establishes math mode, and curated positional argument domains can
override the surrounding mode. Unknown commands, unmatched groups, and uncurated
slots remain unknown. A rule that needs math requires `Math`; one that needs
text requires `Text`. A fix whose meaning changes by mode must skip unknown
regions.

Signature-dependent rules are similarly conservative. For example,
`missing-required-argument` reads curated signatures, including
environment-local meanings, and skips names redefined in the file. Bulk CWL
arities and ambient package discovery cannot establish that a required argument
is missing.

### expl3 semantic checks

Expl3 rules share a lazy `Expl3Index` in `RuleContext`. It reads attached
arguments through typed accessors and semantic slot shapes, retaining the raw
argspec letters needed to check variant compatibility. A command-shaped node
alone does not establish that the command executes.

The index starts in top-level code and enters recognized unexpanded definition
bodies and trailing `T`/`F` branches. Other arguments stay opaque. Incomplete
calls and expansion wrappers leave subsequent sibling consumption unknown. In
`.dtx`, only macrocode bodies count as code, including implicit expl3 regions
identified by colon-bearing control words.

Within known definitions, a source-mapped token view tracks parameter counts and
removes one level of doubled hashes per enclosing definition. Outer parameter
substitutions remain unknown. This distinguishes `##5` in a nested message from
the surrounding function's `#5` without expanding macros. Character-level spans
also distinguish parameter five followed by `1` in `#51`.

The shared facts support checks for incompatible variants, protected predicate
definitions, and invalid message parameters. These rules report warnings without
fixes because the intended signature or message is unknown.

### Autofixes

A `Fix` contains one or more edits applied atomically, including across files.
The apply engine is a pure function of source, fixes, and applicability flags,
shared by the CLI and editor code actions. It rejects malformed or overlapping
fixes. `lint --fix` repeats linting and application until it reaches a fixed
point.

A fix must stand on its own as a raw edit: formatting will not run inside its
application to repair spacing or attachment. A rule can report a problem while
withholding a fix when the source does not justify a safe rewrite. For example,
`redundant-script-braces` retains braces where removal could change binding, and
around operators such as `\max`, whose `\mathop` expansion cannot serve as an
unbraced script field.

`Safe` fixes preserve meaning and are eligible for `lint --fix`. Fixes that may
change typeset output are `Unsafe` and require `--unsafe-fixes` or an explicit
editor code action. Neither kind is responsible for satisfying line width.
Inline suppression uses the shared [comment directives](#comment-directives),
with an optional rule name to narrow the scope.

## The language server

Editor navigation depends on the local project and TeX installation, so the
language server can read metadata that the parser and formatter cannot. It keeps
that information separate from parser shape and formatter signature resolution.
The server uses `lsp-server` and `lsp-types`, with a synchronous main loop and
thread pool that accommodate salsa's unwind-based cancellation.

### The live buffer

An open document is an immutable `TextBuffer` containing an `Arc<str>`, the
negotiated position encoding, and a lazily initialized `LineTable`. The main
loop and worker jobs share it through `Arc<TextBuffer>`. Each job therefore
keeps a consistent text and index even if a later edit has already produced a
new buffer. Handlers use `line_index()` to share the table for that document
version.

`LineTable` stores line-start offsets and a flag identifying lines with
non-ASCII bytes. `LineIndex` pairs that table with its source text and answers
position queries. ASCII columns are byte distances; UTF-16 queries scan the
relevant line when necessary. The table must always describe the paired text,
since a mismatched pair can return incorrect positions without failing.

When an edit produces a new buffer, `LineTable::patch` updates an initialized
table. Starts before the edit remain unchanged, starts after it shift by the
byte delta, and the boundaries are rescanned. Badness treats bare `\r` as a line
ending, so boundary checks must account for a CRLF pair being split or joined.
Inserting `x` into `a\r\nb` between `\r` and `\n`, for example, creates an
additional line. A table that has not yet been requested stays lazy. Debug
builds compare every patched table with a fresh scan.

Read jobs check the captured text against the database through
`text_is_current`. Worker writes are processed in order; only text-free analysis
requests can coalesce. The edit chain used for incremental parsing travels with
the buffer update, as described under [Intra-file reparse](#intra-file-reparse).

### Project and installation data

Shipped CTAN metadata maps package names to descriptions and catalog identifiers
for hover and completion. The optional TEXMF index adds installed `.sty`,
`.cls`, and `.dtx` files for links, definitions, and completion. It discovers
roots with `kpsewhich -var-value` and caches the index under a distribution
fingerprint. The index is controlled by editor settings and never supplies
formatter signatures.

Existing `.aux` files provide label numbers and table-of-contents entries. A
dedicated line scanner reads them and follows `\@input` chains; the LaTeX parser
is unsuitable because these files are written under `\makeatletter`.
Modification time and length detect fresh compile results without requiring a
watcher. This data enriches label hover and document symbols, while the
formatter remains independent of it.

Bibliography resolution separates pure extraction of resource names from path
lookup. A local file wins. Plain `BIBINPUTS` and `TEXBIB` entries provide a
fallback, and `kpsewhich --progname=bibtex --format=bib` can resolve the full
Kpathsea grammar. The CLI loads the result as a citation dependency. The LSP
publishes the mapping from the written path to the actual path as an explicit
salsa input, so queries depend on resolved data rather than reading the
environment themselves.

Citation completion returns the bibliography namespace with a `filterText`
containing each key, title, and author list. The client can then match any of
those fields using standard LSP filtering.

Badness does not run TeX engines or parse `.synctex.gz`. Forward search launches
a configured PDF viewer in response to a user action, without blocking a read
worker while the viewer runs. Filesystem paths and document URIs pass through
`uri_to_fs_path` and `path_to_uri` to retain Windows drive handling.

## Validation

The parser and formatter have separate correctness obligations. Parser tests
require byte-for-byte reconstruction even for malformed input. Formatter tests
require unchanged non-trivia content, preserved protected regions, and
idempotence. Incremental reparsing adds exact agreement with a full parse,
including diagnostics. These properties are checked together where the
subsystems meet.

Two broader checks address what those invariants cannot prove. The texlab
differential oracle compares simplified tree structures over real source
corpora. Differences need an explanation, but texlab is a reference rather than
a required byte target. Typeset fixtures test whether whitespace changes alter
TeX's output, something CST comparisons cannot determine. `task typeset:check`
compiles fixtures before and after formatting and compares the results. It is
required when changing key-value signature behavior or optional-argument
lowering and runs separately from default CI.
