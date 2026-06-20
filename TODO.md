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
into both the CLI and LSP.

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
lexer mode); texlab differential parse oracle.

- [x] Block-vs-inline refinement: a lone block env is no longer wrapped in a
  `PARAGRAPH`. The signature DB carries a `block` flag (derived from
  `math`/`list`/`no_indent`, with an explicit opt-in for
  figure/center/verbatim/ theorem-likes/etc.); `parse_block` consults
  `is_block_environment` and skips the wrapper for a run whose sole
  non-trivia element is a block env.
- [x] Trivia-attachment policy --- decided (AGENTS.md decision #9):
  rust-analyzer rule. Default float-at-nearest-enclosing-node; a `%` comment
  run immediately before a documentable construct binds *leading* into it; a
  blank line breaks the bind. Trivia stays bare leaf tokens.
- [x] Leading comment-bind implemented **grammar-locally** (the `tree_builder`
  stays a mechanical replay). An own-line `%` run immediately before a
  documentable construct (any `COMMAND` or `ENVIRONMENT` node --- decided
  purely on node kind, no signature-DB lookup, so the syntactic layer stays
  semantics-free) binds *leading* into it; a same-line trailing comment
  never binds; a blank line breaks the bind (the bind is the maximal
  blank-line-free suffix). `parser/grammar.rs`
  `binding_run`/`comment_starts_line` detect it, and the existing `precede`
  idiom wraps the comments + construct (the construct self-opens, then its
  `Start` is pulled back over the comments). The formatter's three
  environment lowerers emit the bound run on its own line above `\begin`
  (`lower_environment_leading`). Covered by parser snapshots,
  roundtrip/losslessness cases, and a format fixture.

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
excluded, comment-bearing bodies take the plain path); `\left…\right` spacing; alignment-aware `align`/matrix column grids; list
environments (signature-DB `list` flag --- `itemize`/`enumerate`/`description`
--- one `\item` per line, each body reflowed with continuation lines
hanging-indented under the item text via `Ir::Align`); collapsible token-list
arguments (signature-DB `collapse` flag --- the cite family's key list folds a
multi-line authored form to one line, never width-reflowed, with the `inline`
flag flowing the command into the paragraph fill instead of preserving it as a
command-only line, so multi-line and one-line forms format identically;
`collapse` bails to the block form on a blank line, a `%` comment, or
force-break content). Protected regions untouched; idempotence + losslessness
asserted.

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
- [x] **Bug: a comment line inside an alignment breaks idempotence.** A
  commented-out row (`% & … & … \\`, common as authored scaffolding) was
  folded into the next row's first cell by `build_alignment_grid`, inflating
  that column's width so padding *grew every format pass* — and worse, the
  comment rendered first on the row, commenting out the real cells after it.
  Fixed: `finish_cell` now rejects any cell containing a `COMMENT` token (a
  comment runs to end of line, so it cannot share an aligned row), so the
  environment falls back to generic lowering with the comment on its own
  line. The documented intent ("a comment … falls back") was relying on
  `contains_forced_break`, which a comment's newline-free text never trips.
  Surfaced once whole-file formatting of real papers became reachable.
