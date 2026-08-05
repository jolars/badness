# Width-dependent failures (S0 sweep)

Files whose failure status varies across the sweep widths 60/72/80/100/120
— each is a column-arithmetic hybrid candidate. Files failing at every
width are in the width-80 baseline set files and omitted here.

## `latex3`, `--checks all`

| file | kind | fails at widths |
| --- | --- | --- |
| `./l3kernel/l3coffins.dtx` | idempotency | 60 |
| `./l3kernel/l3doc.dtx` | idempotency | 60 |
| `./l3packages/xparse/xparse-2018-04-12.sty` | idempotency | 60,120 |
| `./l3packages/xparse/xparse.dtx` | idempotency | 100 |
| `./l3packages/xparse/xparse-generic.tex` | idempotency | 60,72 |
| `./l3trial/l3bigint/l3bigint.dtx` | idempotency | 60 |
| `./l3trial/l3ldb/l3precom.dtx` | idempotency | 60,72 |
| `./l3trial/xbox/xbox.dtx` | idempotency | 60 |
| `./texmf/tex/latex/base/latexrelease.sty` | idempotency | 60,72,80 |
| `./xpackages/xor/xo-or.dtx` | idempotency | 60 |

## `latex3`, `--checks trivia`

| file | kind | fails at widths |
| --- | --- | --- |
| `./l3packages/xparse/xparse-generic.tex` | trivia | 60,72,80 |
| `./texmf/tex/latex/base/latexrelease.sty` | trivia | 60,72,80 |

## `latex2e`, `--checks all`

| file | kind | fails at widths |
| --- | --- | --- |
| `./base/doc/ltnews24.tex` | idempotency | 120 |
| `./base/lthooks.dtx` | idempotency | 60,72 |
| `./base/lttagging.dtx` | idempotency | 60 |
| `./required/latex-lab/latex-lab-context.dtx` | idempotency | 120 |
| `./required/latex-lab/latex-lab-unicode-math.dtx` | idempotency | 60 |
| `./texmf/tex/latex/l3kernel/l3doc.cls` | idempotency | 60 |
| `./texmf/tex/latex/l3packages/xparse/xparse-2018-04-12.sty` | idempotency | 60,120 |
| `./texmf/tex/latex/l3packages/xparse/xparse-generic.tex` | idempotency | 60,72 |
| `./texmf/tex/latex/l3packages/xparse/xparse.sty` | idempotency | 100 |
| `./texmf/tex/latex/tagpdf/tagpdf-debug.sty` | idempotency | 60,72 |
| `./texmf/tex/latex/tagpdf/tagpdf-mc-code-generic.sty` | idempotency | 60 |
| `./texmf/tex/latex/tagpdf/tagpdf.sty` | idempotency | 60,72 |

## `latex2e`, `--checks trivia`

| file | kind | fails at widths |
| --- | --- | --- |
| `./base/doc/ltnews24.tex` | trivia | 120 |
| `./base/ltboxes.dtx` | trivia | 60,80,100,120 |
| `./base/lterror.dtx` | trivia | 100,120 |
| `./base/ltlength.dtx` | trivia | 100,120 |
| `./base/ltmiscen.dtx` | trivia | 80,100,120 |
| `./base/ltpictur.dtx` | trivia | 60,80,100,120 |
| `./base/ltspace.dtx` | trivia | 80,100,120 |
| `./required/amsmath/amsbsy.dtx` | trivia | 60,72,100 |
| `./required/amsmath/amscd.dtx` | trivia | 120 |
| `./required/cyrillic/lcy.dtx` | trivia | 60 |
| `./required/latex-lab/latex-lab-toc-kernel-changes.dtx` | trivia | 60,72,80,120 |
| `./required/tools/somedefs.dtx` | trivia | 60,80,100,120 |
| `./required/tools/xr.dtx` | trivia | 120 |
| `./texmf/tex/latex/l3packages/xparse/xparse-generic.tex` | trivia | 60,72,80 |
| `./texmf/tex/latex/l3packages/xparse/xparse.sty` | trivia | 72,80,100,120 |
| `./texmf/tex/latex/tagpdf/tagpdf-base.sty` | trivia | 60,72,80,100 |
| `./texmf/tex/lualatex/luamml/luamml.sty` | trivia | 60,72,80,100 |

