---
name: formatter-fixture
description: >-
  Add formatter coverage for one LaTeX construct, using the latexindent corpus
  as a coverage map and default latexindent as a taste reference. Use when asked
  to take the next formatter construct, add a formatter fixture for a named
  construct, or mine the latexindent corpus for formatter coverage. Survey the
  shapes, propose canonical expected output for user approval, then implement,
  register, and validate the rule. Do not use for a reported formatter
  regression or broad corpus-failure triage.
---

# Formatter fixture

Use this workflow for one construct at a time. A valid result may be a new
layout rule, a lock-in fixture for behavior that is already correct, or a
recorded blocker when the construct cannot be formatted safely with the current
CST.

Use `smoke-test-triage` for reported regressions. Use the failure-inventory
workflow in `tests/gate_baselines/README.md` for broad corpus sweeps.

## Select the construct

Prefer a user-named target. If the user asks for the next construct without
naming one, read [RECAP.md](RECAP.md), re-survey its current lead against the
repository and corpus, and select one bounded construct. Do not read the recap
for a named target.

Start from a green baseline:

```sh
cargo test --workspace
```

## Use the corpus as evidence

The pinned latexindent corpus lives at `corpora/latexindent` after
`task gate-corpora:fetch`. Its inputs are a coverage map: inspect several files
for distinct structural shapes, including nesting, comments, blank lines, and
unusual arguments.

Hand-author all probes and fixtures. Never copy corpus files—latexindent is
GPL-3.0, while Badness is MIT.

For each representative input:

1. Inspect the CST with `badness parse probe.tex`.
2. Inspect current output with `badness --no-config format < probe.tex`.
   Formatting a path rewrites it in place, so use stdin while surveying.
3. Draft the canonical layout from Badness's formatter rules before consulting
   latexindent.
4. Run the hand-authored probe through default latexindent:

   ```sh
   latexindent - < probe.tex
   ```

Do not pass `-s`; it suppresses the stdout needed for comparison. Do not treat
the corpus's committed `*-mod*.tex` or `*-output.tex` files as default output;
the upstream harness produces them with configured `-m` behavior.

Classify every representative shape:

- **Corroborates:** both formatters choose the same relevant layout.
- **Explained divergence:** a known model difference explains the result—for
  example, Badness reflows where default latexindent preserves authored breaks.
- **No opinion:** latexindent leaves a dimension it does not format unchanged,
  such as intra-line spacing.
- **Unexplained divergence:** neither model explains the difference. Resolve it
  or raise it to the user; do not land the fixture with this verdict open.

The formatter contract outranks the comparison. Latexindent supplies evidence,
not authority.

## Proposal checkpoint

Before editing production code or adding the fixture, show the user:

1. The minimal `input.tex` and exact proposed `expected.tex`.
2. The structural rule that determines that output.
3. The latexindent invocation and verdict for each surveyed shape.
4. Any conflict with an existing fixture, invariant, or parser shape.

Wait for the user to edit or accept the proposal. Push back when the requested
form would require a hard-coded command or environment name, depend on a lone
source newline, move a comment unsafely, or contradict an existing formatter
rule.

## Implement and lock the rule

After approval:

1. Add the smallest general formatter rule needed. Do not compensate for a
   parser mistake in the formatter.
2. Add a hand-authored fixture under
   `crates/badness-formatter/tests/fixtures/formatter/<slug>/`.
3. Read [references/fixture-registration.md](references/fixture-registration.md)
   and register the fixture in the correct table.
4. Add focused unit coverage when the production rule has meaningful branches
   that the end-to-end fixture does not isolate.

If every candidate rule is arbitrary, trivia-dependent, or typeset-unsafe,
stop. Record the parser or semantic blocker in `TODO.md`; do not manufacture a
formatter-only workaround.

## Keep layout rule-based

- Preserve non-trivia bytes, protected regions, and comment binding.
- Require losslessness, idempotence, and trivia convergence; a fixture is not
  permission to weaken an oracle.
- Derive layout from structure, configuration, and permitted preserved-trivia
  predicates.
- Never key layout on a single authored newline versus a space. Blank-line
  presence, comment presence or own-line status, and `.dtx` margin or guard
  structure are permitted. A genuine Tier 2 newline-shape read needs an
  explicit fixed-point argument and tests.
- Prefer an existing structural gate or a small generalization of one. Never
  special-case a spelling merely to match `expected.tex`.

## Validate

Run focused tests while developing, then run:

```sh
task check
```

If production layout changed, also run `task gate-corpora:check`. Re-record a
baseline only for attributable movement, and document it in
`tests/gate_baselines/README.md`. Run `task typeset:check` when changing keyval
signature behavior or optional-argument lowering.

Do not bypass the pre-commit hook.

## Record durable results

- The fixture and tests record ordinary behavior; do not narrate every
  implementation in this file.
- Update `AGENTS.md` only for a durable directive or cross-subsystem boundary
  that future work could violate.
- Update `docs/src/development/architecture.md` only for tour-level rationale or
  an architectural change.
- Close or refine an existing `TODO.md` item in place; add a blocker there when
  it represents real project work.
- If the recap was relevant, update [RECAP.md](RECAP.md) compactly. Keep current
  leads, exclusions, and links there—never add a backlog or completion log to
  this `SKILL.md`.

Commit according to the repository's normal policy after validation passes.

## Report back

Report:

1. The construct and its canonical rule.
2. Fixture slugs and registration tables.
3. Latexindent verdicts, including any intentional divergence.
4. Checks run and gate-baseline movement.
5. Any blocker and where it was recorded.