- [x] **Grid-align alignment environments that contain interspersed comments.**
  `build_alignment_grid` now carries non-row lines: a `GridItem` is either a
  `Row` or a `Passthrough` (kept verbatim between rows, never counted toward
  column widths), and `AlignRow` gained a `trailing_comment`. A comment-only
  physical line becomes a passthrough; an end-of-line comment (after the
  row's `\\`, or trailing the final row) trails its row; a mid-row comment
  (more cells follow) still falls back. The same passthrough mechanism
  handles horizontal-rule commands (a new `rule` flag on `CommandSig`:
  `\hline`, `\midrule`, `\toprule`, …), and with
  `tabular`/`tabular*`/`array` flagged `align`, text tables now grid-align
  with their rules preserved. The alignment analog of the paragraph/math
  comment-line handling.

## Linter

Done: `badness lint` + `linter/{diagnostic,render}` surfacing parse diagnostics
(annotate-snippets render); rule layer (`linter/{rules,check}`, `Rule` trait +
registry) wired into the CLI and the LSP `publishDiagnostics` path;
`linter/suppression` (`% badness-ignore`); deprecated-command (`\bf`-style) and
single-file duplicate-label lints.

**Dispatch: single shared walk** (arity's `run_rules` shape). `lint_document`
builds a `by_kind` table (`Vec<Vec<usize>>` sized `SyntaxKind::COUNT`, indexed by
`kind as usize`) from each rule's `interests()`, then does *one*
`descendants_with_tokens` traversal calling `Rule::check` on subscribed rules per
element — never O(rules × nodes). Node-shape rules (deprecated-command,
obsolete-environment, dollar-display-math, mismatched-delimiter) implement
`interests()` + `check()`; model/cross-file rules (duplicate-label, undefined-ref)
leave `interests()` empty and implement `check_file()`, run once after the walk.
Suppression stays a separate post-pass.

- [x] More lints: unmatched delimiters (parser already surfaces unclosed/stray
  delimiters and `\begin`/`\end` mismatches as `parse` diagnostics; the
  `mismatched-delimiter` lint adds reversed `\left`/`\right` orientation), undefined
  refs (`undefined-ref`, via the cross-file resolver), stylistic checks
  (`obsolete-environment` for `eqnarray` → `align`; `dollar-display-math`).
  Remaining stylistic ideas: missing `~` before `\cite`/`\ref`, typography.
- [x] Autofix infra (mirrors arity's `linter/fix.rs`): `Fix { content, start,
  end, applicability, description }` + `Applicability::{Safe, Unsafe}` on
  `Diagnostic`, a pure `apply_fixes(source, &[Fix], include_unsafe)` engine
  (right-to-left splice, overlap drop), `check_document` for the fixpoint, and
  `lint --fix`/`--unsafe-fixes` with a per-file fixpoint loop. Tenet 5
  ("autofixes never introduce formatting errors") is enforced by a
  `format → fix → format`-idempotent test harness in `tests/lint.rs`.
- [x] Add the `\[…\]` autofix to `dollar-display-math`. *Not* a formatter
  rewrite: `$$` is the plain-TeX primitive and `\[` routes through LaTeX's
  display hooks, so the swap changes typeset output (it ignores `fleqn`; the
  `\abovedisplayskip`/ `\belowdisplayskip`/`\predisplaypenalty` spacing
  differs) --- which would break the formatter's meaning-preservation
  contract. A lint is the right home for an almost-always-wanted *semantic*
  change. Fires only on a parser-built `DISPLAY_MATH` node (never on
  `$a$$b$`, two inline maths); a single whole-node replacement swaps the
  delimiters while copying the body verbatim, format-clean by construction
  (Tenet 5), and is withheld when the display math is unclosed.
- [ ] Wire the remaining report-only fixes onto the autofix infra:
  `deprecated-command`'s `\bf → \bfseries` (the natural first one) and
  `obsolete-environment`'s `eqnarray → align`.

## Semantic layer & signatures

Done: `semantic_model` (flat label/ref def-use model, `Eq`-backdating); built-in
signature DB (`data/signatures.json`); project include graph
(`\input`/`\include`/`\import`/`\subfile`, salsa firewall +
reachability/cycles); `\newcommand`/`\newenvironment`/`xparse` signature
scanning (`semantic/define.rs`, `semantic/xparse.rs`; scanned overlaid over
built-in; consumed by the formatter's `\begin` arity glue).

- [x] Cross-file label resolution (`file_labels` firewall → project-level
  `resolved_labels`, `project/labels.rs`) + cross-file `duplicate-label` and
  `undefined-ref` diagnostics. The label namespace is the undirected connected
  component of the include graph (so independent documents don't collide);
  `undefined-ref` is root-gated (fires only in a *closed*, document-rooted
  namespace). Wired into the CLI (`run_lint`); the LSP passes `resolution: None`
  for now (no workspace scan yet). The per-file `unreferenced_labels`/
  `unresolved_refs` remain *facts*; an `unused-label` lint (cross-file) is the
  natural follow-on but is deferred (it can false-positive on labels referenced
  from outside the analyzed set).
- [x] Unbraced `\newcommand\foo…` form (parsed with `\foo` as a sibling;
  recovered by a scanner-side sibling heuristic in `semantic/define.rs`
  (`resolve_command_def`), no parser change).
- [x] Verbatim-argument **commands** (the command analog of verbatim
  environments). The DB `verbatim` flag now drives a lexer mode
  (`lex_verbatim_command`) that captures the final argument as one `VERB`
  token — *brace*-style (`\code{…}`, `\url{…}`, balanced, may span lines) or
  *delimiter*-style (`\lstinline|…|`), chosen by its first character — after
  any leading non-verbatim args (`\mintinline`'s language). Built-ins added:
  `\url`, `\path`, `\lstinline`, `\mintinline`, and the curated class
  command `\code` (jss). Cleared the `\code{$ …}` "unclosed `$`" false
  positive. (`\verb`/`\verb*` keep their dedicated delimiter-only path.) The
  `VERB` body attaches as a **child** of the `COMMAND` node (via
  `attach_arguments`, like any greedy argument — decision #8), not a stranded
  sibling; a standalone `\verb…` token is guarded out by its `\` prefix. The
  next bullet generalizes this to arbitrary user macros.
- [ ] Detect verbatim-argument commands by **scanning their definitions**
  (extends the existing `semantic/define.rs` scanner). When a `\newcommand`/
  `\def` body reassigns a special char's catcode to "other" before grabbing
  an undelimited argument (`\@makeother\$`, `\catcode`\``\$=12`,
  `\dospecials` loops, …) — possibly via a chained helper macro (jss's
  `\code` defers its `#1` to `\@codex`) — mark that command's argument
  verbatim. Heuristic and **conservative by construction**: a wrong verbatim
  flag *suppresses* real diagnostics inside the body (the worse failure), so
  prefer false negatives. Reasoning about catcode execution sits at the
  boundary of AGENTS.md decision #1 — record the decision there if pursued.
- [ ] Salsa `document_signatures` query once an LSP consumer (hover/completion)
  wants the scanned command sigs.
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
`initializationOptions` + `didChangeConfiguration`; stdio smoke test.

arity (`../arity/src/lsp.rs`) is the feature template: it ships formatting,
range formatting, code actions, hover, definition, references, document
highlight, document symbol, and prepare-rename/rename. Most of these map
directly onto badness's existing semantic layer (label/ref def-use model,
signature DB with sectioning/arity/verbatim/prose, cross-file include graph).

### Configuration & sync

- [ ] config over LSP --- today `EditorSettings` carries only
  `line_width`/`indent_width`; `wrap` is hardcoded `Reflow`. Plumb
  `WrapMode` (and any future format knobs) through `EditorSettings` →
  `FormatStyle`, keeping the namespaced/bare parsing.
- [ ] Pull diagnostics (`textDocument/diagnostic` + `workspace/diagnostic`) as a
  capability alongside the current push model, for clients that prefer it.
- [ ] `workspace/didChangeWatchedFiles` so on-disk edits to non-open includes
  (the project graph's leaves) refresh cross-file analysis.

### Formatting

- [ ] Range formatting (`textDocument/rangeFormatting`) --- format the smallest
  enclosing node(s) covering the selection; clamp to node boundaries so a
  partial selection never corrupts the tree. Mirror arity's
  `on_range_formatting`.
- [ ] On-type formatting (`textDocument/onTypeFormatting`), e.g. re-indent on
  `}`/`\end{…}` close. *Lower priority; opt-in trigger characters.*

### Navigation & structure

- [x] Document symbols (`textDocument/documentSymbol`) --- a nested outline from
  the signature DB's `sectioning` levels (part/chapter/section/…), plus
  float and theorem-like environments (tagged via a new `outline` category
  in the signature DB) and labels as leaves. Built by the LSP-agnostic
  `semantic::outline` module.
- [ ] Folding ranges (`textDocument/foldingRange`) --- environments, sectioning
  spans, and long comment blocks.
- [ ] Selection ranges (`textDocument/selectionRange`) --- expand-selection from
  the CST's node hierarchy (group → argument → command → environment).
- [ ] Workspace symbols (`workspace/symbol`) --- labels and sectioning titles
  across the project include graph.

### Labels & references (def-use model)

- [ ] Go-to-definition (`textDocument/definition`) --- a `\ref`/`\eqref`/`\cref`
  jumps to its `\label`; cross-file via the include graph once
  `resolved_labels` lands (see *Semantic layer*).
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
- [x] Completion (`textDocument/completion`) --- command and environment names
  from the signature DB (built-in + scanned defines, via the new
  `document_signatures` salsa query), `\ref`-family keys from the label model,
  `\begin{…}`/`\end{…}` names (with an auto-`\end{…}` snippet on `\begin`), and
  file paths in `\includegraphics`/`\input`/`\include`/`\subfile`/`\import`/
  `\bibliography`/`\addbibresource`. (`src/completion.rs` + `src/lsp.rs`.)
  - [ ] `\cite` key completion --- deferred: there is no citation/bibliography
    model yet. Needs a `\bibitem` scan and/or `.bib` ingest (analog of the label
    model) before cite keys can be offered.
  - [ ] CWL ingest to widen command/environment name coverage.
  - [ ] `completionItem/resolve` to attach signature/doc detail lazily (mirror
    arity's resolve path) --- `resolve_provider` is currently `false`.
- [ ] Signature help (`textDocument/signatureHelp`) --- show the active argument
  while typing a command's `{…}`/`[…]` arguments.

### Code actions (autofixes)

- [ ] Code actions (`textDocument/codeAction`) surfacing linter autofixes once
  the autofix infra lands (Linter section, Tenet 5: fixes must be
  format-clean by construction). `deprecated-command`'s `\bf → \bfseries` is
  the natural first quick-fix; wire `CodeActionKind::QUICKFIX` + a resolve
  path mirroring arity's `on_code_action`.

### Infrastructure

- [ ] Client capability negotiation --- gate advertised providers and
  UTF-8/UTF-16 position encoding on what `initialize` reports.
- [ ] README editor-wiring docs (Neovim/VS Code `initializationOptions`,
  `badness lsp` invocation).

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
- [x] Directory-walking file discovery for `format` and `lint`
  (`file_discovery::collect_tex_files`, `ignore`-crate walk respecting
  `.gitignore`, `.tex` only). **\[copy\]** from arity.

## BibTeX / BibLaTeX

- [x] Parser — `src/bib/` module mirroring the LaTeX parser (lossless rowan CST +
  flat event stream + side-channel byte-range errors). Own `SyntaxKind` /
  `BibLang`; copied (EXTRACTION CANDIDATE) `events.rs` + `tree_builder.rs`. Handles
  regular entries (`{…}` and `(…)`), the reserved `@string` / `@preamble` /
  `@comment` forms, brace/quoted/literal values with `#` concatenation, nested
  braces, brace-protected quotes, inter-entry junk, and error recovery. Tests:
  `tests/bib_{parser,lexer_snapshots,roundtrip}.rs` + `tests/bib_corpus/`.
- [x] **Phase 0 — Differential parse oracle.** texlab ships a full BibTeX parser
  (`texlab_parser::parse_bibtex` → `texlab_syntax::bibtex`), already a vendored
  dev-dependency, so the bib oracle mirrors the LaTeX one (`tests/parse_{oracle,compat}.rs`).
  - `tests/support/bib_skeleton.rs` — projects both CSTs onto a common skeleton
    (entry type + field names; cite keys and value internals dropped, since that is
    where the generic and semantic parsers legitimately diverge).
  - `tests/bib_parse_oracle.rs` — hard gate in `cargo test`. texlab's bib parser has
    **no error channel** (unlike its LaTeX parser), so the LaTeX-style "must not error"
    check is vacuous; the gate instead enforces an *entry-recognition floor* (texlab
    recognizes ≥ as many `@entry`/`@string`/`@preamble` as badness on a badness-clean file).
  - `tests/bib_parse_compat.rs` (`#[ignore]`, `task bib-parse-compat`) — soft Dice gauge
    → `BIB_PARSE_COMPAT.md` + `tests/bib_parse_compat_allowlist.toml`. Baseline: 100%
    skeleton similarity across the corpus.
  - Corpus grown (`tests/bib_corpus/`): biblatex entry types/fields, accents/commands &
    nested braces in values, `@string`/`#` chains, crossref, the reserved forms.
  - Vendored the real-world `biblatex-examples.bib` (biblatex 3.21, LPPL; ~92 entries,
    15 entry types — 7→92 entries, 5→15 types over the hand-written slice; provenance in
    `tests/bib_corpus/README.md`). Parses losslessly, recognizes all 92 entries + 8
    `@string`s, and is **fully skeleton-concordant with texlab** (no parser gaps, no
    recorded deviations). *Next:* widen further (e.g. an ACL Anthology slice) for
    long-tail constructs.
- [x] **Phase 1 — Semantic model + field/entry signature DB.** Bib analog of
  `data/signatures.json` + `src/semantic/`. Landed: `data/bib_fields.json` (entry types with
  required/optional fields incl. `one-of` alternations, field categories — name lists, dates,
  verbatim-ish `url`/`doi`) loaded by `bib::semantic::signature` (`builtin()`, serde +
  `LazyLock`, case-insensitive). `bib::semantic::Model` (mirrors `src/semantic/builder.rs`)
  walks the CST via new `bib::ast` accessors to collect entries, cite keys, and `@string`
  defs/uses, then a resolve pass flags duplicate keys and undefined `@string` refs
  (month-macro `jan`..`dec` whitelist). Model exposes *facts* only — diagnostics are Phase 3,
  salsa `bib_semantic_model` is Phase 4. Real-corpus test (`tests/bib_semantic.rs`): 92
  entries + 8 `@string`s collected from `biblatex-examples.bib`, zero false duplicate/undefined
  findings.
- [x] **Phase 2 — Formatter.** `src/bib/formatter/` lowers the bib CST → the shared
  Wadler IR (`formatter/{ir,printer,style}.rs`, reused; only the lowering is bib-specific,
  mirroring `src/formatter/core.rs`). Landed deterministic style (Tenet 1): one field per
  line, fields indented one `indent_width` step, entry-type/field-name lowercasing (cite
  keys + `@string` names preserved), `=` aligned within each entry (precomputed text
  padding, *not* `Ir::Align` — that is continuation indent), quote→brace value
  normalization where safe (non-`Verbatim` field + balanced inner braces; bare `LITERAL`
  macro/number never wrapped; `@string`/`@preamble` values kept as authored; `#`
  concatenation preserved as ` # `), no trailing comma,
  one blank line between blocks. `@comment` bodies and inter-entry `JUNK` preserved (junk's
  outer whitespace trimmed so blank-line normalization stays idempotent). Refuses any input
  the parser flags. Tests (`tests/bib_format.rs` + `tests/fixtures/bib_format/`):
  exact-output fixtures, a meaning-preservation oracle (`semantic::Model` entries/keys/
  `@string` defs+uses, plus a `field_values` value-content check), and idempotence/clean/
  round-trip invariants over the corpus (incl. `biblatex-examples.bib`). bibtex-tidy /
  `biber --tool` remain soft convergence gauges only. *Deferred to Phase 4:* CLI/LSP
  routing for `.bib`; *future config:* brace-vs-quote/trailing-comma/paren-normalization
  toggles. **Value reflow landed in Phase 2b below.**
- [x] **Phase 2b — Value reflow (wrap long field values where safe).** Done.
  `lower_value_reflowed` (`src/bib/formatter/core.rs`) reflows a long single-piece value
  to `line_width` via the shared `Ir::Fill` primitive, with a hanging indent under the
  `=` (`Ir::align(prefix_width + 1, …)`, where `prefix_width = width + len(" = ")` is a
  pure function of the entry's field-name set, so it is stable across passes).
  **Category gating** (consults the signature DB): `Literal` prose (`title`,
  `journaltitle`, `abstract`, …) reflows at any brace-/math-depth-0 whitespace
  (`split_brace_aware`); **`Name`** (`author`/`editor`) reflows **only** at top-level
  ` and ` boundaries, breaking *after* "and" so the next name starts the continuation
  line, never inside a name (`split_top_level_and` + a `concat([" and", Line])`
  separator — a braced ` and `, as in `{Barnes and Noble}`, stays one atom); **never**
  `Verbatim` (`url`/`doi`/`eprint`/`file`) or `Date`, **never** a `#`-concatenated value
  or a single bare `LITERAL` macro/number. Brace- and `$…$`-spanning tokens stay glued,
  so inner braces and math never straddle a wrap. Invariants hold: idempotence (every
  whitespace run — incl. newline+indent — collapses to one break, so a reflowed value
  re-reflows identically) and meaning preservation (the `semantic::Model` oracle plus a
  new `field_values` value-content-modulo-whitespace-and-delimiters check, both asserted
  on every fixture and every clean corpus file). No opt-out knob; `line_width` alone
  tunes it. New fixtures: `reflow_literal_wrapped`, `reflow_author_and`,
  `author_hardwrap_normalized`, `reflow_inner_braces`, `verbatim_url_intact`,
  `concat_no_reflow`, `single_macro_intact`, `prewrapped_idempotent`,
  `multiline_value_reflowed`.
- [x] **Phase 2c — Field & entry sorting (default-on).** Done. New module
  `src/bib/formatter/sort.rs` (`canonical_fields` + `sorted_blocks`), consumed by
  `lower_entry`/`lower_root`; reorders existing CST nodes only (meaning preserved, lowering
  stays a pure replay), reading just the syntactic `ast` accessors + the static signature
  DB. Hard-wired deterministic — no `FormatStyle` toggle (Tenet 1).
  - **Field order within an entry:** the signature DB's required-then-optional sequence
    (each `OneOf` alternative in listed order), with fields the DB does not list (incl.
    every field of an unknown entry type) alphabetized after the known ones. A **stable**
    `sort_by_cached_key` keeps repeated field names (two `note =`) in source order, so
    BibTeX's last/first-wins is preserved.
  - **Whole-file entry order:** entries sorted by cite key (case-insensitive, stable) via
    **barrier segmentation** — every non-`ENTRY` block (`@string`, `@preamble`, `@comment`,
    inter-entry `JUNK`) is a fixed barrier, and only the maximal runs of consecutive
    entries *between* barriers are sorted. One mechanism gives `@string`-before-use
    (an entry never crosses a `@string` def it began behind) **and** keeps
    `@preamble`/`@comment`/`JUNK` pinned.
    - **crossref/xdata guard (safe v1):** a segment containing any entry with a
      `crossref`/`xdata` field is left in source order — skipping any run with a
      cross-reference *source* guarantees no parent is reordered ahead of a child within
      the run, and barriers fix cross-run order. *Future refinement:* a precise
      topological sort over the key graph (would sort crossref runs too).
  - Invariants: idempotence (both sorts stable and total; barrier/`JUNK` positions stable
    across reparse) and meaning preservation. The `tests/bib_format.rs` `meaning()` and
    `field_values()` oracles were made order-insensitive (sorted → multiset compares), so
    they still pin the *bag* of entries/keys/`@string`/field-values while allowing reorder.
    New fixtures: `sort_fields_canonical`, `sort_fields_unknown_alpha`,
    `sort_fields_duplicate_stable`, `sort_entries_by_key`, `sort_string_before_use`,
    `sort_crossref_pinned` (plus `verbatim_field_preserved` regenerated to canonical order).
- [x] **Phase 3 — Linter rules (initial slice).** A **parallel** `src/bib/linter/` module
  (mirroring the LaTeX `src/linter/`, the way `src/bib/formatter/` mirrors `src/formatter/`),
  with its own `BibRule` trait + `BibRuleContext` + kind-indexed driver, **reusing the
  language-agnostic** `Diagnostic`/`Fix`/`Severity`/`apply_fixes` from `crate::linter`
  wholesale (`AGENTS.md` "copy now, extract later"). The trait is *not* generified over the
  language — the two `SyntaxKind` enums and model types are distinct.
  - Rules shipped (5, all single-file-sound, **report-only**): `duplicate-key` (off
    `Model::duplicate_keys`), `missing-required-field` + `unknown-field` (off the field DB
    signatures, regular `ENTRY` only), `empty-field` (`{}`/`""`/`{  }`), `unused-string`
    (new `Model::unused_string_defs`). Each has per-rule unit tests; integration in
    `tests/bib_lint.rs`; registry⇔id-list agreement test.
  - CLI wiring: `file_discovery::collect_lint_files` + `FileKind` tag `.tex`/`.bib`;
    `main.rs run_lint` partitions by kind (`.bib` → `bib::linter::check_document`, no
    cross-file step) and renders both streams through the shared `render_findings`; the
    `--fix` pass skips `.bib` (no autofixes yet). `badness lint foo.bib` works end-to-end.
  - **Deferred (conscious decisions):**
    - *Autofixes:* none yet — kept report-only so `format → lint --fix → format --check`
      stays trivially green (Tenet 5). `empty-field` deletion (field + separating comma) is
      the first candidate, once validated against the `tests/bib_format.rs` oracle.
    - *Suppression:* no carrier — bib has **no comment token** (free text is `JUNK`,
      structured comments are `@comment`), so there is no `% badness-ignore` analog. The
      driver leaves a no-op seam where the LaTeX `SuppressionMap` runs.
    - *Rules:* `undefined-string` (needs a cross-file gate like `undefined-ref`),
      `title-capitalization` protection, `encoding-hints` — all Phase 4.
- [x] **Phase 4 — Incremental + LSP + project-graph integration.** Landed in three
  independently-committable sub-phases.
  - **4a — Infrastructure.** salsa `parsed_bib_document` (`no_eq`) + `bib_semantic_model`
    (`Eq`-backdated) queries with `Analysis`/`IncrementalDatabase` accessors
    (`incremental.rs`); `.bib` routed through `badness format` + `--check` (`run_format_paths`
    and `formatter/check.rs` now use `collect_lint_files` + `FileKind` dispatch, new
    `CheckError::BibFormatError`); LSP file-kind routing (`file_kind_for`, threaded through
    `WorkerJob`/`AnalyzeRequest`) for diagnostics (`analyze_bib`), formatting, and a flat
    document-symbol outline (new `src/bib/outline.rs`). Completion stays `.tex`-only.
  - **4b — Bib-local rules, autofix, suppression.** Suppression carrier =
    `@comment{badness-ignore <rule>: …}` (new `src/bib/linter/suppression.rs`,
    `BibSuppressionMap`), wired at the former no-op seam. New rules `undefined-string`
    (single-file-sound), `title-capitalization` (acronym heuristic: ≥2-cap runs or mid-word
    caps unprotected by braces), `encoding-hints` (non-ASCII runs, `Hint`). `empty-field`
    gained a format-clean deletion autofix (withheld when it would change `=` alignment);
    `main.rs` `fix_file`/`apply_fixes_to_paths` now fix `.bib` too.
  - **4c — Cross-file `undefined-citation`.** `collect_bib_resource_targets` +
    `BibTarget` (`\bibliography{a,b}`/`\addbibresource`) in `project/include.rs`;
    `CitationRef` + `cite_command` recognizer + `\nocite{*}` wildcard in `semantic/`;
    `ResolvedCitations` (new `src/project/citations.rs`, component union mirroring
    `ResolvedLabels`); `undefined-citation` rule gated on closed/rooted/no-wildcard;
    `RuleContext.citations` + `lint_document`'s 5th arg; `main.rs run_lint` collects bib
    keys per `.bib` and builds the resolver. LSP passes `None` (no project assembly yet,
    same as `undefined-ref`).
  - **Deferred:** a salsa `resolved_citations`/`file_cite_names` query (no consumer until
    the LSP assembles a project — it passes `None` today); cross-file `@string` resolution
    for `undefined-string`.

--------------------------------------------------------------------------------

## Open decisions to revisit

Collected from the areas above:

- [ ] How much of `\newcommand` / `xparse` to model. *(Semantics)*
- [ ] Formatter opinionatedness: configurable vs. fixed. *(Formatter)*
