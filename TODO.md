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
(printer-owned, idempotent); paragraph reflow (`WrapMode`, `Ir::Fill`,
default `Reflow`); prose-argument reflow (signature-DB `prose` flag, soft
`Ir::group` around the fill engine); aggressive math lowering (collapse spacing,
tight scripts, strip redundant single-token script braces); `\left…\right`
spacing; alignment-aware `align`/matrix column grids. Protected regions
untouched; idempotence + losslessness asserted.

- [ ] `Sentence`/`Semantic` (sembr) wrap modes --- both fall back to `Preserve`
      today. *Demoted, much later.*
- [ ] Widen the prose-argument table (CWL ingest could feed it); consider gluing
      a prose arg onto its command line when a source break separates them.
- [ ] Join alignment-cell continuation lines (currently triggers the plain-body
      fallback); column-spec-aware L/C/R alignment for text `tabular`/`array`.
- [ ] Decide formatter opinionatedness: which choices are configurable vs. fixed.
      *(open decision)*

## Linter

Done: `badness lint` + `linter/{diagnostic,render}` surfacing parse diagnostics
(annotate-snippets render); rule layer (`linter/{rules,check}`, `Rule` trait +
registry) wired into the CLI and the LSP `publishDiagnostics` path;
`linter/suppression` (`% badness-ignore`); deprecated-command (`\bf`-style) and
single-file duplicate-label lints.

- [ ] More lints: unmatched delimiters, undefined refs (needs the cross-file
      resolver), stylistic checks.
- [ ] Autofix infra; enforce "autofixes never introduce formatting errors"
      (Tenet 5). `deprecated-command`'s `\bf → \bfseries` is the natural first
      fix.

## Semantic layer & signatures

Done: `semantic_model` (flat label/ref def-use model, `Eq`-backdating); built-in
signature DB (`data/signatures.json`); project include graph
(`\input`/`\include`/`\import`/`\subfile`, salsa firewall +
reachability/cycles); `\newcommand`/`\newenvironment`/`xparse` signature scanning
(`semantic/define.rs`, `semantic/xparse.rs`; scanned overlaid over built-in;
consumed by the formatter's `\begin` arity glue).

- [ ] Cross-file label resolution (`file_labels` firewall → project-level
      `resolved_labels`) + duplicate-label / undefined-ref diagnostics. Today's
      `unreferenced_labels`/`unresolved_refs` are per-file *facts*, not lints.
- [ ] Unbraced `\newcommand\foo…` form (parses with `\foo` as a sibling; needs
      scanner-side sibling heuristics, not parser changes).
- [ ] Salsa `document_signatures` query once an LSP consumer (hover/completion)
      wants the scanned command sigs.
- [ ] CWL corpus ingest (an import format converted *into* the signature schema)
      once ecosystem breadth (e.g. LSP completion) needs it.
- [ ] How much of `\newcommand` / `xparse` to model for the signature DB.
      *(open decision)*

## Language server

Done: `src/lsp.rs` + `badness lsp` (single-threaded, salsa-backed `lsp-server`
loop **\[diverge\]**); lifecycle, full-document sync,
`textDocument/formatting`, `publishDiagnostics` (parse + lint); cached-tree
reuse (`compute_format` → `format_node`); stdio smoke test.

- [ ] `--wrap`/config over LSP --- today `EditorSettings` carries only
      `line_width`/`indent_width`; `wrap` is hardcoded `Reflow`.
- [ ] Range formatting.
- [ ] Document symbols + folding.
- [ ] Hover + completion from the signature DB.
- [ ] Go-to-definition / rename for labels and refs.
- [ ] Incremental `didChange` sync.
- [ ] README editor-wiring docs.

## Performance & hardening

- [ ] Fuzzing (losslessness must hold on arbitrary input).
- [ ] Large-doc benchmarks (`hyperfine`, criterion); flamegraph hot paths.
- [ ] Intra-file incremental reparse (reuse green subtrees on contained edits).
- [ ] Extract shared crate(s) from the **\[copy\]** files (IR engine first),
      depended on by both badness and arity.
- [ ] `wasm32` build for a web playground.

## Tooling & infrastructure

- [~] `build.rs` man/completions/markdown
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
      decision; out of scope for badness, but the `AGENTS.md` rationale applies).
