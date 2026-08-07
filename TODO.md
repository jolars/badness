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
  gate corpora before flipping any consumer. Rationale in `parser.md`
  (§ *Why greedy: text purity, not uniformity*).

## Formatter

- [ ] **Trivia-invariant layout: the umbrella fix for the idempotency bug family
  (multi-session).** Recorded as an invariant in `AGENTS.md` and detailed in
  `formatter.md` (§ *Trivia-invariant layout*). Layout may read only trivia
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
    `formatter.md` (§ *Bracket re-attachment is stable in-region*); pinned by
    the `expl_bracket_attachment` fixture and `bracket_attachment_stability`.
  - [ ] *Sibling-attached branch explosion*: `\prop_get:NnNTF \p {k} \l {T}
    {F}` forms one unit now, but the R4 explosion still fires only via
    head-attached branches (`lower_expl_conditional` returns `None`); the
    consumer's slot-to-sibling mapping could drive it.
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
  `TokenList`) the formatter dispatches whitespace and break policy on
  (`DocumentBody` stays an environment-body concept via
  `EnvironmentSig::no_indent`; add it when a command-arg case appears). What
  remains is the non-determinism fix: `spans_multiple_lines` decides
  block-vs-inline from incidental source newlines, sidestepped for the
  `TokenList` kind but still governing every `Opaque` multi-line group. Give
  `Opaque` groups a deterministic layout policy that does not depend on
  incidental whitespace. *(An instance of the trivia-invariant-layout violation
  above — `spans_multiple_lines` reads the unsafe lone-newline predicate. Fix it
  under that umbrella, Tier 1, not on its own.)*
- [ ] **Long collapsed cite list overflow.** A `collapse` arg folds to one line
  even when the key list exceeds the width; it never breaks *at commas* (one
  key per line) as a fallback. Needs the token-list content kind to break on
  its own separators rather than the paragraph fill.
- [ ] **Keyval-aware optional-argument layout (parked; grew out of issue #47).**
  The landed fix only collapses a fitting multi-line `[…]`; commas pass through
  wherever the author put them. The Black/Ruff *magic trailing comma* (a trailing
  `,` before `]` forcing one-key-per-line) was prototyped and **declined by
  design**: it is deterministic but not canonical — content steering layout
  conflicts with the formatter-is-sole-authority tenet. The parked replacement,
  two independent pieces:
  - *Count-based expansion:* expand one key per line when a `[…]` has more than N
    top-level keys (or overflows the width); else collapse. Canonical — layout is
    a pure function of content + width. Splits must stay meaning-safe: only at a
    top-level comma already followed by whitespace (whitespace ↔ newline is
    TeX-identical); a glued `[a=1,b=2,…]` has no safe split point and stays
    inline. Semantics to settle: N (default, knob or fixed), and that comma count
    is a proxy for keyval-ness (a comma-rich textual optional would expand too).
  - *Formatter-owned trailing comma, signature-gated:* for an argument the
    signature DB can *prove* keyval (a new `ContentKind::Keyval` or flag), add
    the trailing comma when expanded and drop it when collapsed, Black-style —
    safe because keyval/xkeyval/pgfkeys/l3keys and `\ProcessOptions` clists all
    ignore empty entries. Data: curated built-ins first (`\usepackage`,
    `\includegraphics`, `tcolorbox`, `minted`, …); the CWL corpus marks these via
    `#keyvals:` sections, which `gen_cwl_signatures.py` currently skips. Content
    insertion on a wrong flag changes typeset output, so hold it to the curated
    standard of the math-env routing; never for scanned user definitions, never
    for unknown commands.
- [ ] Widen the prose-argument table (CWL ingest could feed it); consider gluing
  a prose arg onto its command line when a source break separates them.
- [ ] **Head/definiendum split on a soft trailing block (rest-aware measurement
  gap).** In an expl3 statement, a bare command followed by another command that
  greedily absorbs a wide `{body}` (`\cs_set_protected:Npn \__foo_aux: { … }`) is
  width-split at the head — `\cs_set_protected:Npn` / `\__foo_aux:` land on
  separate lines — because the statement fill measures the `\__foo_aux: {body}`
  atom *flat* (~90 cols) and breaks before it, even though that atom will *hang*
  its body and only needs the command name (~25 cols) on the line. Both parts fit
  together with the body hanging. Same rest-aware-measurement gap as #71's
  head-hug (`Ir::group_hug`), which currently fires only for *forced* breaks; a
  soft-hanging trailing block gets no hug. Stable/idempotent, just not the
  prettiest. Surfaced by the #94 fixture (`expl_trailing_empty_branch`, whose
  single-statement body is kept precisely because this split is what makes the
  block's break soft-then-hard across passes).

  **Now fixture-visible, and more prominent since the `group_expanded` fix.**
  `expl_forced_block_body_mode` splits `\int_set:Nn` / `\l_@@_groups_int` although
  they join at 36 columns and the *input* already has them joined — so on this
  shape the formatter degrades its input. That fixture is the reproducer; its
  registry comment in `tests/format.rs` marks the split NOT ENDORSED. Before the
  mode fix the head stayed joined only by accident (a flat-dispatched fill skips
  per-atom measurement entirely) while the block hybridized anyway, so the fix
  traded pretty-but-unstable for stable-but-ugly. The right trade, but it makes
  this entry the most visible remaining formatter wart.

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

- [ ] How much of `\newcommand`/`xparse` to model for the signature DB. *(open
  decision)*

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
- [ ] Intra-file incremental reparse (reuse green subtrees on contained edits).
- [x] `wasm32` build for a web playground. Landed as the `badness-wasm` shim
  crate + the docs playground page (`docs/src/playground.md`), formatter-only;
  linting in the playground would first need the linter core extracted from the
  root crate (its logic is fs/salsa-free, but its crate is not wasm-clean).

## Editor integration

texlab bundles PDF-workflow features. Only position mapping (no typesetting by
badness) is admissible; the rest are explicit non-goals recorded here so they are
not re-proposed.

- [ ] **Forward/inverse SyncTeX search (no typesetting).**
  `textDocument/forwardSearch` (a custom LSP method) locates a configured PDF and
  drives an external viewer; inverse search receives a viewer position over IPC
  and answers with `window/showDocument`. Badness never typesets—it only maps
  source↔PDF positions via SyncTeX and shells the viewer. texlab:
  `crates/commands/fwd_search` + the `ipc` crate.

## BibTeX/BibLaTeX

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

## Open decisions to revisit

- [ ] How much of `\newcommand`/`xparse` to model. *(Semantics)*
- [ ] Formatter opinionatedness: configurable vs. fixed. *(Formatter)*
- [ ] `.dtx` two-layer model: a preprocessor that splits doc/code layers, or a
  single lexer mode with margin-aware tokens? *(Package infrastructure)*
- [ ] Math preview on hover: skip (LaTeX Workshop covers it), render in the
  VS Code extension, or a server-side Rust renderer? *(Language server; see
  `### Hover`)*
