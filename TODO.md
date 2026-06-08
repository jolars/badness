# badness — Roadmap

A LaTeX formatter, linter, and language server on a lossless rowan CST, mirroring
**ravel** (`../ravel`, the same tool for R). See `AGENTS.md` for load-bearing design
decisions, invariants, and the copy-from-ravel strategy.

Single-crate package (not a workspace). Parser and formatter are **intentionally
interleaved**: the formatter is the primary tool for stress-testing the parser.

Strategy: **copy ravel's language-agnostic skeleton to bootstrap, extract a shared
crate later** once badness's formatter works and boundaries are proven. Files marked
**[copy]** are lifted ~wholesale from ravel; **[rewrite]** are LaTeX-specific;
**[diverge]** intentionally differs from ravel.

Status: `[ ]` todo · `[~]` in progress · `[x]` done

---

## Session handoff (resume here)

**Where we are:** Phase 0 ✅, Phase 1 ✅, the **Phase 2 formatter MVP** ✅, and the
first two real format rules ✅ — **whitespace normalization** and **environment
indentation**. The parser is a lossless, error-tolerant recursive-descent grammar
over a rowan CST; `badness fmt` parses → lowers to a Wadler IR → prints. The
lowering: runs of `WHITESPACE`/`NEWLINE` trivia collapse to a single break
(trailing whitespace trimmed, 2+ blank lines → one, exactly one final newline);
the body of every `\begin{…} … \end{…}` is indented one step (nesting
recursively, `\begin`/`\end` flush). All indentation is computed by the printer
(`Ir::indent`), never preserved from input, so re-indentation is idempotent.
Paragraph structure, intra-line spacing, and protected regions (`\verb`, verbatim
bodies, comments) are preserved. Group/argument indentation and paragraph reflow
are the next rules.

**Build & verify** (everything is green as of this commit):
```sh
cargo test          # 46 tests: parser/lexer + printer engine + formatter invariants
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt -- --check
task snapshots      # regenerate insta snapshots (INSTA_UPDATE=always cargo test)

# CLI smoke checks:
printf '\\section{Hi}   \n\n\n\ntext.  ' | cargo run -- fmt   # → \section{Hi}\n\ntext.\n
printf '\\begin{itemize}\n\\item a\n\\end{itemize}\n' | cargo run -- fmt  # body indented 2 sp
cargo run -- fmt --check tests/corpus/*.tex                   # basic/math/edge now report
                                                             # (indentation + edge's final
                                                             # newline) — corpus is a parser
                                                             # fixture set, not pre-formatted
```

**Code map:**
- `src/syntax.rs` — `SyntaxKind` (tokens + nodes) + rowan `Language` binding.
- `src/parser/lexer.rs` — total lossless lexer; modes: `\verb`, verbatim envs,
  `\makeatletter`.
- `src/parser/grammar.rs` — the recursive-descent grammar (events + errors).
  Key methods: `parse_block` (paragraphs), `environment`/`finish_environment`
  (mismatch recovery), `command`, `group`, `optional`, `dollar_math`,
  `delim_math`, `verbatim_body`.
