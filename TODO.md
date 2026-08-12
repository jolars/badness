# Badness TODO

A LaTeX formatter, linter, and language server on a lossless rowan CST,
following **rust-analyzer's** architecture. See `AGENTS.md` for load-bearing
design decisions and invariants.

Single-crate package (not a workspace). Parser and formatter are **intentionally interleaved**: the formatter is the primary tool for stress-testing the parser.

Status: `[ ]` todo · `[~]` in progress · `[x]` done

## Parser

- [ ] **Arity-directed expl3 attachment (decision #8's recorded candidate
  deviation).** In-region, the argspec suffix rides in the `CONTROL_WORD` token,
  so attachment directed by `semantic::expl3::expl3_slots` would be exactly as
  text-pure as greed — and greedy is a systematically wrong guess there (every
  `N`/`V` slot breaks the run, so `\tl_set:Nn \l_a {x}` attaches `{x}` to the
  definee; the formatter's S4 peel-back exists only to undo this). Keys on token
  shape alone (colon-suffixed names only lex as one token in-region — no grammar
  region-awareness needed); `w`/`D`/colonless fall back to greed. Deliberately
  unimplemented until the migration questions have answers: the mixed-shape CST
  every consumer must then handle, the false-positive blast radius moving from
  layout into the tree (linter/LSP see wrong structure where today only layout
  pays), and the parse-compat divergence ledger vs. texlab. Natural trigger: an
  LSP feature needing correct argument ownership in-region (expl3 signature
  help). The semantic statement model is the migration's differential oracle —
  test grammar attachment against `semantic::expl3`'s segmentation over the
  gate corpora before flipping any consumer. Rationale in `docs/src/development/architecture.md`
  (§ *Argument grouping and bracket policy*).

- [x] ~~**Conditional block structure (`\if…\else…\or…\fi`): a gated `CONDITIONAL`
  node.**~~ **Landed.** The `latexindent` corpus's largest uncovered construct
  (402 files), surveyed under the formatter-fixture skill and handed to the
  parser because every formatter-only rule for it was trivia-reading,
  typeset-unsafe, or lopsided. `\if…\else…\or…\fi` now parses as a
  `CONDITIONAL` of `CONDITIONAL_BRANCH`es behind a shape gate that demotes
  silently, and the formatter lays it out all-or-nothing, so
  `\ifnum1<2 b \else c \fi` and the same content spelled across lines are one
  fixed point instead of two. Rationale in `AGENTS.md` decision #1 and
  `docs/src/development/architecture.md` (§ *The conditional gate*,
  § *Conditionals*).

  The gate's load-bearing lesson, recorded because it is not obvious: **the token
  scan must reach the same closer the recursive walk will.** Counting a `\fi` the
  walk consumes inside another construct promises a pairing it cannot honor, and
  the walk then runs on looking for a closer that is gone — `ltboxes.dtx` puts
  three `\fi`s inside a `$…$` and carried the construct over 160 lines and every
  `macrocode` chunk between, stranding the cursor past `macrocode_end` for every
  chunk-bounded scan downstream. Hence math and environment level tracking
  alongside braces, `macrocode` frames as hard boundaries, and the walk bounded
  by the located closer index. Fixed two latex2e gate baselines whose formatting
  had been corrupting content (`ltdirchk.dtx`, `ltfsstrc.dtx`).

  Three pieces deliberately deferred:

  - **Conditionals spanning a blank line** (~11% of corpus occurrences) demote,
    because the gate anchors on a paragraph break exactly as the `$`/`\[` gates
    do. That keeps `CONDITIONAL` a within-paragraph construct — it can never
    straddle a `PARAGRAPH` boundary, so no paragraph nests inside one. Those
    keep their pre-node layout, and with it the two-fixed-point bug.
  - **Math-mode conditionals.** `math_atom` carries its own copy of the
    environment gate; conditionals are text-mode only for now.
  - **expl3 in-region conditionals** (`\if_int_compare:w … \else: … \fi:`, 14.5%
    of corpus openers) are skipped: the formatter owns in-region layout through
    `semantic::expl3`'s statement segmentation, and a node there would contend
    with it.

- [ ] **Lexer state-machine cleanup (audit follow-up; token stream
  unchanged).** The mode catalog is sound; the state handling is accreted.
  The whole machine is 16 mutable locals inside the 411-line `lex_with`, 11
  of them booleans, and the four one-shot `pending_*` flags are hand-cleared
  at seven sites with *inconsistent subsets* — `pending_char_constant` is
  carried silently across several early-`continue` branches, probably
  unreachable but undocumented either way. One coherent refactor: a `Lexer`
  struct over the locals (`lex_with` becomes a short loop over `try_*`
  methods); `Option<Pending>` replacing the four flags (the arming command
  sets are mutually exclusive, so one slot is faithful, and the seven reset
  sites collapse to `pending = None`, resolving the carry-through asymmetry
  by construction); `Option<MacrocodeSave>` folding `in_macrocode` +
  `saved_at_letter` + `saved_expl_syntax` so the save/restore pairing is
  type-enforced. Deduplicate while there: the `\begin`/`{`/name/`}`
  four-token push appears three times verbatim, the `\begin{name}` probe
  twice, inline-whitespace skipping five times, and every control word's
  letter run is scanned twice (`lex_verbatim_command`, then `lex_control`).
  Riders: fix the misattached doc block at `lexer.rs:310–329` (two essays,
  both attached to `is_literal_token_command`, leaving
  `is_char_constant_command` undocumented); retire the `VerbCtx` spelling in
  internal signatures (keep the public alias); kill the per-environment
  `format!("\\end{…}")`; name the curated `ltxdoc`-family class list. The
  losslessness corpus makes the whole change cheap to verify.

- [ ] **Grammar hygiene (audit follow-up; independent of the closer-map plan
  under *Performance & hardening*).**
  - Delete the shadow counters: `group_depth` is always `group_opens.len()`,
    `math_depth` always `math_dollar.len()` — both mutated in lockstep at
    every site, a desync waiting for the first construct that pushes one and
    forgets the other.
  - Deduplicate the DOC_COMMENT precede-splice (`parse_block` vs
    `conditional`, two near-verbatim ~20-line copies; drift breaks comment
    binding in one context only). Longer-term, promote the `precede` idiom
    into the event layer as rust-analyzer's `Marker` does, retiring the four
    manual `events.remove`/`insert` sites.
  - Split `Parser::new`'s fused pre-scan into a testable `PreScan` struct —
    four scans, three pieces of interleaved running state (`def_name_slots`,
    `expl_on`, `opener_scan`), currently testable only through full parses.
    Also where the closer map will land, so it fronts that work.
  - `debug_assert!(!self.at_end())` at the top of `math_atom`: its `None` arm
    consumes nothing and relies on every caller guarding EOF first (all five
    today do); a forgotten guard loops to `PARSER_STEP_LIMIT`. Convert the
    unchecked caller contract into a tripwire.
  - Small helpers: `at_env_end` (the `at_command(END_CMD) &&
    env_name_follows` pair appears ~10 times verbatim), a named blank-line
    constant (`newlines >= 2` restated nine times), reuse `is_trivia` at its
    three inline restatements, and an allocation-free `peek_end_name` (it
    builds a `String` per call inside forward scans).
  - Start carving `grammar.rs` (3,234 lines) into modules: trivia machinery
    and the curated fact predicates first, the math/`\left…\right`
    sublanguage as a candidate second. Fix the stale module doc at
    `src/parser.rs:6` while there — it claims `build_tree` re-attaches
    trivia; that logic moved to the grammar (`binding_run`) and the doc was
    never updated.

- [ ] **Comment consolidation (consolidate, never purge).** Comment density
  in the parser crate is 30–39% per file and overwhelmingly the house-style
  constraint-and-provenance kind — keep that. The cuttable part is
  *restatement*: the lexer states the short-verb semantics in four places
  and the macrocode-frame rules twice; call sites restate 25-line helper
  docs. Cut each fact to one canonical location (the helper's doc) with
  one-line call-site pointers — roughly a third of the comment mass, zero
  information loss. The per-gate re-explanations of the shared scan skeleton
  die with the closer-map work. `catcode_signal` (under *Semantic layer &
  signatures*) is the cautionary tale for why this matters: the real hazard
  at this density is a comment asserting something the code stopped doing.

## Formatter

- [x] ~~**`]` is deleted inside prose-reflowable command arguments.**~~ **Fixed.**
  `\emph{a [b] c}` formatted to `\emph{a [b c}` at default settings — the
  whitespace-only invariant (tenet 1) broken outright, with no width pressure and
  no perturbation needed. Cause: `splice_prose_group` matched *any* closer
  (`R_BRACE | R_BRACKET`) as the prose group's delimiter, so a `]` inside a brace
  argument was pulled out of the body and then silently overwritten by the
  group's real `}`. The `open` arm had always been guarded by `open.is_none()`;
  the asymmetry was the whole bug. The close arm now takes the node's *own*
  matching kind, passed in like [`lower_prose_group`] already did. Kind-matching
  alone suffices — the formatter runs only on clean parses, where a `GROUP` holds
  exactly one `R_BRACE` and the parser ends an `OPTIONAL` at its first `]`.
  Surfaced by the `latexindent` corpus (`oneSentencePerLine/pcc-program-review3*`);
  pinned by `reflow_bracket_in_prose_argument`; 3 `content-change` entries
  resolved, no additions.
- [x] ~~**`--checks all` does not run the non-trivia-content oracle.**~~ **Fixed:
  `CheckKind::ContentChange`.** The comparison lived only in the trivia path and
  in `assert_format_invariants`, so the primary gate, the smoke-test workflow and
  every `*.all.txt` baseline were blind to content corruption that needs no
  perturbation — `--checks all` called `\emph{a [b] c}` clean. `all` now compares
  `nontrivia_content` across the first format pass (`.bib` skipped; the
  comparison is LaTeX-CST-based). Landed *before* the fix above so the bug failed
  a gate first. Re-record surfaced **92 pre-existing** `content-change` entries
  (latex2e 69, latex3 11, latexindent 12) and **no new bugs**: the counts land
  exactly on what the trivia sets already recorded, i.e. the `.dtx` doc-layer
  family below, which the baseline README had already predicted was reachable
  without perturbation. It also falsifies that README's "production formatting
  corrupts nothing" — that held only because the gate could not look.
- [ ] **Trivia-invariant layout: the umbrella fix for the idempotency bug family
  (multi-session).** Recorded as an invariant in `AGENTS.md` and detailed in
  `docs/src/development/architecture.md` (§ *Trivia-invariant layout*). Layout may read only trivia
  predicates the formatter *preserves*; blank lines, comments, and column-0
  margins/guards qualify, **a lone newline vs. a space does not**. Since `fmt(x)`
  is by construction a trivia-perturbation of `x`, layout invariant under trivia
  perturbation is idempotent *by proof* — which is the point: the current regime
  defends idempotence one decision at a time, and the supply of decisions is
  unbounded.

  Enforcement is to delete the information at the boundary — the lowering
  consumes a normalized `Gap = Glued | Space | BlankLine | Comment | Guard |
  Margin` with no `Newline` variant — plus a trivia-perturbation oracle. Tier-2
  modes that are *defined* by authored breaks (`WrapMode::Stable`/`Sentence`/
  `Semantic`, `ReflowKind::Statement`) keep a widened gap and owe a written
  fixed-point argument, as `ReflowKind::Statement`'s flush continuation already
  has.

  **Stages, each gated on the corpus failing-file set not growing** (`badness
  debug format --checks all --report .` over `latex3/latex3` @ `3d1d347`, plus
  `latex3/latex2e` and `pgf-tikz/pgf`; compare *sets*, not counts):

  - [x] **S0 — the oracle, before any refactor.** (a) Run the corpus under
    `assert_format_invariants` at several line widths (60/72/80/100/120): every
    hybrid is a column-arithmetic accident, so widths multiply detection. (b) Add
    the trivia-perturbation oracle itself. No production change; the deliverable
    is the true failure inventory, which every later stage is gated against.
    Expect the count to jump — that is the point.

    *Landed.* The oracle (`formatter::perturb`, `badness debug format --checks
    trivia`, wired into the invariants sweep in `tests/format.rs`) gates on
    **convergence** — every TeX-identical newline<->space perturbation must
    format to a fixed point upholding the invariants — because the strict
    `fmt(perturbed) == fmt(original)` form flags the conservative generic
    path's deliberate authored-break preservation on essentially every file;
    strict stays in the API (`check_trivia_invariance`) as the post-umbrella
    end-state gate. Inventory recorded in `tests/gate_baselines/` (compare
    sets, not counts): `--checks all` @ 80 exactly reproduces the pre-S0
    baseline (latex3 16/288; latex2e 13/384; pgf 15/397), and `--checks
    trivia` @ 80 yields latex3 151, latex2e 148, pgf 15 (pgf: format-errors
    only — fully convergent). The trivia sets are dominated by a
    **pre-existing reflow-on-`.dtx` content-violation family** (see the new
    entry below), which masks later variants in the same files; the
    non-fixed-point residue (latex3 4, latex2e 18) is predominantly expl3
    package code — the S2–S4 target set — plus `latexrelease.sty` (#97
    residue). The width sweep also surfaced ~20 width-dependent idempotency
    files beyond the width-80 baseline (`SWEEP.md`).
  - [x] **S1 — one representation of "forced".** A `propagate_breaks` prepass
    marking every group containing a hard break as `expand`, replacing the three
    current representations (`Ir::Group{expand}`, `contains_forced_break`
    recomputed per decision site, `lower_expl_group`'s hand-rolled branch).
    Behavior-neutral. Makes the landed `group_expanded` fix fall out
    automatically rather than being a special case.

    *Landed.* `Ir::propagate_breaks` — one bottom-up copy-on-write walk at the
    lowering->printer seam — saturates every non-hug group's `expand` from its
    content with `contains_forced_break` semantics (an `IfBreak` shields its
    branches, a conditional group's flat-most candidate decides).
    `lower_expl_group`'s forced form now differs from the soft form only in its
    in-shape `HardLine` boundary, and `Ir::group_expanded` is deleted — the #97
    mode pin falls out of the prepass. Hug groups are never marked (their inner
    is forced by construction), and two measurements stop trusting the flag:
    `flat_width` recurses (a comment-only-forced group still has a flat width)
    and the hug-mode `fits` lets content decide. The latter is S1's one
    deliberate, narrow flip — post-pass the flag cannot distinguish an
    explicitly forced block from a soft group carrying hard breaks — so an
    interior-comment-forced or sibling-coupled block detonating in a head-hug
    prefix now hugs K&R-style (`\global\setbox9 \vtop{%`) instead of splitting
    the head onto its own line. Gate: `--checks all` and `--checks trivia`
    failing-file sets unchanged on all three corpora; a full byte-diff sweep
    against the pre-S1 binary differs on 12 of 1068 files, all this hug family
    (`xpackages` `.dtx`, `latex-lab-amsmath.dtx`, `latex-lab-firstaid.dtx`).
  - [x] **S2 — `Mode::Flat` becomes an honest contract.** Define it as "the whole
    subtree, laid out flat, is verified to fit". Then fix the two producers that
    claim it without checking — `pick_candidate` selects on *first-line* fit
    (`printer.rs`), `Cmd::PreferredFill` pushes atoms flat unconditionally — by
    returning `Mode::Break` (the *choice of candidate* is the decision; children
    then decide for themselves). **Only then** make `Ir::Group` honor an incoming
    `Mode::Flat` instead of recomputing. The three must land together: mode
    propagation alone measured 1 -> 9 idempotency failures precisely because the
    two producers lie. Expect substantial golden churn; hand-derive each.

    *Landed.* The three landed together plus two more liars the honest
    contract flushed out in corpus tracing. `Ir::Group` and both conditional
    arms honor an incoming `Flat` (a nested conditional resolves to its
    flat-most candidate, matching every measurement predicate); `expand` stays
    first, so a saturated forced group never pins. `pick_candidate` announces
    `Break`; `Cmd::PreferredFill` atoms inherit the fill's mode. Flushed out
    in-flight: (1) the `Group` arm measured from the raw `w.col`, dropping the
    pending indent after a newline — the same wrong-column acceptance the
    `AllLines` arm had already fixed for itself; pre-S2 the nested
    re-decisions papered over it, post-pin it printed as overflow, so both
    `Group` measurements and `pick_candidate` now start from `current_col()`.
    (2) `step_fill`'s last-atom flat claim ignored the trailing content the
    lowering glues after a statement fill, so it is now rest-aware like
    `group_fits` — which finally takes the folded continuation hang
    (`\prop_get:cnN {…}{…}\l__tag_…`). (3) The hug's prefix-only `fits`
    cannot claim full `Flat` for content past the first forced break
    (`\@@_if_key_value:VTF {T}{F}` pinned `F` into a 125-column line), so a
    hug now dispatches `Mode::FlatPrefix`: trivia renders flat (the head
    stays glued) but groups re-decide — `group_hug` survives S2, settling
    that uncertainty. No golden churn at all (the fixture set never reached
    the divergence corners); the corpus churn is 54 files, overflowing lines
    strictly reduced (18 files fewer, 0 more). Gate: sets unchanged except
    `latexrelease.sty` leaving both latex3 sets (the #97 residue below —
    resolved by S2 alone, without waiting for S3); baselines re-recorded,
    `SWEEP.md` refreshed (five width-dependent files fully converged; one
    shared `\str_if_eq:` fragment now flips at width 60 via the known
    `SplitAtNewlines` Tier-2 family, S4's target).
  - [x] **S3 — collapse the fit predicates.** With mode propagated, a group inside
    a flat parent is never *asked* whether it fits, so the rest-awareness
    disagreement dissolves rather than needing a patch (S2 already resolved
    the `latexrelease.sty` entry below this way). Delete what is now dead of
    `flat_width` / `first_line_fits` / `all_lines_fit` / `fits` / `group_fits`,
    and make the survivors share one traversal so they cannot drift again — the
    `rest_fits` drift (a later `Group` measured in its real mode, a later
    `ConditionalGroup` measured flat-most) was exactly that failure.

    *Landed, behavior-neutral.* Two shared walkers replace the five bodies:
    `flat_end` with a `FlatMeasure` policy (`Footprint`/`Fits`/`HugPrefix`)
    is the one flat simulation behind `flat_width`, the hug fit, and
    `group_fits`'s flat phase; `line_fits` with a `CommentFit` policy
    (`Fails`/`SharesLine`, the one deliberate context difference) is the one
    first-emitted-newline measurement behind `first_line_fits` and
    `rest_fits`, which shrink to seeds. `fits` and `atom_is_unfittable` are
    deleted; `all_lines_fit` and `print_flat` share the `wide()` probe. The
    `rest_fits` drift is gone by construction: a later conditional group is
    picked via `pick_candidate` (was flat-most), a later hug group measured
    with its hug flags (was plain), a later `Break`-mode preferred fill by
    its first atom (was whole-flat). Gate: all six failing-file sets
    byte-identical to `tests/gate_baselines`, and full byte-diff sweeps at
    widths 60/80/120 differ on 0 of 1069 files — S2 had already removed
    every reachable disagreement, so no baselines were re-recorded.
  - [x] **S4 — Tier 1 for expl3: retire `Statements::SplitAtNewlines`.** Landed
    as `semantic::expl3::expl3_slots` (per-slot arity from the argspec suffix:
    `N V` one token, `n c v o x e f` a brace group, trailing `T F` branches,
    `p` parameter text shape-scanned to the first explicit `{` — TeX's own
    static rule, so the `Npn` family is fully structural; `w`/`D`/unknown
    letters fall back) plus `semantic::expl3::segment_expl_statements`
    (pure-shape segmentation with
    peel-back of greedily over-attached arguments) and the `core.rs` rewiring
    (`Statements::Structural`, boundary-map commits, region toggles as
    zero-arity units). The formatter owns one-call-per-line; a width wrap
    re-derives the same unit on every pass. The fallback (underivable heads,
    plus a unit's same-line trailing junk) is the Tier-2 residue and carries
    its fixed-point argument in `semantic::expl3`: greedy self-refilling
    lines (plain `Ir::Fill`, not sticky), no break before a recognized head
    mid-line, junk-glued statements all-hard. Gate: `--checks all` sets
    byte-identical to baseline at width 80 for all three corpora; both
    width-60 SWEEP symptoms (`xtemplate-2023-10-10.sty`, `lttemplates.dtx`)
    cleared; the strict trivia-invariance oracle holds for recognized-only
    streams (first Tier-1 shapes to pass it). Trivia sets: 4 entries
    resolved (`xtemplate` ×2 — the motivating case — `pdfmanagement.sty`,
    `tagpdf-base.sty`), 4 added from a *different*, pre-existing family
    (mode/rest printer coupling; follow-up below).

  **The "subsumes three entries" claim, corrected:** S4 resolved the expl3
  instances. *Opaque-group layout non-determinism* (`spans_multiple_lines`)
  still governs non-expl3 `Opaque` groups and stays open below; *Residual
  K&R <-> Allman flip* was resolved by S2; *Hanging continuation indent* got
  its "node that owns the whole statement" for expl3 (the call unit) — TikZ
  paths remain out of scope as that entry says.

  **In-flight uncertainties, settled:** greedy `{}`-attachment covers leading
  all-group specs entirely (`\str_if_eq:nnTF {a}{b}{T}{F}` is one node);
  `N`/`V`/`p` specs are exactly the peel-back cases, common enough (every
  `\tl_set:Nn`, every `Npn` definition) that S4 pays for itself. Every Tier-2
  mode now carries a written fixed-point argument (the expl3 fallback's is in
  `semantic::expl3`).

  **S4 follow-ups:**
  - [x] ~~*Out-of-region prefix flips an in-region group's inline/block
    form.*~~ **Misattributed; the real cause was the forced-break dispatch
    firing inside a fallback statement.** The out-of-region prefix symptom no
    longer reproduces at all (`word {g} \ExplSyntaxOn …` and
    `\somecmd {g} \ExplSyntaxOn …` give identical output) — that half went
    with the grouped-sibling-walk fix, along with the `xparse-2020-10-01.sty`
    ×2 entries. The two remaining entries (`lipsum.sty`, `expl3.sty`) were
    neither a printer mode/rest coupling nor mega-line-only: both are plain
    **idempotency** failures at the *default* wrap mode, and both are
    `lower_expl_code`'s node dispatch branching on the lowered child's
    `contains_forced_break()`. Inside a fallback statement that predicate is
    newline-keyed — a width wrap inside the child's body prints newlines the
    reparse re-segments into several fallback statements, so a soft group
    flips forced on pass 2. Every arm of the dispatch reacts by *committing
    the line*, which is exactly the hard sibling gap a `StickyFill` produces
    on its own (a forced atom's `flat_width` is `None`, so
    `step_fill`'s `remainder_broken` fires unconditionally) — hence structural
    and `Ignore` streams agree, and a fallback line's plain greedy fill does
    not. Fixed by gating the **hanging brace group** off that dispatch when
    `in_fallback`; the group still breaks (its `flat_width` is `None`), only
    the *sibling* gap is left to the fill. Gate: 5 `non-fixed-point` entries
    resolved (`lipsum.sty`, `expl3.sty`, `tagpdf-mc-code-generic.sty`,
    `tagpdf-mc-code-lua.sty`, `luamml.sty`), no additions in any of the six
    baseline files, latex3 and pgf byte-unchanged. Production output moved in
    19 files, every diff the same shape: a sibling stranded on its own line
    after a multi-line group (`,`, `{#1}`, `\fi:`) re-glues onto the closing
    `}`. Pinned by `expl_fallback_forced_group_sibling` /
    `expl_fallback_forced_group_glue` and
    `a_multi_line_group_node_does_not_end_a_fallback_line`.
  - [x] ~~*Forced-break dispatch residue: the other three sub-arms.*~~ **Done:
    the head-hug moved into the fill.** A fallback (or junk-glued) line now
    commits as an `Ir::HugFill` — a greedy fill whose atoms, when they carry a
    forced break and so have no flat width, are measured by their *first line*
    (`FlatMeasure::HugPrefix`, `Ir::group_hug`'s own claim) and print
    `Mode::FlatPrefix`. That is the pass-invariant head-hug the entry asked
    for: a soft atom's prefix *is* its flat width, so the soft→forced flip
    across passes cannot move it. With it, **no** arm of the dispatch reads
    `contains_forced_break()` on a fallback line. Two supporting details: the
    fill's rest-awareness is not applied to a hug claim (like `group_hug`'s it
    never covered the rest of the line, and a statement that ends one atom
    earlier next pass must place that atom identically — `xo-place.dtx`), and
    every *early* line commit builds its head with the same fill kind
    `commit_line` would (`line_fill`), or the trailing-command arm's plain
    `Ir::Fill` head breaks the atoms that hugged mid-line. Gate: **17
    `non-fixed-point` entries resolved** (latex3 10, latex2e 7 — the two this
    entry predicted plus fifteen the reflow-default flip exposed), **no
    additions** in any of the six sets, pgf byte-unchanged. Production moved in
    19 files and every hunk is a *join*: the pairs the entry worried about stay
    joined (`\vbox to \Gin@req@height{%`, `\hbox_set_to_wd:Nnn
    \l_shipout_box \l_shipout_box_wd_dim`) and 14 files' authored abutments
    (`}\@ehc`, `}.`, `}{`) re-glue. Pinned by `expl_fallback_hug_head`,
    `expl_fallback_abutting_sibling`, `dtx_expl3_fallback_head_fill` and the
    `hug_fill_*` printer units.
  - [x] ~~*In-region `BracketPolicy` audit*.~~ **Verified stable.** Only
    `Greedy` and `Forbid` are reachable in-region — `Tight` rides the curated
    math `\begin`, demoted to a plain command by the issue-#60 carve-out
    (`in_macro_code`) — and every gate and closer-reachability scan treats a
    space and a lone newline identically, so the only perturbation attachment
    could see is a created/removed *flush* junction before a `[`, which no
    layout path produces (the R3 respace skips `OPTIONAL`, the fill breaks at
    authored gaps only, a math command is one verbatim atom). The issue-#55
    second-order scan flip needs a bare flush `[` with a reachable closer,
    which cannot exist (flush + reachable ⇒ attached). Detail in
    `.claude/rules/formatter.md` (§ *expl3*); pinned by
    the `expl_bracket_attachment` fixture and `bracket_attachment_stability`.
  - [x] ~~*Sibling-attached branch explosion*~~ **Done: the slot mapping now
    escapes the scan.** `consume_unit` already resolved these branches (that is
    why the call is one unit), it just discarded which slot took what —
    `Group | Branch => take_group()`. It now records each `Branch`'s range in an
    `Expl3Unit`, and `expl3_unit` exposes the scan for one head so the formatter
    can ask without a `StatementMap` (the layout runs inside a command's attached
    arguments too). `lower_expl_conditional_unit` splits the unit at the first
    branch — the owning sibling's leading children finish the head line, the rest
    are the branch list — and requires the tail's groups to be *exactly* the
    recorded branches, which rejects both an over-attached trailing group and a
    group that merely contains a branch deeper down. So all four shapes now
    explode alike: branches on the head (`\tl_if_empty:nTF`), peeled off one
    sibling (`\seq_if_in:NnTF \l_seq {item}`), split across two
    (`\prop_get:NnNTF \p {k} \l`), and at the stream level once a `WORD` relation
    breaks attachment (`\int_compare:nNnTF {a} = {1}`). Gate: all six
    `gate-corpora` baselines byte-identical, and the corpus-wide two-pass
    non-idempotent set is the same 16 files before and after. Production moved in
    58 files / 300 hunks (latex3 25, latex2e 33, pgf 0), every one a branch list
    moving to +2 — these calls were *collapsed onto one line* before, so the
    formatter was undoing the house style on correctly authored code. Pinned by
    `expl_conditional_sibling_branches` and the now-registered
    `expl_relation_slot_statement` (committed in `4a3d92b`, in no fixture table
    until now, so it had never run).

    Two deliberate remainders. The **trailing (mid-line) arm** stays
    head-attached-only: mid-statement the conditional is not the head of its own
    unit, the segmentation already decided it is an argument being passed as a
    token, and re-scanning it as a head claimed the *outer* call's arguments as
    branches — a misread at all eight latex2e/latex3 sites it reached
    (`\@@_patch_check:NNnn \cs_if_exist:NTF #1 { undef }`, `\exp_not:N \…:nTF`).
    Pinned by `expl_conditional_sibling_trailing`. And `lower_node`'s node-keyed
    all-or-nothing arm keeps needing head-attached branches by construction: it
    has no sibling stream to resolve a unit from.
- [x] **The sectioning line break reads the lone-newline predicate (Tier-1
  violation, no oracle catches it).** `\subsection{X}\nprose` kept the break;
  `\subsection{X} prose` glued the prose onto the head line — the same bytes to
  the next parse, so exactly the predicate the trivia-invariant-layout invariant
  forbids reading. No gate fired: the perturbation oracle only reports a
  *content* change or a non-fixed-point, and both spellings were self-consistent
  fixed points. Fixed by making a sectioning command a block-level statement — a
  break before it and after it, read from the signature DB's
  `CommandSig::sectioning` (`command_is_sectioning`) rather than the source
  trivia, so headings no longer reach `line_is_command_only` at all. Pinned by
  `sectioning_starts_own_line` and `sectioning_blank_line_and_comment`. **The
  rest of the family is still open:** the strict oracle
  (`fmt(perturbed) == fmt(original)`, already written in `perturb.rs` and
  currently failing wherever an authored break is deliberately preserved) is the
  only mechanical route to it — the corpus surfaces this class by eye only, and
  the four gate baselines did not move when this one landed.
  Surfaced by the `latexindent` corpus (`oneSentencePerLine/`).
- [ ] **`commands/figureValign-mod*`: 12 idempotency + `content-change` failures,
  one family.** `%`-terminated argument braces
  (`\includegraphics[…]%\n{%\n…%\n}`) — a comment ends every line inside an
  argument group, so the layout's comment handling and its argument grouping
  disagree across passes. All 12 `latexindent.all.txt` idempotency entries and 12
  of the 15 `content-change` entries are this one shape. Minimize before fixing;
  the files are large but the construct repeats.
- [ ] **Math operator spacing is inconsistent between script args and command
  args** (surfaced by issue #42's examples). A braced script argument is lowered
  through the math seq path and gets operator spacing (`\sum_{i=1}^m` ->
  `\sum_{i = 1}^m`, `\Big \}^{1/2}` -> `\}^{1 / 2}`), while a command argument in
  math mode (`\frac{1}{n^{m+1}}`) is left untouched — the two should agree.
  Related conventions question: `/` (and arguably `*`) is conventionally set
  tight (`1/2`, per Knuth), and script-size content is conventionally tight
  overall, so the likely resolution is tight `/` everywhere and no operator
  spacing inside `^`/`_` arguments — decide, then make both paths agree.
- [ ] **Opaque-group layout non-determinism.** The content-kind taxonomy has
  landed: `ArgSpec` now carries a `ContentKind` enum (`Opaque`/`Prose`/
  `TokenList`/`Keyval`) the formatter dispatches whitespace and break policy on
  (`DocumentBody` stays an environment-body concept via
  `EnvironmentSig::no_indent`; add it when a command-arg case appears). What
  remains is the non-determinism fix: `spans_multiple_lines` decides
  block-vs-inline from incidental source newlines, sidestepped for the
  `TokenList` and `Keyval` kinds but still governing every `Opaque` multi-line
  *brace* group. Give `Opaque` groups a deterministic layout policy that does not
  depend on incidental whitespace. *(An instance of the trivia-invariant-layout
  violation above — `spans_multiple_lines` reads the unsafe lone-newline
  predicate. Fix it under that umbrella, Tier 1, not on its own.)* The `[…]` half
  of this is done: an optional argument is now a group over its top-level entries
  (`docs/src/development/architecture.md` § *Optional arguments, tables, and math spacing*), so `lower_optional` reads the
  predicate only under `WrapMode::Preserve` and friends, which are defined by it.
- [ ] **Long collapsed cite list overflow.** A `collapse` arg folds to one line
  even when the key list exceeds the width; it never breaks *at commas* (one
  key per line) as a fallback. Needs the token-list content kind to break on
  its own separators rather than the paragraph fill.
- [ ] **Formatter-owned trailing comma (parked; the last piece of issue #47).**
  A `[…]` is now a width-driven group over its top-level entries, and a
  `ContentKind::Keyval` argument may also break at a glued comma
  (`docs/src/development/architecture.md` § *Optional arguments, tables, and math spacing*). What is left of the old parked
  item is the Black-style trailing comma: for a proven-keyval argument, add the
  `,` when expanded and drop it when collapsed — safe as *TeX*, because
  keyval/xkeyval/pgfkeys/l3keys and `\ProcessOptions` clists all ignore empty
  entries. **Blocked on a tenet, not on data:** inserting or deleting a `,` is a
  non-trivia token edit, which the whitespace-only invariant forbids and
  `assert_format_invariants` actively catches. Landing it means amending that
  invariant and its oracle to carve out this one insertion — a decision worth
  taking on its own, not as a ride-along. The count-based *expansion* half was
  declined: width alone is already canonical, and an N-key threshold would need
  the comma count to proxy for keyval-ness, exploding comma-rich textual
  optionals. The Black/Ruff *magic trailing comma* (a trailing `,` in the
  **source** forcing one-key-per-line) stays declined too — content steering
  layout conflicts with the formatter-is-sole-authority tenet.
- [ ] Widen the prose-argument table (CWL ingest could feed it); consider gluing
  a prose arg onto its command line when a source break separates them.
- [ ] **Key-value continuation indent in an expl3 fallback statement (open scope
  call).** A key whose value continues on the next line should indent the value
  one step, which is what an author writes and what upstream overwhelmingly does
  (a sweep of latex3's `.dtx` sources: of 87 code lines ending in `=` with the
  value on the following line, **65 indent it by +2**, the rest split between 0
  and incidental alignment):

  ```tex
  ,begin-vspace:e =
    \tl_if_empty:nTF {#2}
      { \newtheoremstyle@vspace@default }
      {#2}
  ```

  Badness neither produces nor preserves this: it emits the continuation *level
  with the key*, discarding an authored `+2`. The cause is structural, not a
  layout bug. `,begin-vspace:e = ` is a **fallback** statement (no derivable
  arity), and in a fallback stream a newline is a statement *boundary* — the
  Tier-2 residue. So `\tl_if_empty:nTF …` on the next line is a *sibling
  statement* at the same base indent, not a continuation of the entry; and
  indentation is always computed, never read, so the author's step is dropped.

  Emitting +2 needs the formatter to know that `,key = ` is an *incomplete*
  entry — key-value modelling inside a stream it explicitly declines to model.
  The narrowest form is a rule like "a fallback statement whose last non-trivia
  token is `=` hangs its successor +2", which reads only non-trivia token
  content and is therefore trivia-invariant and permissible. **The open call is
  whether to have it at all**: the l3styleguide is silent on key-value dialects,
  so this is badness inventing layout for a dialect it cannot name — the same
  objection recorded against the 2e-brace-tightening entry below. Upstream's
  65/87 gives the rule an empirical basis; the tenet-#1 pressure is that a
  `Keyval` content kind (see the parked keyval entry above) would be the
  principled carrier, not a token-shape heuristic in `lower_expl_code`.

  Surfaced while fixing the sibling-coupling and all-or-nothing conditional bugs
  (issue #101); the conditional fix removed the *worse* half of this shape (the
  branch list no longer splits across two indents), leaving only the
  continuation indent itself.
- [ ] **Revisit tight braces for 2e-named commands inside expl3
  (`expl_group_is_spaced`).** The rule gives an expl3 function's argument
  canonical `{ value }` spacing (documented l3 style, per the l3styleguide) but
  tightens a 2e-*named* command's argument to `{tight}`, so
  `\@ifpackageloaded { textcomp }` becomes `\@ifpackageloaded {textcomp}`. The
  spaced form for expl3 functions is genuinely idiomatic l3; the tight form for
  2e commands is badness's own extrapolation (the style guide is silent on 2e
  code embedded in a region), chosen for determinism. Whether tightening — vs.
  preserving, or spacing — is the right default for a 2e command in an expl3
  region is an open call; the tightening can read as worse than the input.
- [ ] **Hanging continuation indent for wrapped statements (B', deferred ---
  blocked on structure).** A wrapped brace-body line ideally hangs its continuation
  one step in (`\node[…] at (2,3)`/`····{…};`) to read as a continuation rather
  than a sibling. This **cannot be idempotent** under the generic CST: the wrap
  becomes a real source newline, and on re-parse the continuation is just a line at
  the body indent (no marker says "continuation"), so the next pass flushes it ---
  `fmt(fmt(x)) != fmt(x)`. Flush-B sidesteps this precisely because there is no
  indent delta. The real fix needs a node that *owns the whole statement*, so layout
  derives from structure (source newlines insignificant). For the motivating case
  (`\node[…] at (2,3) {…};`) that node is a **TikZ path statement**: `at` keyword,
  `(coord)`, `;` terminator, `{label}`—none of which are TeX-surface facts
  (`;`/`at`/`()` carry no special catcode in plain TeX), so grouping them is
  package-specific grammar, out of scope for the generic parser (decisions #1, #2;
  non-goals). Belongs in a future sanctioned **TikZ-aware mode** (its own grammar,
  corpus, and AGENTS.md amendment), not a formatter patch. *(The general form of
  "needs a node that owns the whole statement" is S4 of the trivia-invariant-layout
  entry above, which delivers it for expl3 from argspec arity. TikZ paths stay out
  of scope — no static signal — so this entry survives S4 for `.tex` bodies.)*

## Linter

- [ ] **Config knob for user-declared ref/cite command families (grew out of
  issue #104).** The #104 example still draws `unreferenced-label` on every
  label referenced only through a custom wrapper (`\eqrefs{thm:eq1,thm:eq4}`
  expanding to `\eqref` calls): the semantic builder's ref-family name set
  (`semantic::builder::ref_command`) is fixed, and seeing through the wrapper
  would take macro expansion (out of scope, decision #1). A `badness.toml`
  knob declaring extra ref-family (and cite-family) command names — the
  analog of the parked user-declared-verbatim-envs knob above — would let a
  project name its wrappers; the declared names feed the builder's name sets
  (semantic layer only, never argument attachment, so decision #8's
  text-purity is untouched). Needs plumbing: config does not currently flow
  into `SemanticModel::build`, and the shared name sets also serve completion
  and the LSP, which should honor the same declarations.

- [ ] **Mine the ChkTeX warning catalog (~44 warnings) for missing rules.**
  LaTeX Workshop adds no lint rules of its own (it only shells out to
  ChkTeX/lacheck, both off by default), so ChkTeX's catalog is the source to
  compare against. Badness already covers the high-value territory (ellipsis,
  dash length, straight quotes, `$$`, space-before-`\footnote`, intersentence
  spacing); remaining candidates include space before punctuation or
  parentheses and missing italic correction (`\/`).

- [x] **`codeexample` unknown to the signature DB.** pgfmanual's `codeexample`
  env holds verbatim-like example source that is *also* executed. Because it was
  not in `data/signatures.json` (which lists `verbatim`, `lstlisting`, `minted`,
  `Sinput`, …), the prose rules fired inside it: on the pgf corpus this drove
  ~1900 `straight-quotes`, ~370 `ellipsis`, and ~100 `dash-length` findings, and
  — worse — the *default* (`Safe`) `ellipsis` fix rewrote `...`→`\dots` inside
  executed code (`\immediate\write\w{...}` → `{\dots}`). Resolved by curating
  `codeexample` into the built-in DB as a `verbatimBody` env, following the
  precedent of the equally package-specific Sweave (`Sinput`/`Soutput`/`Scode`)
  and `Code`/`CodeInput`/`CodeOutput` entries; its body now lexes to one opaque
  `VERBATIM_BODY` token, so the prose rules never see it.
  - [ ] *Follow-up (open):* a project-config knob for user-declared verbatim envs
    would generalize this to package-specific envs badness cannot name. Config
    does not currently flow into the signature DB or the lexer's `VerbCtx`, so
    this is a separable feature, not a data edit.
  - [ ] *Out of scope (catcode limitation):* the sibling `|…|` active-char
    shortverb (`\catcode`\|=13` + `\gdef|{…\verb|…}`) drives the same class of
    FP (`straight-quotes`, `unclosed-math-delimiter`, `sectioning-level-jump` on
    `|\part|`, `missing-nonbreaking-space` on `\ref` inside `|…|`) but is a
    genuine catcode limitation, not statically resolvable.

The remaining linter findings from the cam-notes sweep are recorded below as open
follow-ups (each with a minimal reproducer); none is fixed yet.

- [x] **`dash-length` corrupts pgf/TikZ coordinate arithmetic under
  `--unsafe-fixes`.** The `in_math` guard covered only `$…$`, not a pgfplots
  expression in `{…}`: `printf '\\addplot3 {(y^2-1)^2};\n' | badness lint --fix
  --unsafe-fixes` yielded `{(y^2--1)^2}`, a meaning-bearing minus turned into an
  en-dash. Resolved by two shared pgf gates: `in_pgf_picture` (a `tikzpicture`/
  `pgfpicture`/pgfplots-`axis`-family ancestor, so coordinate arithmetic like
  `(2-1,3)` is skipped) and `in_pgfmath_argument` (the `\addplot`/`\pgfmath…`
  expression argument, attached or detached past the numeric `\addplot3` variant),
  where a `-` between numbers is a pgfmath subtraction, not a typeset range.
  - [ ] *Follow-up (open):* prose FPs on index-pair/term names (`0-1 law`,
    `1-2 plane`, `1-1 function` — 22 of 25 findings) where the hyphen is
    intentional; these are `Unsafe`-gated so `--fix` withholds them, so they are
    noise rather than corruption. Distinguishing `0-1 law` from `pages 5-10`
    statically is the open part.

- [ ] **`makeat-macro` residual on plain-`.tex` package internals.** Recognizing
  `*.code.tex` as package flavor fixed 98.9% of the pgf `makeat-macro` FPs, but
  generic-implementation files named plainly (`pgfutil-common.tex`,
  `support/pgf-regression-test.tex` — `\input` under `\makeatletter`, no
  `\makeatletter` of their own, no `.code.tex` signal) still emit ~590 findings.
  There is no clean static signal distinguishing these from a document that
  genuinely forgot `\makeatletter`, so this is a known limitation rather than a
  fixable gap; noted for completeness.

## Semantic layer & signatures

- [ ] **Verbatim-ish command arguments: `\href` with a literal `%` in its URL.**
  `\href{https://…/Chang_1983_Handbook%20for%30Spoken%40Mathematics.pdf}{…}`
  fails to parse (`unclosed {`) because the `%` lexes as a comment start.
  hyperref reads `\href`'s first argument under a modified catcode regime, so the
  `%` is literal there — this is a `ContentKind`-adjacent claim the signature DB
  does not currently carry (a *verbatim argument*, distinct from `Opaque`). 18
  `format-error` entries in the `latexindent` gate corpus, dominated by the
  `href` family in `test-cases/verbatim` and `test-cases/fine-tuning` — which is
  exactly what that corpus's `verbatim/` directory exists to probe. Scope the
  claim like the `Keyval` one: curated, compile-verified per command, never
  inferred. Check `\url`, `\path`, and `\lstinline` in the same pass.
- [ ] How much of `\newcommand`/`xparse` to model for the signature DB. *(open
  decision)*
- [ ] **`catcode_signal` does not meet its own bar** (`semantic/define.rs`).
  It requires `body.contains("\\catcode") && body.contains("12")` with no
  adjacency, so a body carrying `\catcode…=\active` and an unrelated `12pt`
  matches — while the comment claims the two-token requirement is what keeps
  it strict. The failure direction is the bad one: a false positive here
  suppresses real diagnostics, exactly what the verbatim scan is documented
  to avoid. Require the `12` in assignment position after `\catcode` (or at
  least within a bounded window); add the `12pt` counterexample as a test.
- [ ] **Make `EnvironmentSig::reflow`/`block` computed, not stored.** Both
  are derivations of other fields, and mutation sites must hand-sync them —
  `define.rs` writes `sig.reflow = false` manually after setting
  `verbatim_body` at two sites, and a forgotten sync is silent. Computed
  methods remove the field, the hand-sync, and the derivation duplicated
  across the const fns and `From<RawEnvironment>`.
- [ ] **`is_cite_command` accepts any `\cite*`-prefixed name**
  (`semantic/builder.rs`): `\citebox` or `\citecolor` gets its argument
  recorded as citation keys — an open-ended false-positive surface, unlike
  the neighboring closed-table predicates, and nothing documents the choice.
  Either write down why open-prefix recall is intended or close the set.
- [ ] **Semantic-layer hygiene (audit follow-up).**
  - `ast::command_name` (and `ControlWord::name`, `nth_group_text`) return
    `SmolStr`/`&str` instead of `String` — called per command node in every
    tree walk and in expl3's segmentation hot loops; the cheapest real
    allocation win the audit found.
  - Split the completion word-list tiers (package/class names, colors, tikz
    libraries, CTAN metadata, `arg_enums`) out of `signature.rs` into their
    own module — they have nothing to do with signatures, and the file drops
    to ~1,100 lines.
  - Collapse `merge_from`/`merge_from_package` into one origin-parametrized
    helper; table-ize `builder::build`'s four identical key-family arms (the
    layer's only 100+-line function); extract expl3's `is_recognized_head`
    predicate (spelled three ways today); consider per-index flags for
    `StatementMap`'s four parallel `Vec<bool>` so illegal states are
    unrepresentable; hash-map `builder::resolve` (currently O(refs ×
    labels)); move `define.rs`'s private `is_trivia` mirror into `syntax`
    beside `is_collapsible_trivia`.

## Language server

### Feature status vs LaTeX Workshop

A second reference diff, against **LaTeX Workshop** (the dominant VS Code LaTeX
extension). It is not an LSP: its intellisense, hover, and outline are
regex-driven extension code, and its formatting and linting shell out
(latexindent/tex-fmt, ChkTeX/lacheck), so badness already leads on language
smarts. Coexistence is the deliberate story (docs `guide/editor-setup.md`):
LaTeX Workshop keeps build, PDF preview, and SyncTeX. The features it has that
badness lacks and wants are filed in the sections below, tagged *(LW)*:
citation filter-by-title, command argument placeholders, keyval `label={…}`
scanning, graphics hover preview, package-doc hover links, a texmf bib
fallback, and surround/promote-demote code actions. Math-preview-on-hover is
the one big item needing a design decision (see `### Hover` and Open
decisions). Not adopted: `@a`-style abbreviation snippets and two-letter
environment snippets (editor-snippet territory), graphics thumbnails inside
completion items (VS Code-only), and sub/superscript history completion
(niche).

### Configuration & sync

- [x] config over LSP—the LSP now discovers `badness.toml` per document
  (`GlobalState::resolve_settings`, cached by anchor dir, cleared on
  `didChangeConfiguration`). A discovered config wins outright
  (file-wins); editor settings are the fallback. Both `[format]` (`line-width`,
  `indent-width`, `wrap`) and `[lint]` (`select`/`ignore`, applied via
  `RuleSelection` in the analyze/diagnostic/code-action paths) are honored. Two
  follow-ups remain:
  - Deliberately *not* done: plumbing `wrap` (or other knobs) through
    `EditorSettings` itself. A discovered config's `wrap` flows via `FormatConfig`,
    so no new editor knob was needed; `EditorSettings` stays `line_width`/`indent_width`.
- [ ] `workspace/diagnostic` (the workspace-wide pull)—deferred: it is a
  streaming/long-poll protocol (held-open request, per-uri result ids, partial
  results) that fits the one-shot id-bound read-job model poorly. Advertise
  `workspace_diagnostics: true` and add it once that plumbing exists; editors
  drive interactive diagnostics through `textDocument/diagnostic` meanwhile.

### Completion

Badness offers command, environment, label, cite-key, bib field/type, and file
completion (`src/completion.rs`, `src/bib/completion.rs`). texlab's completion
breadth is its biggest lead (`crates/completion/providers/`); the specialized
sources below are missing.

- [ ] *(Design decision)* **Package-scoped command completion.** texlab suggests
  only commands provided by the loaded packages (a package→command component
  model). Badness's signature DB is flat (curated + CWL + scanned); scoping
  completion to `\usepackage`-loaded packages needs package→command attribution.
  Open question, not a mechanical add.
- [x] **Citation completion filterable by title/author *(LW)*.** LaTeX
  Workshop's `\cite{` completion matches on the entry's title and other fields,
  not just the key (`intellisense.citation.filterText`)—type a word from the
  paper's title instead of remembering the key. Badness already resolves
  entries cross-file and lazily (`lsp/completion_resolve.rs`); widen
  `filter_text` (and `sort_text`) on citation items to key + title + authors.
  VS Code truncates `filterText` at 128 chars, so field order matters (key
  first). Done: `cite_completion_items` returns the full namespace (no
  server-side key filter) with `filterText` = key + title + authors and
  `sortText` = key; `title`/`authors` cached on the bib semantic `Entry`.
- [ ] **Command argument placeholder snippets *(LW)*, opt-in.** Environment
  completion already inserts snippet bodies with tab stops; commands could emit
  placeholders for required/optional arguments straight from the signature DB
  (`\frac{$1}{$2}`). Gate on the client's snippet capability and an editor
  setting—LaTeX Workshop's equivalent (`intellisense.argumentHint.enabled`) is
  off by default, since placeholder churn annoys as many users as it helps.
- [ ] **Labels from keyval options *(LW)*.** LaTeX Workshop scans `label={…}`
  inside environment option blocks (`lstlisting`, beamer frames) and
  configurable custom label commands (`\linelabel`). Check whether the label
  scanner catches the keyval form; if not, it is a bounded static pattern for
  the semantic layer, feeding completion, navigation, and the
  `undefined-ref`/`unreferenced-label`/`duplicate-label` rules alike.

### IntelliSense (signature DB)

### Hover

- [ ] **Graphics preview on hover *(LW)*.** Hovering an `\includegraphics`
  argument returns hover markdown embedding the image itself
  (`![](file:///…/fig.png)`)—VS Code renders images in hover markdown. No
  rendering on our side, just a file reference: reuse the target resolution
  from `lsp/document_link.rs`; png/jpg/svg only, degrading to the resolved
  path for `.pdf`/`.eps`.
- [x] **Documentation link in package hover *(LW)*.** LaTeX Workshop's
  `\usepackage` hover offers a "View documentation" link via `texdoc`. The
  package hover now pairs a texdoc documentation link
  (`https://texdoc.org/pkg/<name>`, keyed on the package name texdoc resolves,
  serving the documentation PDF) with the existing CTAN catalogue link (keyed
  on the `ctan` catalogue id).
- [ ] *(Design decision)* **Math preview on hover *(LW)*.** LaTeX Workshop's
  most-loved language feature: hovering math renders it (MathJax,
  client-side); texlab lacks it too, so it is also a differentiator. Options:
  (a) skip—LaTeX Workshop covers it, and coexistence is the story; (b) render
  in the VS Code extension—breaks the thin-client principle and is VS
  Code-only; (c) server-side SVG via a Rust math renderer (ReX or similar) as
  a data-URI image in hover markdown—editor-agnostic, but ships a math layout
  engine, which is typesetting in all but name (pressure on the AGENTS.md
  non-goal). Lean (a) for now; whichever way, record the decision in
  AGENTS.md.

### Code actions

- [ ] **Surround selection with environment/command *(LW)*.** LaTeX Workshop
  ships these as client-side commands; badness can host them editor-agnostically
  as code actions or `executeCommand`s alongside `changeEnvironment`
  (`lsp/code_action.rs`).
- [ ] **Section promote/demote *(LW)*.** Recursively shift sectioning levels
  across a selection (`\section` ↔ `\subsection`); the sectioning hierarchy is
  already in the signature DB, so this is a mechanical rewrite.

## Performance & hardening

- [~] Large-doc benchmarks (`hyperfine`, criterion); flamegraph hot paths.
  Formatter speed bench vs `tex-fmt`/`latexindent` landed (`benches/compare_format.sh`,
  `task bench`, writes `benches/benchmark_results.json`, which feeds the docs
  benchmark page `docs/src/reference/benchmarks.md`). In-process parse/format micro-bench +
  flamegraph hot paths landed (`benches/formatting.rs`, `task bench:micro`/`bench:profile`;
  see the profiling item below). Still pending: bib + lint benchmarks.
- [ ] **95 corpus files change their `%` comments** (`comment-change` in the gate
  baselines). Surfaced by the comment oracle added alongside the conditional node —
  the `content-change` check compares `nontrivia_content`, and a comment is trivia
  to the CST, so this whole class was invisible. All 95 predate the conditional
  work (verified file by file against `main`; that change fixes 12 of them and
  regresses none). Two shapes so far:
  - **Adjacent comments merge.** `%\n% just backwards compatibility…` comes out as
    `% % just backwards compatibility…`, the empty comment's `%` swallowed onto the
    next line (`pgfrcs.code.tex`, `latexrelease.sty`). Byte-identical meaning to
    TeX, but the formatter still rewrote a protected region.
  - **`.dtx` guards re-lex as comments.** A `%<+debug>` that no longer opens its
    line is a comment, not a docstrip guard, so the extracted file changes — a
    meaning change, not a cosmetic one. Every `.dtx` in the list is this shape.
    Likely the same margin/guard column-0 pinning the reflow already backstops.
- [~] **Replace the per-opener gate scans with a precomputed closer map
  (container stack; multi-session).** *Three of six gates migrated so far —
  conditionals, aliases, and the `\begin` brace-escape gate run the shared
  batch driver; the math and bracket family is C2.3–C2.5, to be finished for
  **uniformity**, since measurement has since voided the perf case for those
  three (see C2.3).* Every shape gate today runs one forward
  token scan per live opener, each a hand transcription of the same
  bookkeeping — brace depth with `plain_braces`, `\begin`/`\end` counting,
  blank-line runs, the macrocode bound — eight copies in `grammar.rs`, growing
  by one per gate, and a fix to one copy does not propagate (the issue-#95
  `\left`-in-macro-code fix lives in its copy alone). The worst case is
  O(n·openers). For the conditional gate: every ordinary anchor cuts the scan
  short — an unbalanced `}`, an unowed `\end`, a blank line, a math delimiter,
  the `macrocode` chunk end — which is why the conditional-heaviest packages
  in TeXLive (`biblatex.sty`, `latexrelease.sty`, `memoir.cls`, `chemstr.sty`)
  measure within noise of the pre-node parser; but 8000 *top-level* openers
  with no anchor in reach cost ~2.2s against ~0.07s, growing 4x per doubling.
  No corpus file is anywhere near it, and a scan budget would be a hard-coded
  special case. The honest fix is one pass over the token stream maintaining
  explicit stacks of open containers (braces, environments, math,
  conditionals, aliases, macrocode frames), computing for each opener index
  the closer it can reach; the walk then consults the map instead of
  scanning.

  Two constraints make this a rewrite, not a refactor:

  - **The walk stays authoritative; the map is an upper bound.** The gates'
    existing one-directional contract carries over unchanged: the map must
    never promise a closer past the one the recursive walk reaches, but the
    walk may still close *earlier* — it re-gates nested openers and demotes
    during the walk, and the demoted-name set is walk state, not token
    state. `Conditional::closer` stays fallible; no consumer may treat the
    map as ground truth, and no walk simplification may lean on it.
  - **Anchor policies stay per-kind and explicit.** The gates deliberately
    diverge (the alias gate has no paragraph anchor; `\left` counts inside
    macro code; the `$` gate's blank-line anchor applies only between
    top-level atoms of the math body; bracket claims resolve left to right).
    The unified pass owns the *bookkeeping*, never averages the *policies* —
    each policy must be restated stack-relative and verified byte-equivalent
    against the gate corpora. The math family's top-level-atom anchor is the
    hardest restatement; budget for it.

  Stages, each gated on the corpus failing-file sets not growing (compare
  sets, not counts):

  - [x] **C0 — bound every gate by its last-closer index** (done; widened
    from the two named gates once the survey found a *ninth* scan and a
    third unbounded one, `bracket_closes_before_math_end`). A gate succeeds
    only at one closer token shape, so truncating its scan at the last
    occurrence in the file is verdict-preserving — past it only refusals
    remain — and a file with none refuses without scanning. Six fields in
    the `new()` pre-scan, the `last_alias_closer` treatment:
    `last_r_bracket` (shared by `bracket_closes_in_text` and
    `bracket_closes_before_math_end`), `last_display_math_closer` and
    `last_inline_math_closer` (`delim_math_closes`), `last_right`
    (`left_right_closes`), `last_r_brace` (`environment_escapes_group`),
    and `last_fi` (`conditional_closer` — this one alone collapses the
    8000-opener case to ~25ms end-to-end). Scan work is metered
    (`Parser::scan_work`) and pinned linear by unit tests in `grammar.rs`.
    Residual superlinear shapes, deferred to the stages below:
    conditionals whose lone `\fi` sits at EOF (C1's case);
    `dollar_closes`, where the closer is the opener's own token kind so
    the bound is vacuous (`${` per line ratchets depth upward and no
    anchor ever fires); and `environment_escapes_group` in practice,
    since every `\begin{…}` carries a `}` in its own name group, pushing
    the bound to EOF. That list turned out to be too short: the bound only
    helps a file with *no* closer, so a single reachable closer at EOF
    defeats it for every gate (measured under C2).
  - [x] **C1 — conditionals first** (done, with the mechanism amended). Not
    a `new()`-time map after all: `conditional_closer` reads walk state —
    above all `in_def_body`, which is set during greedy argument attachment,
    so precomputing it in the pre-scan would mean re-implementing attachment
    outside the walk, the very transcription-drift disease this item exists
    to cure. Instead the gate is *batched*
    (`Parser::conditional_closers_from`): one scan seeded at the queried
    opener settles every same-frame opener it passes — nested openers are
    counted only at brace depth 0, so they share the seed's frame exactly
    and `\fi` matching is pure LIFO over a pending stack — memoized against
    the walk state the scan read (`macrocode_end`, `in_def_body`,
    `group_depth > 0`; `plain_braces` is pinned by the frame, everything
    else is pre-scanned token state). One rule is load-bearing: a refuted
    entry is **settled, never popped**, because the per-opener scan counts
    nested openers by name and never un-counts one, so a later `\fi` must
    still be consumed by the refuted entry's slot
    (`a_refuted_nested_opener_still_consumes_a_fi` — popping instead hands
    the `\fi` to the outer opener and builds a `CONDITIONAL` the per-opener
    scan never did). Opener recognition stayed in `parser::conditional`,
    shared with the linter's `ConditionalIndex`. The
    openers-with-one-`\fi`-at-EOF shape, which defeats the C0 bound, is now
    one linear pass (~34ms for 8000 openers end-to-end), pinned by
    `conditional_batch_keeps_shared_frame_openers_linear`.
  - [ ] **C2 — migrate the remaining gates onto one batch driver**, easiest
    policy first: alias, then `environment_escapes_group`, then the math and
    bracket family. This item's original wording ("each migration deletes one
    transcribed scan and its copy of the bookkeeping") belongs to the
    `new()`-time map C1 abandoned. Every remaining gate reads walk state as
    well — `in_def_body`, `group_depth`, `plain_braces`, `macrocode_end`, and
    `math_dollar.last()` for `bracket_closes_before_math_end` — so each is a
    *batch*, not a precomputation, and repeating C1 by hand six times would
    leave nine copies of a scan **plus** seven copies of the batch machinery:
    the transcription-drift disease this item exists to cure, made worse. So
    the driver comes first, and each gate joins it.

    The residuals are also wider than C0 recorded. C0 named `dollar_closes`
    and `environment_escapes_group`, but its bound only helps a file with *no*
    closer, so **one reachable closer at EOF defeats it for every gate**. N
    openers, one closer, no anchors in reach, `format --check` end to end at
    2000/4000/8000: `environment_escapes_group` (`{` then N `\begin{itemize}`)
    69/230/984ms; `bracket_closes_in_text` (N `\cmd[`, one `]`) 31/102/271ms;
    `left_right_closes` (N `\left(`, one `\right)`) 22/68/230ms;
    `dollar_closes` (N `${`) 15/48/180ms. `delim_math_closes` measures linear
    on its own shape, and alias needs both halves defined in the same file, so
    those two are migrated for uniformity rather than for speed.

    - [x] **C2.0 — extract the batch driver; re-express conditionals on it**
      (done). `conditional_closers_from` became `Parser::gate_batch`, owning
      only the *bookkeeping*: the bound (`macrocode_end` ∧ `tokens.len()` ∧
      last closer + 1), trivia skip, newline runs, brace depth under
      `plain_braces`, environment counting with the `macrocode`-frame break in
      both directions, the `group_depth > 0` stray-`}` rule, the
      `pending`/`live` stack with `envs_at_push`, settled-never-popped, and
      `tick_scan` metering. `gated_closer` is the memoized front and holds the
      C0 early-out; `WalkKey` is the memo key, with `plain_braces` now riding a
      version counter bumped in `macrocode_body` so the key is a fact rather
      than the old "pinned by the frame" argument. Per-gate policy is the
      `GatePolicy` trait — `PARAGRAPH_ANCHOR`, `last_closer`, `opens_at`,
      `closes_at`, and a defaulted `pairs` — with `ConditionalGate` its first
      client; hooks arrive with the client that needs them, so the trait says
      only what a real gate has asked for. The driver never averages the
      policies. Not the C3 skeleton built first: it is *extracted from C1's
      working batch*, so the map stays the foundation and C3 becomes a
      subtraction. Verdict-preserving, as the acceptance gate below demands —
      the suite, all eight gate-corpora baselines, and the texlab
      parse-compat report are unchanged, and `bench:micro` is flat within
      run-to-run noise (parse-only spread on the small documents is ±30%
      between runs either way).
    - [x] **C2.1 — alias** (done). `AliasGate` is conditional's policy minus
      the paragraph anchor, plus the named-closer test — the first client of
      the defaulted `pairs` hook, which needed no driver change. The
      `alias_closer_memo` slot became `alias_batch`, and its key widened from
      `(opener, group_depth)` to the shared `WalkKey`, which also closes a
      latent staleness hole: the old key omitted `in_def_body` even though the
      scan reads it through `in_macro_code`. The shadow differential ran green
      over the suite, all four corpora (release + `-C debug-assertions`), and
      a torture file covering nesting, crossing, group escape, math, an
      environment in the way, a closer inside one, mismatched names, a
      definition body, and a paragraph-spanning body; then the reference came
      out, since it ticks `scan_work` and so hides the very linearity the pin
      measures. Adversarial shape (N openers, one closer at EOF) at
      1000/2000/4000: 15/59/192ms before, 9/18/46ms after, pinned by
      `alias_batch_keeps_shared_frame_openers_linear`.
    - [x] **C2.2 — `environment_escapes_group`** (done). `EnvGate` is the
      driver's first *demotion* gate, and three policies invert with it, each
      a new hook: a stray `}` **closes** instead of refuting
      (`StrayBrace::Closes` — same token event, opposite verdict), math is
      **not** an anchor (`MATH_REFUTES = false`; for a positive gate refusing
      behind a math delimiter is the conservative direction, here it would
      *keep* an environment the scan cannot vouch for, and the pre-batch scan
      had no math anchor), and the openers are themselves `\begin`s
      (`OPENER_IS_ENV_BEGIN`, so the driver counts one in `envs` before
      pushing its entry — an entry's `envs_at_push` must exclude its own
      environment, which its per-opener scan never saw). `\end` stays the
      level anchor rather than a closer, and running out of file still keeps
      the environment, so `finish_environment`'s unclosed-environment
      diagnostic and `end_orphans_a_demoted_begin` are untouched. The
      `group_depth == 0` and `doc_margin_exempt` pre-checks stay outside the
      batch: they are per-opener walk state, so they are applied per query and
      a rejected opener never consults the batch at all. Shadow differential
      green over the suite, all four corpora, and a torture file (escape past
      inline and display math, nested `\begin`s where the outer escapes and
      the inner pairs, two openers on one brace, a `macrocode` opener, nested
      groups).

      **Measured, and it corrects the attribution above.** Gate scan work is
      now exactly linear (5 ticks per opener: 2500/5000/10000 at n =
      500/1000/2000). Timing `parser::parse` alone on the escaping shape
      (`{`, N `\begin{itemize}`, one `}`) at 1000/2000/4000: **6.6/25.1/99.3ms
      → 0.7/1.2/3.5ms**, quadratic to linear, ~28x at n = 4000. The
      69/230/984ms figures in the C2 preamble are `format --check` end to end
      and were never the gate's alone; nor is `badness lint` a stand-in for
      the parser, since it runs every linter rule. Measure `parse` directly
      when judging a parser change. New item below for the rest.
      **The perf case for what remains is void — finish it for uniformity.**
      Measuring `parse` directly (1000/2000/4000 openers) after C2.2:
      - `delim_math_closes`, N `\[` with one `\]` at EOF: **0.1/0.2/0.4ms**.
        Already linear; a batch has nothing to fix there.
      - `dollar_closes`, the `${`-per-line shape C0 recorded:
        **3.8/13.9/53.8ms**, quadratic — and **a batch cannot help it**. After
        the first `${` the brace depth ratchets upward and never returns to 0,
        so no later opener sits at depth 0 in the seed's scan: the batch
        settles its seed alone and every opener re-batches. Killing that shape
        needs the *map* (a per-frame "next `$` at the same brace depth"), the
        design C1 abandoned as un-precomputable from walk state — so do not
        repeat C0's claim that batching reaches it.

      So C2.3–C2.5 are a **structural** change, deliberately: one driver, one
      copy of the bookkeeping, every gate's divergence stated as a named
      policy instead of a hand-transcribed scan. Judge them on the code, not
      on a benchmark; the linter and formatter are two orders of magnitude
      more expensive on the same inputs anyway (item below), and that is where
      speed work belongs.

    - [x] **C2.3 — `delim_math_closes`, then `dollar_closes`** (done).
      `DelimMathGate` and `DollarGate` are both **single-entry** as planned
      (`opens_at` always false), and the plan's knobs all landed:
      `ENVS_AT_ANY_DEPTH`, `CLOSER_NEEDS_ENV_BALANCE`, `MATH_REFUTES = false`,
      and `StrayBrace`'s third variant — renamed as a trio
      (`RefutesInGroup`/`ClosesInGroup`/`RefutesAlways`) so the `group_depth`
      condition is in the name rather than in the driver. The display seed is
      passed as `open + 1` from the caller, no hook. `last_dollar` is recorded.

      Three things the plan did not have:

      - **The `_`-arm widening had to come with C2.3, not C2.5.** These gates
        close on a `DOLLAR` and a `CONTROL_SYMBOL`, so the driver had to ask
        `opens_at`/`closes_at` for any token at the entries' own level before
        either could run at all. Done, and it costs the narrow gates nothing
        measurable (`bench:micro` parse-only within run-to-run noise on all
        three documents). C2.5 inherits it.
      - **A `macrocode` frame is not an anchor for these two.** The pre-batch
        scans counted `\begin{macrocode}` as an ordinary environment where
        every pairing gate treats the frame as a hard boundary — an
        unanticipated divergence, kept as `MACROCODE_FRAME_ANCHORS` because
        C2 migrates verdicts unchanged. Flipping it on is invisible to the
        suite and to all four corpora but does change a `.dtx` doc-layer `\[`
        that spans a chunk; if it should be unified, that is its own commit
        with its own test, not a silent rider on a structural migration.
      - **The `$` gate runs unmemoized** (`Parser::gate_verdict`). A memo slot
        keyed on the walk state is not merely idle for a single-entry gate, it
        is wrong: a demoted `$$` re-enters `element` on its *second* `$`, which
        is the very index the display query seeded, under an unchanged walk
        state, asking `display: false`. `$$ a $` is the shape — first `$`
        plain, second opening inline math — pinned by
        `demoted_display_dollar_regates_its_second_dollar_as_inline`.

      Shadow differential green: the suite, a per-file pass over all 6277
      corpus files with `-C debug-assertions`, and a torture `.tex`/`.dtx`
      (closers inside groups, stray braces at both group depths, paragraph
      breaks at and below the body's own level, issue #70's environment-nested
      break, environments open at the closer, unowed `\end`, foreign
      delimiters, lone `$` inside `$$`, macro-code data, definition bodies,
      doc-layer openers scanning into a chunk, an opener inside one). The
      harness bites: flipping `MACROCODE_FRAME_ANCHORS` for the experiment
      above panicked on the torture `.dtx` at once.

      **A process finding for C2.4/C2.5:** `task gate-corpora:check` does
      **not** surface a panic — the per-file exit code is swallowed by the
      distillation pipeline, and the flipped-knob build passed all eight
      baselines while panicking on a four-line `.dtx`. Run the debug-assertions
      binary file-by-file (`xargs -P8 -I{}`) as the shadow pass; the baseline
      ratchet is a verdict check, not a panic check.

      Linearity as measured (`parse` inside `bench:micro`, 1000/2000/4000
      openers): `\[`-with-`\]`-at-EOF **178/340/681 µs**, exactly linear, and
      pinned by `math_batch_stays_linear_with_one_closer_at_eof` for both
      delim flavors and both `$` shapes. The `${` ratchet shape is unchanged
      (38.6 → 35.9 ms end to end at n = 4000, the new bound if anything
      helping), and stays quadratic by design — the test says so rather than
      pinning a shape the gate cannot fix.
    - [ ] **C2.4 — `left_right_closes`.** A third environment-counting mode: a
      brace group is opaque, skipped wholesale. Also the one gate whose
      opener/closer recognition deliberately ignores `in_macro_code` (issue
      \#95) — the fix that lives in exactly one copy today, and the concrete
      payoff of consolidating.
    - [ ] **C2.5 — the bracket family** (`bracket_closes_in_text`,
      `bracket_closes_before_math_end`,
      `bracket_closes_before_macrocode_end`). The `abuts_command` claim
      countdown *is* the driver's nested-opener stack once an "opener" is
      defined as a command-abutting `[`. Two things to settle first:
      `bracket_closes_before_math_end` must carry `math_dollar.last()` in its
      memo key, and it is the only bracket gate that does **not** filter
      `plain_braces` on `L_BRACE`/`R_BRACE` — decide whether that is
      deliberate or a latent macrocode bug before preserving it into the
      driver.

      The driver change these three were to share — asking
      `opens_at`/`closes_at` for any token at the entries' own level rather
      than only for a `CONTROL_WORD` — **landed in C2.3**, which needed it for
      its own `DOLLAR` and `CONTROL_SYMBOL` closers. An `R_BRACKET` closer is
      already served.

    **Migration technique**, per stage: keep the old per-opener scan as a
    `#[cfg(debug_assertions)]` reference and assert `batch(open) ==
    reference(open)` on every query for the life of the migration commit, run
    the suite and a debug corpus pass, then delete the reference before
    merging. C2's correctness claim is stronger than C1's one-directional
    contract — verdicts must be *bit-identical* to the per-opener scan under
    the same walk state — and this makes it mechanically checkable.

    **Acceptance gate**, identical at every stage, one commit per stage:
    `cargo test` plus snapshots; `task gate-corpora:check` (two-sided ratchet,
    sets not counts); `task parse-compat` and `task bib-parse-compat`
    **unchanged**, since a verdict-preserving migration must not move the
    differential; a `<gate>_batch_stays_linear_with_one_closer_at_eof`
    scan-work test per gate, on the shapes measured above; and `task
    bench:micro` flat on real documents, so a memo miss never costs the common
    case.
  - [x] **C3 — decide whether to finish** (decided: **finish it**). The
    fallback this stage exists to authorize — part of the family on the map,
    part on its own scans — is *not* taken. The measurement that closed the
    perf case (C2.3) did not make the math gates hard to restate; it only made
    them unrewarding to benchmark, and the item's original complaint was never
    really about speed: it was eight hand transcriptions of one piece of
    bookkeeping, where a fix to one copy does not propagate (#95). One driver
    with named per-gate policies is the end-state. Revisit only if a gate's
    policy genuinely cannot be stated without averaging it against another's.
- [ ] **The formatter *and the linter* are superlinear where the parser is
  now linear** (found while measuring C2.2/C2.3, and it is the larger half of
  every "quadratic gate" number this roadmap has been quoting). Two shapes,
  with `parse` timed directly against the CLI on the same input:
  - `{`, N `\begin{itemize}`, `}` at N = 4000: `parse` 3.5ms,
    `format --check` 133ms.
  - N `\[` with one `\]` at EOF at N = 4000: `parse` **0.4ms**, `lint`
    **287ms** (93ms at N = 2000, so ~3.1x per doubling).

  The parser is no longer implicated in either. The second one is the louder
  finding: a linter rule is quadratic on math-delimiter-heavy input. Profile
  before guessing (`task bench:profile` takes `BADNESS_BENCH_DOC`). The shapes
  are degenerate, but the gate work this roadmap has been shaving is now the
  smaller term by two orders of magnitude, which is worth knowing before
  spending more on C2.
- [ ] **Fuzz/property losslessness harness — the one missing oracle layer.**
  Everything today is curated corpus + snapshots; nothing exercises
  `PARSER_STEP_LIMIT` or the recovery paths with arbitrary bytes, and
  `reconstruct(text) == text` over random input is a perfect fuzz target
  (`proptest` or `cargo-fuzz`). While in the tests: split `tests/parser.rs`
  (1,744 lines, 157 tests) by area — math, verbatim, comments, conditionals,
  aliases.
- [ ] **`build.rs` renders positional same-typed bool lists** in the
  generated constructor calls (`command(&[…], None, false, false, false)`;
  nine positional args for environments), so a swapped `verbatim`/`rule`
  compiles silently. Named-struct constructors, or `/*verbatim*/`-style
  inline comments in the rendered source, make the generated code
  self-checking.
- [ ] **No orphan guard on formatter fixtures.** A directory under
  `crates/badness-formatter/tests/fixtures/formatter/` that appears in none of
  `FIXTURES`/`MATH_WRAP_FIXTURES`/`DTX_*`/`PACKAGE_FIXTURES`/`INS_FIXTURES`
  silently never runs — `expl_relation_slot_statement` shipped that way. Since
  each table is one looping test, a slug is not a test name, so filtering cannot
  detect it either. Add a test that walks the fixture dir and asserts every slug
  is registered exactly once (and every registered slug exists on disk).
- [ ] **Mine the `latexindent` corpus for construct coverage** (human-in-the-loop,
  ongoing). Skill: `.claude/skills/formatter-fixture/`. The corpus is read
  primarily as a coverage map — which constructs occur and in what shapes.
  latexindent's own outputs are a soft target only: usable as inspiration where
  our tenets underdetermine a construct, never a form to match case by case,
  since it is a config-driven indenter whose committed outputs are one settings
  stack's answer to a different question. Measured gaps against the 198 existing
  slugs: `items` (157 files, one fixture), `filecontents`, bare/named brace
  groups. Sectioning/`headings` is done (two slugs, and the Tier-1 lone-newline
  bug that lived there). `ifelsefi` (402 files) is done too, via the
  `CONDITIONAL` node under *Parser* and eight fixtures — do not re-derive a
  formatter-only rule for it, the survey already showed every such rule is
  trivia-reading, typeset-unsafe, or lopsided.
- [ ] Intra-file incremental reparse (reuse green subtrees on contained edits).
- [x] `wasm32` build for a web playground. Landed as the `badness-wasm` shim
  crate + the docs playground page (`docs/src/playground.md`), formatter-only;
  linting in the playground would first need the linter core extracted from the
  root crate (its logic is fs/salsa-free, but its crate is not wasm-clean).

## Editor integration

texlab bundles PDF-workflow features. Only position mapping (no typesetting by
badness) is admissible; the rest are explicit non-goals recorded here so they are
not re-proposed.

- [x] **Forward/inverse SyncTeX search (no typesetting).**
  `textDocument/forwardSearch` (a custom LSP method, texlab-wire-compatible)
  resolves the root document's PDF from `[build]` and launches a viewer
  configured through editor settings, with `%f`/`%p`/`%l` substituted
  (`lsp/forward_search.rs`). Inverse search receives a viewer position over IPC
  and answers with `window/showDocument` (`ipc.rs`, `badness inverse-search`).
  Badness never typesets. It also never *maps*: investigating texlab showed it
  parses no SyncTeX either, because every SyncTeX-aware viewer links libsynctex
  and so takes a file and a line, never a coordinate. Servers publish per-process
  advertisements rather than sharing texlab's single fixed socket, which a second
  editor window silently steals.
- [ ] *(Design decision)* **Native `.synctex.gz` reader.** Would let forward
  search drive viewers with *no* SyncTeX support at all by resolving a page
  number (qpdfview, a browser), and report an honest `Failure` when a line
  produces no output instead of launching a viewer onto nothing. Costs a gzip
  dependency, a parser with real traps (compressed `,=` points, `Input:` lines
  interleaved mid-file, `./`-segment path matching, leaf-vs-enclosing-box lookup
  semantics), and a fixture corpus validated against the `synctex` CLI with no
  existing oracle to lean on. The seam is already in place: `SearchTarget` in,
  `ForwardSearchStatus` out — a backend behind it changes no LSP surface, no
  `[build]` key, and no config. Not worth it until a page-only viewer is a real
  target.

## BibTeX/BibLaTeX

- [x] **`expected ',' between fields` when a comment separates a field value from
  its comma.** Was 33 `format-error` entries in the `latexindent` gate corpus
  (`keyEqualsValueBraces/contributors-mod*`); all gone, `latexindent.all` 202 →
  169 with no additions. The blank-line variant was a red herring — it always
  parsed — and the real gap was that the bib layer had no `%` comment at all. The
  two BibTeX readers disagree: classic `bibtex` 0.99d rejects a `%` inside an
  entry, biber/btparse ends the comment at the newline; we follow biber, as the
  rest of the bib layer does. `%` is context-dependent (literal inside a braced or
  quoted value), so the lexer stays context-free with a bare `PERCENT` token and
  the grammar builds the `COMMENT` node only where it skips trivia inside an
  entry. The formatter keeps every comment: same-line ones ride their field's
  line, the rest bind forward to the field below them and travel with it through
  the canonical sort. texlab models no bib comment, so the gauge divergence is a
  recorded deviation. Value reflow also grew a guard for the *other* `%`: one
  inside a value is BibTeX-ordinary but a LaTeX comment, so its line breaks are
  content and the value is emitted byte-exact. That hazard predated this work (it
  was already reachable through the `namedGroupingBracesBrackets` family) and no
  CST oracle can see it. See `architecture.md` § *`%` comments in `.bib`*.
- [ ] `% badness-ignore` in `.bib`. Now that a `%` comment exists inside an entry,
  the LaTeX-side directive carrier could work here too; today only the
  `@comment{badness-ignore …}` entry form does (`bib/linter/suppression.rs`). The
  two would need one directive grammar and a decision about what an in-entry
  comment attaches to (the field below it, presumably, matching the formatter's
  forward bind).
- [ ] **`task bib-error-compat`: biber as a `.bib` *error* oracle.** The gap the
  `%`-comment bug exposed — `bib-parse-compat` cannot see over-strictness at all,
  because texlab's bib parser has no error channel (the skill says so outright),
  so a whole family of files we wrongly refused sat in a gate baseline instead of
  failing a gauge. biber does have one: btparse reports real syntax errors, e.g.
  `ERROR - BibTeX subsystem: …, line 2, syntax error: found "author", expected
  end of entry ("}" or ")") (skipping to next "@")`, plus an `INFO - ERRORS: n`
  tally. Cross-tabulate per corpus file, `badness has diagnostics` × `biber has
  ERRORS`: agreement on the diagonal, **badness dirty + biber clean = over-strict**
  (this bug's class), badness clean + biber dirty = under-strict (we would format
  something biber rejects).

  Two constraints, both learned by trying it:

  - **Boolean per file only** — never error counts or positions. biber recovers by
    skipping to the next `@`, so it under-counts badly; in a three-entry probe it
    never reported an unterminated `@misc` at EOF at all, having swallowed it
    during recovery.
  - **Do not project `biber --tool` output onto a skeleton.** Tool mode exposes
    biber's *data model*, not its parse, and the transformation would swamp any
    real divergence: `author = {Ann Author and Bo Beispiel}` comes back as
    `{Author, Ann and Beispiel, Bo}`, `year` + `month` merge into `DATE = {2021-11}`
    (both source field names *gone*), `#` concatenation is resolved, and
    `--output-format=biblatexml` additionally explodes names into
    `<bltx:namepart>` and resolves `@string` uses away. texlab stays the right
    *structural* oracle precisely because it is coarse and syntactic; biber's job
    here is only "is this legal BibTeX".

  Placement: biber is an external binary, not a crate, so this cannot be an
  in-process dev-dependency like `texlab-parser` (and Text::BibTeX is not
  separately installed — biber bundles it). Same bucket as `task typeset:check`:
  needs a local install, runs on demand, never in CI.
- [ ] Cross-file `undefined-string`: a `@string` defined in one `.bib` and used
  in another resolves only once a project-level `@string` union exists (today
  single-file-sound, same caveat as `unused-string`).
- [ ] `unused-entry`: a `.bib` entry never targeted by any `\cite`-family
  command, project-aware behind the same closed+rooted namespace gate as
  `unreferenced-label`/`undefined-ref` (the bib linter has `unused-string` but no
  `unused-entry`). Report-only. texlab: `UnusedEntry`.
- [ ] Bib document-symbol outline completeness: `src/bib/outline.rs` surfaces
  regular entries only; consider `@string`/`@preamble`/`@comment` blocks (and a
  richer `SymbolKind`/detail).
- [ ] Shared component-finder: `ResolvedCitations` duplicates the union-find +
  component assignment from `ResolvedLabels` (`project/citations.rs`); factor one
  helper when a third consumer appears.
- [ ] **`subfiles`' `\subfix` wrapper opens the citation namespace.** The
  package's path fixer is the idiomatic way a subfile names a shared resource
  (`\addbibresource{\subfix{references.bib}}`), but a macro inside the group
  makes `nth_group_text` return `None`, so the target is `BibTarget::Dynamic`
  and the whole component goes open — silently disabling `undefined-citation`
  project-wide for exactly the projects issue #112 was about. Conservative (a
  loss of coverage, never a false positive), hence not urgent. Unwrapping it is
  a *shape* fact, not meaning — `\subfix{p}` is transparent by construction, the
  same class of static recognition as the `subfiles` class-option gate — so it
  fits decision #8; the open part is where the unwrap belongs so `include.rs`,
  `document_link`, and completion cannot drift on it.
- [ ] **`project::package` does not collapse `.`/`..` in load targets.**
  `include.rs`'s resolvers now normalize lexically (`resolve_against`), so
  `\input{../shared}` and a `subfiles` parent resolve; `package.rs`'s `resolve`
  still does not, so `\usepackage{../mypkg}` never matches a member and its
  signatures stay out of scope. Benign (a missing local scope, not a wrong one)
  and a separate subsystem, so it was left out of the #112 fix. Lift the helper
  to `project.rs` when touching it.
- [ ] **Central-bib fallback via the texmf index *(LW)*.** LaTeX Workshop
  resolves `\bibliography{refs}` through `kpsewhich` (plus a `bibDirs`
  setting) for users who keep one master `.bib` in their texmf tree. Extend
  citation resolution to fall back to the read-only texmf index
  (`project::texmf`) for bib paths that don't resolve project-locally.
  LSP-only, sanctioned by the AGENTS.md environment-awareness tiers
  (completion, hover, go-to-definition); the `undefined-citation` lint and the
  CLI stay hermetic and project-local.

## CST / AST / trivia

- [ ] **[low, latent] No `SyntaxNodePtr`/`AstPtr`.** RA stashes stable node
  pointers in salsa data to re-resolve across reparses; badness sidesteps this by
  storing the `GreenNode` directly (decision #7) and carrying diagnostics as
  byte-ranges (decision #4), so the need has not arisen. Latent: a future feature
  that must stash a *stable node identity* in a salsa query (resolving a
  completion/hover target to a specific node across edits) has no primitive for
  it, and byte-ranges alone do not survive edits.
- [ ] **Collapse the four near-identical token walks in `ast/nodes.rs`**
  (`Group::inner_text`/`inner`, `NameGroup::text`/`range`): all four walk
  `children_with_tokens`, skip the delimiters, bail on nested nodes, and
  accumulate text and/or a range. The drift risk is demonstrated, not
  hypothetical — the issue-#104 `HASH` rejection made it into two of the
  four. One shared helper.
- [ ] **Mark the free-function AST shims `#[deprecated]`** (or file the
  removal issue) once the formatter/linter call sites migrate — two parallel
  APIs for the same reads with no forcing function is a standing invitation
  for new code to pick the wrong one.
- [ ] **Share the cross-language boilerplate that is past due**: one
  `SyntaxError` for `parser::core` and `bib::core` (two identical structs
  today, a type-level fork consumers handle twice), an `impl_rowan_lang!`
  macro for the duplicated `Language`/transmute boilerplate (leaves one
  audited `unsafe` instead of two), and a compile-time `ROOT`-is-last
  assertion making the "do not add variants after `ROOT`" comment
  mechanical. Leave the rest of the bib parallel alone — it is disciplined,
  self-labeled duplication with the unification path recorded in place, and
  genericizing events/tree_builder/`Parser` at n=2 would be a premature
  abstraction.

## Open decisions to revisit

- [ ] How much of `\newcommand`/`xparse` to model. *(Semantics)*
- [ ] Formatter opinionatedness: configurable vs. fixed. *(Formatter)*
- [ ] `.dtx` two-layer model: a preprocessor that splits doc/code layers, or a
  single lexer mode with margin-aware tokens? *(Package infrastructure)*
- [ ] Math preview on hover: skip (LaTeX Workshop covers it), render in the
  VS Code extension, or a server-side Rust renderer? *(Language server; see
  `### Hover`)*
