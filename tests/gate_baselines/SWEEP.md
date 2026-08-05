# Width-dependent failures (S2 sweep)

Files whose failure status varies across the sweep widths 60/72/80/100/120
— each is a column-arithmetic hybrid candidate. Files failing at every
width are in the width-80 baseline set files and omitted here.

Re-recorded after S2 (`Mode::Flat` honest contract). Relative to the S0
sweep: `l3coffins.dtx`, `l3doc.dtx`, `xparse.dtx`, `xparse-generic.tex`
(all), `xbox.dtx`, `latex-lab-unicode-math.dtx`, and `xparse.sty` (all)
converged at every width; several other entries shrank. Two files are new
at width 60 (`xtemplate-2023-10-10.sty`, `lttemplates.dtx`): one shared
`{ cs_ \str_if_eq:nnT {#3} { global } { g } set:Npn }` fragment whose
width-60 break lands exactly at the width, and whose re-parse then splits
it into separate `SplitAtNewlines` statements — the known Tier-2
statement-boundary violation S4 retires, not a printer disagreement.

## `latex3`, `--checks all`

| file | kind | fails at widths |
| --- | --- | --- |
| `./l3packages/xparse/xparse-2018-04-12.sty` | idempotency | 60 |
| `./l3packages/xtemplate/xtemplate-2023-10-10.sty` | idempotency | 60 |
| `./l3trial/l3bigint/l3bigint.dtx` | idempotency | 60 |
| `./l3trial/l3ldb/l3precom.dtx` | idempotency | 60,72 |
| `./texmf/tex/latex/base/latexrelease.sty` | idempotency | 60,72 |
| `./xpackages/xor/xo-or.dtx` | idempotency | 60 |

## `latex3`, `--checks trivia`

| file | kind | fails at widths |
| --- | --- | --- |
| `./l3packages/xparse/xparse-generic.tex` | trivia | 72,80 |
| `./texmf/tex/latex/base/latexrelease.sty` | trivia | 60,72 |

## `latex2e`, `--checks all`

| file | kind | fails at widths |
| --- | --- | --- |
| `./base/doc/ltnews24.tex` | idempotency | 120 |
| `./base/lthooks.dtx` | idempotency | 60 |
| `./base/lttagging.dtx` | idempotency | 60 |
| `./base/lttemplates.dtx` | idempotency | 60 |
| `./required/latex-lab/latex-lab-context.dtx` | idempotency | 120 |
| `./texmf/tex/latex/l3kernel/l3doc.cls` | idempotency | 60 |
| `./texmf/tex/latex/l3packages/xparse/xparse-2018-04-12.sty` | idempotency | 60 |
| `./texmf/tex/latex/l3packages/xtemplate/xtemplate-2023-10-10.sty` | idempotency | 60 |
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
| `./texmf/tex/latex/l3packages/xparse/xparse-generic.tex` | trivia | 72,80 |
| `./texmf/tex/latex/l3packages/xparse/xparse.sty` | trivia | 72,80,100,120 |
| `./texmf/tex/latex/tagpdf/tagpdf-base.sty` | trivia | 60,72,80,100 |
| `./texmf/tex/lualatex/luamml/luamml.sty` | trivia | 60,72,80,100 |
