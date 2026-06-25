# Formatter benchmark: badness vs tex-fmt & latexindent

Wall-clock formatting speed of `badness` against
[`tex-fmt`](https://github.com/wgunderwood/tex-fmt) and
[`latexindent`](https://github.com/cmhughes/latexindent.pl), measured with
[hyperfine]. Every tool formats stdin → stdout, so the comparison is free of
file-mutation and exit-code noise.

**This is not a CI gate and not a parity target.** Timings are machine- and
run-dependent, and this file measures *speed only*, never output equivalence.
The tools also do different work: `latexindent` only indents by default and
does no line reflow, while `badness` and `tex-fmt` wrap — so a raw speed
comparison is a snapshot of each tool at its defaults, not equal work.
Regenerate with `task bench`.

[hyperfine]: https://github.com/sharkdp/hyperfine

## Setup

- **badness**: `0.4.0`
- **tex-fmt**: `0.5.7`
- **latexindent**: `3.24.7`
- **backend**: hyperfine (min runs: 3)
- **host**: linux/x86_64, Intel(R) Core(TM) Ultra 7 155U

Corpus is real LaTeX: a committed `small.tex` baseline plus documents fetched
by `benches/documents/download.sh` (gitignored). Documents `badness` cannot
yet format (parser diagnostics) are skipped.

## Results

### small.tex (baseline) (1233 bytes, 48 lines)

| Tool | Mean (ms) | Min (ms) | Max (ms) | Relative |
| --- | ---: | ---: | ---: | --- |
| badness | 8.0473 | 4.7254 | 18.9338 | baseline |
| tex-fmt | 2.6119 | 1.0449 | 6.5071 | 3.1× faster |
| latexindent | 94.6362 | 83.3428 | 128.1073 | 11.8× slower |

### cv.tex (6273 bytes, 275 lines)

| Tool | Mean (ms) | Min (ms) | Max (ms) | Relative |
| --- | ---: | ---: | ---: | --- |
| badness | 8.6402 | 5.5038 | 15.8906 | baseline |
| tex-fmt | 2.5125 | 0.5746 | 9.1256 | 3.4× faster |
| latexindent | 105.2542 | 92.9865 | 128.7437 | 12.2× slower |

### masters_dissertation.tex (95383 bytes, 2458 lines)

| Tool | Mean (ms) | Min (ms) | Max (ms) | Relative |
| --- | ---: | ---: | ---: | --- |
| badness | 23.8625 | 19.5557 | 34.5632 | baseline |
| tex-fmt | 3.4177 | 1.2994 | 8.7068 | 7.0× faster |
| latexindent | 2561.8930 | 2526.9406 | 2579.4858 | 107.4× slower |
