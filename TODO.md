# badness --- Roadmap

A LaTeX formatter, linter, and language server on a lossless rowan CST,
mirroring **arity** (`../arity`, the same tool for R). See `AGENTS.md` for
load-bearing design decisions, invariants, and the copy-from-arity strategy.

Single-crate package (not a workspace). Parser and formatter are **intentionally
interleaved**: the formatter is the primary tool for stress-testing the parser.

Files marked **\[copy\]** are lifted \~wholesale from arity; **\[rewrite\]** are
LaTeX-specific; **\[diverge\]** intentionally differs from arity.

Status: `[ ]` todo · `[~]` in progress · `[x]` done

--------------------------------------------------------------------------------

## Where we are

The foundation is complete: a lossless, error-tolerant recursive-descent parser
over a rowan CST; `badness format` (parse → Wadler IR → print) with whitespace
normalization, environment + group/argument indentation, paragraph reflow, a
structured math model with alignment-aware column formatting; salsa
incrementality + a semantic layer (label/ref model, signature DB, project
include graph); a minimal salsa-backed LSP; and a linter with a rule layer wired
into both the CLI and LSP. A full BibTeX/BibLaTeX pipeline (parser, formatter,
linter, LSP) ships alongside.

Work below is organized **by area**. Use formatter ambiguities to drive parser
fixes (AGENTS.md tenet 3). The differential oracle --- texlab (parse) --- remains
available as a hardening track throughout.

--------------------------------------------------------------------------------

## Parser

Done: event-stream recursive descent → green tree; side-channel diagnostics;
paragraphs, control sequences, groups, comments, environments (with mismatch
recovery), greedy argument grouping; `\verb`/verbatim lexer modes (incl.
argument-taking `lstlisting`/`minted`/`Verbatim`, skipping `\begin` args via the
signature DB); `\makeatletter` letter-mode; recovery anchors + progress
guarantee; losslessness asserted; structured math model (`MATH` nodes, atoms,
precedence-climbing `^`/`_`, `\left…\right` matching with a delimiter-isolation
lexer mode); texlab differential parse oracle; block-vs-inline refinement (a
lone block env is not `PARAGRAPH`-wrapped, via the signature DB `block` flag);
trivia attachment per AGENTS.md decision #9 (rust-analyzer rule, grammar-local
leading comment-bind, blank line breaks the bind).

No open parser items; new parser work is driven by formatter ambiguities and the
package-infrastructure section below.

## Formatter

Done: `badness format` (parse → Wadler IR → print); **\[copy\]** IR + printer
engine; whitespace normalization, environment + group/argument indentation
(printer-owned, idempotent); paragraph reflow (`WrapMode`, `Ir::Fill`, default
`Reflow`); prose-argument reflow (signature-DB `prose` flag --- commands with
the signature-DB `inline` flag like `\footnote`/`\emph` flatten into the
surrounding fill so the body wraps as running text with `{`/`}` glued to
adjacent words; block-level prose commands `\section`/`\caption` block-break
their braces via a soft `Ir::group`); aggressive math lowering (collapse
spacing, tight scripts, strip redundant single-token script braces); display
math (`\[…\]`/`$$…$$`) lowered as an indented block with delimiters on their own
lines, breaking a too-wide body before its top-level binary/relation operators
(amsmath style: the first relation anchors a hanging indent via `Ir::Align`,
later operators start continuation lines aligned under the first term after it;
a curated operator-name table classifies relations vs. binaries, unary `+`/`-`
excluded, comment-bearing bodies take the plain path); `\left…\right` spacing;
alignment-aware `align`/matrix column grids **and** text tables
(`tabular`/`tabular*`/`array`), carrying interspersed comments and
horizontal-rule commands (`\hline`/`\midrule`/… via a `rule` flag) as
passthrough lines that never count toward column widths; list environments
(signature-DB `list` flag --- `itemize`/`enumerate`/`description` --- one
`\item` per line, each body reflowed with continuation lines hanging-indented
under the item text via `Ir::Align`); collapsible token-list arguments
(signature-DB `collapse` flag --- the cite family's key list folds a multi-line
authored form to one line, never width-reflowed, with the `inline` flag flowing
the command into the paragraph fill; bails to the block form on a blank line, a
`%` comment, or force-break content). Protected regions untouched; idempotence +
losslessness asserted.