- `src/parser/events.rs` + `tree_builder.rs` — events → green tree.
- `src/parser/core.rs` — `parse()` / `reconstruct()` / `Parse { green, errors }`.
- `src/formatter.rs` + `formatter/` — the formatter. **[copy]** engine: `ir.rs`,
  `printer.rs`, `style.rs`, `context.rs` (lifted ~wholesale from ravel, each
  marked `EXTRACTION CANDIDATE`). **[rewrite]** `core.rs` — `format`/
  `format_with_style` + the LaTeX lowering: `lower_node` dispatches `ENVIRONMENT`
  to `lower_environment` (body wrapped in `Ir::indent`, leading/trailing breaks
  trimmed via `trim_leading_break`/`trim_trailing_break`, verbatim envs kept on the
  generic path via `has_verbatim_body`); everything else goes through
  `lower_element_stream` where runs of `WHITESPACE`/`NEWLINE` collapse to one break
  (`classify_trivia`: 0 newlines → inline ws kept; 1 → `hard_line`; 2+ →
  `empty_line`; indentation dropped — the printer owns it). A final unconditional
  fixup trims the trailing edge to exactly one newline. `check.rs` — `--check` over
  explicit paths (ravel's, minus `file_discovery`).
- `src/main.rs` — clap CLI: `badness fmt [paths] [--check] [--line-width]
  [--indent-width]`; stdin→stdout when no paths.
- `src/text/line_index.rs` — byte ↔ line/col (UTF-16) for later LSP.
- `tests/parser.rs` — tree snapshots + recovery assertions (asserts losslessness).
- `tests/format.rs` — fixture pairs (`tests/fixtures/formatter/<name>/{input,
  expected}.tex`) + idempotence, parse-stability (trivia-elided), and
  losslessness-of-output over the unit cases and corpus, plus an error-refusal
  case and a snapshot.

**Next step** — continue replacing identity behavior one construct at a time:
**group/argument indentation** (multi-line `{…}` / `[…]` bodies), then paragraph
reflow. Deferred whitespace follow-ups: collapsing *internal* multiple spaces.
Each rule is a small diff; use formatter ambiguities to drive parser fixes
(AGENTS.md). The differential oracles — `latexindent` (formatter) and
texlab/tree-sitter-latex (parse) — remain available as hardening tracks.

**Decisions recorded:**
- *(whitespace)* the final-newline fixup is *unconditional* — for any non-empty
  document the formatter trims the trailing edge (ASCII ws/newlines only, so
  trailing Unicode content survives) and appends exactly one `\n`; empty input
  stays empty.
- *(indentation)* all indentation is computed by the printer; leading whitespace in
  the input is dropped (not preserved), which is what makes re-indentation
  idempotent. Environment indentation is **uniform** — `document` and math
  environments (`align`, `equation`) indent like any other; a `document`/per-name
  opt-out belongs in a future config, not a special case (Tenet 1).
- *(known gap)* argument-taking environments (`\begin{tabular}{cc}`) put the trailing
  arg group on its own indented body line — correct handling needs the signature DB
  (already tracked under Phase 4 / Phase 1 follow-ups). Verbatim nested in an
  environment: `\begin{verbatim}` indents but the body and `\end` stay column-0
  (body is byte-preserved). Both are lossless and idempotent today.

Parser-adjacent ambiguities to watch (no parser change needed now): (1) indentation
after a newline lives in the *same* trivia run as the newline — the run classifier,
not the parser, splits them; (2) a `COMMENT` breaks a trivia run, so blank-line
collapsing around comments is a future paragraph/semantic concern, not a formatter
hack.

**Known deferred (not blockers, all lossless today):** arg-taking verbatim envs
(`lstlisting`/`minted`/`Verbatim`); block-vs-inline paragraph refinement (a lone
block env is wrapped in a `PARAGRAPH`); structured math (Phase 3); `build.rs`
man/completions and directory-walking file discovery for `fmt`. See the
Phase 1 follow-ups list below.

---

## Phase 0 — Foundations

- [ ] Module layout mirroring ravel: `parser/`, `formatter/`, `linter/`,
      `semantic/`, `project/`, `text/`, `syntax.rs`, `incremental.rs`, `cli.rs`.
- [ ] Align `Cargo.toml` deps with ravel: rowan 0.16, salsa 0.26, smol_str, insta,
      annotate-snippets, clap (+ build-deps), **lsp-server + lsp-types** *(not
      tower-lsp-server)*.
- [ ] `syntax.rs`: `SyntaxKind` (token + node kinds) + rowan `Language` impl. **[rewrite]**
- [ ] `text/line_index.rs`: byte ↔ (line, col) / UTF-16. **[copy]** (swap `Position` type)
- [ ] `parser/events.rs` (`Start`/`Tok(idx)`/`Finish`) + `tree_builder.rs`. **[copy]**
- [ ] Lossless lexer skeleton; trivia (whitespace, comments, blank lines) preserved
      but separable. **[rewrite]**
- [ ] Round-trip harness: `reconstruct(text) == text`, byte-for-byte.
- [ ] `insta` snapshot scaffolding + initial `.tex` corpus.
- [ ] `Taskfile.yml` mirroring ravel's targets (build, test, fmt, lint, bench).

## Phase 1 — Core parser

- [ ] Event-stream recursive-descent parser → green tree via `tree_builder`.
- [x] Diagnostics on a side channel by byte range (no `Error` event), carried
      alongside the tree (`Parse { green, errors }`, `parser/grammar.rs`).
- [x] Grammar coverage:
  - [x] Text runs grouped into `PARAGRAPH` nodes delimited by blank lines
        (`parse_block` / `trivia_run_is_separator`).
  - [x] Control sequences (`\foo` → `COMMAND`, control symbols as tokens);
        `\makeatletter`/`\makeatother` letter-mode in the lexer.
  - [x] Groups `{ … }` with unbalanced-brace recovery.
  - [x] Comments (`% …` to end of line) — handled in the lexer.
  - [x] Environments `\begin{name} … \end{name}`; mismatch recovery unwinds the
        implicit stack with one diagnostic per unclosed env.
  - [x] Generic greedy argument grouping: trailing `{…}` → `GROUP`, `[…]` →
        `OPTIONAL`, stopping at a paragraph break.
  - [x] Inline `$ … $`, display `$$ … $$`, `\[ … \]`, `\( … \)`.
  - [x] `~` ties, `\\`, `&`, `^`, `_`, `#` as distinct tokens.
  - [x] `\verb`/`\verb*` (one `VERB` token) and verbatim-like environments
        (`verbatim`, `verbatim*` → one `VERBATIM_BODY` token) as lexer modes.
        *Argument-taking verbatims (`lstlisting`/`minted`/`Verbatim`) deferred —
        need signature-aware arg handling.*
- [x] Recovery anchors: `\end`, `\begin`, blank line, `}`, `]`, `$`, EOF.
- [x] Progress guarantee: every grammar loop bumps ≥1 token or breaks
      (`debug_assert` in `bump`; `pos` only advances there).
- [x] **Enforce losslessness** — asserted per-case in `tests/parser.rs` and over
      the corpus in `tests/roundtrip.rs`.
- [ ] Differential parse oracle: cross-check against texlab / tree-sitter-latex over
      a corpus (ravel's `air_parser_harness` analog).

**Phase 1 follow-ups:**
- [x] `PARAGRAPH` node grouping over blank-line-delimited runs.
- [x] `\makeatletter`/`\makeatother` letter-mode in the lexer (Core decision #1).
- [x] Verbatim lexer mode for `\verb` and verbatim-like environments.
- [ ] Argument-taking verbatim envs (`lstlisting`/`minted`/`Verbatim`) — needs
      the signature DB to know where the raw body starts.
- [ ] Structured math model (scripts/delimiters) — currently flat tokens (Phase 3).
- [ ] Block-vs-inline refinement: a lone block environment is currently wrapped
      in a `PARAGRAPH`; the signature DB can later avoid that.

## Phase 2 — CLI + formatter MVP (interleaved with Phase 1)

- [~] `cli.rs` + `build.rs` (man/completions/markdown via clap_mangen/_complete/
      clap-markdown). **[copy]** — clap `fmt` subcommand lives in `src/main.rs`;
      `build.rs` man/completions still deferred.
- [x] `badness fmt`: parse → re-emit; first milestone is identity (round-trip).
- [x] `formatter/ir.rs` + `printer.rs`: Wadler IR + layout engine. **[copy]** (extract first)
- [~] LaTeX format rules: **whitespace normalization done** (trailing-ws trim,
      blank-line collapse, single final newline) and **environment indentation done**
      (printer-owned, idempotent re-indent, verbatim-protected); group/argument
      indentation and paragraph reflow still to come. **[rewrite]** — replaced the
      identity `lower_node`.
- [x] Protected regions never touched (`verbatim`, `\verb`, comments) — verified by
      the `protected_verbatim` / `protected_comment_trailing_space` fixtures now that
      rules touch surrounding text. (`lstlisting`/arg-taking verbatims still deferred.)
- [x] **Invariants:** idempotence `fmt(fmt(x)) == fmt(x)`; stability `parse(fmt(x)) ≅
      parse(x)` (trivia-elided); losslessness of formatted output — asserted per
      fixture and over the unit/corpus cases in `tests/format.rs`.
- [ ] Differential formatter oracle: fixed point `latexindent(badness(x)) == badness(x)`,
      `#[ignore]`d, triaged into adopt/record (ravel's `air_compat` analog).
- [ ] Use formatter ambiguities to drive parser fixes.

## Phase 3 — Math

- [ ] Structured math model over the generic math tree.
- [ ] Precedence-climbing for `^` / `_` binding and primes (the one Pratt site).
- [ ] `\left … \right` delimiter matching.
- [ ] Alignment-aware formatting: `align`, `matrix`/`pmatrix`, `&` columns, `\\` rows.

## Phase 4 — Salsa + semantic layer

- [ ] `incremental.rs`: `#[salsa::input] SourceFile { text }`, `parsed_document`
      query storing `GreenNode` (`no_eq, unsafe(non_update_types)`). **[copy]**
- [ ] `semantic_model` tracked query; linter/LSP reuse it (no re-parse from text).
- [ ] Signature DB (analog of ravel `rindex/`): built-in command/environment table +
      CWL-style data. **[rewrite]**
- [ ] `\newcommand`/`\newenvironment`/`xparse` signature scanning (signatures only,
      no execution).
- [ ] Project graph: `\input` / `\include` / `\import` resolution.
- [ ] Label/reference model (`\label` / `\ref` / `\cref`).

## Phase 5 — Linter

- [ ] Diagnostics framework over CST + semantics (reuse parse error channel).
- [ ] `linter/suppression` (`% badness-ignore` style) + annotate-snippets render. **[copy shape]**
- [ ] Lints: unmatched delimiters, undefined/duplicate refs, deprecated commands,
      stylistic checks.
- [ ] Autofix infra; enforce "autofixes never introduce formatting errors" (Tenet 5).

## Phase 6 — LSP

- [ ] `lsp.rs` on **lsp-server + lsp-types**: sync main loop, single-writer edits,
      snapshot readers on a threadpool. **[diverge from ravel]**
- [ ] Cancellation via salsa (`Cancelled` unwind) on document change.
- [ ] Publish diagnostics; full + range formatting.
- [ ] Document symbols, folding ranges.
- [ ] Hover + completion from the signature DB.
- [ ] Go-to-definition / rename for labels and refs.

## Phase 7 — Performance & hardening

- [ ] Extract shared crate(s) from the **[copy]** files (IR engine first), depended
      on by both badness and ravel.
- [ ] Intra-file incremental reparse (reuse green subtrees on contained edits).
- [ ] Fuzzing (losslessness must hold on arbitrary input).
- [ ] Large-doc benchmarks (`hyperfine`, criterion); flamegraph hot paths.
- [ ] `wasm32` build for a web playground.

---

## Open questions / decisions to revisit

- [ ] Trivia-attachment policy (leading vs. trailing) — pick one, document it.
- [ ] How much of `\newcommand` / `xparse` to model for the signature DB.
- [ ] Formatter opinionatedness: which choices are configurable vs. fixed.
- [ ] CWL data sourcing/licensing for the built-in signature DB.
- [ ] Whether ravel should also migrate tower-lsp-server → lsp-server (separate
      decision; out of scope for badness, but the rationale in `AGENTS.md` applies).
