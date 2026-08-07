---
paths:
  - "src/linter/**/*.rs"
  - "src/linter.rs"
  - "src/bib/**/*.rs"
---

# Linter rules

Narrative overview: `docs/src/development/architecture.md` § *The linter*.
Contributor recipe for a new rule: `CONTRIBUTING.md`.

## Dispatch

- **No rule walks the tree on its own.** Join the driver's single shared
  traversal one of three ways:
  - **node-shape** — declare `interests()`, implement `check()`
  - **whole-file** — empty `interests()`, implement `check_file()`, for
    semantic-model or cross-file findings
  - **streaming** — return a `StreamVisitor` from `stream()` when the finding
    depends on element sequence (running toggle, previous heading level)
- **Use the shared side indexes**, don't re-derive them per rule: `math_regions`
  for "ignore math" tests, `conditionals` for `\if…\else…\fi` branch paths. Add
  a new index there as soon as a second rule needs the same derived view.
- Cross-file resolution (`resolution`, `citations`, `packages`) is `None` when
  there is no project view. Handle that as *inert*, never as *wrong*.

## Rule declarations

- `id` is stable kebab-case; it is the `% badness-ignore` target and the
  reported `rule`. Renaming one is a breaking change for users.
- **`emits_fix` must match reality** — the `--fix` fixpoint loop only runs
  fix-emitting rules each round. A test checks this.
- A non-empty `description` and at least one triggering `example` are required;
  the docs tests enforce it and examples are linted live when the reference
  renders.
- Register in the three lockstep lists in `rules.rs`: module, re-export,
  `all_rules()`.
- The rules reference pages are generated (`task docs:rules`). Never hand-edit.

## Fixes

- **A fix decides what to rewrite, never how to lay it out.** It owes
  correctness — the result still parses, still lossless — but **not line width**.
- **When a fix can't meet that bar for some shape, make it correct by
  construction or withhold it for that shape, and still report the finding.** A
  raw edit has no formatter spacing to lean on, so a rule may be strictly more
  conservative than a layout pass would be (`redundant-script-braces` withholds
  when a following character would re-glue).
- **The formatter never runs inside `--fix`.** The pipeline is fix-then-format.
- `Safe` preserves meaning and applies under `lint --fix`; `Unsafe` (anything
  that could change typeset output) needs `--unsafe-fixes` or an explicit code
  action.
- **Edits within a fix are atomic**, and atomicity spans files for a cross-file
  fix: every edit lands, or none does.
- `apply_fixes` is a pure function over source, fixes, and the unsafe flag — it
  must never touch the filesystem. It is shared by the CLI and the LSP
  code-action path.

## Registry

Config (`select`/`ignore`) narrows the active set as a **post-filter** through
`RuleSelection`, so the shared driver stays config-unaware and the dispatch
table stays identical across files.
