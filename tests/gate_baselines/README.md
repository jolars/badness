# Trivia-invariant-layout gate baselines (S0)

The failure inventory the S1–S4 stages of the trivia-invariant-layout plan
(TODO.md) are gated against: **compare sets, not counts** — a stage may shrink
these sets but must not grow them.

Recorded at badness commit `268c5d8` (S0); re-recorded after S2, which
removed `latexrelease.sty` from both latex3 sets (15 `all`, 150 `trivia`)
and refreshed `SWEEP.md`. Re-recorded again after S4: the `all` sets are
byte-identical to S2's, and the `trivia` sets trade four
statement-boundary entries resolved (`xtemplate-2023-10-10.sty` in both
corpora — the S4 motivating case — plus latex2e's `pdfmanagement.sty` and
`tagpdf-base.sty`) for four additions in a different, pre-existing family
(`xparse-2020-10-01.sty` in both corpora, latex2e's `lipsum.sty` and
`expl3.sty`): the out-of-region prefix mode/rest printer coupling filed as
an S4 follow-up in TODO.md, reachable only through the
all-newlines-to-spaces mega-line variants. Re-recorded once more after the
grouped-sibling-walk fix (`head_command_has_grouped_sibling_arg` now
segments the same stream the layout segments — container braces stripped,
sliced to the in-region run): three `non-fixed-point` entries resolved
(`xparse-2020-10-01.sty` in both corpora — two of the four S4 additions
were this, not the mode/rest coupling — plus latex2e's long-standing
`xparse.sty`), no additions, and production output is byte-identical over
all three corpora at widths 60/80/120. Over the pinned gate corpora
fetched by `task gate-corpora:fetch` (`scripts/fetch_gate_corpora.sh`):

| corpus | repo @ pin | files |
| --- | --- | --- |
| latex3 | `latex3/latex3` @ `3d1d347d8937863c0786988b14d307a6091ee397` | 288 |
| latex2e | `latex3/latex2e` @ `3a9fdd88bdc53f16a0c2158aa70d259607de333a` | 384 |
| pgf | `pgf-tikz/pgf` @ `1c7fc0fdc3ec8a6bdcfd68785c6bbd43ec110178` | 397 |

## Regeneration

From each corpus root, with a release build:

```sh
badness --no-config debug format --checks all --report .     # -> <corpus>.all.txt
badness --no-config debug format --checks trivia --report .  # -> <corpus>.trivia.txt
```

Distill each report's `### k. \`path\` (kind)` headings into sorted
`path<TAB>kind` lines (`.all.txt`), adding a third class column for the
trivia files from each failure's `- Variant:` reason (`.trivia.txt`). The
width sweep behind `SWEEP.md` reruns both checks with
`--line-width 60|72|100|120`.

## The sets

- `<corpus>.all.txt` — the pre-existing `--checks all` gate (losslessness +
  idempotency at width 80). At S0, latex3's 16 (15 `format-error` +
  `latexrelease.sty` idempotency) matched the baseline recorded in TODO.md
  before S0, i.e. S0 changed no production layout; S2 resolved the
  `latexrelease.sty` entry, leaving latex3 at 15 `format-error`.
- `<corpus>.trivia.txt` — the `--checks trivia` convergence-oracle inventory
  at width 80 (wrap pinned to reflow). Counts: latex3 149 (151 at S0),
  latex2e 146 (148 after S4), pgf 15.
- `SWEEP.md` — failures that appear or vanish across widths 60–120; each is a
  column-arithmetic hybrid candidate.

## Classification (third column of `.trivia.txt`)

- `format-error` — the formatter refuses the file (statically unmodelable
  constructs; matches the smoke-test workflow's ALLOWLIST families).
  latex3 15, latex2e 13, pgf 15. pgf has **no** trivia failures at all.
- `content-change` — `fmt` violated the whitespace-only invariant on a
  perturbed input (latex3 132, latex2e 117). Two root causes identified, both
  **pre-existing reflow-on-`.dtx` doc-layer bugs**, reachable without any
  perturbation via `badness format --wrap reflow`:
  1. prose reflow relocates `^^A` doc comments into positions where they
     re-lex as content (the l3 `\title{...^^A...}` preamble pattern —
     essentially every l3 `.dtx`);
  2. content on docstrip-guarded lines is dropped entirely
     (`alltt.dtx` loses `\ProvidesFile{alltt.drv}`).
  These dominate the trivia sets and mask any later failure in the same file
  (the check stops at the first failing variant), so fixing them will both
  shrink these sets and potentially surface currently-hidden non-fixed-points.
- `non-fixed-point` — a perturbed variant formats to a non-fixed-point: the
  idempotency-hybrid family the umbrella exists to fix (latex3 3 after S2,
  latex2e 18; predominantly expl3 package code — xparse, xtemplate, tagpdf,
  pdfmanagement, luamml — the S2–S4 target set). `latexrelease.sty` was the
  known issue-#97 residue (rest-aware fill disagreement); S2 resolved it.

The generator's meaning-safety was spot-checked: 1 `dropped_unsafe` variant
in ~157k eligible gaps over 120 latex2e files.
