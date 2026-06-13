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

Phases 0--4 are done: a lossless, error-tolerant recursive-descent parser over a
rowan CST; `badness format` (parse → Wadler IR → print) with whitespace
normalization, environment + group/argument indentation, and paragraph reflow
(`--wrap`, default `Reflow`, via the `Ir::Fill` node); salsa incrementality + a
semantic layer (label/ref model, signature DB, project include graph); and a
minimal salsa-backed LSP (`badness lsp`: full-document formatting + pushed
parser diagnostics).

Prose-argument reflow has landed: under `WrapMode::Reflow`, an argument the
signature DB marks `prose` (a `\footnote`/`\caption` body, a sectioning title)
reflows to the line width — joined when short, wrapped when long, via a soft
`Ir::group` around the paragraph-fill engine (`formatter/core.rs`:
`lower_command`/`lower_prose_group`). `ArgSpec` grew a per-position `prose` flag
(object form in `data/signatures.json`); the table is a conservative starter set
(text-formatting commands, footnotes/captions, sectioning titles) and is
incrementally tunable. Non-prose groups (`\newcommand` body, `\label`) are left
exactly as authored.

**Next up --- pick by priority:**

- *Formatter:* `Sentence`/`Semantic` wrap modes (port panache's sentence rules /
  sembr; both fall back to `Preserve` today) — *demoted, much later*. Or: widen
  the prose-argument table (CWL ingest could feed it), and consider gluing a
  prose arg onto its command line when a source break separates them.
- *LSP:* `--wrap`/config over LSP (today `EditorSettings` carries only
  `line_width`/`indent_width`; `wrap` is hardcoded `Reflow`); README
  editor-wiring docs. (The `format_node(tree)` cached-tree reuse is already done
  — `lsp.rs` `compute_format`.)
- *Hardening:* the `latexindent` differential formatter oracle (more useful now
  that reflow — including prose args — has landed).

Use formatter ambiguities to drive parser fixes (AGENTS.md tenet 3). The
differential oracles --- `latexindent` (formatter) and texlab/tree-sitter-latex
(parse) --- remain available as hardening tracks.

--------------------------------------------------------------------------------

## Phases

- [x] **Phase 0 --- Foundations.** Module layout, core deps, `syntax.rs`,
  `text/line_index.rs`, `parser/events.rs` + `tree_builder.rs`, lossless
  lexer, round-trip harness, insta scaffolding, `Taskfile.yml`.

- [x] **Phase 1 --- Core parser.** Event-stream recursive descent → green tree;
  side-channel diagnostics; paragraphs, control sequences, groups, comments,
  environments (with mismatch recovery), greedy argument grouping, math
  (`$…$`, `$$…$$`, `\[…\]`, `\(…\)`), `\verb`/verbatim lexer modes,
  `\makeatletter` letter-mode; recovery anchors + progress guarantee;
  losslessness asserted; texlab differential parse oracle.
  Open follow-ups:
  - [x] Argument-taking verbatim envs (`lstlisting`/`minted`/`Verbatim`) --- the
    lexer consults the built-in signature DB to skip the `\begin` arguments and
    find where the raw body starts.
  - [ ] Structured math model (scripts/delimiters) --- currently flat tokens
    (Phase 5).
  - [ ] Block-vs-inline refinement: a lone block env is wrapped in a
    `PARAGRAPH`; the signature DB can later avoid that.

- [x] **Phase 2 --- CLI + formatter MVP.** `badness format` (parse → Wadler IR →
  print); **\[copy\]** IR + printer engine; whitespace normalization,
  environment + group/argument indentation (printer-owned, idempotent),
  paragraph reflow (`WrapMode`, `Ir::Fill`); protected regions untouched;
  invariants (idempotence, parse-stability, losslessness) asserted.

  Open follow-ups:

  - [~] `build.rs` man/completions/markdown
    (clap_mangen/\_complete/clap-markdown). **\[copy\]** --- the `format`
    subcommand lives in `main.rs`; `build.rs` still deferred.
  - [x] Directory-walking file discovery for `format` and `lint`
        (`file_discovery::collect_tex_files`, `ignore`-crate walk respecting
        `.gitignore`, `.tex` only). **\[copy\]** from arity.

- [x] **Phase 3 --- Salsa + semantic layer.** `incremental.rs` salsa harness;
      `semantic_model` (flat label/ref def-use model, `Eq`-backdating); built-in
      signature DB (`data/signatures.json`); project include graph (`\input`/
      `\include`/`\import`/`\subfile`, salsa firewall + reachability/cycles).
      Open follow-ups:
      - [x] `\newcommand`/`\newenvironment`/`xparse` signature scanning
            (signatures only, no execution) feeding the semantic DB.
            `semantic/define.rs` scans the braced-name forms into a per-document
            `SignatureDb`; `semantic/xparse.rs` parses the full xparse arg-spec
            grammar; `Signatures` overlays scanned over built-in
            (scanned-first), and the formatter's `\begin` arity glue consumes
            it. Remaining: the unbraced form `\newcommand\foo…` (parses with
            `\foo` as a sibling, so skipped — needs scanner-side sibling
            heuristics, not parser changes); a salsa `document_signatures` query
            once an LSP consumer (hover/ completion) wants the scanned command
            sigs.
      - [ ] Cross-file label resolution (`file_labels` firewall → project-level
            `resolved_labels`) + duplicate-label / undefined-ref diagnostics.
            Today's `unreferenced_labels`/`unresolved_refs` are per-file
            *facts*, not lints.
      - [ ] CWL corpus ingest (an import format converted *into* the signature
            schema) once ecosystem breadth (e.g. LSP completion) needs it.

