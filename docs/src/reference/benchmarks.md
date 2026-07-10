# Benchmarks

Wall-clock speed of `badness` against comparable tools, measured with
[hyperfine]: the **formatter** against
[`tex-fmt`](https://github.com/wgunderwood/tex-fmt) and
[`latexindent`](https://github.com/cmhughes/latexindent.pl), and the **linter**
against the classic TeX Live checkers
[`lacheck`](https://ctan.org/pkg/lacheck) and
[`chktex`](https://ctan.org/pkg/chktex).

These numbers measure *speed only*, never output or diagnostic equivalence, and
the tools do genuinely different amounts of work:

- `latexindent` only indents; it does no line breaking.
- `tex-fmt` breaks overfull lines greedily but does not reflow: it won't rewrap
  lines that already fit, so it moves far less text than `badness`, which reflows
  each paragraph to the target width.
- Among the linters, `lacheck` is a small classic checker, `chktex` is
  regex-driven, and `badness lint` does a full CST parse plus its rule set.

The absolute milliseconds are the real latencies—what you actually wait—but they
are machine- and run-dependent. And because the tools do different work, a
cross-tool difference is not a claim that one tool is faster at the same job.

The figures below are regenerated manually with `task bench` and committed as a
machine-readable artifact (`benches/benchmark_results.json`); they are never
re-measured when this site is built or in CI.

[hyperfine]: https://github.com/sharkdp/hyperfine

## How the formatter is measured

Each tool is invoked exactly as a user would pipe a document through it:

  | Tool          | Invocation                                              |
  | ------------- | ------------------------------------------------------- |
  | `badness`     | `badness format --no-config --stdin-filepath bench.tex` |
  | `tex-fmt`     | `tex-fmt --stdin`                                       |
  | `latexindent` | `latexindent -g /dev/null -`                            |

The corpus is real LaTeX: a committed `small.tex` baseline plus larger documents
(`cv.tex`, `masters_dissertation.tex`, `phd_dissertation.tex`) fetched by
`benches/documents/download.sh` from a pinned `tex-fmt` release. Documents
`badness` cannot yet format (parser diagnostics) are skipped, as are comparison
tools missing from `PATH`.

### Whole-project (folder) benchmark

One entry, `project`, measures **recursive folder formatting** rather than a
single file: each tool walks a real multi-file LaTeX thesis (the pinned
[`kks32/phd-thesis-template`], its `.tex` fragments) and formats every file in
read-only `--check` mode—the folder analog of the `stdin -> stdout` runs above
(full formatting work, nothing written). Only `badness` and `tex-fmt` appear
here: `latexindent` has no recursive directory mode, so it is excluded from this
comparison by design.

  | Tool      | Invocation                          |
  | --------- | ----------------------------------- |
  | `badness` | `badness format --check <dir>`      |
  | `tex-fmt` | `tex-fmt --check --recursive <dir>` |

The benchmark runs against a throwaway copy of the fetched project so both tools
walk an identical, un-gitignored, `.tex`-only tree (`badness format` is
`.tex`-only, while `tex-fmt` would otherwise also touch `.bib`/`.cls`). Any file
`badness` cannot format yet is dropped from *both* tools, keeping the comparison
symmetric. This is a different mode from the single-file rows, so read its ratio
on its own terms, not against them.

[`kks32/phd-thesis-template`]: https://github.com/kks32/phd-thesis-template

## How the linter is measured

The linter runs over the same single-file corpus. Linters are read-only, so each
tool is handed the document path directly (no stdin plumbing—`lacheck` only
reliably reads a real file):

  | Tool      | Invocation                        |
  | --------- | --------------------------------- |
  | `badness` | `badness lint --no-config <file>` |
  | `chktex`  | `chktex -q <file>`                |
  | `lacheck` | `lacheck <file>`                  |

Findings are the normal case, and the tools signal them differently: `chktex`
exits `2`, `badness lint` exits `1`, and `lacheck` always exits `0`. A non-zero
exit here is not a run error, so hyperfine is told to ignore it
(`--ignore-failure`); the shell-loop fallback does the same.

There is no folder analog for the linter comparison: neither `lacheck` nor
`chktex` has a recursive directory mode, so—like `latexindent` in the formatter
folder benchmark—they would have no counterpart to measure against.

## Formatter setup

{{ benchmark-meta }}

## Formatter results

{{ benchmark-results }}

## Linter setup

{{ lint-benchmark-meta }}

## Linter results

{{ lint-benchmark-results }}