- [ ] `Sentence`/`Semantic` (sembr) wrap modes --- both fall back to `Preserve`
  today. *Demoted, much later.*
- [ ] **Argument content-kind taxonomy.** `prose`/`collapse` are two ad-hoc
  bools on `ArgSpec`; the real model is a per-argument *content kind*
  (opaque, token-list, prose, document-body) the formatter dispatches
  whitespace and break policy on. Generalize once a third case appears. The
  non-determinism fix (`spans_multiple_lines` deciding block-vs-inline from
  incidental source newlines) is sidestepped for collapse-flagged args but
  still governs every *unflagged* multi-line group --- revisit when the
  taxonomy lands.
- [ ] **Long collapsed cite list overflow.** A `collapse` arg folds to one line
  even when the key list exceeds the width; it never breaks *at commas* (one
  key per line) as a fallback. Needs the token-list content kind to break on
  its own separators rather than the paragraph fill.
- [ ] Mark `\ref`/`\eqref`/`\cref`/`\autoref`/`\nameref` `inline` so they flow
  too (left out of this pass: their keys are single tokens where interior
  spaces can matter, so they are *not* `collapse`, but they are still
  inline).
- [ ] Widen the prose-argument table (CWL ingest could feed it); consider gluing
  a prose arg onto its command line when a source break separates them.
- [ ] Join alignment-cell continuation lines (currently triggers the plain-body
  fallback).
- [ ] Column-spec-aware L/C/R cell alignment and `\multicolumn` for the table
  environments (`tabular`/`array` are now grid-aligned, but every column is
  left-aligned regardless of its `{lcr}` spec). Also: `\cmidrule(lr){2-3}`
  paren trim specs (the parenthesized part isn't recognized as part of the
  rule line, so such a line is treated as a cell and the table falls back),
  and the same-line `\\ \hline` form (only own-line rule commands become
  passthrough lines today).

## Linter

Done: `badness lint` + `linter/{diagnostic,render}` surfacing parse diagnostics
(annotate-snippets render); rule layer (`linter/{rules,check}`, `Rule` trait +
registry) wired into the CLI and the LSP `publishDiagnostics` path;
`linter/suppression` (`% badness-ignore`); a **single shared walk** dispatch
(arity's `run_rules` shape --- `by_kind` table from each rule's `interests()`,
one `descendants_with_tokens` traversal; model/cross-file rules implement
`check_file()`); the autofix infra (`linter/fix.rs`: `Fix` +
`Applicability::{Safe, Unsafe}`, a pure `apply_fixes` engine, `check_document`
fixpoint, `lint --fix`/`--unsafe-fixes`, with the
`format → fix → format`-idempotent harness enforcing Tenet 5). Lints shipped:
`deprecated-command` (`\bf`-style), `obsolete-environment` (`eqnarray` → `align`),
`dollar-display-math` (with a `\[…\]` autofix), `mismatched-delimiter`,
single-file + cross-file `duplicate-label`, `undefined-ref`.

- [ ] Wire the remaining report-only fixes onto the autofix infra:
  `deprecated-command`'s `\bf → \bfseries` (the natural first one) and
  `obsolete-environment`'s `eqnarray → align`.
- [ ] More stylistic lints: missing `~` before `\cite`/`\ref`, typography.
- [ ] `unused-label` (cross-file) --- deferred: can false-positive on labels
  referenced from outside the analyzed set.

## Semantic layer & signatures