- [x] **Phase 4 --- Minimal LSP.** `src/lsp.rs` + `badness lsp` subcommand:
      single-threaded, salsa-backed `lsp-server` loop **\[diverge\]**;
      lifecycle, full-document sync, `textDocument/formatting`,
      `publishDiagnostics`; stdio smoke test.

- [x] **Phase 5 --- Math.**
  - [x] Structured math model over the generic math tree. A math-mode descent
        wraps each body in a `MATH` node and parses atoms (`math_atom`,
        `math_group`) instead of the text-mode `element`.
  - [x] Precedence-climbing for `^`/`_` binding and primes (the one Pratt
        site). `math_scripted` binds scripts into `SCRIPTED`/`SUBSCRIPT`/
        `SUPERSCRIPT`; primes ride inside `WORD` tokens (no special handling).
        The formatter lowers math aggressively (collapse spacing, tight scripts,
        strip redundant single-token script braces where safe).
  - [x] `\left … \right` delimiter matching. A lexer mode isolates the single
        delimiter token after `\left`/`\right` (word-character delimiters like
        `(` would otherwise glue into the next word run); the parser builds a
        `LEFT_RIGHT` node (delimiters as direct children, body wrapped in `MATH`)
        with count-based nesting (matching TeX), unclosed-`\left`/stray-`\right`/
        missing-delimiter recovery; the formatter sets a non-empty body off with
        one space just inside each delimiter (`\left( x + y \right)`; that spacing
        also stops a control-word delimiter like `\langle` from gluing onto the
        body), leaving an empty body tight (`\left.\right.`). Math-mode contexts
        only (`$`/`\[`/`\(`); `\left` inside `equation`/`align` arrives with the
        alignment-aware item.
  - [x] Alignment-aware formatting: an `align`/matrix-family environment
        (signature-DB `align` flag) lays its top-level `&` columns into a
        left-aligned grid — single space around `&`, last cell of each row never
        padded, the `\\` row break (with its `[len]`) preserved. Each cell lowers
        through the generic stream and renders flat (so inline math/groups
        normalize as elsewhere). A cell that can't sit on one line (a comment or
        nested block) falls back to the plain indented body; a nested alignment
        environment is still aligned in its own right. Follow-ups: join cell
        continuation lines (currently triggers fallback); column-spec-aware
        L/C/R alignment for text `tabular`/`array`.

- [] **Phase 6 --- Linter.** `badness lint` + `linter/{diagnostic,render}`
     surface parse diagnostics; annotate-snippets render done. Rule layer
     (`linter/{rules,check}`, `Rule` trait + registry) landed and wired into both
     CLI and the LSP `publishDiagnostics` path.

  - [x] `linter/suppression` (`% badness-ignore` style). **\[copy shape\]**
  - [~] Lints: deprecated commands (`\bf`-style font switches) and duplicate
          labels (single-file) done. Still: unmatched delimiters, undefined refs
          (needs the cross-file resolver), stylistic checks.
  - [ ] Autofix infra; enforce "autofixes never introduce formatting errors"
          (Tenet 5). Deferred; `deprecated-command`'s `\bf → \bfseries` is the
          natural first fix.

- [ ] **Phase 7 --- Full LSP.** Range formatting; linter diagnostics published
      alongside parse errors; document symbols + folding; hover + completion
      from the signature DB; go-to-definition / rename for labels and refs;
      incremental `didChange` sync.

- [ ] **Phase 8 --- Performance & hardening.**
  - [ ] Extract shared crate(s) from the **\[copy\]** files (IR engine
        first), depended on by both badness and arity.
  - [ ] Intra-file incremental reparse (reuse green subtrees on contained
        edits).
  - [ ] Fuzzing (losslessness must hold on arbitrary input).
  - [ ] Large-doc benchmarks (`hyperfine`, criterion); flamegraph hot paths.
  - [ ] `wasm32` build for a web playground.

- [ ] **Phase 9 --- BibTeX / BibLaTeX.** Parser (likely a `bib.rs` module, maybe
      its own crate); formatter + linter rules; LSP support; salsa incremental
      parsing + semantic model integrated with the LaTeX project graph (resolve
      `\bibliography` references).

--------------------------------------------------------------------------------

## Open questions / decisions to revisit

- [ ] Trivia-attachment policy (leading vs. trailing) --- pick one, document it.
- [ ] How much of `\newcommand` / `xparse` to model for the signature DB.
- [ ] Formatter opinionatedness: which choices are configurable vs. fixed.
- [ ] Whether arity should also migrate tower-lsp-server → lsp-server (separate
      decision; out of scope for badness, but the `AGENTS.md` rationale
      applies).
