---
paths:
  - "src/linter/**/*.rs"
  - "src/linter.rs"
  - "src/bib/**/*.rs"
---

# Linter rules

Narrative overview: `docs/src/development/architecture.md` § *The linter*.
Contributor recipe for adding rules: `CONTRIBUTING.md`.

## Dispatch model

- Do not walk the tree independently per rule.
- Plug into the shared driver via:
  - node-shape (`interests()` + `check()`)
  - whole-file (`check_file()`)
  - streaming (`stream()` visitor)
- Reuse shared indexes (`math_regions`, `conditionals`, etc.); extract a shared
  index once two rules need the same derived view.
- Missing project-resolution context is inert (`None`), not automatically wrong.

## Rule declaration contract

- `id` is stable kebab-case and user-facing (suppression target); treat renames
  as breaking changes.
- `emits_fix` must match behavior; fix-loop scheduling depends on it.
- Keep description/examples non-empty and runnable in docs generation.
- Register new rules in all lockstep registries (`rules.rs` module/export/list).

## Fix contract

- A fix chooses *what* to rewrite, not final layout style.
- Preserve parseability and meaning for `Safe` fixes.
- Withhold autofix for shapes where safety cannot be guaranteed; still report.
- Unsafe fixes require explicit unsafe opt-in.
- Fix edits are atomic (including cross-file fix sets).
- `apply_fixes` remains pure over source+fixes+flags (no filesystem side effects).

## Suppression contract

- Use shared directive grammar from `badness_parser::directives`.
- `% badness-lint <verb> [<rule>]` is lint axis; `% badness <verb>` suppresses
  both lint + format over the same region.
- `% badness-format ...` must not suppress lint diagnostics.
- Retired `% badness-ignore` spellings still resolve for compatibility.
- For `.bib`, directive carrier is `@comment{...}` (not `%` comments).

## Registry behavior

- `select` / `ignore` configuration is a post-filter (`RuleSelection`) over a
  shared dispatch model; keep the driver config-agnostic.
