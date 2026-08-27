---
name: formatter-fixture
description: >-
  Grow badness's formatter fixture coverage one LaTeX construct at a time,
  seeded from the latexindent gate corpus. You surface representative inputs,
  draft the canonical formatting under the tenets, check it against latexindent
  as a reference implementation and account for every divergence, the user edits
  or accepts `expected.tex`, then you implement the rule and register the
  fixture. The corpus is a coverage map and latexindent itself is the taste
  reference — never a byte-target, but every divergence from it gets a verdict.
  Use for "take the next construct", "add a formatter fixture for X", or "mine
  the latexindent corpus for X".
---

Use this skill to add formatter coverage for a *construct*, not to fix a
reported bug (that is `smoke-test-triage`) and not to triage a corpus sweep
(that is the failure inventory in `tests/gate_baselines/README.md`).

## How to read the corpus

`corpora/latexindent` (pinned in `scripts/fetch_gate_corpora.sh`, fetched by
`task gate-corpora:fetch`) is latexindent.pl's own test suite: ~5.3k small
hand-written files of deliberately adversarial LaTeX. Its primary job here is to
answer **"which constructs occur in the wild, and in what nasty shapes"** — a
coverage map that the expl3-heavy latex3/latex2e/pgf corpora do not provide.

### Check against latexindent, and give every divergence a verdict

latexindent.pl is the closest thing LaTeX has to a reference implementation of
"what formatted LaTeX should look like", and its test suite shows why: 711 of its
files are named for the upstream issue that produced them, across 127 distinct
issues, plus a handful named for the TeX StackExchange question behind them
(`te-752552`). That is a decade of real users bringing real documents and pushing
back on the answer. Treat it as **a reference you check against on every
construct**, not as a rival to be discounted — its accumulated taste is real
signal, and it has seen shapes you will not think to try.

Two things it is not. It is not a byte-target: *as invoked below* it indents —
fixing leading whitespace, preserving the author's line breaks, and leaving
intra-line spacing alone — where badness reflows and owns layout outright. (It
*can* reflow, under `-m` with `modifyLineBreaks`/`textWrapOptions`; that is a
configured mode, and the default is the one worth comparing against, for the
reason below.) And it does not outrank the tenets. Neither of those makes it
something to skip; they make it something to **account for**.

**Run it — do not read the committed outputs.** The harness writes results
in-tree and asserts `git diff` is clean, so every committed `*-mod1.tex` /
`*-output.tex` is one YAML settings stack's answer with `-m` (modify line breaks)
on — read the directory's `*-test-cases.sh` to see which
(`-l=env-all-on,env-mod-lines9` and friends). Those are answers to a *configured*
question, not latexindent's own default judgment. Get that by running it yourself
on a probe you hand-authored, at default settings:

```sh
latexindent probe.tex      # formatted document on stdout
latexindent - < probe.tex  # same, via stdin
```

It ships with TeX Live, so `texlive.enable` puts it in the dev shell already.
**Do not pass `-s`** — that is silent mode and it suppresses the stdout you want.
At default settings `modifyLineBreaks` is off, so latexindent will not add or
remove a break: it answers "where does this belong, given the breaks the author
wrote", which is exactly the question worth borrowing on.

Every shape you check gets one of four verdicts, and you report them:

- **Corroborates** — same layout. Cheap confidence, especially on a rule you
  derived but could not fully pin from the tenets.
- **Explained divergence** — traceable to a known model difference: we reflow and
  it preserves, we own layout and it indents, we normalize intra-line whitespace
  and it never does. Name the difference in one line and move on.
- **No opinion** — the construct falls outside its model, so byte-identical
  output is *not* agreement. Intra-line whitespace is the common case: a probe
  that comes back unchanged because latexindent never touches that spelling tells
  you nothing. Say "no opinion", never "agrees".
