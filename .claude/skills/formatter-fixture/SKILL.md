---
name: formatter-fixture
description: >-
  Grow badness's formatter fixture coverage one LaTeX construct at a time,
  seeded from the latexindent gate corpus. You surface representative inputs,
  propose the canonical formatting under the tenets, the user edits or accepts
  `expected.tex`, then you implement the rule and register the fixture. The
  corpus supplies coverage, never answers: latexindent's own outputs are a
  different tool's config-driven behavior and are deliberately never consulted.
  Use for "take the next construct", "add a formatter fixture for X", or
  "mine the latexindent corpus for X".
---

Use this skill to add formatter coverage for a *construct*, not to fix a
reported bug (that is `smoke-test-triage`) and not to triage a corpus sweep
(that is the failure inventory in `tests/gate_baselines/README.md`).

## Why the corpus, and what it is not for

`corpora/latexindent` (pinned in `scripts/fetch_gate_corpora.sh`, fetched by
`task gate-corpora:fetch`) is latexindent.pl's own test suite: ~5.3k small
hand-written files of deliberately adversarial LaTeX. It exists here to answer
**"which constructs occur in the wild, and in what nasty shapes"** — a coverage
map that the expl3-heavy latex3/latex2e/pgf corpora do not provide.

**Never read latexindent's expected output.** Its harness writes results in-tree
and asserts `git diff` is clean, so every committed `*-mod1.tex` /
`*-output.tex` is the output of one specific YAML settings stack from that
tool's config model — and latexindent is an *indenter* that preserves author
line breaks, where badness owns layout outright. Those files answer a different
question. Treating them as targets means reverse-engineering another tool's
config surface into badness's rules, one special case per divergence, which is
exactly the failure mode this skill exists to avoid. If you want to know what a
construct *is*, read the input; if you want to know how badness should lay it
out, derive it from the tenets.

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
   the reasoning. Keep the input minimal and hand-written. Hand it to the user.
4. **The user edits or accepts.**
5. **Push back when warranted.** If the choice is unprincipled, breaks a tenet
   (especially "layout is decided solely by the formatter's rules"), reads a
   forbidden trivia predicate, or conflicts with an existing fixture, name the
   conflict and the affected fixture and resolve it before writing code.
   Diverging from a prior decision is allowed but must be conscious.
6. **Implement the rule**, then **register and lock** it (below).
7. **Guardrails**, then commit.

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
test) fail naming your slug, then restore it. Also add a
`… eol=lf` line to `.gitattributes` if you introduce a new extension under
`crates/*/tests/fixtures/**` (Windows CI compares bytes).

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

## Coverage gaps (ranked starter backlog)

Measured against the 195 existing fixture slugs, which cluster in reflow (32),
math (30), expl3 (29), dtx (25), align (18), and tabular (10). Thin or absent,
each with a large corpus directory behind it:

1. **Sectioning / `headings`** — no fixture slug at all, and there is a *known*
   Tier-1 bug here: `\subsection{X}\nprose` keeps the break while
   `\subsection{X} prose` glues (TODO.md, Formatter). Decide the canonical form
   before authoring, since the fix and the fixture are the same change.
2. **`ifelsefi`** (`\if…\else…\fi` outside expl3) — no coverage; 402 corpus
   files. Note the parser's shape gates interact here.
3. **`items`** (`\item` lists) — one `list_item_continuation_hang` fixture
   against 157 corpus files; issue #82's hang rule deserves more shapes.
4. **`filecontents`** — no coverage; the environment's body is protected, so
   this is mostly a protected-region question.
5. **`unnamed-braces` / `namedGroupingBracesBrackets`** — four `group_*`
   fixtures; bare and named brace groups at statement level.

Skip constructs whose corpus family is currently a known failure until the
underlying bug lands (`oneSentencePerLine` and `commands/figureValign` both wait
on Formatter entries in TODO.md) — authoring an `expected.tex` against a
formatter that corrupts the input wastes the user's review.

## Report-back

1. Construct landed, and the canonical rule in one sentence.
2. Fixture slug(s) added and which table each was registered in.
3. Gate: `cargo test --workspace` green; any `gate-corpora` baseline movement.
4. Any parser/semantic blocker surfaced and where it was recorded (TODO.md
   section). "None" if clean.
5. Ranked next target.
