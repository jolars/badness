---
name: bench
description: Use when the user wants to (re)run the formatter speed benchmark or refresh the docs benchmark page — "run the benchmark", "task bench", "update the benchmark numbers", "refresh benchmarks". Regenerates the committed JSON artifact that feeds docs/src/reference/benchmarks.md, then fact-checks that page against the code.
---

# bench

Wall-clock formatting-speed benchmark of `badness` against
[`tex-fmt`](https://github.com/wgunderwood/tex-fmt) and
[`latexindent`](https://github.com/cmhughes/latexindent.pl), via `hyperfine`.
Driven by `benches/compare_format.sh`.

**This is a visibility tool, not a CI gate and not an output-parity target.** It
measures *speed only*, never output equivalence — the tools do different work
(`latexindent` only indents by default and does no line reflow). Treat the
*ratios*, not the absolutes, as the finding; timings are machine- and
run-dependent.

The benchmark is **regenerated manually** by this skill. It is never run at
mdbook-build time or in CI — the docs page only renders the committed JSON.

## Steps

1. **Fetch the corpus** if it is missing (the larger documents are gitignored;
   `small.tex` is committed):

   ```sh
   task bench:download
   ```

2. **Run the benchmark.** This builds the release binary and rewrites the
   committed artifact `benches/benchmark_results.json`:

   ```sh
   task bench
   ```

   Best results need `tex-fmt`, `latexindent`, `hyperfine`, and `jq` on `PATH`
   (without hyperfine+jq it falls back to a mean-only shell loop and min/max
   become `null`). Tools absent from `PATH` are skipped; documents `badness`
   cannot format yet (parser diagnostics) are skipped with a note.

3. **Fact-check the docs page** `docs/src/reference/benchmarks.md`. The numbers
   render automatically from the JSON via the `doc-utils` preprocessor, but the
   surrounding **prose is hand-written and must stay true to the code**. Verify
   it against the actual implementation in `benches/compare_format.sh`, and
   correct any drift:
   - The exact tool invocations and flags: `badness format --no-config
     --stdin-filepath`, `tex-fmt --stdin`, `latexindent -g /dev/null -` (every
     single-file run is stdin → stdout).
   - The **whole-project (folder) benchmark** (`project` entry): a recursive
     `--check` over a throwaway copy of the fetched project — `badness format
     --check <dir>` vs `tex-fmt --check --recursive <dir>`, **badness vs tex-fmt
     only** (`latexindent` has no recursive mode). The staged tree is `.tex`-only
     and un-gitignored so both tools walk an identical set; files badness can't
     format are dropped from both via a generated `.ignore`.
   - What is and isn't measured: speed only, each tool at its defaults, no output
     equivalence; `latexindent` does no reflow.
   - The corpus and its source: `small.tex` committed; `cv.tex`,
     `masters_dissertation.tex`, `phd_dissertation.tex` fetched by
     `benches/documents/download.sh` from the pinned `tex-fmt` tag; the folder
     benchmark's project corpus is the pinned `kks32/phd-thesis-template` (its
     `.tex` fragments), fetched into `benches/documents/project/`.
   - The disclaimers above ("not a CI gate, not a parity target; ratios over
     absolutes") are present.

4. **Sanity-check rendering** (optional but recommended):

   ```sh
   cargo build --manifest-path docs/doc-utils/Cargo.toml
   mdbook build docs
   ```

   Confirm `docs/book/reference/benchmarks.html` shows the tables and no literal
   `{{ benchmark-results }}` / `{{ benchmark-meta }}` markers remain.

## How the page is wired

- `benches/compare_format.sh --out benches/benchmark_results.json` writes the
  artifact (schema: `meta`, `documents`, `results`).
- `docs/doc-utils/src/lib.rs` (`insert_benchmarks`) reads that JSON one level up
  from the book root at build time and substitutes the `{{ benchmark-results }}`
  and `{{ benchmark-meta }}` markers in `docs/src/reference/benchmarks.md`.
- The companion in-process micro-bench / flamegraph workflow lives in
  `benches/README.md` and is **out of scope** for this skill.
