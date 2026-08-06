# Width-dependent failures (S4 sweep)

Files whose failure status varies across the sweep widths 60/72/80/100/120 —
each is a column-arithmetic hybrid candidate. Files failing at every width are
in the width-80 baseline set files and omitted here.

Re-recorded after the fallback-statement forced-break gate (a hanging brace
group inside a *fallback* statement no longer dispatches on forced-ness, which
is newline-keyed there). Relative to the post-S4 sweep the tables shrank again:
latex3's `xparse-generic.tex` idempotency@60 and `l3prefixes.tex` trivia@60
converged, as did latex2e's `xparse-generic.tex`, `tagpdf-debug.sty` and
`tagpdf.sty` idempotency rows. The one new row, `tagpdf-mc-code-generic.sty`
trivia@60, is a *demotion*: the file left the width-80 trivia baseline entirely
and now fails only at width 60.

The previous re-record (after S4, structural expl3 statement boundaries)
collapsed the tables from the S2 sweep: `xtemplate-2023-10-10.sty` and
`lttemplates.dtx` at width 60 — the shared
`{ cs_ \str_if_eq:nnT {#3} { global } { g } set:Npn }` fragment that was the
recorded Tier-2 statement-boundary violation S4 was to retire — converged, as
did `latexrelease.sty`, `l3bigint.dtx`, `lthooks.dtx`, `lttagging.dtx`,
`l3doc.cls`, the tagpdf width-60 family, and most of the latex2e trivia rows
(statement boundaries no longer read the wrap).

## `latex3`, `--checks all`

  | file                                        | kind        | fails at widths |
  | ------------------------------------------- | ----------- | --------------- |
  | `./l3packages/xparse/xparse-2018-04-12.sty` | idempotency | 60              |
  | `./l3trial/l3ldb/l3precom.dtx`              | idempotency | 60,72           |
  | `./xpackages/xor/xo-or.dtx`                 | idempotency | 60              |

## `latex3`, `--checks trivia`

  | file                                 | kind   | fails at widths |
  | ------------------------------------ | ------ | --------------- |
  | `./l3trial/xgalley/xgalley-demo.tex` | trivia | 60              |

## `latex2e`, `--checks all`

  | file                                                        | kind        | fails at widths |
  | ----------------------------------------------------------- | ----------- | --------------- |
  | `./base/doc/ltnews24.tex`                                   | idempotency | 120             |
  | `./texmf/tex/latex/l3packages/xparse/xparse-2018-04-12.sty` | idempotency | 60              |

## `latex2e`, `--checks trivia`

  | file                                                  | kind   | fails at widths |
  | ----------------------------------------------------- | ------ | --------------- |
  | `./base/doc/ltnews24.tex`                             | trivia | 120             |
  | `./base/lterror.dtx`                                  | trivia | 100,120         |
  | `./base/ltlength.dtx`                                 | trivia | 100,120         |
  | `./required/amsmath/amsbsy.dtx`                       | trivia | 60,72,100       |
  | `./required/amsmath/amscd.dtx`                        | trivia | 120             |
  | `./required/cyrillic/lcy.dtx`                         | trivia | 60              |
  | `./required/tools/xr.dtx`                             | trivia | 120             |
  | `./texmf/tex/latex/tagpdf/tagpdf-mc-code-generic.sty` | trivia | 60              |

## `pgf`

No width-dependent failures in either check.