- **Unexplained divergence** — neither model accounts for the difference. **This
  is the item: stop and raise it with the user before landing the fixture.** The
  usual cause is that our rule is wrong, or right but under-specified in a way
  that happens to bite this shape. Resolving it may mean changing the proposal,
  or recording why we diverge on purpose — but it is never left unremarked.

Three habits keep the checking honest:

- **Form your own proposal first, then check.** Looking first anchors you.
- **The tenets outrank it.** Where a tenet and latexindent disagree, the tenet
  wins — but that is an *explained divergence* to record, not a reason to have
  skipped the check.
- **Justify on the rule, not on the agreement.** "latexindent does it this way"
  is evidence, not a reason; "this is what the reflow rule yields, and
  latexindent independently lands on it" is how it goes in a commit message.

**Hand-author fixture inputs and probes; never copy corpus files.** latexindent
is GPL-3.0 and badness is MIT — the fetch-don't-vendor setup keeps `corpora/` out
of the tree, and a copied fixture would undo that. The median corpus file is ~200
bytes, so restating a construct minimally is cheap, and it doubles as a
comprehension filter: if you cannot restate the shape, you have not understood
it yet.

## The loop (one construct per session)

1. **Pick a construct.** Prefer a user-named target. Otherwise take one from the
   coverage gaps below. Baseline must be green: `cargo test --workspace`.
2. **Survey the shapes.** Find the construct's directory under
   `corpora/latexindent/test-cases/`, read a handful of *input* files, and pull
   out the distinct shapes (nesting, trailing comments, blank lines, unusual
   argument forms). Inspect the CST (`badness parse <file>`) and today's output
   (`badness --no-config format < file`) for each. Pipe via stdin — `format
   <file>` rewrites in place.
3. **Draft the canonical form.** Author the layout you believe is right under the
   tenets — deterministic, rule-based, input-independent — and write down the
   reasoning. Keep the input minimal and hand-written. Do this *before* step 4.
4. **Check against latexindent.** Run your probe through it at default settings
   (see above) for each distinct shape, and give each one a verdict:
   corroborates / explained divergence / no opinion / unexplained divergence. An
   **unexplained divergence is a blocker** — work it out before proposing, and if
   you cannot, put the question to the user as part of the proposal rather than
   burying it.
5. **Propose `expected.tex`** to the user: the form, the rule it rests on, and
   the latexindent verdicts — including anything that moved your answer, and the
   invocation you used.
6. **The user edits or accepts.**
7. **Push back when warranted.** If the choice is unprincipled, breaks a tenet
   (especially "layout is decided solely by the formatter's rules"), reads a
   forbidden trivia predicate, or conflicts with an existing fixture, name the
   conflict and the affected fixture and resolve it before writing code.
   Diverging from a prior decision is allowed but must be conscious.
8. **Implement the rule**, then **register and lock** it (below).
9. **Guardrails**, then **record the rule** (below), then commit.

## Registering a fixture — the trap

`crates/badness-formatter/tests/fixtures/formatter/<slug>/{input,expected}.tex`
is *not* self-registering. Unlike a fixture-dir-is-membership setup, badness
drives fixtures from explicit tables in
`crates/badness-formatter/tests/format.rs`:

- `FIXTURES` — `(slug, WrapMode, line_width)`, the main table
- `MATH_WRAP_FIXTURES`, `DTX_FIXTURES`, `DTX_REFLOW_FIXTURES`,
  `PACKAGE_FIXTURES`, `INS_FIXTURES` — the specialized ones

**There is no orphan guard.** A fixture directory absent from every table
silently never runs, and has shipped that way before
(`expl_relation_slot_statement` sat unregistered across a commit). Each table is
driven by *one* looping test, so a slug is not a test name — filtering by it
(`cargo test … <slug>`) reports `0 tests` whether or not the fixture is
registered, and proves nothing. To confirm it really runs, corrupt
`expected.tex`, watch `formatter_fixtures_match_expected` (or the table's own
test) fail naming your slug, then restore it.

