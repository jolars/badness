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
fixes (AGENTS.md tenet 3). The differential oracles --- texlab/tree-sitter-latex
(parse) --- remain available as hardening tracks throughout.

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

- [ ] Block-vs-inline refinement: a lone block env is wrapped in a `PARAGRAPH`;
  the signature DB can later avoid that.
- [ ] Trivia-attachment policy (leading vs. trailing) --- pick one, document it.
  *(open decision)*

## Formatter

Done: `badness format` (parse → Wadler IR → print); **\[copy\]** IR + printer
engine; whitespace normalization, environment + group/argument indentation
(printer-owned, idempotent); paragraph reflow (`WrapMode`, `Ir::Fill`, default
`Reflow`); prose-argument reflow (signature-DB `prose` flag --- commands with the
signature-DB `inline` flag like `\footnote`/`\emph` flatten into the surrounding
fill so the body wraps as running text with `{`/`}` glued to adjacent words;
block-level prose commands `\section`/`\caption` block-break their braces via a
soft `Ir::group`); aggressive math lowering (collapse spacing, tight
scripts, strip redundant single-token script braces); display math
(`\[…\]`/`$$…$$`) lowered as an indented block with delimiters on their own
lines; `\left…\right` spacing; alignment-aware `align`/matrix column grids; list
environments (signature-DB `list` flag --- `itemize`/`enumerate`/`description`
--- one `\item` per line, each body reflowed with continuation lines
hanging-indented under the item text via `Ir::Align`). Protected regions
untouched; idempotence + losslessness asserted.

- [ ] `Sentence`/`Semantic` (sembr) wrap modes --- both fall back to `Preserve`
  today. *Demoted, much later.*
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

- [ ] More lints: unmatched delimiters, undefined refs (needs the cross-file
  resolver), stylistic checks.
- [ ] Lint `$$…$$` display math with a `\[…\]` autofix. *Not* a formatter
  rewrite: `$$` is the plain-TeX primitive and `\[` routes through LaTeX's
  display hooks, so the swap changes typeset output (it ignores `fleqn`; the
  `\abovedisplayskip`/ `\belowdisplayskip`/`\predisplaypenalty` spacing
  differs) --- which would break the formatter's meaning-preservation
  contract. A lint is the right home for an almost-always-wanted *semantic*
  change. Fire only on a parser-built `DISPLAY_MATH` node (never on
  `$a$$b$`, two inline maths); swapping the delimiter tokens on the
  already-blocked node is format-clean by construction (Tenet 5).
- [ ] Autofix infra; enforce "autofixes never introduce formatting errors"
  (Tenet 5). `deprecated-command`'s `\bf → \bfseries` is the natural first
  fix.

## Semantic layer & signatures

Done: `semantic_model` (flat label/ref def-use model, `Eq`-backdating); built-in
signature DB (`data/signatures.json`); project include graph
(`\input`/`\include`/`\import`/`\subfile`, salsa firewall +
reachability/cycles); `\newcommand`/`\newenvironment`/`xparse` signature
scanning (`semantic/define.rs`, `semantic/xparse.rs`; scanned overlaid over
built-in; consumed by the formatter's `\begin` arity glue).

- [ ] Cross-file label resolution (`file_labels` firewall → project-level
  `resolved_labels`) + duplicate-label / undefined-ref diagnostics. Today's
  `unreferenced_labels`/`unresolved_refs` are per-file *facts*, not lints.
- [ ] Unbraced `\newcommand\foo…` form (parses with `\foo` as a sibling; needs
  scanner-side sibling heuristics, not parser changes).
- [x] Verbatim-argument **commands** (the command analog of verbatim
  environments). The DB `verbatim` flag now drives a lexer mode
  (`lex_verbatim_command`) that captures the final argument as one `VERB`
  token — *brace*-style (`\code{…}`, `\url{…}`, balanced, may span lines) or
  *delimiter*-style (`\lstinline|…|`), chosen by its first character — after
  any leading non-verbatim args (`\mintinline`'s language). Built-ins added:
  `\url`, `\path`, `\lstinline`, `\mintinline`, and the curated class
  command `\code` (jss). Cleared the `\code{$ …}` "unclosed `$`" false
  positive. (`\verb`/`\verb*` keep their dedicated delimiter-only path.) The
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

- [ ] Document symbols (`textDocument/documentSymbol`) --- a nested outline from
  the signature DB's `sectioning` levels (part/chapter/section/…), plus
  environments (`figure`/`table`/`theorem`) and labels as leaves.
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
- [ ] Completion (`textDocument/completion`) --- command and environment names
  from the signature DB (built-in + scanned defines), `\ref`/`\cite` keys
  from the label/citation model, and `\begin{…}`/`\end{…}` pairing. Wants
  the `document_signatures` salsa query (see *Semantic layer*); CWL ingest
  widens coverage.
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

- [ ] Parser (likely a `bib.rs` module, maybe its own crate); formatter + linter
  rules; LSP support; salsa incremental parsing + semantic model integrated
  with the LaTeX project graph (resolve `\bibliography` references).

--------------------------------------------------------------------------------

## Open decisions to revisit

Collected from the areas above:

- [ ] Trivia-attachment policy (leading vs. trailing). *(Parser)*
- [ ] How much of `\newcommand` / `xparse` to model. *(Semantics)*
- [ ] Formatter opinionatedness: configurable vs. fixed. *(Formatter)*
- [ ] Whether arity should also migrate tower-lsp-server → lsp-server (separate
  decision; out of scope for badness, but the `AGENTS.md` rationale
  applies).
