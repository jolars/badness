# Badness TODO

A LaTeX formatter, linter, and language server on a lossless rowan CST,
following **rust-analyzer's** architecture. See `AGENTS.md` for load-bearing
design decisions and invariants.

Single-crate package (not a workspace). Parser and formatter are **intentionally interleaved**: the formatter is the primary tool for stress-testing the parser.

Status: `[ ]` todo · `[~]` in progress · `[x]` done

## Parser

- [x] ~~**Arity-directed expl3 attachment (decision #8's sanctioned deviation —
  approved, staged).**~~ **Landed.** In-region colon-suffixed heads attach by
  argspec arity (`grammar/expl3.rs`: a pure token scan produces an `Expl3Plan`
  the walk replays exactly, so the gate mirrors the walk by construction);
  `w`/`D`/colonless and the `\::n` drivers stay greedy, a blank line at
  paragraph level aborts while inside a group it commits the consumed prefix,
  and in-math heads never scan (an `N` slot would swallow the enclosing
  closer). Keyed on token shape alone — a colon-carrying `CONTROL_WORD` can
  only have lexed in-region — which also covers implicit `.dtx` regions the
  toggle index cannot see.

  The staged plan ran as recorded: the migration oracle diffed grammar
  attachment against `semantic::expl3`'s independent consumption over the gate
  corpora (67k statement-leading heads, 265 files) and was triaged to zero
  disagreements outside the benign leftover class before any consumer flipped;
  it surfaced two scan-vs-walk boundary bugs (an in-math head consuming its
  enclosing `\]`; a paragraph separator read as a partial-commit blank line)
  now pinned as corpus fixtures. The formatter consumers then shrank to node
  reads: conditional branches come from the head node
  (`lower_expl_conditional_unit` and the two-path dispatch deleted), the
  grouped-sibling suppression reads the owner's earlier children
  (`GroupedSiblingCache` deleted), and `semantic::expl3`'s consumption remains
  as the shape-agnostic statement-extent resolver — the underivable-head
  fallback — now extended over greedy-attachable tails so statement extents
  survived the flip unchanged. Two stabilization rules rode along, each from
  an idempotency break the new shapes made reachable (`fallback_line`'s
  bare-line-break boundary; unbreakable bare-operand gaps inside a call).
  Gate results and provenance in `tests/gate_baselines/README.md`; rationale
  in `docs/src/development/architecture.md` (§ *Argument grouping and bracket
  policy*).

  Unlocked follow-ups, now that argument ownership is in the tree:

  - **signatureHelp inside expl3 regions** (`src/lsp/signature_help.rs`): the
    cursor's owning call is the parent chain, and the argspec is a DB-free
    signature source.
  - **`missing-required-argument` in regions**: a recognized head's argument
    count is now a node fact.
  - **In-region conditionals** (`\if_int_compare:w … \else: … \fi:`): their
    recorded deferral reason — a `CONDITIONAL` node would contend with the
    segmentation's layout ownership — is retired.

- [ ] **Trailing comment after `\ExplSyntaxOff` relocates and rebinds**
  (pre-existing, surfaced by the migration triage): formatting
  `\ExplSyntaxOff % comment` moves the comment to its own line, where the next
  parse binds it as the following construct's `DOC_COMMENT` and changes that
  construct's layout — an idempotency break at default settings. Reduced from
  `array-2024-06-01.sty` lines 380–392; reproduces identically on the
  pre-migration tree, so it is the trailing-comment-never-relocates rule
  failing at the region boundary, not an attachment fact.

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
    with it. *That contention retired with arity-directed attachment (the
    landed entry above), so this is now an ordinary candidate rather than a
    carve-out.*

- [ ] **Keep carving `grammar.rs`** (3,959 lines after the first cut, which
  took `grammar/facts.rs` and `grammar/trivia.rs`; `grammar/prescan.rs` came
  out with it). Two candidates remain, each its own commit:

  - The **math / `\left…\right` sublanguage** (`dollar_math` through
    `stray_right`, plus `split_math_word`), ~460 lines and highly
    self-contained. `math_environment_body` currently sits in the environment
    section and is the one routine the split has to decide about.

  - The **gate machinery** (`WalkKey`, `GateBatch`, `VerdictSink`, the policy
    vocabulary, `trait GatePolicy`, and the nine gate policies), ~805 lines.
    Postdates the original audit note. It drags the `scan_work` linearity
    tests along, so `.claude/rules/parser.md` ("pinned linear by the tests in
    `grammar.rs`") needs a matching one-word update.

  The rest of the hygiene item is done: the shadow counters, the DOC_COMMENT
  precede dedup (`precede`/`extend_back`/`doc_comment_bind`), the `PreScan`
  extraction, the `math_atom` EOF tripwire, the environment-delimiter helpers,
  `BLANK_LINE_NEWLINES`, the `is_trivia` reuse, the borrowing `peek_end_name`,
  and the stale `parser.rs` module doc. Still open from that note: promoting
  `precede` into the event layer as a real rust-analyzer `Marker` with a
  `DropBomb`, which is a mechanical diff across every `open`/`close` site.

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

- [ ] **The block form's closer break lacks the `open_glued` mirror.**
  `lower_bracketed` guards the *open* side — no break after a glued `{`, because
  the synthesized end-of-line reads as a space token TeX typesets — but emits an
  unconditional hard line before the *close* delimiter, so an author-glued
  `beta}` comes out `beta\n}`: a space token before the group closes (a trailing
  interword space in horizontal mode, a trailing space in a `\def` replacement
  text). Same on the bracket side for a textual optional's block form (`}]` →
  `}\n]`, visible in `optional_block_decline_deterministic`). Invisible to every
  oracle by construction: trivia to the CST checks, and the perturbation
  generator never touches a glued junction because there is no gap there.
  Surfaced reviewing `group_blank_line_keeps_block`. The fix is a `close_glued`
  mirror (last body element not collapsible trivia ⇒ keep `}` glued to the last
  body line), but it reshapes every multi-line block group corpus-wide — every
  `…}`-glued definition body — so it needs its own baseline re-record, a
  `tests/typeset/` case pinning the space-token claim, and a check that the
  Tier-2 fixed-point argument on `spans_multiple_lines` still holds (a glued
  closer's block form no longer ends with a newline before the closer, so the
  "output re-reads multi-line" clause must rest on the *lead* break or the body
  instead).

- [ ] **The paragraph reflow's forced-break rule splits glued sibling atoms.**
  A third member of the glued-divider family, and the one that lives in
  `reflow_elements` rather than in a delimiter lowering. Its block-statement arm
  ends the line at any element whose IR carries a forced break, without asking
  whether the *next* element is glued to it, so `{a%\n}{b%\n}` — two brace
  groups the author abutted, each forced open by its own `%` — comes out
  `{a%\n}\n{b%\n}`. The `%` eats the source newline, so TeX saw `{a}{b}` and now
  sees `{a} {b}`: an interword space materialized at a junction with no gap,
  exactly what the glued-divider principle forbids. Invisible to every oracle for
  the usual reason — trivia to the CST checks, and the perturbation generator
  cannot reach a junction that has no trivia to perturb. Pre-existing and
  independent of who routes content into the paragraph; surfaced by the A/B space
  sweep for `begin_tail_is_body`, which moved one corpus file
  (`mand-args/env-third-mand-args-percent-after-body`) into the body where it
  already applied. The fix is the same `close_glued` shape as the entry above —
  suppress the line end when the following element abuts — and it wants a
  `tests/typeset/` case pinning the space-token claim before it lands, since the
  ratchet cannot see the difference either way.

