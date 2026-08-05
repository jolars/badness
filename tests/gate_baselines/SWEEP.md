# Width-dependent failures (S4 sweep)

Files whose failure status varies across the sweep widths 60/72/80/100/120
— each is a column-arithmetic hybrid candidate. Files failing at every
width are in the width-80 baseline set files and omitted here.

Re-recorded after S4 (structural expl3 statement boundaries). Relative to
the S2 sweep the tables collapsed: `xtemplate-2023-10-10.sty` and
`lttemplates.dtx` at width 60 — the shared
`{ cs_ \str_if_eq:nnT {#3} { global } { g } set:Npn }` fragment that was
the recorded Tier-2 statement-boundary violation S4 was to retire —
converged, as did `latexrelease.sty`, `l3bigint.dtx`, `lthooks.dtx`,
`lttagging.dtx`, `l3doc.cls`, the tagpdf width-60 family, and most of the
latex2e trivia rows (statement boundaries no longer read the wrap).
`l3prefixes.tex` and `xgalley-demo.tex` are new at width 60 in the trivia
sweep, and `xparse-generic.tex` moved from the trivia column to a width-60
idempotency entry — all in the out-of-region prefix mode/rest coupling
family recorded as an S4 follow-up in TODO.md, not statement-boundary
reads.

## `latex3`, `--checks all`

| file | kind | fails at widths |
| --- | --- | --- |
| `./l3packages/xparse/xparse-2018-04-12.sty` | idempotency | 60 |
| `./l3packages/xparse/xparse-generic.tex` | idempotency | 60 |
| `./l3trial/l3ldb/l3precom.dtx` | idempotency | 60,72 |
| `./xpackages/xor/xo-or.dtx` | idempotency | 60 |

## `latex3`, `--checks trivia`

| file | kind | fails at widths |
| --- | --- | --- |
| `./l3kernel/doc/l3prefixes.tex` | trivia | 60 |
| `./l3trial/xgalley/xgalley-demo.tex` | trivia | 60 |

## `latex2e`, `--checks all`

| file | kind | fails at widths |
| --- | --- | --- |
| `./base/doc/ltnews24.tex` | idempotency | 120 |
| `./texmf/tex/latex/l3packages/xparse/xparse-2018-04-12.sty` | idempotency | 60 |
| `./texmf/tex/latex/l3packages/xparse/xparse-generic.tex` | idempotency | 60 |
| `./texmf/tex/latex/tagpdf/tagpdf-debug.sty` | idempotency | 60,72 |
| `./texmf/tex/latex/tagpdf/tagpdf.sty` | idempotency | 60 |

## `latex2e`, `--checks trivia`

| file | kind | fails at widths |
| --- | --- | --- |
| `./base/doc/ltnews24.tex` | trivia | 120 |
| `./base/lterror.dtx` | trivia | 100,120 |
| `./base/ltlength.dtx` | trivia | 100,120 |
| `./required/amsmath/amsbsy.dtx` | trivia | 60,72,100 |
| `./required/amsmath/amscd.dtx` | trivia | 120 |
| `./required/cyrillic/lcy.dtx` | trivia | 60 |
| `./required/tools/xr.dtx` | trivia | 120 |
