---
name: smoke-test-triage
description: Triage and fix badness smoke-test regressions (idempotency,
  losslessness, format-error, timeout) from CI debug-format reports and linked
  issues.
---

Use this skill when asked to investigate failures reported by the smoke-test
scan (`.github/workflows/smoke-test.yml`) or `debug format` CI issues,
especially idempotency and losslessness regressions.

## Goals

1. Reproduce the exact failure from the report.
2. Minimize to a stable local fixture.
3. Add regression coverage in the right test surface.
4. Fix root cause (not symptom).
5. Validate targeted cases, then the full repository checks.

## Triage workflow

1. Read the issue/report details first:
   - failing check type (`idempotency`, `losslessness`, `format-error`,
     `timeout`, or `unknown`)
   - sample file path
   - upstream repo + commit SHA
   - badness commit/version used by the scan
   - report excerpt and the approximate diff start line

2. Reproduce in a local clone of the target repository:
   - checkout the exact target commit from the report
   - run:
     - `badness debug format --checks all --report <sample-file>`
   - if needed, collect pass artifacts with:
     - `badness debug format --checks all --dump-dir <dir> --dump-passes <file>`
   - the scan runs with the target repo's own `badness.toml` when it has one
     (falling back to `--no-config` only when that config is invalid), so
     reproduce under the same config.

3. Minimize:
   - reduce to the smallest snippet that still reproduces
   - keep the source realistic (catcode toggles, `.dtx` guards, verbatim
     bodies, and math-environment edges are common triggers; `.sty`/`.cls`/
     `.dtx` files lex under the `Package` flavor, so keep the extension when
     minimizing package sources)
   - confirm reproduction is deterministic across repeated runs

4. Classify the failure before fixing — **check the CST before any
   formatter-side fix**:
   - **Losslessness failure ⇒ always a parser bug** (tenet: losslessness is
     the parser's job, `reconstruct(text) == text` byte-for-byte). Fix in
     `src/parser/` (or `src/bib/` for `.bib` files), never by compensating in
     the formatter.
   - **Idempotency failure ⇒ find which pass diverges and why.** Use the
     `--dump-dir` artifacts to compare input vs `once` vs `twice`, then
     inspect `badness parse` on the input and on the first-pass output. If
     the CST of the formatted output is *structurally* wrong — mis-attached
     arguments, a group parsed differently after reflow, trivia bound to the
     wrong node — **the bug is parser-side, no matter which pass shows the
     symptom**. Idempotency drift is a downstream symptom of upstream shape
     divergence. The texlab differential gauge (`/parse-compat`,
     `task parse-compat`) is a useful structural reference for suspicious
     parses.
   - **Anti-pattern: fixing in the formatter because the symptom lives
     there.** If you find yourself reaching for a formatter helper to make
     pass1 == pass2 (normalizing whitespace, special-casing a node shape),
     stop and re-check the parse. A formatter fix is only correct when the
     CST is already right and the divergence is purely in rendering. There is
     deliberately no parse-*stability* invariant — the formatter may
     normalize structure on purpose (e.g. `x^{2}` → `x^2`) — but such
     rewrites must be meaning-preserving and reach a fixed point on the
     second pass.
   - **`format-error` ⇒ the formatter refused the input** (parse diagnostics
     or an unsupported construct). Run `badness parse <file>` and read the
     errors: either the parser mis-parses valid LaTeX (fix the grammar or
     recovery; see the recovery anchors in AGENTS.md decision #5) or the file
     is genuinely broken upstream (record it, no badness fix needed).
   - **`timeout` ⇒ a hang or pathological slowness.** Reproduce with the
     `/profile` skill's micro-bench; never-infinite-loop on unexpected input
     is a parser invariant.
   - If uncertain, state the best hypothesis and why before implementing —
     and include the relevant `badness parse` output in the hypothesis.

5. Add regression fixture(s):
   - Parser bugs (losslessness, mis-parse): add a corpus file under
     `tests/corpus/` (LaTeX) or `tests/bib_corpus/` (BibTeX) — the roundtrip
     suites assert losslessness over every corpus file — plus a snapshot
     test in `tests/parser.rs` when the tree shape is the point.
   - Formatter bugs (idempotency, layout): add a snapshot case in
     `tests/format.rs` (insta); the format harness asserts idempotence.
   - When a fixture lands in a new extension under `tests/fixtures/**` or
     `tests/corpus/**`, add the matching `… eol=lf` line to `.gitattributes`
     (the formatter emits LF and Windows CI compares bytes).

6. Fix implementation at root cause:
   - parser lossless/CST bugs → `src/parser/` (or `src/bib/`)
   - formatting/idempotency bugs → `src/formatter/`
   - avoid papering over by changing expected outputs only
   - preserve existing behavior for unrelated fixtures

7. Validate:
   - targeted first:
     - the new test (`cargo test --test format <case>` or
       `cargo test --test parser <case>` / `--test roundtrip`)
     - `badness debug format --checks all --report <fixture-or-sample-file>`
   - then full validation:
     - `cargo test`
     - `cargo clippy --all-targets --all-features -- -D warnings`
     - `cargo fmt`
   - for parser/CST changes, also run `/parse-compat` (or `task parse-compat`)
     and triage any new divergence

## Badness-specific guidance

- The debug command's per-file wrap mode follows the file kind
  (`.sty`/`.cls`/`.dtx`/`.ins` → `preserve`, `.tex` → `reflow`) unless the
  repo's config overrides it — match this when reproducing with a snippet in
  a different extension.
- Protected regions (`verbatim`, `lstlisting`, `\verb`, comments) must never
  be altered; a diff inside one is automatically a bug.
- Prefer one focused regression fixture per bug; do not update unrelated
  golden fixtures.
- expl3 regions are formatter-owned layout (whitespace is
  catcode-insignificant there); idempotency drift inside
  `\ExplSyntaxOn…\ExplSyntaxOff` points at `formatter::core::expl3_regions`.

## Report-back format

When done, report:

1. Whether the issue reproduced (and the exact command).
2. Minimal reproducer summary.
3. Fixture(s) added/updated.
4. Root cause and code path changed.
5. Validation commands run and outcomes.