- [ ] **Math operator spacing is inconsistent between script args and command
  args** (surfaced by issue #42's examples). A braced script argument is lowered
  through the math seq path and gets operator spacing (`\sum_{i=1}^m` ->
  `\sum_{i = 1}^m`, `\Big \}^{1/2}` -> `\}^{1 / 2}`), while a command argument in
  math mode (`\frac{1}{n^{m+1}}`) is left untouched — the two should agree.
  Related conventions question: `/` (and arguably `*`) is conventionally set
  tight (`1/2`, per Knuth), and script-size content is conventionally tight
  overall, so the likely resolution is tight `/` everywhere and no operator
  spacing inside `^`/`_` arguments — decide, then make both paths agree.

- [x] **Opaque-group layout non-determinism (the last Tier-1 read) is retired.**
  Under `Reflow` a brace group is width-driven (`lower_opaque_group`): flat when
  it fits (byte-identical to the generic path except a lone-newline run renders
  as one space), first-fit wrapped at its authored gaps otherwise, with only
  preserved predicates (interior blank line, direct comment) and forced-break
  content declining to the block form. `lower_optional`'s fallbacks are
  deterministic in the mirror image (unconditional block on decline, no
  single-line guard, trailing-separator padding restored). The surviving
  `spans_multiple_lines` readers are the delimited-group Tier-2 residue behind
  the non-`Reflow` modes and the doc-margined corner, with the fixed-point
  argument written on the predicate. Three convergence rules came out of the
  gate corpora: an *edge* blank erases (the block form trims edge blanks, so
  declining would key on a predicate the emitter destroys), a non-single-space
  edge glues verbatim (vanishing it hands pass 2 a different flat spelling),
  and the command-only residue is off inside prose argument bodies
  (`ReflowKind::ProseArg` — its hardened break leaked upward through
  `contains_forced_break` readers). Detail in
  `docs/src/development/architecture.md` (§ *Trivia-invariant layout*).

  Recorded follow-ups: extending the width-driven path to the other
  `wraps_prose` modes (today `Stable`/`Sentence`/`Semantic` keep the Tier-2
  block residue), and grid width enforcement (a joined cell may now push a
  tabular row past the width — grids never enforced width, but the reachable
  surface grew).

- [ ] **Long collapsed cite list overflow.** A `collapse` arg folds to one line
  even when the key list exceeds the width; it never breaks *at commas* (one
  key per line) as a fallback. Needs the token-list content kind to break on
  its own separators rather than the paragraph fill.

- [ ] **Widen mandatory-keyval admission (follow-up to the `{…}` segmentation).**
  `ContentKind::Keyval` on a *mandatory* group is now consumed
  (`lower_segmented_group`; `keyval_group_splits_entries`), so the setters
  `\pgfkeys`/`\tikzset`/`\lstset`/… take one entry per line instead of a prose
  reflow that wrapped mid-key. Two halves were deliberately left out and neither
  is a bug:

  - The bulk CWL tier still drops a `%keyvals` mark on a `{…}`
    (`gen_cwl_signatures.py`, `_parse_arg_shape`). The reason it gave — "nothing
    consumes the flag there" — has expired, but the other half has not: the mark
    is mechanical, and a wrong `Keyval` on a mandatory group changes typeset
    output where the same mistake on a bracket is contained. Lifting the scoping
    means first *measuring* which names would gain it (needs the pinned CWL
    source) and putting the textual ones through `task typeset:check`.
  - Environments are unwired: `lower_begin` keeps `keyval && is_bracket`. The
    corpus case is tabularray's `\begin{tblr}{hlines={white},…}` (latexindent's
    `keyEqualsValueBraces/issue-378`), and it pulls in two things a command does
    not have — the grid router reads the colspec group, and a verbatim-body
    environment's `\begin` line may never break at all.

