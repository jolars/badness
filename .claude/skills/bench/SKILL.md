---
name: bench
description: Use when the user wants to (re)run the formatter/linter speed benchmark or refresh the docs benchmark page — "run the benchmark", "task bench", "update the benchmark numbers", "refresh benchmarks". Regenerates the committed JSON artifact that feeds docs/src/reference/benchmarks.md, then fact-checks that page against the code.
---

# bench

Wall-clock speed benchmark of `badness` against comparable tools, via
`hyperfine`: the **formatter** against
[`tex-fmt`](https://github.com/wgunderwood/tex-fmt) and
[`latexindent`](https://github.com/cmhughes/latexindent.pl), and the **linter**
against [`lacheck`](https://ctan.org/pkg/lacheck) and
[`chktex`](https://ctan.org/pkg/chktex). Driven by `benches/compare_format.sh`.

It measures *speed only*, never output or diagnostic equivalence — the tools do
genuinely different amounts of work (`latexindent` only indents; `tex-fmt` breaks
overfull lines greedily but does not reflow, so it moves far less text than
`badness`; the three linters find different problem classes). The absolute
milliseconds are real latencies but are machine- and run-dependent, and a
cross-tool difference is not a claim that one tool is faster at the same job.

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

   Best results need `tex-fmt`, `latexindent`, `lacheck`, `chktex`, `hyperfine`,
   and `jq` on `PATH` (without hyperfine+jq it falls back to a mean-only shell
   loop and min/max become `null`). Tools absent from `PATH` are skipped;
   documents `badness` cannot format yet (parser diagnostics) are skipped with a
   note. `lacheck` and `chktex` are shipped with TeX Live.

3. **Fact-check the docs page** `docs/src/reference/benchmarks.md`. The numbers
   render automatically from the JSON via the `doc-utils` preprocessor, but the
   surrounding **prose is hand-written and must stay true to the code**. Verify
   it against the actual implementation in `benches/compare_format.sh`, and
   correct any drift:
   - The exact **formatter** invocations and flags: `badness format --no-config
     --stdin-filepath`, `tex-fmt --stdin`, `latexindent -g /dev/null -` (every
     single-file run is stdin → stdout).
   - The exact **linter** invocations (file-path, read-only): `badness lint
     --no-config <file>`, `chktex -q <file>`, `lacheck <file>`. Findings make
     `chktex` exit 2 and `badness` exit 1 while `lacheck` always exits 0, so
     hyperfine runs with `--ignore-failure` (matched by `|| true` in the
     shell-loop fallback). There is **no folder benchmark for linters**
     (`lacheck`/`chktex` have no recursive mode), mirroring `latexindent`'s
     exclusion from the formatter folder run.
   - The **whole-project (folder) benchmark** (`project` entry): a recursive
     `--check` over a throwaway copy of the fetched project — `badness format
     --check <dir>` vs `tex-fmt --check --recursive <dir>`, **badness vs tex-fmt
     only** (`latexindent` has no recursive mode). The staged tree is `.tex`-only
     and un-gitignored so both tools walk an identical set; files badness can't
     format are dropped from both via a generated `.ignore`.
   - What is and isn't measured: speed only, each tool at its defaults, no output
     or diagnostic equivalence; `latexindent` only indents; `tex-fmt` breaks
     overfull lines greedily but does not reflow (it won't rewrap lines that
     already fit), so it moves less text than `badness`; the linters check
     different things.
   - The corpus and its source: `small.tex` committed; `cv.tex`,
     `masters_dissertation.tex`, `phd_dissertation.tex` fetched by
     `benches/documents/download.sh` from the pinned `tex-fmt` tag; the folder
     benchmark's project corpus is the pinned `kks32/phd-thesis-template` (its
     `.tex` fragments), fetched into `benches/documents/project/`.
   - The framing is honest: absolute times are real latencies but machine- and
     run-dependent, and a cross-tool difference is not a same-job speed verdict
     (the tools do different work). Don't reintroduce "quality gate"-style meta
     commentary.

4. **Sanity-check rendering** (optional but recommended):

   ```sh
   cargo build --manifest-path docs/doc-utils/Cargo.toml
   mdbook build docs
   ```

   Confirm `docs/book/reference/benchmarks.html` shows the tables and no literal
   `{{ benchmark-results }}` / `{{ benchmark-meta }}` / `{{ lint-benchmark-results }}`
   / `{{ lint-benchmark-meta }}` markers remain, and that both the formatter and
   linter charts render.

## How the page is wired

- `benches/compare_format.sh --out benches/benchmark_results.json` writes the
  artifact (schema v2: `meta`, `documents`, `results` for the formatter, and
  `lint_results` for the linter — same row shape, the `formatter` key holding the
  tool name).
- `docs/doc-utils/src/lib.rs` (`insert_benchmarks`) reads that JSON one level up
  from the book root at build time and substitutes the `{{ benchmark-results }}`,
  `{{ benchmark-meta }}`, `{{ lint-benchmark-results }}`, and
  `{{ lint-benchmark-meta }}` markers in `docs/src/reference/benchmarks.md`.
- The companion in-process micro-bench / flamegraph workflow lives in
  `benches/README.md` and is **out of scope** for this skill.