Done: `semantic_model` (flat label/ref def-use model, `Eq`-backdating); built-in
signature DB (`data/signatures.json`); project include graph
(`\input`/`\include`/`\import`/`\subfile`, salsa firewall +
reachability/cycles); `\newcommand`/`\newenvironment`/`xparse` signature
scanning (`semantic/define.rs`, `semantic/xparse.rs`; scanned overlaid over
built-in; consumed by the formatter's `\begin` arity glue), incl. the unbraced
`\newcommand\foo…` form; **cross-file label + citation resolution**
(`project/{labels,citations}.rs`, undirected-component namespace, root-gated
`undefined-ref`/`undefined-citation`); **verbatim-argument commands and
environments** (DB `verbatim`/`verbatim_body` flags driving lexer modes, plus
**user-definition scanning** in `semantic/define.rs` --- a `\newcommand`/xparse/`\def`
body that reassigns a special char's catcode to "other", directly or through a
helper chain, flags the command/environment verbatim; consumed by a bounded
two-pass parse; AGENTS.md decision #1); the `document_signatures` salsa query
(consumed by completion); **LSP cross-file project assembly** (salsa `Project`
from open buffers + on-disk siblings, `resolved_labels`/`resolved_citations`,
path-keyed salsa, `.bib` tracked as inputs).

- [ ] CWL corpus ingest (an import format converted *into* the signature schema)
  once ecosystem breadth (e.g. LSP completion) needs it.
- [ ] How much of `\newcommand` / `xparse` to model for the signature DB. *(open
  decision)*

## Language server

Done: `src/lsp.rs` + `badness lsp` (single-threaded, salsa-backed `lsp-server`
loop **\[diverge\]**); ra-style threading (main loop / sole-writer worker / read
pool, `decide`-scheduled analyze with supersede-on-newer-edit); lifecycle,
incremental text sync (`apply_content_changes` UTF-16 splice),
`textDocument/formatting`, `publishDiagnostics` (parse + lint, version-gated);
cached-tree reuse (`compute_format` → `format_node`); `EditorSettings` over
`initializationOptions` + `didChangeConfiguration`; document symbols
(`textDocument/documentSymbol`, nested outline from the signature DB's
`sectioning` levels + float/theorem-like environments + labels, built by
`semantic::outline`); stdio smoke test.

arity (`../arity/src/lsp.rs`) is the feature template: it ships formatting,
range formatting, code actions, hover, definition, references, document
highlight, document symbol, and prepare-rename/rename. Most of these map
directly onto badness's existing semantic layer.

### Configuration & sync

- [ ] config over LSP --- today `EditorSettings` carries only
  `line_width`/`indent_width`; `wrap` is hardcoded `Reflow`. Plumb
  `WrapMode` (and any future format knobs) through `EditorSettings` →
  `FormatStyle`, keeping the namespaced/bare parsing.
- [ ] Pull diagnostics (`textDocument/diagnostic` + `workspace/diagnostic`) as a
  capability alongside the current push model, for clients that prefer it.
- [ ] `workspace/didChangeWatchedFiles` + dynamic `client/registerCapability`
  for `**/*.{tex,bib}` so on-disk edits to non-open includes/`.bib` files (the
  project graph's leaves) reanalyze --- the deferred follow-up to LSP project
  assembly (re-read + re-upsert + `RelintAll`).

### Formatting

- [ ] Range formatting (`textDocument/rangeFormatting`) --- format the smallest
  enclosing node(s) covering the selection; clamp to node boundaries so a
  partial selection never corrupts the tree. Mirror arity's
  `on_range_formatting`.
- [ ] On-type formatting (`textDocument/onTypeFormatting`), e.g. re-indent on
  `}`/`\end{…}` close. *Lower priority; opt-in trigger characters.*

### Navigation & structure

- [x] Folding ranges (`textDocument/foldingRange`) --- environments, sectioning
  spans, and long comment blocks.
- [ ] Selection ranges (`textDocument/selectionRange`) --- expand-selection from
  the CST's node hierarchy (group → argument → command → environment).
- [ ] Workspace symbols (`workspace/symbol`) --- labels and sectioning titles
  across the project include graph.

### Labels & references (def-use model)

- [x] Go-to-definition (`textDocument/definition`) --- refs jump to their
  `\label`, cite-family commands to their `.bib` entry; cross-file via the
  include graph. *Follow-up:* a multi-key list command (`\cref{a,b}`,
  `\cite{a,b}`) shares one command range, so the cursor resolves *every* key;
  per-key sub-ranges await the deferred `LabelRef`/`CitationRef` range split
  (see `semantic::label`).
- [x] Find references (`textDocument/references`) --- all uses of a label or
  cite key across the namespace, invokable from a use site *or* a definition site
  (the `\label`, and an `@entry` key in a `.bib`); honors `includeDeclaration`.
  Inverts go-to-def via new `namespace_members`/`bib_citers` resolver accessors
  (`src/project/{labels,citations}.rs`). *Follow-up:* per-key sub-ranges for
  multi-key list commands still deferred (shared with go-to-def, see
  `semantic::label`).
- [ ] Document highlight (`textDocument/documentHighlight`) --- highlight a
  label and its refs within the file.
- [ ] Rename (`textDocument/rename` + `prepareRename`) --- rename a label and
  every referencing command atomically; project-wide via the include graph.
  Restrict the prepare range to label/ref key tokens.

### IntelliSense (signature DB)

- [ ] Hover (`textDocument/hover`) --- command/environment signature (arity, arg
  kinds, sectioning level), the resolving `\label` for a ref, and the
  `\newcommand`/`xparse` definition for user-defined macros.
- [x] Completion (`textDocument/completion`) --- command/environment names from
  the signature DB (built-in + scanned, via `document_signatures`), `\ref`-family
  keys, `\begin{…}`/`\end{…}` names (auto-`\end` snippet), and file paths in
  `\includegraphics`/`\input`/…/`\addbibresource`. (`src/completion.rs`.)
  - [ ] `\cite` key completion --- classify a `\cite`-family argument cursor,
    offer keys from the resolved bibliography (citation model + `.bib` ingest
    already exist; needs the wiring in `completion.rs`).
  - [ ] CWL ingest to widen command/environment name coverage.
  - [ ] `completionItem/resolve` to attach signature/doc detail lazily
    (`resolve_provider` is currently `false`).
- [ ] Signature help (`textDocument/signatureHelp`) --- show the active argument
  while typing a command's `{…}`/`[…]` arguments.

### Code actions (autofixes)

- [ ] Code actions (`textDocument/codeAction`) surfacing linter autofixes
  (Tenet 5: fixes must be format-clean by construction). `deprecated-command`'s
  `\bf → \bfseries` is the natural first quick-fix; wire
  `CodeActionKind::QUICKFIX` + a resolve path mirroring arity's
  `on_code_action`.

### Infrastructure

- [ ] Client capability negotiation --- gate advertised providers and
  UTF-8/UTF-16 position encoding on what `initialize` reports.
- [ ] README editor-wiring docs (Neovim/VS Code `initializationOptions`,
  `badness lsp` invocation).

## Package & class infrastructure (`.sty` / `.cls` / `.dtx` / `.ins`)

The document-level tools are mature; the next frontier is the **package
ecosystem** --- class and package sources, and the literate `.dtx` format they
ship in. This is a large, multi-area subproject (parser + formatter + semantic).
It stays inside the AGENTS.md non-goals: bounded, statically-recognizable
patterns only, signatures *extracted, never executed*, no docstrip run, no TeX
engine. Local project files only --- a `texmf`/CTAN/`kpsewhich` search is out of
scope (the same boundary the include graph and CWL ingest keep).

### Parsing

- [x] **File-kind detection (`.sty`/`.cls`).** `FileKind` gained `Sty`/`Cls`,
  threaded through file discovery, the CLI, and the LSP the way the `.bib` kind
  already is (`FileKind::is_latex`/`latex_flavor`/`default_wrap` route them
  through the LaTeX pipeline). `.dtx`/`.ins` kinds remain (deferred with the
  docstrip work below).
- [x] **`@`-as-letter for `.sty`/`.cls`.** The package loader does
  `\makeatletter` implicitly, so `@` is a letter throughout these files. The
  lexer starts in letter-mode for these kinds via the `LatexFlavor::Package`
  flavor (a static, extension-driven catcode fact --- sanctioned exactly like
  the explicit `\makeatletter` mode, decision #1); a trailing `\makeatother`
  still applies.
- [x] **expl3 (LaTeX3) syntax mode (letters, explicit toggles).** `\ExplSyntaxOn`
  … `\ExplSyntaxOff` make `_` and `:` catcode-11 *letters*, so `\seq_new:N`,
  `\tl_set:Nn`, `\__module_internal:nn` lex as single control words (and a bare `_`
  is text, not a subscript). A sanctioned lexer mode like `\makeatletter`/`\verb`
  (decision #1) --- it reads only the static fact "we are inside an expl3 region",
  resolves no macro meaning. An independent boolean flag threaded like `at_letter`;
  composes with the `@`-as-letter mode (the `@@` convention `\g_@@_x_tl` needs
  both). Opened for the rest of the file by `\ProvidesExplPackage`/
  `\ProvidesExplClass`/`\ProvidesExplFile` (handled left-to-right as an
  `\ExplSyntaxOn`). Without it every expl3 control word mis-lexes (the word stops at
  the first `_`/`:`), corrupting argument grouping and the downstream signature scan
  --- a prerequisite for parsing modern packages.
- [ ] **expl3 full catcode model (deferred).** Model `~` as a literal space
  (catcode 10) and spaces/tabs as ignored (catcode 9) inside expl3 regions. Formatter
  territory (insignificant-whitespace reflow), beyond the letters-only mechanism above.
- [ ] **expl3 implicit detection in toggle-less `.dtx` (deferred).** Real expl3
  package sources (e.g. `ltx-talk-structure.dtx`) carry no in-file `\ExplSyntaxOn`/
  `\ProvidesExpl*`; expl3 is declared in the parent `.dtx`/build, and `@@` is a
  docstrip module prefix (`%<@@=mod>`). Treat `macrocode` bodies as expl3 when the
  file carries a static expl3 signal (a `%<@@=mod>` guard or `\ProvidesExpl*`
  anywhere). Needs a file-level scan plus the `macrocode` save/restore interaction
  (mirror `at_letter`).
- [ ] **`.ins` installation scripts.** Recognize the kind; share the docstrip
  guard syntax with `.dtx` (see below). They are docstrip drivers
  (`\input docstrip`, `\generate{\file{…}{\from{…}{…}}}`, `\endbatchfile`) ---
  parse + format as code (`WrapMode::Preserve`), never run the extraction.
- **`.dtx`/`.ins` docstrip surface syntax.** A distinct literate format that
  interleaves two layers: a documentation margin (lines whose leading `%` is a
  comment margin) and code (lines with no leading `%`). Implemented as a bounded
  line-oriented lexer mode (`LexConfig.dtx`, sanctioned by decision #1), reusing
  the LaTeX grammar for both layers via a `DOC_MARGIN` trivia token. Broken down:
  - [x] **M0 file-kind plumbing.** `FileKind::Dtx` (extension-detected),
    `LexConfig { flavor, dtx }` threaded through `lex_with`/`parse_with_flavor`/
    `format_with_style_flavored`/`check_document` (a bare `LatexFlavor` coerces
    in). `latex_flavor` → `Document`, `default_wrap` → `Preserve`.
  - [x] **M1a margins.** Line-leading `%` (not `%<`) lexes as a one-byte
    `DOC_MARGIN` trivia (never swallows the space); threaded through every
    `grammar.rs` trivia scanner like whitespace (so `%\n%\n` is a `\par` break,
    `%␣x\n%␣y` one paragraph) and kept out of the leading-comment bind.
  - [x] **M1b macrocode.** `%␣*\begin{macrocode}` … `%␣*\end{macrocode}` frame
    lines pair through the ordinary environment grammar; the body lexes as real
    code under the package regime (`@` a letter), a stray `%` line inside is a
    code comment, and a missing terminator recovers losslessly.
  - [x] **M2 guards.** `%<*tag>` / `%</tag>` / inline `%<tag>` as a `GUARD`
    token (flat floating leaf — *no* `GUARD_BLOCK` node; guard nesting is
    orthogonal to LaTeX nesting). A line-leading `%<…>` (through the closing
    `>`) lexes as a `GUARD` trivia leaf in *any* layer (`macrocode` bodies
    included — docstrip processes guards line-by-line); an inline guard's
    trailing code lexes normally, a malformed `%<` (no `>` before EOL) falls
    back to a comment. `GUARD` floats like `DOC_MARGIN` through every
    `grammar.rs` trivia scanner.
  - [ ] **M3 doc/ltxdoc semantic signatures.** `\DocInput`,
    `\DescribeMacro`/`\DescribeEnv`, `\StopEventually`, `macro`/`environment`
    envs; classify `macrocode`/`macrocode*` as code-not-prose. Pure
    signature-DB work; enables the deferred doc-comment binding.
  - [ ] **M4 driver / `\iffalse` + `.ins`.** `\iffalse…\fi` stays
    un-evaluated (already lossless as ordinary commands); `.ins` deferred.

### Formatting

- [x] **`.sty`/`.cls` as code, not prose.** `FileKind::default_wrap` makes
  `.sty`/`.cls` default to `WrapMode::Preserve` (a package body is code, not
  running text) when no `--wrap` is given, with the existing group/argument
  indentation and macro-definition lowering (definition bodies stay as
  authored). `\ProvidesPackage`/option-processing boilerplate ordering is
  preserved for free under `Preserve` (it never reorders). Letter-mode is
  respected via the `Package` flavor. The expl3-mode freedom (whitespace
  non-semantic inside `\ExplSyntaxOn`, `~` a literal space) is deferred with the
  expl3 lexer mode above.
- [ ] **`.dtx` two-layer formatting.** Preserve the docstrip margins and `%<…>`
  guards byte-for-byte (protected); format the documentation prose layer and
  the `macrocode` code layer independently; never disturb the leading-`%`
  margin or guard lines. Idempotence + losslessness as elsewhere.

### Semantic / integration

- [ ] **Package load graph.** Treat `\usepackage`/`\RequirePackage`/`\LoadClass`/
  `\LoadClassWithOptions`/`\documentclass` as edges (the analog of the
  `\input`/`\include` graph in `project/include.rs`), resolving **local**
  `.sty`/`.cls` files only. Pull each loaded package's exported macro
  signatures into the document's signature scope.
- [ ] **Signature extraction from package sources.** Run the existing
  `semantic/define.rs` scanner across loaded `.sty`/`.cls` (and `macrocode`
  blocks of a `.dtx` when no generated `.sty` is present), extending it to
  `\DeclareRobustCommand`/`\DeclareDocumentCommand` and friends. Prefer a
  generated `.sty` over its `.dtx` source when both exist.
- [ ] **Package metadata & options (recognize, never execute).**
  `\ProvidesPackage`/`\ProvidesClass` (name/date/version),
  `\NeedsTeXFormat`, `\DeclareOption`/`\ProcessOptions`/`\ExecuteOptions` ---
  surfaced as signatures/metadata for hover/diagnostics, never run.
- [ ] **Package-aware diagnostics.** Once the load graph exists: unknown-option,
  duplicate `\RequirePackage`, missing `\ProvidesPackage`, and resolving
  user-macro definitions to their defining package for hover/go-to-definition.

## Performance & hardening

- [ ] Fuzzing (losslessness must hold on arbitrary input).
- [ ] Large-doc benchmarks (`hyperfine`, criterion); flamegraph hot paths.
- [ ] Intra-file incremental reparse (reuse green subtrees on contained edits).
- [ ] Extract shared crate(s) from the **\[copy\]** files (IR engine first),
  depended on by both badness and arity.
- [ ] `wasm32` build for a web playground.

## Tooling & infrastructure

- [ ] `build.rs` man/completions/markdown
  (clap_mangen/\_complete/clap-markdown). **\[copy\]** --- the `format`
  subcommand lives in `main.rs`; `build.rs` still deferred.

## BibTeX / BibLaTeX

Done (Phases 0–4): a lossless bib parser (`src/bib/`, own `SyntaxKind`/`BibLang`,
copied `events.rs`/`tree_builder.rs`, handling all entry/reserved forms +
recovery); a texlab differential parse oracle (entry-recognition floor +
soft Dice gauge, corpus incl. the vendored `biblatex-examples.bib`, fully
skeleton-concordant); a semantic model + field/entry signature DB
(`data/bib_fields.json`, `bib::semantic`); a deterministic formatter
(`src/bib/formatter/`, shared Wadler IR --- one field per line, `=` alignment,
quote→brace normalization, value reflow gated by field category, default-on
field + entry sorting with a crossref/xdata guard); a parallel linter
(`src/bib/linter/`, `BibRule` trait reusing the shared `Diagnostic`/`Fix`
infra --- `duplicate-key`, `missing-required-field`, `unknown-field`,
`empty-field` (with autofix), `unused-string`, `undefined-string`,
`title-capitalization`, `encoding-hints`; suppression via
`@comment{badness-ignore …}`); and Phase 4 integration (salsa
`parsed_bib_document`/`bib_semantic_model`, `.bib` routed through
`format`/`--check`/LSP diagnostics+formatting+outline, cross-file
`undefined-citation` via `ResolvedCitations`).

- [ ] Cross-file `undefined-string`: a `@string` defined in one `.bib` and used
  in another resolves only once a project-level `@string` union exists (today
  single-file-sound, same caveat as `unused-string`).
- [x] Bib-aware LSP completion: `@string` macro names in value position, field
  names per entry type (type-scoped, hiding fields already present), and entry
  types after `@` (`src/bib/completion.rs`); plus `\cite` key completion on the
  `.tex` side, resolved cross-file via `ResolvedCitations` (`src/lsp.rs`
  `cite_completion_items`).
- [ ] Enrich `\cite` completion items: today they carry only `label` (the key) +
  `REFERENCE` kind. Populate `detail`/`documentation` from the resolved entry —
  `entry_type` is already on `Entry` (free), and title/author can be read from the
  entry's fields (`crate::bib::ast`). Purely additive in `cite_completion_items`.
- [ ] Bib document-symbol outline completeness: `src/bib/outline.rs` surfaces
  regular entries only; consider `@string`/`@preamble`/`@comment` blocks (and a
  richer `SymbolKind`/detail).
- [ ] `title-capitalization` refinement: the acronym heuristic flags mid-word
  capitals, so CamelCase names (`McDonald`, `DeForest`) are false positives ---
  a curated name-particle allowlist or a smarter word model would tighten it.
- [ ] Shared component-finder: `ResolvedCitations` duplicates the union-find +
  component assignment from `ResolvedLabels` (marked EXTRACTION CANDIDATE in
  `project/citations.rs`); factor one helper when a third consumer appears.

--------------------------------------------------------------------------------

## Open decisions to revisit

- [ ] How much of `\newcommand` / `xparse` to model. *(Semantics)*
- [ ] Formatter opinionatedness: configurable vs. fixed. *(Formatter)*
- [ ] `.dtx` two-layer model: a preprocessor that splits doc/code layers, or a
  single lexer mode with margin-aware tokens? *(Package infrastructure)*
