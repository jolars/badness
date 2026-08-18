# Trivia-invariant-layout gate baselines

The failure inventory every change to the layout engine is gated against:
**compare sets, not counts** — a change may shrink these sets but must not grow
them. Run `task gate-corpora:check`.

The strict trivia-invariance oracle deliberately has **no** baseline here.
`fmt(perturbed) == fmt(original)` is the end-state contract, so it still fails
wherever the formatter preserves an authored break — 274/286 latex3, 368/384
latex2e, 307/397 pgf, 3122/5209 latexindent as of the opaque-group width-driven
layout (it was 282/375/361/3851 at first record, and 275/372/354/3804 after the
`CommandSig::block` fix). A near-total set makes a useless ratchet; the numbers
are here so the Tier-2 preservation surface has something visible to shrink
against. Survey it with `task gate-corpora:strict-survey`.

The re-record log below is the provenance of these checked-in sets, so it is
kept verbatim. Its `S0`–`S4` labels are the stages of the staged
trivia-invariant-layout plan that drove the recordings; that plan has been
retired from `TODO.md` now its stages are all delivered, so read the labels as
dates — the detail is in `git log`.

Recorded at badness commit `268c5d8` (S0); re-recorded after S2, which removed
`latexrelease.sty` from both latex3 sets (15 `all`, 150 `trivia`) and refreshed
`SWEEP.md`. Re-recorded again after S4: the `all` sets are byte-identical to
S2's, and the `trivia` sets trade four statement-boundary entries resolved
(`xtemplate-2023-10-10.sty` in both corpora — the S4 motivating case — plus
latex2e's `pdfmanagement.sty` and `tagpdf-base.sty`) for four additions in a
different, pre-existing family (`xparse-2020-10-01.sty` in both corpora,
latex2e's `lipsum.sty` and `expl3.sty`): the out-of-region prefix mode/rest
printer coupling filed as an S4 follow-up in TODO.md, reachable only through the
all-newlines-to-spaces mega-line variants. Re-recorded once more after the
grouped-sibling-walk fix (`head_command_has_grouped_sibling_arg` now segments
the same stream the layout segments — container braces stripped, sliced to the
in-region run): three `non-fixed-point` entries resolved
(`xparse-2020-10-01.sty` in both corpora — two of the four S4 additions were
this, not the mode/rest coupling — plus latex2e's long-standing `xparse.sty`),
no additions, and production output is byte-identical over all three corpora at
widths 60/80/120.

Re-recorded once more after the **fallback-statement forced-break gate**: a
hanging brace group inside a *fallback* statement no longer takes the
forced-break dispatch in `lower_expl_code`, because forced-ness is newline-keyed
there (a width wrap inside the group's body mints statement boundaries the
reparse reads as hard breaks) and a fallback line commits as a plain greedy fill
with no sticky cascade to make the two paths agree. This was the real cause of
the two entries the S4 note above filed under the out-of-region prefix mode/rest
coupling — that diagnosis was wrong, and both were reachable as plain
*idempotency* failures at the default wrap mode, not only through the mega-line
trivia variant. Five `non-fixed-point` entries resolved (`lipsum.sty` and
`expl3.sty` — the two named S4 residues — plus `tagpdf-mc-code-generic.sty`,
`tagpdf-mc-code-lua.sty` and `luamml.sty`), no additions in any of the six
files; latex3 and pgf byte-unchanged. Production output moved in 19 files across
the three corpora, every diff the same shape: a sibling stranded on its own line
after a multi-line group (`,`, `{#1}`, `\fi:`) re-glues onto the closing `}`.
Re-recorded once more after **`reflow` became the default for every file kind**
(the per-extension `WrapMode` default is gone) together with the `.dtx`
margin-safety gates that made it safe. This is the largest movement the
inventory has seen:

- **169 `content-change` entries resolved** — latex3 132 → 11, latex2e 117 → 69.
  The `^^A`-relocation and guarded-line families are gone: reflowing a managed
  command argument across doc-margined lines is now refused
  (`!contains_doc_margin` on the `COMMAND` arm), a `DtxProse` reflow that
  commits anything outside the `%` margin abandons the reflow
  (`LineBuilder::margin_escaped`), a paragraph whose first line is unmargined
  never gains a canonical margin (`dtx_paragraph_starts_margined`), and a
  doc-margined out-of-region expl3 run is no longer prose-reflowed
  (`run_carries_doc_margin`).
- **31 `non-fixed-point` additions** — latex3 2 → 19, latex2e 11 → 23. Every one
  is in a file that was already recorded: the trivia check stops at the first
  failing variant, so removing a file's `content-change` failure exposes the
  non-convergence behind it. The original inventory note predicted exactly this.
- **16 `idempotency` additions to the `all` sets** (latex3 15 → 16, latex2e 13 →
  28). All `.dtx` plus `array-2024-06-01.sty`, and all already recorded in the
  `trivia` sets: these are pre-existing layout non-convergence that the old
  `.dtx` `Preserve` default hid from the production path. The flip makes them
  reachable at the default; the fix is the S-series work, not a wrap default.
- **No file that previously passed now fails**, in either gate, and the `all`
  sets contain **no** `content-change` entry — production formatting corrupts
  nothing. pgf is byte-identical in both gates. *(Corrected later: the `all`
  gate did not yet run the non-trivia-content comparison, so it could not have
  recorded a `content-change` entry whatever the formatter did. See the
  content-change note below — production formatting did corrupt 80 of these
  files.)*

Re-recorded once more after **a relation became an acceptable expl3 `N` slot**
(`\int_compare:nNnTF {…} = {1}` no longer degrades to a fallback statement): one
`non-fixed-point` entry resolved (latex3's `xpackages/xor/xo-or.dtx`), no
additions.

Re-recorded once more after the **hugging fallback fill** (`Ir::HugFill`), which
retired the last of the forced-break dispatch inside fallback statements: an
atom that carries a forced break is measured by its first line, so a fallback
line places it without reading the non-pass-invariant forced-break predicate at
all. **17 `non-fixed-point` entries resolved** — latex3 45 → 34 (`l3fp-aux.dtx`,
`l3fp-trig.dtx`, `l3chk.dtx`, `l3tree.dtx`, `l3galley.dtx`, `xgalley.dtx`,
`xcontents.dtx`, `xfm-test-cls.dtx`, `xo-footnote.dtx`, `xo-grid.dtx`), latex2e
105 → 98 (`ltmeta.dtx`, `latex-lab-firstaid.dtx`, `latex-lab-l3doc-tagging.dtx`,
`latex-lab-tikz.dtx`, `latex-lab-title.dtx`, `l3pdffield.sty`,
`tagpdf-debug.sty`) — with **no additions** in any of the six sets and pgf
byte-unchanged. Production output moved in 19 files across the three corpora and
every hunk is a *join* (no hunk emits more lines than it replaced): head↔block
pairs stay on one line (`\vbox to \Gin@req@height{%`,
`\hbox_set_to_wd:Nnn \l_shipout_box \l_shipout_box_wd_dim`) and authored
abutments onto a block's closing brace (`}\@ehc`, `}.`, `}{`) re-glue.

Re-recorded once more after **`--checks all` gained the non-trivia-content
comparison** and the **`]`-deletion fix** it exposed. Two independent movements:

- **3 `content-change` entries resolved** (latexindent's
  `oneSentencePerLine/pcc-program-review3*`), by fixing `splice_prose_group`: it
  matched *any* closer as a prose group's delimiter, so a `]` inside a brace
  argument (`\emph{a [b] c}`) was pulled out of the body and then overwritten by
  the group's real `}` — deleted outright, at default settings, no perturbation
  needed. The `open` arm had always been guarded (`open.is_none()`); the `close`
  arm now takes the node's own matching kind. Pinned by
  `reflow_bracket_in_prose_argument`.
- **92 `content-change` additions to the `all` sets** (latex2e 69, latex3 11,
  latexindent 12) — **no new bugs**. The `all` gate never ran the
  whitespace-only comparison, so this class was invisible to it; the counts land
  exactly on the `content-change` totals the *trivia* sets already recorded
  (latex2e 69, latex3
  11) plus latexindent's `figureValign` family (12), which is what the
      classification note below predicted when it said these reproduce "without
      any perturbation via `badness format --wrap reflow`". The gate now says
      so.

Net: `all` grew latex2e 28 → 97, latex3 16 → 27, latexindent 190 → 202, pgf
unchanged; `trivia` shrank latexindent 161 → 158, the others unchanged. No file
that previously passed either gate now fails.

Re-recorded once more after the bib parser learned **`%` comments**. The 33
`.bib` `format-error` entries below (`keyEqualsValueBraces/contributors-mod*`)
are gone: `latexindent.all.txt` 202 → 169, every other set byte-unchanged, no
additions. The over-strictness was `%`, not the blank line the inventory note
below guessed at — the blank-line variants already parsed. Ground truth is biber
2.21 / btparse, where a `%` runs to end of line outside braced and quoted
values; classic `bibtex` 0.99d has no comment syntax and rejects all of these,
and texlab models none either (recorded as a deliberate deviation in
`bib_parse_compat_allowlist.toml`).

Re-recorded once more after **a prose argument's edge comments took
`lower_bracketed`'s two guards** (`lower_prose_group`). The soft group a prose
argument is wrapped in could render flat with a `%` inside it, so a body ending
in a comment came out `\caption{x%}` — the closing brace *commented out*, a
content deletion — and a comment glued to `{` was relocated to its own line,
inserting a space token into the group. **The whole `commands/figureValign-mod*`
family is gone**: `latexindent.all` 181 → 145 (12 files × `comment-change` +
`content-change` + `idempotency`), `latexindent.trivia` 157 → 145 (12
`content-change`), no additions, the other six sets byte-unchanged. Production
output moved in 59 files across all four corpora and every hunk is one of the
two shapes: a `{`-glued `%` re-joining its opener (a *join*: `\title{\n  %` →
`\title{%`, the bulk of it), or a `{%}` splitting into `{%` / `}`.

**No re-record** was needed for the **opaque-group width-driven layout** (the
retirement of the last Tier-1 `spans_multiple_lines` read): all eight sets are
byte-unchanged — no additions, no resolutions. Getting there took three
convergence rules the gate itself surfaced during development (an edge blank
erases rather than declining, a non-single-space edge glues verbatim, and the
command-only residue is off inside `ReflowKind::ProseArg` bodies — the
latexindent `poly-switch-blank-line`/`mand-args` families and pgf's coil tables
and `pgfmanual-en-tikz-transparency.tex` were the reproducers). Production
layout moves broadly (multi-line brace groups rejoin or refill at width), but
every trivia/`all` failure family recorded here is orthogonal to it. The
strict-survey drop (top of this file) is the change's measurable win.

Over the pinned gate corpora fetched by `task gate-corpora:fetch`
(`scripts/fetch_gate_corpora.sh`):

  | corpus      | repo @ pin                                                             | files |
  | ----------- | ---------------------------------------------------------------------- | ----- |
  | latex3      | `latex3/latex3` @ `3d1d347d8937863c0786988b14d307a6091ee397`           | 288   |
  | latex2e     | `latex3/latex2e` @ `3a9fdd88bdc53f16a0c2158aa70d259607de333a`          | 384   |
  | pgf         | `pgf-tikz/pgf` @ `1c7fc0fdc3ec8a6bdcfd68785c6bbd43ec110178`            | 397   |
  | latexindent | `cmhughes/latexindent.pl` @ `748f0f68397793b4646fa48762b0041b889cfcb4` | 5329  |

`latexindent` is not package source and is read differently from the other
three. It is latexindent.pl's own test suite (`test-cases/` plus
`documentation/`): \~5.3k small hand-written files of deliberately adversarial
LaTeX — blank lines in display math, verbatim-argument commands, unmatched
braces, alignment torture, 120 `.bib`. Where the other three are expl3-heavy
`.dtx`/`.sty`, this is document-level pathology, so the two overlap barely at
all. The median file is \~200 bytes: a failure here is a near-minimal repro
already, which is most of its value.

**The gate reads inputs only; its expected outputs are not an oracle here.**
latexindent's harness writes results in-tree and asserts `git diff` is clean, so
every committed `*-mod1.tex`/`*-output.tex` is the output of one specific YAML
settings stack from that tool's config model — and latexindent is an *indenter*
that preserves author line breaks, where badness owns layout outright. Those
files are a different function's answers, not a stricter or looser version of
ours, so a divergence from them is not a failure and nothing in this directory
compares against them. (For *fixture authoring* they are a legitimate soft
target where our own tenets underdetermine a construct — the guards are in
`.agents/skills/formatter-fixture/`. What neither use permits is matching them
case by case, which would reverse-engineer another tool's config surface into
badness's rules.) The corpus is GPL-3.0 to badness's MIT, which the
fetch-don't-vendor setup (`corpora/` is gitignored) already keeps clean; a
fixture derived from a case here should be hand-authored, never copied.

## Checking and regeneration

The ratchet is machine-checked: `task gate-corpora:check`
(`scripts/check_gate_baselines.sh`) re-runs both gates over the pinned corpora
and diffs the distilled failure sets against these files, two-sided — an added
line is a regression, a removed line is a stale baseline, and either fails the
check. It needs the corpora (`task gate-corpora:fetch`) and network, so it is a
pre-merge/re-record step for formatter changes, not part of `cargo test`.

To regenerate by hand, from each corpus root with a release build:

```sh
badness --no-config debug format --checks all --report .     # -> <corpus>.all.txt
badness --no-config debug format --checks trivia --report .  # -> <corpus>.trivia.txt
```

Distill each report's `### k. \`path\`
(kind)`headings into sorted`path<TAB>kind`lines (`.all.txt`), adding a third class column for the trivia files from each failure's`-
Variant:`reason (`.trivia.txt`) — the same distillation`check_gate_baselines.sh`implements, so its diff output can be applied to the baseline files directly. The width sweep behind`SWEEP.md`reruns both checks with`--line-width
60\|72\|100\|120\`.

## The sets

- `<corpus>.all.txt` — the pre-existing `--checks all` gate (losslessness +
  idempotency at width 80). At S0, latex3's 16 (15 `format-error` +
  `latexrelease.sty` idempotency) matched the baseline recorded in TODO.md
  before S0, i.e. S0 changed no production layout; S2 resolved the
  `latexrelease.sty` entry, leaving latex3 at 15 `format-error`.
- `<corpus>.trivia.txt` — the `--checks trivia` convergence-oracle inventory at
  width 80 (wrap pinned to reflow). Counts: latex3 149 (151 at S0), latex2e 141
  (146 after the grouped-sibling-walk fix, 148 after S4), pgf 15.
- `SWEEP.md` — failures that appear or vanish across widths 60–120; each is a
  column-arithmetic hybrid candidate.

## The `latexindent` inventory at first record

190 `all` (178 `format-error`, 12 `idempotency`) and 161 `trivia` (145
`format-error`, 15 `content-change`, 1 `non-fixed-point`) over 5329 files. (Now
145 / 145 — see the re-record notes above.) Both gates run in seconds — 1.4s and
16s — because the files are small. Triaged into families, most of the
`format-error` bulk is the corpus being adversarial on purpose rather than a gap
on our side:

- **`unmatched \]` — 102 files, almost all `test-cases/specials`.** A blank line
  inside `\[…\]`, which is a genuine TeX error ("Missing $ inserted"), so the
  shape gate refusing it is correct modeling. Corpus noise, not a bug.
- **`expected ',' between fields` — 33 `.bib`,** the
  `keyEqualsValueBraces/contributors-mod*` family: a blank line or comment
  between a field value and the following `,`. **Now fixed** — the cause was the
  `%` comment alone (the blank-line variants always parsed), and the bib parser
  was over-strict. See the last re-record note above.
- **`unclosed {` — 18 files,** dominated by the `href` family in
  `test-cases/verbatim` and `test-cases/fine-tuning`:
  `\href{…%20for%30Spoken…}`, a URL with literal `%`. hyperref reads that
  argument verbatim-ish, so the `%` is not a comment. A real semantic-layer gap
  (verbatim-argument modeling), and precisely what latexindent's `verbatim/`
  directory exists to probe.
- **`unclosed environment` / `unmatched }` — 20 files,** mostly deliberately
  partial documents (`test-cases/broken/`, files carrying `\end{document}` with
  no `\begin{document}`). Corpus noise.
- **12 `idempotency`, all `commands/figureValign-mod*`** — one family, not
  twelve: `%`-terminated argument braces (`\includegraphics[…]%\n{%\n…%\n}`).
  **Now fixed** — the shape that actually bit was the *empty* one,
  `\caption%\n{%\n}`, whose prose-argument group rendered flat as `\caption{%}`
  and commented its own closing brace out. See the re-record note above.

The 15 `content-change` entries were the severe class and reduced to **two**
causes: the 12 `figureValign` files above, and the three
`oneSentencePerLine/pcc-program-review3*`, which minimized to a **production
content-deletion bug** — `\emph{a [b] c}` formatted to `\emph{a [b c}`, dropping
the `]` at default settings in signature-known prose arguments (`\emph`,
`\textbf`, `\footnote`; `\caption`, `\section`, unknown commands and bare groups
were unaffected). **Both are now fixed**: the `]` deletion first (see the
re-record notes above), then `figureValign`, which had meanwhile moved into
`latexindent.all.txt` once that gate learned to see this class. `--checks all`
reported the bug clean at first record — the non-trivia-content oracle lived
only in the trivia path and in `assert_format_invariants` — which is why closing
that hole was done first, so the bug failed a gate before it was fixed.

A class of finding these two gates cannot flag at all: a layout decision that
*reads* the forbidden lone-newline predicate but is self-consistent on both
spellings. The sectioning-command line break was one (`\subsection{X}\nprose`
kept the break while `\subsection{X} prose` glued) and has since been fixed; the
command-only-line rule was the same shape and is now fixed for **curated block
commands** (`CommandSig::block` intercepts them as block-level statements), with
a residue re-filed in `TODO.md` for un-signatured and scanned-definition
commands, whose block-ness only the authored break can carry. Neither gate moved
when the sectioning half landed, which is what `--checks trivia-strict` exists
to cover.

## Classification (third column of `.trivia.txt`)

- `format-error` — the formatter refuses the file (statically unmodelable
  constructs; matches the smoke-test workflow's ALLOWLIST families). latex3 15,
  latex2e 13, pgf 15, latexindent 145. pgf has **no** trivia failures at all;
  latexindent's 145 are triaged by family in the section above and are mostly
  deliberately-invalid input.
- `content-change` — `fmt` violated the whitespace-only invariant on a perturbed
  input (latex3 132, latex2e 117). Two root causes identified, both
  **pre-existing reflow-on-`.dtx` doc-layer bugs**, reachable without any
  perturbation via `badness format --wrap reflow`:
  1. prose reflow relocates `^^A` doc comments into positions where they re-lex
     as content (the l3 `\title{...^^A...}` preamble pattern — essentially every
     l3 `.dtx`);
  2. content on docstrip-guarded lines is dropped entirely (`alltt.dtx` loses
     `\ProvidesFile{alltt.drv}`). These dominate the trivia sets and mask any
     later failure in the same file (the check stops at the first failing
     variant), so fixing them will both shrink these sets and potentially
     surface currently-hidden non-fixed-points.
- `non-fixed-point` — a perturbed variant formats to a non-fixed-point: the
  idempotency-hybrid family trivia-invariant layout exists to fix (latex3 2,
  latex2e 11; predominantly expl3 package code — xparse, xtemplate, tagpdf,
  pdfmanagement, luamml). `latexrelease.sty` was the known issue-#97 residue
  (rest-aware fill disagreement), resolved by the honest-`Mode::Flat` work.

The generator's meaning-safety was spot-checked: 1 `dropped_unsafe` variant in
\~157k eligible gaps over 120 latex2e files.

## Re-record: arity-directed expl3 attachment (decision #8 landed)

The parser now attaches in-region colon-suffixed heads by argspec arity (the
staged migration TODO.md carried; oracle-verified over 67k statement-leading
heads before the flip). Invariant gates: no additions anywhere, and
`latex2e.all` **shrank** — `ltfssdcl.dtx` and `ltpara.dtx` no longer fail
idempotency, since structural argument ownership removed the greedy-tree
boundary accidents behind their hybrids.

The `non-fixed-point` survey churned within its class, recorded here over two
re-records (the flip itself, then the fallback-boundary and bare-argument-glue
rules that stabilized the shapes the flip surfaced — `fallback_line`'s
bare-line-break boundary and `lower_expl_code`'s unbreakable bare-operand gaps).
Net: six entries healed (`ltfssdcl.dtx`, `latex-lab-testphase-bookmark.sty` in
latex2e; `l3debug.dtx`, `l3check.dtx`, `l3kernel-extras.dtx`, `xhj.dtx` in
latex3) and five appeared (`latex-lab-firstaid.dtx`,
`latex-lab-l3doc-tagging.dtx` in latex2e; `l3ldbparse.dtx`, `xo-here.dtx`,
`xo-or.dtx` in latex3). The new entries are the familiar shape —
`all-newlines-to-spaces` over fallback-heavy `\exp_after:wN` chains failing to
re-fill to a fixed point — i.e. the Tier-2 authored-line residue the strict
survey exists to track, not a new invariant risk (losslessness and idempotency
stay green on every one of them). `HYBRID_TEX` in `tests/debug_format.rs` is a
hand reduction of that same `\exp_after:wN`-chain family rather than of any one
entry: the corpus files it was reduced from re-fill to a fixed point at the
whole-file level, which is why none of them is a baseline entry.

A latent, pre-existing idempotency bug surfaced during this triage and is *not*
part of the migration: a trailing comment after `\ExplSyntaxOff` relocated to
its own line rebinds as the next construct's doc comment (reduced from
`array-2024-06-01.sty` lines 380–392; reproduces identically on the
pre-migration tree). Recorded in TODO.md.
