# Benchmarks

Wall-clock speed of `badness` against comparable tools, measured with
[hyperfine]: the **formatter** against
[`tex-fmt`](https://github.com/wgunderwood/tex-fmt) and
[`latexindent`](https://github.com/cmhughes/latexindent.pl), and the **linter**
against the classic TeX Live checkers [`lacheck`](https://ctan.org/pkg/lacheck)
and [`chktex`](https://ctan.org/pkg/chktex).

These numbers measure *speed only*, never output or diagnostic equivalence, and
the tools do genuinely different amounts of work:

- `latexindent` is a Perl script that parses LaTeX into a tree and reflows it
  according a set of highly configurable rules. It is the most featureful
  formatter here, but also the slowest.
- `tex-fmt` breaks overfull lines greedily but does not reflow: it won't rewrap
  lines that already fit, so it moves far less text than `badness`, which
  reflows each paragraph to the target width.
- Among the linters, `lacheck` is a small classic checker, `chktex` is
  regex-driven, and `badness lint` does a full CST parse plus its rule set.

The absolute milliseconds are the real latencies—what you actually wait—but they
are machine- and run-dependent. And because the tools do different work, a
cross-tool difference is not a claim that one tool is faster at the same job.

The figures below are regenerated manually with `task bench` and committed as a
machine-readable artifact (`benches/benchmark_results.json`); they are never
re-measured when this site is built or in CI.

[hyperfine]: https://github.com/sharkdp/hyperfine

## Formatter

### How the formatter is measured

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

The whole-project benchmark below measures **recursive folder formatting**
rather than a single file: each tool walks a real multi-file LaTeX thesis (the
pinned [`kks32/phd-thesis-template`], its `.tex` fragments) and formats every
file in read-only `--check` mode—the folder analog of the `stdin -> stdout` runs
above (full formatting work, nothing written). Only `badness` and `tex-fmt`
appear there: `latexindent` has no recursive directory mode, so it is excluded
from that comparison by design.

  | Tool      | Invocation                          |
  | --------- | ----------------------------------- |
  | `badness` | `badness format --check <dir>`      |
  | `tex-fmt` | `tex-fmt --check --recursive <dir>` |

The folder benchmark runs against a throwaway copy of the fetched project so
both tools walk an identical, un-gitignored, `.tex`-only tree (`badness format`
is `.tex`-only, while `tex-fmt` would otherwise also touch `.bib`/`.cls`). Any
file `badness` cannot format yet is dropped from *both* tools, keeping the
comparison symmetric. This is a different mode from the single-file runs, so
read its ratio on its own terms, not against them.

[`kks32/phd-thesis-template`]: https://github.com/kks32/phd-thesis-template

### Setup

{{ benchmark-meta }}

### Single-file results

{{ benchmark-results }}

### Whole-project results

{{ benchmark-project-results }}

## Linter

### How the linter is measured

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

### Setup

{{ lint-benchmark-meta }}

### Results

{{ lint-benchmark-results }}

## Language-server speed and memory

### How the language servers are measured

The harness starts three fresh processes each of `badness lsp` and `texlab run`
against the complete, pinned [`kks32/phd-thesis-template`] workspace. Each
session initializes the server, waits for background work to settle, opens the
same five documents, obtains diagnostics using the server's advertised pull or
push model, primes document symbols and meaningful citation/reference hovers,
and waits for the editor workload to settle. It then times warm document-symbol,
hover, definition, references, and rename requests.

The readiness measurements divide that session into three user-visible waits:

- **Initialize** is the `initialize` request round trip from a fresh process.
- **Workspace ready** runs from process start to the beginning of the final
  quiet window after initialization and background indexing.
- **Open files ready** runs from the burst of `didOpen` notifications through
  diagnostics and the beginning of the next quiet window.

Warm document-symbol and hover requests span the same three chapter files,
selected for real citation or reference keys. Definition, references, and rename
use the `Aup91` citation in `Chapter1/chapter1.tex`; its definition is in
`References/references.bib`. References include the declaration, and rename
constructs a `WorkspaceEdit` without applying it.

Each target gets two unmeasured warmup rounds and 20 measured rounds in every
fresh session. The tables report the median and p95 over all samples. They also
show serialized result size and the range of symbols, locations, or edits each
server returns, including the number of files involved. Those counts expose
cases in which two fast responses did different amounts of work.

On Linux, the harness samples the complete descendant process tree every 150 ms
from `/proc`. **RSS** is the resident memory commonly reported by process
monitors; it counts shared pages once in every process. **PSS** divides shared
pages among the processes that map them, which better estimates how much
physical memory the session occupies. The baseline is recorded after
initialization settles, the settled value after the open-file workload settles,
and the peak is the largest sample through the timed requests. A phase is
settled after five seconds below 5% of one CPU core and fails after 60 seconds.
The memory table reports the median of three fresh runs; the JSON artifact
retains each run's measurements.

The servers do not provide identical features or analysis, so this compares the
user-visible latency, returned work, and resident cost rather than efficiency at
the same work. Results also depend on the operating system, allocator, machine,
and tool versions; read the absolute figures together with the setup below.

Regenerate this section with `task bench:lsp` (`task bench:memory` remains an
alias). The committed `benches/memory_results.json` artifact is only read while
building the site—the benchmark never runs in CI or during an mdBook build.

### Setup

{{ memory-benchmark-meta }}

### Speed

{{ lsp-benchmark-results }}

### Memory

{{ memory-benchmark-results }}
