# Performance

<a id="benchmarks"></a>

These benchmarks compare the speed of Badness's formatter, linter, and language
server with other LaTeX tools, along with the language server's memory use. The
tools differ in formatting style, lint coverage, and editor features, so the
timings alone cannot tell you which tool best suits your work.

## Formatter

We compare `badness` with [`tex-fmt`](https://github.com/wgunderwood/tex-fmt)
and [`latexindent`](https://github.com/cmhughes/latexindent.pl) on individual
documents. Files that `badness` cannot format are excluded from every tool's
results.

{{ benchmark-meta }}

### Single-file results

{{ benchmark-results }}

### Whole-project results

This comparison measures the time to check formatting across the `.tex` files of
[`kks32/phd-thesis-template`]. Both tools compute the formatted output without
writing changes to disk. `latexindent` is omitted because it has no recursive
directory mode.

{{ benchmark-project-results }}

## Linter

We compare `badness lint` with [`lacheck`](https://ctan.org/pkg/lacheck) and
[`chktex`](https://ctan.org/pkg/chktex) on the same individual documents. Each
linter checks for a different set of problems. Neither comparison tool has a
recursive directory mode, so this benchmark covers individual files only.

{{ lint-benchmark-meta }}

{{ lint-benchmark-results }}

## Language Server

We compare Badness with [TexLab](https://github.com/latex-lsp/texlab) by opening
the same five documents in the complete thesis project. The benchmark measures
how long each server takes to start, respond to editor requests, and finish
background work, as well as how much memory it uses.

{{ memory-benchmark-meta }}

### Speed

The startup measurements cover three waits:

- **Initialize** measures the server's response to the editor's initialization
  request.
- **Workspace ready** measures the time from process start until background
  indexing settles.
- **Open files ready** measures the time from opening the documents until
  diagnostics arrive and background work settles.

After startup, we time requests for document symbols, hover information,
definitions, references, and renaming. The chart shows the median response
times. Tooltips and expandable tables include the 95th percentile (p95) and the
number of results returned, which can differ between servers.

{{ lsp-benchmark-results }}

### Memory

The chart shows median memory use across three fresh sessions, including child
processes. **RSS** counts resident memory, including shared pages in each
process. The tooltips also show **PSS**, which divides shared pages among the
processes using them to estimate their share of physical memory.

{{ memory-benchmark-results }}

## Reproducibility

Run these commands from the repository root:

```sh
task bench:download  # Fetch the benchmark documents.
task bench          # Measure formatter and linter speed.
task bench:lsp      # Measure language-server speed and memory.
```

The scripts build `badness` in release mode. The formatter and linter comparison
uses the tools available on `PATH` and skips any that are missing. Install
[`hyperfine`][hyperfine] and `jq` for timing statistics; without them, the
script uses a shell loop that reports only mean times. The language-server
benchmark requires Linux, Python 3, and `texlab`. `task bench:memory` is an
alias for `task bench:lsp`.

The commands write `benches/benchmark_results.json` and
`benches/memory_results.json`. These committed files supply the charts, machine
details, and tool versions shown above. Building the documentation reads these
files without running the benchmarks. Neither benchmark runs in CI.

### Documents

The individual documents are a committed `small.tex` baseline and three files
from a pinned `tex-fmt` release: `cv.tex`, `masters_dissertation.tex`, and
`phd_dissertation.tex`. The thesis project comes from a pinned revision of
[`kks32/phd-thesis-template`]. `benches/documents/download.sh` records both
pins.

The formatter and linter benchmarks skip any document that `badness` cannot
format. For the project comparison, the script copies a fixed set of `.tex`
files into a temporary directory, excluding unsupported files from both tools.
This gives both formatters the same input files without interference from Git
ignore rules. The language servers use the complete project, including its
class, style, bibliography, and image files.

### Formatter and linter commands

For individual documents, each formatter reads from standard input and writes to
standard output:

  | Tool          | Invocation                                              |
  | ------------- | ------------------------------------------------------- |
  | `badness`     | `badness format --no-config --stdin-filepath bench.tex` |
  | `tex-fmt`     | `tex-fmt --stdin`                                       |
  | `latexindent` | `latexindent -g /dev/null -`                            |

The project comparison includes directory traversal and uses check mode:

  | Tool      | Invocation                                 |
  | --------- | ------------------------------------------ |
  | `badness` | `badness format --no-config --check <dir>` |
  | `tex-fmt` | `tex-fmt --check --recursive <dir>`        |

Each linter reads the document from its path:

  | Tool      | Invocation                        |
  | --------- | --------------------------------- |
  | `badness` | `badness lint --no-config <file>` |
  | `chktex`  | `chktex -q <file>`                |
  | `lacheck` | `lacheck <file>`                  |

With `hyperfine`, each command gets one warmup and at least three measured runs.
The script ignores exit codes because lint findings and formatting differences
can produce nonzero exits. The commands and timing loop are defined in
`benches/compare_format.sh`.

### Language-server sessions

`benches/compare_lsp_memory.sh` starts three fresh sessions each of
`badness lsp` and `texlab run`. In each session, the harness initializes the
server, waits for background work to settle, opens five documents, and collects
diagnostics using the server's pull or push model. It then requests document
symbols and citation or reference hovers and waits for background work to settle
again.

The timed symbol and hover requests cover three chapter files. Definition,
references, and rename use the `Aup91` citation in `Chapter1/chapter1.tex`,
whose entry is in `References/references.bib`. References include the
declaration. Rename computes edits without applying them. Each request target
gets two warmup rounds and 20 measured rounds per session. The chart aggregates
these samples across all three sessions. The recorded results also include
response sizes and counts of symbols, locations, edits, and affected files.

The harness samples the server and all descendant processes through Linux
`/proc` every 150 ms. Background work has settled when CPU use stays below 5% of
one core for five seconds. A phase fails if it does not settle within 60
seconds. Workspace and open-file readiness timings end at the start of their
respective quiet periods.

Memory is recorded after initialization (**Baseline**) and after the open-file
workload settles (**Settled**). **Peak** is the largest sample through the timed
requests. The chart shows the median of each measurement across the three
sessions, and the JSON file retains the measurements from each session.

[hyperfine]: https://github.com/sharkdp/hyperfine
[`kks32/phd-thesis-template`]: https://github.com/kks32/phd-thesis-template
