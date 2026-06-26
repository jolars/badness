# Formatter benchmark: badness vs tex-fmt & latexindent

Wall-clock formatting speed of `badness` against
[`tex-fmt`](https://github.com/wgunderwood/tex-fmt) and
[`latexindent`](https://github.com/cmhughes/latexindent.pl), measured with
[hyperfine]. Every tool formats stdin → stdout, so the comparison is free of
file-mutation and exit-code noise.

**This is not a CI gate and not a parity target.** Timings are machine- and
run-dependent, and this file measures *speed only*, never output equivalence.
The tools also do different work: `latexindent` only indents by default and
does no line reflow, while `badness` and `tex-fmt` wrap—so a raw speed
comparison is a snapshot of each tool at its defaults, not equal work.
Regenerate with `task bench`.

[hyperfine]: https://github.com/sharkdp/hyperfine

## Setup

- **badness**: `0.4.0`
- **tex-fmt**: `0.5.7`
- **latexindent**: `3.24.7`
- **backend**: hyperfine (min runs: 3)
- **host**: linux/x86_64, AMD Ryzen 9 7900 12-Core Processor

Corpus is real LaTeX: a committed `small.tex` baseline plus documents fetched
by `benches/documents/download.sh` (gitignored). Documents `badness` cannot
yet format (parser diagnostics) are skipped.

## Results

### small.tex (baseline) (1233 bytes, 48 lines)

| Tool | Mean (ms) | Min (ms) | Max (ms) | Relative |
| --- | ---: | ---: | ---: | --- |
| badness | 1.3562 | 0.8833 | 3.1732 | baseline |
| tex-fmt | 1.7671 | 1.3315 | 3.3858 | 1.3× slower |
| latexindent | 60.5481 | 58.3684 | 63.9139 | 44.6× slower |

### cv.tex (6273 bytes, 275 lines)

| Tool | Mean (ms) | Min (ms) | Max (ms) | Relative |
| --- | ---: | ---: | ---: | --- |
| badness | 2.0243 | 1.5455 | 5.2977 | baseline |
| tex-fmt | 1.9884 | 1.5810 | 2.9046 | 1.0× faster |
| latexindent | 67.7454 | 65.3954 | 74.4720 | 33.5× slower |

### masters_dissertation.tex (95383 bytes, 2458 lines)

| Tool | Mean (ms) | Min (ms) | Max (ms) | Relative |
| --- | ---: | ---: | ---: | --- |
| badness | 12.4217 | 11.4561 | 14.5525 | baseline |
| tex-fmt | 2.6516 | 2.1647 | 4.6810 | 4.7× faster |
| latexindent | 1520.7835 | 1487.5122 | 1548.5250 | 122.4× slower |