- [ ] **Formatter-owned trailing comma (parked; the last piece of issue #47).**
  A `[…]` — and, since the segmentation above, a proven-keyval `{…}` — is a
  width-driven group over its top-level entries, and a
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
  a prose arg onto its command line when a source break separates them. (The
  block half of the signature widening landed as `CommandSig::block`; the
  gluing clause is now a proposal to narrow the command-only-line rule's
  *residue*, see the Tier-2 entry above.)

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

- [x] **Hanging continuation indent for wrapped statements (B') — shipped via
  the sanctioned picture-statement mode.** The entry's own charter: a node that
  owns the whole statement, so layout derives from structure and the hang is
  idempotent. The node landed *smaller* than the entry predicted — no
  `at`/coordinate/path grammar at all, just statement **extent**: in a curated
  `statementBody` body the parser wraps each run up to a top-level `;` in a
  `STATEMENT` node (retrospective `precede` splice, no gate, no scan; a run
  with no reachable `;` stays plain content), and `lower_statement` derives
  one-statement-per-line plus a hanging continuation indent from it, Tier 1.
  The `;` terminator turned out to be the one statically readable fact the
  entry said TikZ lacked — extent is all the *hang* needs, and it passes
  decision #2's admission test (curated `statementBody` routing plus
  declarations, the math-routing template; misapplication demotes to plain
  content). Content no `;` terminates (`\tikzset` lines, lone `\foreach`
  headers) keeps the flush authored-line fallback, Tier 2 as before. Recorded
  in AGENTS.md (decision #1 sub-bullet, decision #2's `statementBody` bullet,
  Invariants) and architecture.md (§ *Sanctioned lexer modes*, § *Statement
  bodies*). What extent does **not** fix is interior break quality — the next
  entry. Recorded v2 upgrades: a `SubTok` split for a `;` glued mid-`WORD`
  (`(1,1);(2,2)` currently over-extends one statement); declarations naming
  additional statement environments already work via `like = "tikzpicture"`.

- [x] **TikZ semantic unit model (interior path grammar, semantic-side
  first) — landed as `semantic::tikz::statement_glue`.** A glue map, not a
  grammar: for each authored gap in a `STATEMENT`'s top-level element stream,
  one verdict — unit-internal (a single space, never a break) or neutral. The
  curated rules — an operator binds forward, `at` binds both sides, a
  coordinate binds its operation and an operation its argument, a loose `[…]`
  options run glues except after a comma (the keyval entry convention), a
  comment suppresses everything — fixed both known-bad pins: `(6,6)` no longer
  splits from `circle (3);`, and wraps land before path operators. Recorded
  refinements for a later pass: bind an operation's *target* across its
  options run (`edge [post] (p4)` still breaks before the coordinate), and a
  glued-comma split inside loose options runs for space-less keyval
  (`[surf,shader=faceted,…]` currently overflows rather than break mid-key —
  the honest failure, but a licensed comma split would fix it properly). Policy calls made
  against a corpus survey (~6000 statements, pgf's own manual sources plus the
  latexindent user corpus): break-*before*-operator is the ~3:1 idiom; the
  wrap over units is a plain **greedy fill** (users lean greedy 17:3; Tantau
  mixes one-per-line for payload segments with greedy for bare coordinates —
  `Ir::StickyFill`'s cascade was the recorded runner-up if taste shifts). The
  glued `;` seam now **splits**: `statementBody` carries a curated
  whitespace-safety claim (the `ContentKind::Keyval` licensing pattern, proven
  by `tests/typeset/statement_seams.tex` under `task typeset:check`), and
  glued seams are unattested in the surveyed corpora. Unrecognized vocabulary
  degrades to a neutral gap — today's layout — so a wrong classification is a
  worse break choice, never a wrong tree. Any later migration of the model
  into the grammar is judged by decision #2's admission test — the
  falsifiability bar is where it will struggle, since `(a)` as coordinate vs.
  name reference vs. prose is not demotable from text shape alone. The
  linter's pgf gates (next entry) are the recorded second consumer.

- [ ] **Fold the linter's pgf picture set into the `statementBody` flag.**
  `linter::rules::is_pgf_picture_environment` (which keeps `dash-length` off
  coordinate arithmetic) and `data/signatures.json`'s `statementBody` now curate
  the same family twice — and the flag now has *three* readers (formatter
  routing, the parser's statement mode, and the linter's duplicate set), which
  strengthens the case for the fold. Merging them needs a `SignatureDb` on `RuleContext`,
  which carries none today — the same plumbing the user-declared ref/cite
  families entry below wants, and which the Declarations plan (Semantic layer
  § *Declarations*) has to build anyway. Until then the two carry cross-references and must
  be edited in step. Note the sets are not quite identical by design: the flag
  also names `scope` and `pgfonlayer`, which the linter reaches through the
  enclosing `tikzpicture` on its ancestor walk.

## Linter

### Issues

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
  and the LSP, which should honor the same declarations. **Land it as
  `[commands.eqrefs] like = "eqref"`** on the Declarations mechanism (Semantic
  layer § *Declarations*) — the deferred `[commands.*]` half of it — rather than
  as a bespoke list-of-names knob.

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
    this is a separable feature, not a data edit. **Subsumed by** the Declarations
    plan (Semantic layer § *Declarations*): `[environments.x] like = "lstlisting"`
    is exactly this knob, and falls out of steps 3–5 rather than needing its own.

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

### Rules

- [x] **Blank line inside a `ContentKind::Keyval` argument.** Shipped as
  `blank-line-in-keyval` (`Error`, node-shape, `Safe` fix). A blank line is a
  `\par` and a keyval processor walks its entries with non-`\long` macros, so
  the call aborts -- and TeX names the *processor*, not the command the author
  wrote (`\kv@processor@default`, `\pgfkeys@addpath`, `\enit@setlist@i`,
  `\caption@setup@options@`; `\geometry` fails differently again), which is
  what makes the finding worth more than the compiler's message.

  Three scope limits, each measured rather than assumed: a blank line *nested*
  in a value's brace group compiles clean and is not flagged; an unclosed `{` is
  left to the parse error it already draws; and a `Keyval` *optional* cannot
  reach the shape at all, since the parser's bracket gate refuses across a
  paragraph break. Swept over all four gate corpora (6205 files): **zero
  findings in latex2e, latex3, and pgf**, and 780 in latexindent's own
  `modifyLineBreaks` output fixtures -- true positives, confirmed by compiling
  the shape.

  Still open, and deliberately so: the rule reads only the hand-curated tier, so
  its reach grows exactly with the mandatory-keyval admission entry below. Any
  name that entry adds is a name this rule starts protecting.

- [ ] **Mine the ChkTeX warning catalog (~44 warnings) for missing rules.**
  LaTeX Workshop adds no lint rules of its own (it only shells out to
  ChkTeX/lacheck, both off by default), so ChkTeX's catalog is the source to
  compare against. Badness already covers the high-value territory (ellipsis,
  dash length, straight quotes, `$$`, space-before-`\footnote`, intersentence
  spacing); remaining candidates include space before punctuation or
  parentheses and missing italic correction (`\/`).

Follow-ups from `label-before-caption` (floats only, shipped). All three are
scope limits recorded at implementation time, not regressions.

- [ ] **Extend `label-before-caption` to list items.** `\label` before `\item`
  is the same `\@currentlabel` bug: in `\begin{enumerate}\label{i:a}\item
  A\end{enumerate}` the label captures the enclosing counter, so `\ref{i:a}`
  prints a number unrelated to the item. Left out of the initial rule because
  the shapes are more varied than a float's — a label may legitimately sit
  between two `\item`s and belong to the earlier one, and `description`/
  `enumitem` custom labels widen the surface — so the statement-level gate that
  makes the float case safe has to be re-derived before it can fire here.

- [ ] **`label-before-caption` is silent outside floats.** `\captionof` in a
  `minipage` fails the same way (`\begin{minipage}{\textwidth}\label{mp}
  \captionof{figure}{C}\end{minipage}`), but `minipage` is not an
  `OutlineKind::Float`, so the rule never looks. Widening the container set
  means deciding which environments may host a `\captionof` without inventing
  findings on ordinary layout environments; the float set is curated signature
  data precisely so this stays a data question.

- [ ] **`label-before-caption` misses the nested-subfigure case.** The detection
  cutoff is the first counter-stepping command at *any* depth, so a `subfigure`'s
  own `\caption` silences a later statement-level `\label` in the outer float —
  which really does capture the sub-counter. Deliberate: the liberal cutoff is
  what keeps `\subcaptionbox` and the `\caption{Text\label{x}}` idiom from
  producing false positives. Recovering the miss needs a per-scope stepper model
  that knows *which* counter each caption stepped, so it is a modeling change
  rather than a gate tweak.

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

### Declarations (`badness.toml` → parser)

**Decided** (AGENTS.md decision #12, architecture § *Declarations*); not yet
implemented. Grew out of issue #109: the inferred environment-alias scan handles
the reporter's example but cannot reach a pair defined in a sibling `.sty`, built
by machinery the scan cannot follow, or spelled `\startmyenv`/`\endmyenv` around
an environment with no built-in counterpart. The amendment lets `badness.toml`
name constructs the parser cannot see, under the safety property that a
declaration names a *spelling, never a pairing* — every shape gate still runs, so
a wrong declaration demotes rather than corrupts.

This is one mechanism, not one feature. Four parked entries want the same
plumbing and should converge on it rather than grow separate knobs: the
user-declared verbatim envs follow-up under `codeexample` (Linter § Issues), the
user-declared ref/cite command families from #104 (same section), the
`SignatureDb`-on-`RuleContext` the pgf picture fold needs (Formatter), and the
"how much of `\newcommand`/xparse to model" open decision above.

v1 scope is `[environments.<name>]` with `like`, `begin`, `end`. `[commands.*]`,
raw behavior flags, `args` (xparse argspec), per-glob overrides, and the
`% badness-env` directive are all deferred — but the type must be shaped so they
slot in without a wire break, since config spellings are public API.

- [x] ~~**1. `Declarations` type and config parsing (no behavior yet).**~~
  **Landed.** `crates/badness-parser/src/declarations.rs` holds `Declarations`
  over a name-keyed `EnvironmentDecl { like, begin, end }`, plus a `CommandName`
  newtype that strips one leading `\` in the type rather than at a call site, so
  every front end normalizes alike. `Config` gained the top-level
  `[environments.<name>]` map and `Config::declarations()`. Wire spellings are
  pinned by tests in both crates: JSON-level in `declarations.rs`, TOML-level in
  `config.rs` (`toml` is a dependency of the root crate only).

  Two deviations from the step as written, both deliberate:

  - **No CLI mirror.** Serde is a *hard* dependency of `badness-parser` (the
    signature DB is JSON), so unlike `FormatStyle` — whose mirror exists because
    its derives sit behind an off-by-default feature — the CLI can deserialize
    straight into the shared type. A mirror would only add a second wire spelling
    to keep in sync with the one the dprint plugin reads.
  - **No `schema` feature yet.** `SmolStr` has no `JsonSchema`, so the derive
    needs `schemars(with = …)` on the map key, `like`, and the `CommandName`
    newtype — annotations better written against a real schema consumer than
    guessed at now. Add it with the plugin work; nothing else depends on it.

- [x] ~~**2. Resolution and validation.**~~ **Landed.** `Declarations::resolve()`
  checks every rule and projects the declarations into a **`SignatureDb`** —
  deliberately not a bespoke output type, since that struct already holds exactly
  the three maps involved (environments, opener aliases, closer aliases). The
  declared tier therefore folds into a document's scope with the existing
  `merge_from` that the scanned tier uses, the step-3 seed reads it the way
  `parse_ctx` already reads the scan's, and `[commands.*]` slots in later without
  changing this function's signature. `Config::validate` calls it, so a broken
  declaration fails at *load* rather than parsing fine and doing nothing.

  `like` resolves against `builtin()` only (a test pins that a CWL-only
  environment is rejected, since that tier's behavior flags are all default).
  Rejected, each naming the dotted key: unknown `like` target; `begin` without
  `end` or vice versa; delimiters on a verbatim target (TeX truth) or on an
  argument-taking one (v1); delimiters whose key is neither `like`-declared nor
  built-in. A name-only entry is unrestricted — `like = "lstlisting"` is how a
  project names a verbatim environment the scan cannot find, and
  `like = "tabular"` is fine because `\begin{mytab}{ll}` carries its own
  arguments.

  Two rules beyond the plan, both of the same "never a silent no-op" kind: a
  spelling may not be claimed twice (across entries *or* across the two sides —
  otherwise the winner depends on map order), and a spelling must be able to lex
  as one control word. The latter shares the lexer's letter set through a new
  `parser::lexer::is_control_word_name`, checked under `\makeatletter` *and*
  `\ExplSyntaxOn` at once because a declaration does not say which file it will
  apply to.

- [x] ~~**3. Seed `ParseCtx`.**~~ **Landed.**
  `parse_with_declarations(text, flavor, &ResolvedDeclarations)`, with
  `parse_with_flavor` as the empty-declarations wrapper. The seed goes into
  **pass 1**, so a declaring project pays no extra parse, and the second pass is
  decided by `ctx == seed` rather than `ParseCtx::is_empty` — comparing against
  the seed answers "did the *scan* contribute anything" directly, and stays
  correct as fields are added in a way a hand-maintained flag would not
  (`ParseCtx` gained `PartialEq` for it). `ParseCtx::overlay_declarations` is
  applied *after* the scan, which is what makes declared beat scanned; declared
  aliases skip the `command_call_counts` filter, since that filter only exists to
  avoid buying a second pass for an unused alias.

  The parameter is `ResolvedDeclarations`, a **newtype** over `SignatureDb`
  introduced here (step 2 returned the bare db). The distinction is load-bearing
  at exactly this boundary: a value of that type can only have come from a
  declaration block, so the entry point cannot be handed a document's merged
  scope — package scans and the CWL tier reaching the tree is what decision #8
  forbids, and this keeps it out by construction rather than by review.

  Two things already work end to end at this step: a declared alias pairs and
  routes its body by the *target's* curated flags (so `\bea … \eea` for
  `eqnarray` is math), and a declared verbatim environment captures its body —
  the parked `codeexample` knob, closed. Ten integration tests in
  `tests/parser.rs` mirror the inferred-alias cases one for one, including the
  three that pin the safety property: a declared alias with no reachable closer,
  one escaping a brace group, and one inside math all demote exactly as an
  inferred alias does.

  **Not yet reachable from the CLI**: nothing threads declarations into the
  parse call sites yet, so `badness format` still ignores them. Steps 5 and 6.

- [x] ~~**4. `is_math_environment` becomes ctx-aware.**~~ **Landed.** Both it and
  `is_block_environment` moved from free functions over `builtin()` to `ParseCtx`
  methods, joining `is_verbatim_environment`; `grammar.rs`'s five call sites now
  ask the context. The seed carries whole `EnvironmentSig`s rather than one fact
  per map, because body routing reads three flags and a declaration is
  authoritative for all of them at once.

  **Authoritative, not merged**, is the rule that fell out: a declared entry
  answers every predicate for its name *alone*, with no fall-through to the
  built-in or the scan. Declaring `myenv` to be `like = "align"` when the file
  also `\lstnewenvironment`s it verbatim means the declaration wins — pinned by a
  test that first asserts the *undeclared* parse does capture a `VERBATIM_BODY`,
  so it cannot pass vacuously if the scan ever stops finding the definition.

  This also collapsed the duplicate bookkeeping step 3 left behind: a declared
  verbatim environment is no longer copied into the scanned-args map, since
  `verbatim_environment_args` reads the declared signature directly.

- [x] ~~**5. Signature overlay for the semantic side.**~~ **Landed for the
  format path.** `SignatureDb::merge_declarations` is the named top-tier overlay
  (so the two scope builders cannot disagree about precedence), applied last in
  `collect_package_signatures`. `badness format`, `--check`, stdin, and
  `debug format` all honor `[environments.…]` now: the issue's own example
  formats to the output it asks for.

  The sharp edge needed provenance, not just an `or_else`. `environment_at`'s
  alias arm must resolve its target against *curated* data, which now means
  built-in **plus declared** — but a merged scope cannot tell a declared entry
  from a scanned `\newenvironment` of the same name once both are in the
  `environments` map. So `SignatureDb` gained a `declared_environments` marker
  set, carried through both merge paths exactly as the origin maps are (an
  overwrite from a non-declared source clears it). Two tests hold the line: a
  declared alias resolves to a declared target, and a scanned redefinition of
  `eqnarray` still lends its alias nothing — the second asserts the name-keyed
  and node-keyed lookups genuinely diverge, so it cannot pass vacuously.

  `format_stdin_sentence` is new in the CLI bridge. The engine's
  `format_with_style_flavored_sentence` parses with `parse_with_flavor` and so
  cannot see declarations; honoring them for `badness format doc.tex` but not
  `badness format < doc.tex` would have been a trap.

  **Deferred to step 6, deliberately:** the salsa `scope_signatures` site (it
  needs the input step 6 creates) and the **lint path** (`check_document` /
  `check_document_fixable` still parse with `parse_with_flavor`, so `badness
  lint` does not yet see a declared alias as an environment). The lint and LSP
  entries share that plumbing, so threading them is one job, not two halves.

- [x] ~~**6. Salsa input, the lint path, and LSP reload.**~~ **Landed.** Every
  front end that parses a document now sees the declarations: `format`,
  `--check`, stdin, `lint`, `lint --fix`, `parse`, and the language server.

  The input is a **singleton** (`incremental::DeclarationsInput`, `HIGH`
  durability on construction *and* on every write), not a query parameter: both
  readers want it — `parsed_document` seeds the parse, `scope_signatures` folds
  it in as the top tier, mirroring `collect_package_signatures` — and threading a
  parameter would have reached every caller of `parsed_tree_root`. It is created
  eagerly in `IncrementalDatabase::default`, which is what lets readers use
  `get`: the `try_get`-with-fallback alternative registers *no dependency* on the
  miss, so a parse taken before the cell existed would never be invalidated by
  its arrival — quieter than a panic and strictly worse.

  Two guards keep "reparses the world" from becoming "reparses on every
  keystroke". `set_declarations` skips the write on an equal value (a salsa
  input setter bumps the revision unconditionally), and the LSP main loop mirrors
  the last block it published, so `GlobalState::analysis_settings` — the settings
  entry every *parse-bearing* dispatch site now uses — sends a
  `WorkerJob::Declarations` only on a change. It rides the same FIFO channel as
  the job it precedes, so ordering needs no handshake. Accepted and documented:
  a session holding two workspaces with *different* blocks rewrites the cell as
  the active document crosses between them.

  The **read-pool fallbacks** were the part that would have rotted quietly. A
  format, diagnostic, code action, on-type format, or outline that races an edit
  bypasses the cache and reparses its captured buffer; all six now take the
  declarations off the snapshot (`Analysis::declarations`) and a scope from
  `formatter::declared_scope`, so a fallback differs from the cached path only by
  the package tiers it could not reach — never by precedence, and never by which
  constructs it recognizes. `format_stdin_sentence` became
  `format_pathless_sentence` in the process: the LSP fallback is the second
  caller with no path to anchor `.sty` resolution against, and it wanted exactly
  the entry step 5 built for stdin.

  `badness parse` was threaded too, though nothing asked for it: a CST dump that
  contradicts the tree the formatter and linter use is the debugging trap step 5
  refused for stdin. It resolves config by discovery only (the subcommand takes
  no `--config`).

  The parameter count held. The lint chain took `&ResolvedDeclarations` as one
  more explicit argument (`run_lint` → `analyze_source` /
  `collect_project_diagnostics` / `apply_fixes_to_paths` → `fix_file` →
  `check_document_fixable`), and the format path stayed at its four parallel
  axes — the carrier struct is still owed to whichever change adds a fifth.

- [x] ~~**7. Docs and tests.**~~ **Landed.** `configuration.md` gained the
  `[environments]` section — the three shapes, the literal-string note, the
  safety property in the user's terms ("a wrong declaration does nothing to your
  document; it cannot corrupt it"), and every rejection listed, since resolution
  runs at load. The formatting guide's protected-regions bullet points at it,
  which is the only place a user with a `codeexample`-shaped problem would look.

  The tests step 7 asked for that steps 3–6 had not already covered:

  - **Corpus + snapshot + losslessness for a declared pair.**
    `tests/corpus/declared_alias.tex` carries the block it is meant to be read
    under in a header comment, since the declarations cannot travel with the
    file. `roundtrip_declared_corpus_file` asserts losslessness under the
    *declared* reading and pins that the two readings differ (1 environment
    blind, 3 declared) so it cannot pass as the blind sweep restated.
    `declared_alias_tree` and `undeclared_alias_tree` snapshot the same bytes
    both ways: `ENVIRONMENT`/`MATH`/`SCRIPTED` against `PARAGRAPH` and two plain
    commands.
  - **`like = "align"` routes math and grids.** The math half is the
    declaration's own contribution (`a+b` → `a + b` only inside math, asserted
    against the undeclared control). The grid half is *not* distinguishable that
    way — the generic top-level-`&` arm aligns an unknown environment too — so
    the claim is the stronger one: a declared `myenv` lays out byte-for-byte as
    `align` does.
  - **Invariants under a declaring config.** `check_format_invariants` gained a
    `_with` variant taking the formatting function, so the whole oracle suite —
    whitespace-only, comments, idempotence, losslessness, trivia perturbation —
    runs on a pipeline that parses through declarations. Worth its own run: a
    declaration changes the tree's *shape*, so it reaches grid, math, and
    body-indent lowerings the same bytes never reach without it.

  One test written for this item was **deleted rather than fixed**: a declared
  layout is not a fixed point of a declaration-*blind* reformat, and should not
  be. Config is an input, so demanding that is like demanding `line-width = 100`
  output be stable under the default width.

  `task parse-compat` confirmed declaration-blind: `declared_alias.tex` lands
  fully concordant (8/43 identical, up from 7/42) and adds no divergence, which
  is the point — the gauge measures the text-only reading. Noted in the skill's
  `SKILL.md`, the hand-written half (`PARSE_COMPAT.md` is generated).

  Landed earlier, in steps 3–6: the ten parser-level cases in `tests/parser.rs`
  (including all three demotion cases and declared-beats-scanned), every
  validation error in `declarations.rs` plus the TOML surface in `config.rs`, the
  CLI format cases in `tests/cli_format.rs`, the salsa cases in
  `tests/incremental.rs`, the CLI lint and `--fix` cases in `tests/cli_lint.rs`,
  and the LSP reload case (`lsp_watched_config_change_reseeds_declarations`).

- [x] ~~**8. Review follow-ups.**~~ **Landed.** Four gaps the implementation
  review found, each of them the mechanism failing to hold its own stated line
  rather than a new feature:

  - **The LSP invariant was stated but not enforced.** `documentSymbol` and
    `hover` read a tree and resolved settings only for `[build]`, and a dozen
    navigation handlers resolved none at all — so in a two-workspace session a
    hover in one could parse under the other's block. Fixed *above* the
    handlers: `publish_declarations_for_request` runs once in the request
    dispatcher and reads `textDocument.uri` off the raw params, which covers
    handlers not yet written. Sixteen call sites would have been the shape that
    rots; `analysis_settings` now serves only the notification sites that
    publish ahead of an `Edit` job. Pinned by
    `lsp_a_request_is_answered_under_its_own_workspace_declarations`, which
    fails on the pre-fix code.
  - **`[environments.foo]` with nothing under it was a silent no-op** — the one
    outcome `resolve()`'s own doc comment calls the worst available, reached by
    exactly the typo `deny_unknown_fields` does not catch. Now
    `DeclarationErrorKind::EmptyEntry`.
  - **A delimiter spelling could shadow a curated command.**
    `begin = ['\emph']` resolved fine and would have taken effect project-wide.
    Rejected now, against the curated tier alone (CWL carries every package's
    names, so it would refuse spellings on the say-so of packages the project
    never loads). Partial by construction, and a backstop rather than the safety
    property — the shape gate is still what bounds a wrong declaration.
  - **`badness parse` ignored `--config`/`--no-config`**, which are `global`
    flags clap accepts on every subcommand. It resolved by discovery only, so
    `--no-config` dumped a tree no other subcommand would produce — the exact
    trap threading declarations into `parse` was meant to close.

  Three smaller items rode along: the mirror comparison takes an `Arc::ptr_eq`
  fast path (`resolve_settings` returns a cached clone, so the pointer settles
  it without walking two signature databases); a spelling repeated *within* one
  entry reports as a repeat rather than as a collision with itself; and an error
  key quotes a name that is not a bare TOML key, so `environments."my.env"` can
  be pasted back.

  The CLI's `format_pathless_sentence`/`declared_scope` moved into
  `badness-formatter` as `format_with_declarations_sentence`/`declared_scope`
  (both were already wasm-clean). The formatter's own invariant suite was
  running a hand-rolled mirror of that pipeline; it now runs the shipped entry,
  so the oracles cannot pass on a pipeline nothing uses — and the dprint plugin,
  which needs exactly this and cannot see the CLI crate, has it. A
  `badness_formatter::declarations` shim mirrors `parser`/`semantic` so an
  embedder can build the value without naming `badness-parser`.

Deferred, recorded so the shape does not have to change later: `[commands.*]`
(same `like` verb — `[commands.eqrefs] like = "eqref"` is what closes #104's
ref-family knob); `args = "o m m"` in xparse argspec for constructs resembling
nothing built in; per-argument `ContentKind`, which argspec cannot express and
which is the reason `like` stays the primary verb; a `[characters."|"]` map if
the shortverb case is ever taken; per-glob overrides; a string-or-array spelling
for `begin`/`end` (arrays only today, since the one-or-many deserializer costs a
hand-written JSON schema); and a `% badness-env` comment directive feeding the
same `Declarations` type, which would be text-pure and would work in the
sandboxed plugin without config.

- [x] ~~**9. One-sided aliases (issue #117).**~~ **Landed**, and it is a change
  to the *alias mechanism* rather than to declarations — the config surface just
  inherits it. The literal `\begin{X}`/`\end{X}` is a spelling of each side, so
  `\def\bsplit{\begin{split}}` alone pairs `\bsplit … \end{split}` and
  `\def\eeq{\end{equation}}` alone closes a written-out `\begin{equation}`. Two
  rules retired: the inference scan's "both halves must be defined in the file"
  (its stated reason, "a lone opener can never pair anyway", held only while the
  closer had to be an alias too; the cost half is carried by `parse_ctx`'s
  is-it-called filter) and resolution's `MissingCloser`/`MissingOpener`, whose
  variants are gone. The reporter's block was rejected for having no `end`, and
  their way out — `end = ['\end{split}']` — is not a control word, so the
  feature had no expressible workaround at all.

  Three things the implementation turned on. The closer spellings are **separate
  indices** (`alias_closers`, `literal_alias_closers`) joined only by
  `closer_target`, since a literal closer emits the ordinary
  `END > \end NAME_GROUP` and an alias closer the bare word. The **mirror needs
  no gate** — a `\begin{…}` pairs by default and locates nothing — so
  `at_block_end` just also stops at a closer alias naming the innermost open
  environment, and `finish_environment` tests that arm *before* the `\end` one
  (`peek_end_name` would read `\eeq{…}`'s own group as an environment name). And
  `math_atom` had to grow the opener dispatch: `split` is math-only, so a
  text-mode-only feature could not see the reported shape at all — which also
  forced its `\end` arm to learn `end_orphans_a_demoted_begin`, the verdict
  `at_block_end` already relies on and `math_atom` was contradicting.

  Measured on the corpus file the report minimizes,
  `latexindent/test-cases/benchmarks/paperSS122018arxivv2.tex` (267 `\bsplit`
  calls, no `\esplit` anywhere): 266 parse diagnostics before, 0 after, output
  idempotent. Gate baselines unchanged; `parse-compat` adds one allowlisted
  deviation and no unexplained one. Knowingly left: the other gates' `\begin`/
  `\end` level counting still does not see alias delimiters, so a literal
  `\begin{X}` closed by an alias inside a brace group can be demoted — silent
  and conservative, and closing it means teaching every gate the alias maps.

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

The next four entries come from fatou's text-storage investigation
(2026-08-16; implemented on its `experiment/arc-str` branch), checked against
badness's code. They are ordered by leverage; the first two are
near-mechanical ports, the third is a project.

- [ ] **`Arc<str>` document text end to end.** Text is `String` everywhere and
  every keystroke pays for it several times over: `did_change` clones the
  whole buffer into the worker job (`src/lsp.rs:1409`, `WorkerJob::Edit`),
  every read-shaped request clones it again (`doc.text.clone()` at
  `lsp.rs:1613, 1666, 1720, 1766, 1836, 1878, 1916, 1970, ...`), `upsert_file`
  guards the salsa write with an O(N) memcmp (`src/incremental.rs:739-771`),
  and the read pool's staleness checks memcmp once more. Fatou's recipe was
  ~70 lines: the live buffer and `SourceFile.text` hold `Arc<str>` (setters
  take `impl Into<Arc<str>>`), worker jobs and request captures carry the
  handle, and the staleness guards get an `Arc::ptr_eq` fast path in front of
  the content compare — never instead of it, since salsa's setter does no
  equality check of its own. Measured there: a no-op upsert went from 39 us
  to a flat 150 ns at 1 MB. While in the area: `apply_content_changes`
  (`lsp.rs:1549-1568`) rebuilds `LineIndex::with_encoding` per content change
  and handlers rebuild it again per request — cache it beside the text
  (arity's spliced-index `TextBuffer` is the model).

- [ ] **A didChange -> upsert -> parse pipeline bench.** Nothing times the
  keystroke composition: `benches/formatting.rs` and the CLI comparison never
  touch `IncrementalDatabase` or `apply_content_changes`, and
  `tests/scaling.rs` guards growth ratios, not the pipeline. Port fatou's
  `benches/salsa_keystroke.rs` (rows per size: no-op upsert, write phase
  without a parse, end-to-end with the parse; alternate insert/delete so
  every iteration is a real revision). In fatou this exact blind spot hid a
  6x end-to-end regression in an otherwise well-benchmarked PR; in badness it
  currently hides the full cost of the entries above plus the full reparse
  below. Land it before the `Arc<str>` port so the improvement is measured,
  not asserted.

- [ ] **Incremental reparse (the elephant).** Every keystroke is a full parse:
  `parsed_document` (`src/incremental.rs:207-222`) calls
  `parse_with_declarations` on the whole text, salsa memoizes at whole-file
  granularity, and there is no `reparse.rs`. LaTeX parses slowly per byte, so
  this is proportionally worse here than it was anywhere else in the family.
  This is a multi-session project, not a port: fatou's tiered reparse
  (`crates/fatou-parser/src/parser/reparse.rs` + its `src/incremental.rs`
  side channel) is the reference implementation, and panache's TODO has the
  phased graduation protocol (oracle invariant, branch discipline, mechanized
  bench gate) worth copying as *process*. Two notes for whenever it starts:
  `did_change` already has the exact edit range in hand (`lsp.rs:1556-1566`)
  and discards it after splicing — keep it plumbed through, and do not let
  the reparse re-derive it with a whole-text diff (fatou measured that diff
  costing more than the reparse it fed).

- [ ] **Borrowed token text, maybe.** Tokens are `SmolStr`
  (`crates/badness-parser/src/parser/lexer.rs:42`, same in `bib/lexer.rs:29`),
  so short tokens are already allocation-free and the fatou-sized win (-60%
  lexing from `&'src str` tokens) does not transfer wholesale. The payoff
  concentrates in the pathologically long tokens (`VERBATIM_BODY`, `VERB`),
  and the conversion is blocked on the half-dozen sites that push constant
  literals instead of input slices (`push_env_delimiter` at
  `lexer.rs:1646-1662`, the braced-verb path at `lexer.rs:1344-1356`,
  `lexer.rs:1457`) — each provably corresponds to contiguous input bytes, so
  they can be rewritten to slice. Measure with a lexer-only bench first
  (arity's workload-stratified `benches/lex.rs` is the template); do this
  last, if at all.

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

  - [x] **C2 — migrate the remaining gates onto one batch driver** (done in
    five stages, C2.1–C2.5; all nine gates now run on `Parser::gate_batch`),
    easiest policy first: alias, then `environment_escapes_group`, then the
    math and bracket family. This item's original wording ("each migration deletes one
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
    on its own shape, and alias needed both halves defined in the same file, so
    those two are migrated for uniformity rather than for speed. (That second
    reason has since lapsed — issue #117 admits a one-sided alias, so an opener
    with no closer in reach is now an ordinary shape. The migration still stands
    on the uniformity argument, and the linearity tests cover it either way.)

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

      *(Amended by C2.4, which was never measured here: the two gates above are
      single-entry, so a batch has nothing to settle beyond its seed, but
      `left_right_closes` nests densely and did go quadratic to linear, ~36x at
      n = 4000. "The perf case for what remains is void" holds for the shapes
      measured, not for the stages.)*

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

    - [x] **C2.4 — `left_right_closes`** (done). The `in_macro_code` blind spot
      (issue \#95) is now two policy predicates (`opens_at`/`closes_at`) beside
      the driver's own `\begin`/`\end` filter, which is the consolidation this
      item promised. The predicted "third environment-counting mode" was **not**
      one: a brace group is already opaque in the driver, since every arm but
      the brace arms is gated on `depth == 0` and `ENVS_AT_ANY_DEPTH` is false.
      What the gate actually diverges on is *nesting shape*, and it is worth
      more than the brace point:

      - **`Nesting::Interleaved`, the driver's second nesting model.** Every
        other gate counts nested openers and environments as two independent
        tallies, because that is all its per-opener scan knew. This one's scan
        is a single LIFO stack of `{`, `\begin`, and `\left` frames — a pair
        closes by count wherever it sits — and the two halves of that read
        differently. A frame **mismatch** (an `\end` or a `\right` meeting a
        frame of the wrong kind) refuses *every* live entry, since the innermost
        frame is the same one for all of them; the **absence** of frames that
        the blank-line anchor tests is seen only by the innermost entry, so a
        nested pair *shields* the ones around it and the anchor settles one
        entry, not a level. Both are pinned
        (`an_end_inside_a_nested_left_refuses_the_whole_scan`,
        `a_nested_left_shields_the_outer_pair_from_a_paragraph_break`).

      - **`MathAnchor`, replacing `MATH_REFUTES`.** The old boolean could not
        say what this gate needs: it anchors on `$`/`\]`/`\)`, the *closing*
        side, where the pairing gates anchor on `$`/`\[`/`\(`. Which side
        follows from where the construct lives — a conditional lives in text and
        is defeated by math starting, a `\left` lives inside math and is
        defeated by that math ending, and those are exactly `left_right`'s own
        recovery anchors. `None`/`Opening`/`Closing`, one arm each.

      - **A gate/walk looseness surfaced, not introduced.** In the shielded
        shape the gate says the outer pair closes while `left_right` bails at
        the paragraph break and reports it unclosed. It predates the migration
        (the per-opener scan's `stack.is_empty()` behaved identically) and C2
        migrates verdicts unchanged, so it is preserved and pinned rather than
        fixed. Reachable only inside a math *environment*, since the `$`/`\[`
        gates refuse a blank line themselves. Fixing it is its own commit
        against the "a shape gate must mirror the parse it guards" rule.

      **Measured** (`parse` alone via `bench:micro`, N `\left(` in one `$…$`
      with a single `\right)` at the end, 1000/2000/4000): **3.9/14.6/57.0ms →
      0.39/1.0/1.6ms**, quadratic to linear, ~36x at n = 4000. Unlike C2.3's
      single-entry gates this one had real work to save: a `\left` the walk
      demotes is retried as a plain command and every `\left` after it is asked
      in turn, so a run of them re-scanned per opener. Pinned by
      `left_right_batch_keeps_shared_frame_openers_linear`. `bench:micro` on the
      four real documents is flat within run-to-run noise.

      Shadow differential green: the suite, a per-file debug-assertions pass
      over all 6277 corpus files, and a `.tex`/`.dtx` torture pair. The torture
      files discriminate all four of the gate's policy knobs — flipping
      `MATH_ANCHOR`, `NESTING`, `MACROCODE_FRAME_ANCHORS`, or `STRAY_BRACE`
      panics on them — which is the bar a torture file should meet; the
      stray-brace shape needed a `\left` inside a math *environment*, since a
      stray `}` demotes an enclosing `$` before the gate is ever asked. Two
      process notes: the driver's `break` paths must not leave an index in
      `live` pointing past `pending` (the interleaved closer arm pops its entry
      first, and the corpus pass caught the panic at once), and the reference
      copy must not tick `scan_work` or it hides the linearity the pin measures.
    - [x] **C2.5 — the bracket family** (done). The `abuts_command` claim
      countdown *was* the driver's nested-opener stack, exactly as predicted —
      an opener is a `[` whose previous token is a control word or symbol
      (the running flag, which every other kind cleared, is that test one token
      back), and closer matching is LIFO either way, so the family needed **no
      new nesting model**. Both pre-questions settled:

      - **The memo key carries the flavor.** `WalkKey` gained
        `enclosing_math_is_dollar`; nothing else in the key moves when it does,
        since entering a `$` inside `\[…\]` changes no brace, group, or frame
        state.

      - **The missing `plain_braces` filter is not a latent macrocode bug — it
        is arguably the *faithful* reading.** `Parser::optional` bails at any
        `R_BRACE` without consulting `plain_braces`, so the in-math gate mirrors
        the walk and its two siblings are the loose ones: an optional they let
        attach over a chunk-plain `}` still reports "unclosed `[`" and blocks
        the file for the formatter. It is also one-directional (a chunk-unmatched
        `}` can only occur at chunk brace depth 0, so the scan meets it at its
        own depth 0 and refuses, while a chunk-unmatched `{` only adds depth), so
        the unfiltered reading refuses a bracket the filtered one attaches and
        never the reverse. Preserved as `PLAIN_BRACES_ARE_TOKENS`; unifying is
        its own commit, and the pre-existing gate/walk looseness above is the
        thing to fix first.

      What the family did need is **two anchors read depth-blind**
      (`EnvAnchor::Refutes`, `ParagraphAnchor::AnyDepth`), because `optional`'s
      own bail is depth-blind: it bails wherever the cursor stands, so a gate
      reading either at the bracket's own brace level would attach an optional
      the walk then reports unclosed. `ENVS_AT_ANY_DEPTH` widened to
      `ANCHORS_AT_ANY_DEPTH` (the `\]`/`\)` arm joins the `\begin`/`\end` one),
      verdict-preserving for all five earlier gates, since the two that set it
      read math as content anyway. `DollarAnchor` is the driver's first
      **runtime** policy — a `$` in `\[…\]` opens a *transparent* region where
      the entries' own brackets stop counting, and inside `$…$` it refuses — and
      it has to be a method because the flavor is walk state.

      Two more preserved divergences, both named and both discriminated by the
      torture pair:

      - **`ENV_ANCHOR_IN_MACRO_CODE`** — the in-math gate's `\begin`/`\end`
        anchor carries no `in_macro_code` filter, so it is *stricter* than the
        `optional` bail it mirrors. Only ever declines to attach
        (`a_math_bracket_anchors_on_an_environment_inside_macro_code` pins it
        beside its text-mode twin, where the same body attaches).

      - **`DOC_TRIVIA_FLOATS`** — the `macrocode` gate skips only `WHITESPACE`
        in its paragraph run, so a docstrip guard line **breaks** the run where
        every other gate floats it. This one is **corpus-reachable and
        load-bearing**, the only knob of the whole C2 stack that is: flipping it
        panicked the shadow reference on `rotating.dtx` and `rotex.tex`, whose
        `\ProvidesPackage` date optional runs over three guard lines inside one
        chunk. It is also the `saw_blank_line_outside_guards` reading (#71:
        docstrip *deletes* a guard-only line, so it does not part what surrounds
        it) — which means the *other* seven gates are the ones diverging from
        the considered model, and unifying should move toward this gate, not
        away. Recorded as its own item below, and **since resolved that way**:
        the knob is gone and the driver reads guards this gate's way.

      **Measured** (`parse` alone via `bench:micro`, N openers with one closer
      at EOF, 1000/2000/4000). Text (`\cmd[x` per line, one `]`):
      **3.2/12.4/50.5 ms → 0.29/0.56/1.16 ms**, ~44x at n = 4000. In math (the
      same run inside one `$…$`): **1.8/9.4/28.9 ms → 0.39/0.59/1.19 ms**, ~24x.
      Both pinned by `bracket_batch_keeps_shared_frame_openers_linear`. The
      `macrocode` gate is single-entry by policy (its scan ran no countdown), so
      a chunk of `\cmd[` openers whose only `]` sits past the frame stays
      quadratic; recorded in the test rather than pinned, like the `${` ratchet.
      `bench:micro` flat on all four real documents.

      Shadow differential green: the suite, a per-file debug-assertions pass over
      all 6277 corpus files, and a `.tex`/`.dtx` torture pair discriminating all
      six of the family's policy knobs (the `.dtx` half carries the three that
      need a chunk). Baselines, `parse-compat`, `bib-parse-compat` unchanged.

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

  **What the finished driver left on the table** (both surfaced by C2.5, both
  their own commit with their own test — C2 migrated verdicts unchanged):

  - [x] **A docstrip guard line parted a construct for seven of the eight
    gates** — fixed by moving the driver *to* `MacrocodeBracketGate`'s reading,
    as the item predicted. `DOC_TRIVIA_FLOATS` is gone; the driver's trivia arm
    now splits the two kinds it used to lump together, which is what the
    `saw_blank_line_outside_guards` model always said: a `DOC_MARGIN` floats
    like whitespace (a margin-only line is still the documentation layer's blank
    line), a `GUARD` breaks the newline run without being a newline (#71:
    docstrip *deletes* a guard-only line, so it does not part what surrounds
    it). Note the old `false` arm was not quite that model either — it broke on
    a margin too, harmlessly, since the only `DOC_MARGIN` a chunk-bounded scan
    can reach is its own end frame's.

    The predicted direction held — it *keeps* constructs demoted today — and it
    paid on the first corpus that could see it. `trace.dtx`'s second
    `% \iffalse … % \fi` header spans four guard lines, so the float refuted
    `ConditionalGate`'s paragraph anchor, demoted the `\iffalse` to a plain
    command, and let the formatter reflow the guards as prose — collapsing
    `%<driver>` off column 0. That is a **non-trivia content change**, i.e. the
    whitespace-only invariant, and `tests/gate_baselines/latex2e.{all,trivia}`
    had it recorded as a known failure; both baselines are re-recorded without
    it. No other corpus entry moved, in either direction, and `parse-compat` /
    `bib-parse-compat` are byte-identical, as a verdict-*widening* fix on a
    shape texlab has no model for should be.

    The pre-existing test for the macrocode reading
    (`a_guard_line_does_not_part_a_macrocode_optional`) turned out to be passing
    for the wrong reason: `bracket_attachments` parsed with `dtx: false`, so its
    `%<*dtx>` was an ordinary `COMMENT` (which breaks the run anyway) and its
    `%    \begin{macrocode}` frame never opened a chunk — `TextBracketGate` was
    answering. It now goes through a `dtx: true` helper, and two siblings pin
    the unified reading on the other two paragraph-anchor shapes
    (`ParagraphAnchor::AnyDepth` in a doc line, `OwnLevel` for the `$` gate),
    each with a real-blank-line and a margin-only-line control so both halves of
    the trivia split are discriminated.

  - [ ] **`optional` bails at a chunk-plain `}` its own gates skip.** Two of the
    three bracket gates filter `plain_braces`; `Parser::optional` does not
    consult it at all, so an optional they let attach over a chunk-unmatched `}`
    is then reported "unclosed `[`" — a diagnostic that blocks the whole file
    for the formatter, from a brace the macrocode model says is an ordinary
    token. A gate must mirror the parse it guards, so the fix is in `optional`
    (skip a `plain_brace` `}` as `element` already does), not in the gates; the
    in-math gate's unfiltered reading then becomes the odd one out and
    `PLAIN_BRACES_ARE_TOKENS` can go. Pre-existing, not introduced by C2.5.

- [x] **Four quadratics behind the "formatter and linter are superlinear"
  entry — none of them where that entry said.** The original note read the two
  degenerate shapes as "a linter rule is quadratic on math-delimiter-heavy
  input" and "the parser is no longer implicated in either". All three
  conclusions were wrong, and the costs were not degenerate-only: on
  `phd_dissertation.tex` (730 KB, 27 482 lines) the CLI spent **6-10x the real
  work** rendering its output.

  What isolated each one — worth reusing, since none needed a profiler to find:

  - `lint --output concise`/`json` were flat-linear on the very input that made
    `--output pretty` quadratic, so no rule, side index, or tree walk was
    implicated. Holding findings fixed while growing the file (and vice versa)
    showed the cost was the *product*, not findings squared.
  - `format --stdin` was exactly linear on the itemize shape; only `--check`
    without `-q` was not, so the layout engine was never involved.
  - A large file with a *tiny* diff cost nothing extra, so diff cost tracked
    edit distance rather than file size.
  - `\begin` vs `\bezin` — same CST shape, one command name apart — separated a
    `\begin`-path cost from everything else in the parse.

  The four, and their fixes:

  1. **Pretty diagnostic rendering** handed `annotate-snippets` the whole file
     per finding, rebuilding an O(file) source map on every `render()`:
     O(findings x file length). Windowed to the annotated lines
     (`Snippet::line_start`), output byte-identical. Ported from arity
     `11a4558`; fatou and panache still carry it.
  2. **The `--check` diff** ran Myers, `O((N+M)*D)`, and `D` is the whole file
     whenever the formatter relays a document. Switched to
     `Algorithm::Histogram`, plus one buffered writer instead of a `print!` per
     line. Chosen on the pinned gate corpora, not a synthetic: summed diff cost
     over the 60 largest real files is Myers 2418 ms, Patience 919 ms,
     Histogram 643 ms, and Histogram also wins the *worst single file*
     (76 ms against Patience's 657 ms and Myers' 2023 ms).

     **The algorithm is a per-language choice and must not be ported.** The
     ranking inverts across the siblings — arity (R) wants Patience (206 ms vs
     Myers 897 ms vs Histogram 1947 ms), panache (Markdown) is best on plain
     Myers, and fatou (Julia) keeps Patience because its Histogram cliff costs
     seconds rather than the milliseconds it costs here. This was learned the
     expensive way: the fix was ported to fatou verbatim, measured 4.5x *worse*
     there, and briefly took this repo to Patience on that evidence before a
     real-corpus sweep put it back.

  3. **`Parser::on_doc_margin_line`** walked back to the previous `NEWLINE` for
     every `\begin`/`\end` via `doc_margin_exempt`, so a one-line document was
     O(N x line length). Answered from a `PreScan` index instead.
  4. **`contains_doc_margin`** is a match guard on most of `lower_node`'s
     relayout arms and walked each node's whole subtree, so nested groups
     re-walked at every level — quadratic in nesting depth, for every file.
     Gated on `cx.is_dtx`.

  | | before | after |
  |---|---|---|
  | `phd` `lint` | 661 ms | **114 ms** (concise: 111 ms) |
  | `phd` `format --check` | 2204 ms | **219 ms** (`-q`: 169 ms) |
  | `masters` `format --check` | 42.5 ms | **22.6 ms** |
  | N=4000 `\[` `lint` | 224 ms | 15 ms |
  | `{{{x}}}` nested 4000 | 555 ms | 136 ms |

  `tests/scaling.rs` (plus one case in `main.rs`, where `diff_lines` lives) now
  guards the growth *rate* of all four, each verified to fail when its fix is
  reverted. There was no performance test in the repo before this.

- [ ] **`Ir::contains_forced_break` is a per-child subtree walk at lowering
  time**, so nesting depth is still superlinear — 64% of the run on `{{{x}}}`
  nested 4000 deep, the residue after the `contains_doc_margin` gate above.
  `saturate` (`ir.rs`) already computes the identical bit bottom-up in one O(n)
  pass, precisely so it is "computed on the way up, never by re-traversal", but
  it runs once at the printer seam while lowering asks the question repeatedly
  on partial sub-IR — which `core.rs` explicitly sanctions today. So this is a
  documented decision to revisit, not a bug to patch: the bit changes as the IR
  is rebuilt during lowering, so a memo has to be keyed on something that
  cannot go stale. `Ir::contains_group` has the same shape. Deep brace nesting
  is the only shape that reaches it (both bench documents are unaffected), so
  it is not urgent. `tests/scaling.rs` tolerates the residue at a 3.4x bound;
  tighten it to 3.0x when this lands.

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
  ongoing). Skill: `.claude/skills/formatter-fixture/`. The corpus is read as a
  coverage map — which constructs occur and in what shapes — and **latexindent
  itself is the taste reference we check each construct against**: 711 of its
  test files are named for the upstream issue that produced them, across 127
  distinct issues, so its answers carry a decade of real user pushback.
  Never a byte-target: it is an indenter that preserves author breaks and
  never touches intra-line spacing, where we reflow and own layout. But every
  divergence gets a verdict (corroborates / explained / no opinion /
  unexplained), and an *unexplained* one blocks the fixture until it is worked
  out — that is where our rule is usually wrong. Run it at default settings
  (`latexindent probe.tex`, no `-s`) on a hand-authored probe; the committed
  `*-mod*.tex` files are one YAML stack's answer with `-m` on, not its own
  judgment. Measured gaps against the 239 existing
  slugs: `items` (157 files) and bare/named brace groups are no longer thin (11
  and 33 slugs); re-measure before trusting any gap list here. `mand-args` /
  `opt-and-mand-args` / `environments` yielded `begin_tail_is_body` — content the
  greedy parser attaches to `BEGIN` past the declared arity is body, not header —
  which closed a Tier-1 lone-newline read *and* a column-0 indentation bug, and
  surfaced the paragraph-reflow glued-split entry above; the rest of those three
  families is still open. `filecontents` is
  done (`filecontents_protected_body`) — it was purely a protected-region
  question, and the survey found no defect: the sharp edge it now pins is that a
  verbatim-body environment's `\begin` line must never break under width
  pressure, since it defines where the protected body starts and `filecontents`'s
  optional is `Keyval` (which elsewhere licenses a comma split).
  Sectioning/`headings` is done (two slugs, and the Tier-1 lone-newline
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

- [ ] **`deprecated-suppression-syntax`: report the retired `% badness-ignore`
  family.** `% badness-ignore <rule>` and `% badness-ignore-file [<rule>]` are
  undocumented but still resolve (permanently — a directive spelling is
  user-facing API). Nothing tells a user their file carries the old spelling, so
  a warning with a **safe** autofix rewriting to `% badness-lint skip <rule>` /
  `% badness-lint skip-file <rule>` is the missing half of the deprecation. The
  rewrite is entirely inside a comment, so it is textual, trivially lossless,
  and needs no layout decision — exactly the fix contract. The fact is already
  computed: `directives::Directive::deprecated` marks these at parse time, so the
  rule needs `Suppressions` to retain the directives it saw (with the comment
  token's range) rather than only the resolved ranges. Covers both carriers —
  the `%` comment and the `.bib` `@comment{…}` entry.

- [ ] **A meta rule for inert suppression directives.** Ruff's documented wart is
  that a misplaced `# fmt: off` does nothing and says nothing; badness now has
  the same hole. Report a `% badness…` directive that suppresses nothing: an
  `on` with no open region, a `skip` with no following construct, an `off` left
  unclosed at EOF (which runs to end of file on purpose, but is worth saying), a
  `% badness-format` directive in a `.bib` (parsed, deliberately inert), and a
  directive written on a `.dtx` doc-margin line, where the leading `%` is a
  margin rather than a comment so the directive is inert by construction. Wants
  the same retained-directive list as the rule above, so do them together —
  fatou's `meta/*-suppression` rules are the model. A natural companion is
  `unexplained-suppression` (no `: <reason>`).

- [ ] **Format suppression in `.bib`.** The `% badness-format` axis parses in a
  `.bib` `@comment{…}` and deliberately does nothing. The bib formatter is a
  canonical re-emitter rather than a trivia-only pass, so "reproduce this span
  byte for byte" is a genuinely different mechanism there, not a matter of
  routing the resolved ranges through. Until it exists, the axis is silently
  inert, which is the meta rule above's job to report.

- [ ] **A `%`-comment directive carrier inside a `.bib` entry.** Now that a `%`
  comment exists inside an entry, the LaTeX-side carrier could work there too;
  today only the `@comment{…}` entry form does (`bib/linter/suppression.rs`).
  The grammar is already shared, so this is only a decision about what an
  in-entry comment attaches to (the field below it, presumably, matching the
  formatter's forward bind).

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
