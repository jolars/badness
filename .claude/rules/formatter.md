---
paths:
  - "crates/badness-formatter/**/*.rs"
  - "src/formatter.rs"
  - "src/formatter/**/*.rs"
---

# Formatter rules

Narrative rationale lives in `docs/src/development/architecture.md` § *The
formatter*. Keep this file operational.

## Hard invariants

- **Whitespace-only:** formatter edits trivia only, never non-trivia tokens.
- **Idempotent:** `fmt(fmt(x)) == fmt(x)`.
- **Protected regions preserved:** except configured line-ending normalization.
- **Parser bugs are fixed in parser, not patched in formatter.**
- **Content rewrites belong to linter fixes, not format layout.**

## Layout contract

- Formatter is the sole authority on layout policy.
- Layout decisions must be deterministic from content + config + preserved
  trivia predicates.
- Do not add content-meaning shortcuts like hard-coded expansion heuristics.

## Trivia-invariant rule

- Never branch on “single newline vs space” for consumed gaps.
- Allowed preserved trivia predicates: blank-line presence, comment presence/
  own-line status, and `.dtx` margin/guard structure.
- Use normalized gap APIs (`Gap`) for width-driven paths.
- Any place that intentionally reads newline shape is Tier-2 and must keep an
  explicit fixed-point argument and tests.

## Printer and measurement

- `Mode::Flat` is a verified claim, not a preference.
- Measure with correct current column context.
- Keep measurement logic centralized; avoid duplicative walkers with diverging
  behavior.

## Reflow and safety

- Reflow safety is structural and gate-driven, never file-kind default hacks.
- Do not reintroduce file-extension wrap defaults to paper over bugs.
- `.dtx` margin/guard escapes must fall back to preserve path safely.
- `macrocode` framing bytes stay literal where required.

## Statement/expl3 handling

- Structural statement boundaries (e.g., statementBody/TikZ paths) are derived
  from parse structure under Reflow.
- Interior statement wrapping should stay unit-aware and meaning-safe.
- Underivable fallback paths may preserve authored-line behavior but must remain
  idempotent.

## Fix pipeline boundary

- `lint --fix` is fix-first; formatter does not run inside fix application.
- Formatter output quality must not be relied on to make a potentially unsafe
  fix safe.
