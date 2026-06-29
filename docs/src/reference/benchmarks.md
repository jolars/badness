# Benchmarks

Wall-clock formatting speed of `badness` against
[`tex-fmt`](https://github.com/wgunderwood/tex-fmt) and
[`latexindent`](https://github.com/cmhughes/latexindent.pl), measured with
[hyperfine]. Every tool formats stdin → stdout, so the comparison is free of
file-mutation and exit-code noise.

**This is not a quality gate and not a parity target.** Timings are machine- and
run-dependent, and these numbers measure *speed only*, never output equivalence.
The tools also do different work: `latexindent` only indents by default and does
no line reflow, while `badness` and `tex-fmt` wrap—so a raw speed comparison is
a snapshot of each tool at its defaults, not equal work. Treat the *ratios*, not
the absolute milliseconds, as the takeaway.

The figures below are regenerated manually with `task bench` and committed as a
machine-readable artifact (`benches/benchmark_results.json`); they are never
re-measured when this site is built or in CI.

[hyperfine]: https://github.com/sharkdp/hyperfine

## How it is measured

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

## Setup

{{ benchmark-meta }}

## Results

{{ benchmark-results }}