**Corrupt one slug at a time.** The looping test asserts *inside* the loop, so it
aborts at the first mismatch and never reaches the rest of the table. Corrupting
two `expected.tex` files together proves only that the earlier one runs — the
later one is exactly as unverified as if you had skipped the check, and the run
looks like a pass of the whole thing. Do corrupt → run → restore once per
fixture, and read the slug named in each failure.

Restore by **regenerating** (`badness --no-config format < input.tex >
expected.tex`), not `git checkout`: a fixture added this session is untracked, so
`git checkout` on its directory silently leaves the corruption in place.

Also add a `… eol=lf` line to `.gitattributes` if you introduce a new extension
under `crates/*/tests/fixtures/**` (Windows CI compares bytes).

## What keeps this rule-based

The discipline is structural, not a promise to be careful:

1. **The oracles outrank the fixture.** Any special case bought to make one
   `expected.tex` match must still survive losslessness, idempotence,
   trivia-convergence, and the two-sided baseline ratchet over four corpora
   (`task gate-corpora:check`). A hack that satisfies a fixture and breaks a
   baseline is rejected by construction.
2. **A fixture lands only with a named rule.** If the only way to reach the
   expected output is a special case keyed on a specific command or environment
   name, stop — that is tenet 1's "push back against hard-coding special cases",
   and it means the canonical form or the rule is wrong.
3. **Never key layout on a lone source newline.** A width wrap and an authored
   newline are the same bytes to the next parse. Blank lines, comment presence
   and own-line-ness, and column-0 `.dtx` margins are fair game; the gap between
   a newline and a space is not. A mode that genuinely needs it is Tier 2 and
   owes a written fixed-point argument (`AGENTS.md`).

## Guardrails

