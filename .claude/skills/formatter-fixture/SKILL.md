---
name: formatter-fixture
description: >-
  Grow badness's formatter fixture coverage one LaTeX construct at a time,
  seeded from the latexindent gate corpus. You surface representative inputs,
  propose the canonical formatting under the tenets, the user edits or accepts
  `expected.tex`, then you implement the rule and register the fixture. The
  corpus is primarily a coverage map; latexindent's own outputs are a soft
  target only — inspiration where the tenets underdetermine a construct, never
  a form to match. Use for "take the next construct", "add a formatter fixture
  for X", or "mine the latexindent corpus for X".
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

### latexindent's output is a soft target, never a hard one

Know what those files are first. The harness writes results in-tree and asserts
`git diff` is clean, so every committed `*-mod1.tex` / `*-output.tex` is the
output of **one specific YAML settings stack** — read the directory's
`*-test-cases.sh` to see which (`-l=env-all-on,env-mod-lines9` and friends).
latexindent is also an *indenter*: it fixes leading whitespace and largely
preserves author line breaks, where badness reflows and owns layout outright.
So its output is never a target to match, and a divergence from it is not by
itself a defect.

But it does encode a lot of accumulated community taste about what LaTeX
*should* look like, and discarding that wholesale is throwing away real signal.
**Consult it where the tenets underdetermine the answer** — a construct with no
governing rule and no precedent among the existing fixtures — as inspiration for
a canonical form you then justify on our own terms. Three guards keep that from
sliding into reverse-engineering:

- **Form your own proposal first**, then look. Looking first anchors you, and
  the anchor is another tool's config default.
- **Never consult it to settle a question the tenets already decide.** If a rule
  or an existing fixture governs the construct, that is the answer; latexindent
  agreeing or disagreeing changes nothing.
- **Never cite it as the justification.** The fixture and commit must stand on
  the rule. "latexindent does it this way" is not a reason; "this is what the
  reflow rule yields, and it happens to match the convention latexindent
  encodes" is.

If you want to know what a construct *is*, read the input; if you want to know
how badness lays it out, derive it from the tenets and use their output to
sanity-check taste where the tenets are silent.

**Hand-author fixture inputs; never copy corpus files.** latexindent is GPL-3.0
and badness is MIT — the fetch-don't-vendor setup keeps `corpora/` out of the
tree, and a copied fixture would undo that. The median corpus file is ~200
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
3. **Propose `expected.tex`.** Author the canonical form you believe is right
   under the tenets — deterministic, rule-based, input-independent — and explain
   the reasoning. Keep the input minimal and hand-written. *Then*, if no rule or
   existing fixture governs the construct, check latexindent's output for the
   same shape as a taste check (soft target, see above); say so in the proposal
   if it moved your answer, and say which settings stack produced what you
   looked at. Hand it to the user.
4. **The user edits or accepts.**
5. **Push back when warranted.** If the choice is unprincipled, breaks a tenet
   (especially "layout is decided solely by the formatter's rules"), reads a
   forbidden trivia predicate, or conflicts with an existing fixture, name the
   conflict and the affected fixture and resolve it before writing code.
   Diverging from a prior decision is allowed but must be conscious.
6. **Implement the rule**, then **register and lock** it (below).
7. **Guardrails**, then **record the rule** (below), then commit.

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

- **`.claude/rules/formatter.md`** — a bullet whenever the rule is a directive
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

Measured against the 198 existing fixture slugs, which cluster in reflow (32),
math (30), expl3 (29), dtx (25), align (18), and tabular (10). Thin or absent,
each with a large corpus directory behind it:

1. **`items`** (`\item` lists) — one `list_item_continuation_hang` fixture
   against 157 corpus files; issue #82's hang rule deserves more shapes.
2. **`filecontents`** — no coverage; the environment's body is protected, so
   this is mostly a protected-region question.
3. **`unnamed-braces` / `namedGroupingBracesBrackets`** — four `group_*`
   fixtures; bare and named brace groups at statement level.

Done: sectioning / `headings` (`sectioning_starts_own_line`,
`sectioning_blank_line_and_comment`) — a sectioning command is a block-level
statement, breaking before and after from `CommandSig::sectioning`. That landed
the Tier-1 lone-newline fix its TODO entry described; the rest of that family is
still open.

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
3. Gate: `cargo test --workspace` green; any `gate-corpora` baseline movement.
4. Any parser/semantic blocker surfaced and where it was recorded (TODO.md
   section). "None" if clean.
5. Ranked next target.
