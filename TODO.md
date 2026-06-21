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

- [ ] Folding ranges (`textDocument/foldingRange`) --- environments, sectioning
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
- [ ] Find references (`textDocument/references`) --- all uses of a label.
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

- [ ] **File-kind detection.** Extend `FileKind` (today `.tex`/`.bib`) to
  `.sty`/`.cls`/`.dtx`/`.ins`, threaded through file discovery, the CLI, and
  the LSP the way the `.bib` kind already is.
- [ ] **`@`-as-letter for `.sty`/`.cls`.** The package loader does
  `\makeatletter` implicitly, so `@` is a letter throughout these files. Start
  the lexer in letter-mode for these kinds (a static, extension-driven catcode
  fact --- sanctioned exactly like the explicit `\makeatletter` mode, decision
  #1); a trailing `\makeatother` still applies.
- [ ] **expl3 (LaTeX3) syntax mode.** `\ExplSyntaxOn` … `\ExplSyntaxOff`
  reassign catcodes statically: `_` and `:` become *letters* (so
  `\seq_new:N`, `\tl_set:Nn`, `\__module_internal:nn` lex as single control
  words), spaces and `~` are ignored/space, and `~` is a literal space. A
  sanctioned lexer mode like `\makeatletter`/`\verb` (decision #1) --- it reads
  only the static fact "we are between `\ExplSyntaxOn` and `\ExplSyntaxOff`",
  resolves no macro meaning. Auto-on for the whole file under
  `\ProvidesExplPackage`/`\ProvidesExplClass`/`\ProvidesExplFile`. Without it,
  every expl3 control word mis-lexes (the word stops at the first `_`/`:`),
  which corrupts argument grouping and the signature scan downstream --- so this
  is a prerequisite for parsing modern packages at all. Pairs with the
  `@`-as-letter mode above; the two can nest.
- [ ] **`.ins` installation scripts.** Recognize the kind; share the docstrip
  guard syntax with `.dtx` (see below). They are docstrip drivers
  (`\input docstrip`, `\generate{\file{…}{\from{…}{…}}}`, `\endbatchfile`) ---
  parse + format as code (`WrapMode::Preserve`), never run the extraction.
- [ ] **`.dtx`/`.ins` docstrip surface syntax.** A distinct literate format that
  interleaves two layers: a documentation margin (lines whose leading `%` is a
  comment margin) and code (lines with no leading `%`). The `macrocode` /
  `macrocode*` environments delimit real package code
  (`%    \begin{macrocode}` … a terminating `%    \end{macrocode}` line,
  4-space indented), and docstrip module guards `%<*tag>` … `%</tag>` /
  inline `%<tag>` select code per module. Recognize `\DocInput`,
  `\DescribeMacro`/`\DescribeEnv`, the driver `\iffalse…\fi` wrapper. Likely a
  dedicated lexer mode / preprocessor producing the two interleaved layers;
  guards and the `%` margin are **protected regions**, never executed or
  rewritten. Big bullet --- break down once the file-kind plumbing lands.

### Formatting

- [ ] **`.sty`/`.cls` as code, not prose.** Default `WrapMode::Preserve` (a
  package body is code, not running text), with group/argument indentation and
  the existing macro-definition lowering. Respect letter-mode and the expl3
  mode (inside `\ExplSyntaxOn` whitespace is non-semantic, so the formatter has
  more freedom there but `~` is a literal space and must be preserved); keep
  `\ProvidesPackage`/option-processing boilerplate ordering intact.
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