```sh
cargo test --workspace
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

If the change moves layout, also run `task gate-corpora:check` and re-record any
baseline it shifts, documenting the movement in `tests/gate_baselines/README.md`
the way every prior re-record does (what resolved, what was added, whether
production output moved). The pre-commit hook runs `panache-format` and
rustfmt — never `--no-verify`.

## Recording the rule

The output of this skill *is* a new layout rule, so the fixture is only half the
artifact. Before committing, write the rule down where the next session will meet
it — AGENTS.md's own upkeep rule, applied to what you just landed:

- **`AGENTS.md`** — a bullet whenever the rule is a directive
  someone could violate later ("never route X through Y", "keep Z structural").
  Terse: the rule, the one clause that keeps it from looking arbitrary, and the
  function name that carries it. This is the common case and the easiest to skip.
- **`docs/src/development/architecture.md`** — only if the change is visible at
  the level of the tour (a new lowering path, a new gate, a changed subsystem
  boundary). A line-break rule inside an existing lowerer usually is not.
- **`AGENTS.md`** — only if one of the numbered core decisions or the invariant
  list actually changed. Rare; say so explicitly rather than editing by reflex.
- **TODO.md** — if the construct had an open entry, close it *in place* and say
  what is still open. A Tier-1 fix usually resolves one member of a family, not
  the family; leaving the entry as a bare `[x]` loses that.

Also refresh this file's coverage-gap backlog when you take an item off it —
including the slug count, which the next session reads as fact.

## Coverage gaps (ranked starter backlog)

Measured against the 282 existing fixture slugs. **Re-measure before trusting
this list** — it has gone stale twice: `items` and brace groups were listed as
thin at one and four fixtures and were actually at 11 and 33; `specials` and
`diacritics` sat at the top of the list for two sessions and turned out not to be
layout families at all. Count with `ls
crates/badness-formatter/tests/fixtures/formatter/ | grep -icE '<pattern>'`
first, **and read a few of the family's own `.tex` files** before believing the
family is about what its directory name suggests.

Candidates not yet checked against a fresh count:

1. **`environments`** (293), **`mand-args`** (202), **`opt-args`** (217) —
   partly mined (see `begin_tail_is_body` and
   `environment_leading_body_command` and
   `environment_keyval_group_splits_entries` under Done); 38 and 25 slugs now
   match `env`/`arg`, so verify against current slugs before picking.

`items` and bare/named brace groups are no longer thin; re-measure before
returning either family to the ranked backlog.

Off the list, on their content rather than their quality:

- **`tokenChecks`** — its four files probe latexindent's own internal
  placeholder tokens colliding with document text, an implementation concern
  badness does not share.
- **`specials`** — latexindent's *specials* is its user-configurable
  begin/end-pair mechanism, not a LaTeX construct. Its `.tex` content is display
  math, inline math, and `\left`/`\right`, already at 33 `math`-matching slugs.
- **`diacritics`** — two files and two directories with non-ASCII *names*; the
  `.tex` content is a plain nested-environment document. It tests UTF-8 path
  handling, which is a CLI concern with no `expected.tex` to write.

Done: `environment_empty_body` — an environment's structural frame stays
multiline when its lowered body is empty; collapsible whitespace between
`BEGIN` and `END` cannot choose another layout, and a nested empty environment
receives the ordinary body indentation. The formatter already implemented the
rule, so this is a lock-in fixture rather than a production-code change.

Done: environment names containing punctuation
(`environment_special_character_names`) — pairing reads the complete flat
`NAME_GROUP`, including punctuation such as `@` and `*` and the multiple lexer
tokens produced at `_`; the names receive ordinary environment framing and
nesting. The formatter and parser already implemented the rule, so this is a
lock-in fixture rather than a production-code change.

Done: `begin_tail_is_body` — content the greedy parser attaches to `BEGIN` past
the *declared* arity is body, not header, so it indents and reflows with the body
instead of stranding at the `\begin` column. The header ends at the last element
glued to it, which reads only `Gap::Glued`-versus-not.

This is the **worked example of the latexindent check** paying for itself. The
tenets left a live choice between "non-glued tail goes to the body" and "glue the
whole tail up onto the header"; running latexindent at default settings
corroborated the first on three shapes (over-attached group → body level; a glued
`{a}` kept on the header while a following `[b]` dropped to body — its split point
is ours exactly; a header `%` keeping its comment and bodying the group after it),
which settled the choice and pulled the comment case into scope. The fourth shape,
all three groups on one line with multi-space runs, was **no opinion**: it came
back byte-identical only because latexindent never touches intra-line whitespace,
which is not agreement.

Two traps worth knowing before touching this area again: the tail must be
**spliced into the leading paragraph's reflow** (concatenating ahead of it deletes
an inter-word space, since a paragraph trims its own leading newline — no oracle
sees this), and the A/B whitespace-collapse sweep that caught it is the only
mechanical check for that class. It also surfaced a pre-existing glued-split bug
in `reflow_elements` (recorded in TODO.md, not fixed here).

Done: `environment_leading_body_command` — once the structural `BEGIN` header
ends, a following command starts the indented environment body even when the
author wrote it on the header line. The `environments/issue-508` shape exposed
the missing lock-in; the formatter was already correct, so this landed a fixture
and recorded rule without a production-code change. Default latexindent
preserved both inline spellings (explained divergence: it does not add breaks),
while an already-broken control corroborated the body placement and nesting
depth.

Done: inline environments amid prose (`environment_inline_prose_boundaries`) —
an environment expanded as a structural block opens and closes its own lines;
following prose never rides the closer merely because the author used a space
instead of a newline. A trailing comment still rides the closer because moving
it changes TeX spacing and comment binding. Default latexindent preserved the
inline probes (explained divergence: it does not add breaks), corroborated the
suffix boundary and comment attachment where the author supplied breaks, and
corroborated the complete generic and nested multiline controls.

Done: adjacent sibling environments (`environment_adjacent_siblings`) — each
environment keeps its own structural frame, but adjacency alone does not create
a blank line. Authored paragraph breaks remain, nested siblings receive ordinary
body indentation, and a trailing comment stays attached to the preceding closer.
The formatter already implemented the rule, so this is a lock-in fixture rather
than a production-code change. Default latexindent preserved the inline probes
(explained divergence: it does not add breaks), corroborated consecutive frames,
nesting, authored blank lines, and comment attachment in already-expanded
controls, and exposed no unexplained divergence.

Done: comments between declared environment arguments
(`environment_argument_comment_barrier`) — declared arguments ordinarily glue
to the `\begin` header, but a trailing comment is a semantic barrier: a following
mandatory group stays in the header on an indented continuation line, where the
comment cannot consume it. The brace gate is essential; adding indentation
before a following optional can change whether TeX recognizes it. Default
latexindent corroborated the mandatory continuation indent and preserved the
comment boundary; its retention of authored breaks in the no-comment control was
an explained divergence from Badness's formatter-owned reflow.

Done: omitted optional environment slots
(`environment_omitted_optional_slots`) — `lower_begin`'s ordinary path matches
attached groups against positional signature slots instead of counting attached
nodes. Omitted optionals are skipped; once supplied groups exhaust the
signature, a separated brace-shaped element begins the body. An unmatched
delimiter before a pending required slot demotes the remaining header to
ordinary glue boundaries, which keeps incomplete curated signatures from
changing content classification on the next parse. Default latexindent
preserved the authored broken argument lines (explained divergence: it does not
remove breaks), while an already-inline control corroborated the body group's
indentation in all four optional-presence shapes.

Done: `filecontents` (`filecontents_protected_body`) — no defect found; it pins
that a verbatim-body environment's `\begin` line never breaks under width
pressure (it defines where the protected body starts, and `filecontents`'s
optional is `Keyval`, which elsewhere licenses a comma split), plus that the
`\end` marker's indentation is inside `VERBATIM_BODY` and so author-preserved.
A construct whose behavior is already correct legitimately yields a lock-in
fixture plus a recorded rule, not a new rule.

Done: sectioning / `headings` (`sectioning_starts_own_line`,
`sectioning_blank_line_and_comment`) — a sectioning command is a block-level
statement, breaking before and after from `CommandSig::sectioning`. That landed
the Tier-1 lone-newline fix its TODO entry described; the rest of that family is
still open.

Done: `keyEqualsValueBraces` (`keyval_group_splits_entries`,
`keyval_group_declines_on_comment`) — the corpus's **largest family, 585 files**,
and it was sitting under a directory name nobody had decoded. A *mandatory* brace
argument the signature DB proves keyval segments at its top-level commas, exactly
as the bracket does; before, `\pgfkeys{…}` fell to the prose reflow and wrapped
mid-key.

Two things generalize from it. First, **the gap was already written down in the
code** — `gen_cwl_signatures.py` said a `%keyvals` mandatory group "is real but
nothing consumes the flag there" and `core.rs` said `Keyval if is_bracket`. Grep
for a guard whose comment explains why the other half is missing; that is a
construct waiting to be taken. Second, **the whole change was a delimiter
parameter plus eleven lines of signature data** — the rule was already there, in
the wrong scope. Prefer that shape over a new rule.

`task typeset:check` is not optional here and it earns its keep: it is the only
oracle that sees a space token, and on the first run it failed — on an invalid
key in the *test document* rather than a formatter bug. Compile a new
`tests/typeset/` input on its own before trusting a diff from it.

Done: mandatory keyval arguments on environments
(`environment_keyval_group_splits_entries`) — `lower_begin` routes a declared
`ContentKind::Keyval` brace slot through the same top-level-comma segmentation as
the command path. The standard tabularray environments (`tblr`, `longtblr`, and
`talltblr`) are curated as `O{} m` keyval headers but deliberately not as `align`:
their mandatory group is an inner keyval specification, not the raw column spec
`column_alignments` assumes. Their top-level `&` still selects the structural grid
router. A direct compile and `task typeset:check` proved the introduced spaces
typeset identically; all four gate-corpus baselines stayed fixed.

Default latexindent preserved the short inline header and comment boundary,
preserved the overlong inline header because modify-line-breaks was off, and
indented an already-expanded mandatory group two tab levels beneath `\begin`.
Those were explained divergences from Badness's width-owned reflow and its shared
segmented-group frame; its unchanged glued-comma spacing was no opinion. The
initial proposal kept the comment-bearing opener inline, but implementation
exposed its conflict with `keyval_group_declines_on_comment`: a proven keyval
group that cannot segment takes the symmetric block fallback. The user accepted
that correction instead of adding an environment-only exception.

Done: Beamer item overlays (`list_item_overlay_prefix`) — a complete
`<overlay>` prefix and its optional `[label]` are marker syntax and remain glued
to `\item`; ordinary body continuation still hangs from the bare `\item `
column. The current Beamer manual settled the spelling after the first proposal
treated the overlay as body text: its grammar and every example use
`\item<2->`, so latexindent preserving that glue was meaningful evidence, not
merely no opinion about intra-line whitespace.

Done: matched arguments of curated inline prose commands
(`inline_command_argument_glue`) — in ordinary prose and prose-argument reflow,
collapsible trivia before every matched slot is removed, so spaces and authored
newlines both canonicalize to a glued argument chain; code-like, preserve-mode,
and virtual `.dtx` margin streams keep their argument-boundary trivia so an
inner rewrite cannot make an opaque parent newly flat on pass two and a margin
rewrite cannot expose a later fixed-point defect. A trailing comment remains a hard
barrier because it consumes the following line end. The rule closes the gluing
half of the prose-argument TODO while leaving signature-table widening open.
Default latexindent preserved the authored breaks (explained divergence: it
does not remove breaks), had no opinion on an inline space, and corroborated the
comment barrier.

Done: display math amid prose (`display_math_prose_boundaries`) — a nonempty
display-math block closes its line before following content under ordinary and
proven-prose reflow; a trailing comment still rides the closer. Opaque argument
paths deliberately preserve a glued suffix because inserting whitespace there
could change the argument's token sequence. Default latexindent preserved the
inline prose spellings (explained divergence: it does not add breaks),
corroborated the already-expanded prose and opaque-argument shapes, and
preserved the glued opaque suffix without offering an opinion on whether to
break it.

Skip constructs whose corpus family is currently a known failure until the
underlying bug lands (`oneSentencePerLine` and `commands/figureValign` both wait
on Formatter entries in TODO.md) — authoring an `expected.tex` against a
formatter that corrupts the input wastes the user's review.

**`ifelsefi` (402 files) is surveyed and parser-blocked** — the `CONDITIONAL`
node entry under *Parser* in TODO.md carries the full survey. Do not re-derive a
formatter-only rule for it: a per-boundary divider rule half-breaks the
construct (one divider breaks, its glued sibling does not), breaking glued ones
too manufactures a space token TeX contributes to the horizontal list, and
firing only where the author already broke is the trivia read. The coherent
all-or-nothing form needs the construct's extent, which is a parser scan. It is
also the standing example of the more general lesson: when every available rule
for a construct is arbitrary, the construct has no node, and the output of the
session is the recorded blocker rather than a fixture.

## Report-back

1. Construct landed, and the canonical rule in one sentence.
2. Fixture slug(s) added and which table each was registered in.
3. **latexindent check:** the verdict per shape (corroborates / explained
   divergence / no opinion / unexplained divergence), and what any unexplained
   divergence turned out to be. Never omit this section — "did not check" is
   itself the thing to report.
4. Gate: `cargo test --workspace` green; any `gate-corpora` baseline movement.
5. Any parser/semantic blocker surfaced and where it was recorded (TODO.md
   section). "None" if clean.
6. Ranked next target.
