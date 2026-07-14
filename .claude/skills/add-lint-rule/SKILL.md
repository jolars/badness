---
name: add-lint-rule
description: Add a new built-in lint rule to the Badness LaTeX linter — implement the Rule trait (id/severity/description/examples + check), register it in the three lockstep lists, add unit + integration tests with a losslessness-safe fix, and regenerate the living-docs rules reference. Use when asked to add a lint (warning/error/info), with or without an auto-fix.
---

Use this skill when asked to add a new built-in **LaTeX** lint rule, whether or
not it ships an auto-fix.

## Scope boundaries

- **LaTeX linter only.** BibTeX has a parallel `BibRule` registry under
  `src/bib/linter/` (its own `all_rules()`/`ALL_BIB_RULE_IDS`); adding a bib rule
  is the same shape but a different set of files. This skill covers the `.tex`
  linter under `src/linter/`.
- **Rules read the CST/semantic model; they never re-parse or scan text.**
  Per AGENTS.md tenet 3, parsing is the parser's job. If a rule needs a fact the
  CST does not expose, surface it through the parser or `src/ast.rs`/the semantic
  model—never paper over a parser gap inside a rule.
- **A fix owes correctness, never layout** (AGENTS.md tenet 1). An autofix is a
  textual edit that must still parse and stay lossless; it never invokes the
  formatter and owes nothing about line width. If a fix can't be correct by
  construction for some shape, withhold it for that shape (still report the
  finding).
- Rules produce `Diagnostic` values only. The CLI, LSP, and `--fix` path consume
  them through shared code—never emit strings, ANSI, or `eprintln!` from a rule.

## Key files

- `src/linter/rules.rs` — the `Rule` trait, the `Example` struct, `RuleContext`,
  and the **three lockstep lists** every new rule touches: the `pub mod`/`pub use`
  block, the `all_rules()` registry `Vec`, and the `ALL_RULE_IDS` id slice. The
  unit test `registry_and_id_list_agree` fails if the registry and id list drift.
- `src/linter/rules/<rule_name>.rs` — one file per rule: the `pub struct
  <Name>` + its `impl Rule` (including `description()` and `examples()`) + a
  `#[cfg(test)] mod tests`.
- `src/linter/check.rs` — the driver (`lint_document`). Does one shared
  `root.descendants_with_tokens()` walk, dispatching to each rule by its
  `interests()`, then runs the whole-file (`check_file`) pass. Rules never walk
  the tree themselves. You rarely edit this.
- `src/linter/diagnostic.rs` — `Diagnostic`, `Severity`, `Fix`, `Applicability`.
  The types a rule constructs.
- `src/ast.rs` — typed CST accessors (`command_name`, `environment_name`,
  `nth_group_text`, `nth_group_inner`, `first_group_range`, `group_command_name`,
  …). Prefer these over hand-walking `children_with_tokens()`.
- `src/semantic.rs` (`SemanticModel`) and `src/project/` (`ResolvedLabels`,
  `ResolvedCitations`) — for whole-file / cross-file rules.
- `tests/lint.rs` — integration tests over the public `lint_document`. Helpers:
  `lint(src)`, `lint_project(&[(path, src)])`, `lint_with_bib(tex, &[(path, bib)])`,
  plus fix helpers `fix_to_fixpoint` and `assert_fix_is_correct`.
- `src/linter/docs.rs` — the **living-docs** renderer. `render_rule_doc(&dyn
  Rule)` renders one rule's section by running the *real* linter on its
  `examples()` (via `demo_diagnostics`, which lints under a synthetic closed,
  rooted single-file project view so even cross-file rules fire);
  `render_reference_page()` assembles the full page (static preamble + every
  rule + config footer); `explain_rule(id)` backs `badness lint --explain <rule>`.
  You do **not** edit this to add an ordinary rule — a rule with a
  `description()`/`examples()` is automatically catalogued *and* explainable.
- `examples/docgen.rs` — the generator. `cargo run --example docgen` writes
  `render_reference_page()` to `docs/src/reference/linter-rules.md`
  (`task docs:rules`, also a dep of `task docs:build`).
- `docs/src/reference/linter-rules.md` — the rule catalogue, **generated** from
  rule metadata. Never hand-edit it; edit `description()`/`examples()` and
  regenerate. `tests/rule_docs.rs` enforces this: `every_rule_is_documented`
  (non-empty `description()` + ≥1 example), `documented_examples_actually_trigger`
  (each example fires its rule), `rule_docs_render` (insta snapshot per rule), and
  `reference_page_is_committed` (the committed page equals a fresh render — fails
  if you forgot to regenerate).
- `docs/src/guide/linting.md` — the prose guide; lists a few example rule ids and
  the `[lint]` config shape. Keep it roughly in sync (illustrative, not gated).
- `src/config.rs` (`LintConfig`) and `src/linter/suppression.rs` — config surface
  (`select`/`ignore`) and `% badness-ignore` directives. You do **not** edit
  these to add a rule; gating is centralized.

## Workflow

1. **Pick the rule id (kebab-case).** This is the diagnostic's `rule`, the
   `[lint]` `select`/`ignore` key, and the `% badness-ignore <id>` target. It is
   user-facing and stable—renaming it later is a breaking config change. Match
   the tone of existing ids (`deprecated-command`, `dollar-display-math`,
   `missing-nonbreaking-space`, `undefined-ref`).

2. **Decide the shape before writing code:**
   - **Node-shape vs whole-file.** If the finding is local to a CST node kind
     (a command, an environment, a delimiter), it's node-shape: declare
     `interests()` and implement `check()`. If it needs the semantic model or
     cross-file resolution (undefined refs, duplicate labels across files), it's
     whole-file: leave `interests()` empty and implement `check_file()`.
   - **Severity.** `Warning` is the default and almost always right. Reserve
     `Error` for genuinely broken output. Override `default_severity()` only to
     change it.
   - **Gating.** Cross-file rules read `ctx.resolution` / `ctx.citations` and
     must **early-return when they are `None`** (no project view → stay silent).
     If the rule is only sound over a complete namespace, also gate on
     `resolution.is_closed(ctx.path)` / `is_root_component(ctx.path)` the way
     `undefined_ref.rs` does. Conservative by default: a false positive is worse
     than a miss.
   - **Auto-fix.** Ship a `Fix` only when the rewrite is unambiguous and correct
     by construction. `Fix::safe(...)` for meaning-preserving edits (applied by
     `lint --fix`); `Fix::unsafe_(...)` when the edit can change output—e.g.
     inserting a tie changes line breaking (`missing-nonbreaking-space`), applied
     only under `--unsafe-fixes` or as an LSP code action. If several resolutions
     are valid (rename vs delete for `duplicate-label`), omit the fix and say why
     in the docs.

3. **Write a failing test first** (TDD, per AGENTS.md). Prefer an inline unit
   test in the new module's `#[cfg(test)] mod tests`. The idiom (copy from
   `deprecated_command.rs` for node-shape, `undefined_ref.rs` for whole-file):
   ```rust
   fn findings(src: &str) -> Vec<Diagnostic> {
       let root = SyntaxNode::new_root(parse(src).green);
       let model = SemanticModel::build(&root);
       let ctx = RuleContext {
           path: std::path::Path::new("x.tex"),
           root: &root,
           model: &model,
           resolution: None,   // Some(&…) for cross-file rules
           citations: None,
       };
       let mut out = Vec::new();
       // node-shape:
       for el in root.descendants_with_tokens() {
           if MyRule.interests().contains(&el.kind()) {
               MyRule.check(&el, &ctx, &mut out);
           }
       }
       // whole-file: MyRule.check_file(&ctx, &mut out);
       out
   }
   ```
   Cover the positive case, the negative ("must not flag") case, and each edge
   the rule explicitly handles. For a fix, assert the `Applicability`, the tight
   `(start, end)` span, and the applied output via
   `crate::linter::fix::apply_fixes(src, std::slice::from_ref(fix), false).output`.

4. **Implement the rule** in `src/linter/rules/<rule_name>.rs`:
   - Open with a module doc-comment explaining *what* it flags and *why*, and—if
     there's a fix—why it's Safe/Unsafe and correct by construction. Existing
     rules set the bar; match it.
   - `impl Rule`: `id()` returns the kebab id; override `default_severity()` only
     if not `Warning`; for node-shape declare
     `fn interests(&self) -> &'static [SyntaxKind] { &[SyntaxKind::COMMAND] }`
     and implement `check`; for whole-file implement `check_file`.
   - **Add `description()` and `examples()`** (both required in practice—the docs
     tests fail without them). `description()` returns a one-paragraph markdown
     `&'static str` (what it flags and why; if it fixes, why the fix is
     safe/unsafe and correct by construction). `examples()` returns a
     `&'static [Example]` const; each `Example { caption, source }` is a snippet
     that **must actually trigger the rule** under the docs renderer's synthetic
     closed, rooted single-file project view (so a cross-file rule's example can
     be a bare `\ref{…}`/`\cite{…}` with no companion files). Copy the
     `const EXAMPLES: &[Example] = &[…]` pattern from an existing rule and add
     `Example` to the `use super::{…}` import.
   - In `check`, unwrap the element: `let Some(node) = el.as_node() else { return };`
     (interests can match tokens too). Read structure via `src/ast.rs` helpers.
   - Build spans from the CST: `usize::from(range.start())`, `usize::from(range.end())`.
     Keep them **tight**—point at the offending construct (the control word, the
     delimiter), not the whole node or line. This drives both the CLI caret and
     LSP underline.
   - Construct diagnostics as a struct literal with `path: PathBuf::new()` (the
     driver stamps the real path):
     ```rust
     sink.push(Diagnostic {
         rule: self.id(),
         severity: self.default_severity(),
         path: PathBuf::new(),
         start, end,
         message: format!("…"),
         fix,   // Option<Fix>
     });
     ```
   - Prefer a fix over a **tight, precise span** (e.g. just the control-word
     token), not a whole-node rewrite that could drop a greedily-attached group.
     A `Fix` carries one or more disjoint `Edit { start..end, content }`
     replacements in the diagnostic's file, applied atomically (all or none) —
     use `Fix::safe`/`Fix::unsafe_` for the common single-edit case and
     `Fix::safe_edits`/`Fix::unsafe_edits` when a rewrite must touch several
     sites (e.g. a `\begin`/`\end` rename). Cross-file edits are not
     expressible; withhold the fix on shapes where you can't isolate the spans.
   - Imports: `use super::{Example, Rule, RuleContext};` and
     `use crate::linter::diagnostic::{Diagnostic, Fix, Severity};` plus
     `use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};` as needed.

5. **Register it** in `src/linter/rules.rs` (three edits, kept in lockstep by
   `registry_and_id_list_agree`):
   - Add `pub mod <rule_name>;` and `pub use <rule_name>::<Name>;` to the module
     block (alphabetical with the rest).
   - Add `Box::new(<Name>),` to the `all_rules()` `Vec`.
   - Add `"<rule-id>",` to `ALL_RULE_IDS`, **in the same position** as the
     registry entry (the test asserts equal order, not just equal sets).
   There is no `if`-guard and no metadata table—`select`/`ignore` filtering is
   applied centrally by `RuleSelection`, so nothing else needs editing.

6. **Regenerate the docs** (do **not** hand-edit `linter-rules.md`—it is
   generated from `description()`/`examples()`):
   - Run `cargo run --example docgen` (or `task docs:rules`). This rewrites
     `docs/src/reference/linter-rules.md` with your rule's section slotted in at
     registry order, its live diagnostic output, and (for a safe fix) the
     after-fix block.
   - Then run `cargo insta test --review` (or `INSTA_UPDATE=always cargo test
     --test rule_docs`) to create/accept the per-rule snapshot in
     `tests/snapshots/rule_docs__<rule_name>.snap`. Review it before accepting.
   - Optionally add the id to the example list in `docs/src/guide/linting.md`.

7. **Add an integration test** in `tests/lint.rs` if the rule is cross-file or
   worth an end-to-end check: use `lint(src)` for single-file, `lint_project(...)`
   for cross-file labels, `lint_with_bib(...)` for citations, and filter findings
   by your rule id. For fixes, `assert_fix_is_correct(...)` enforces the tenet-1
   contract (fixed text still parses and reconstructs losslessly).

8. **Validate** in this order (single-crate package—no `--workspace`):
   - Targeted: `cargo test --lib <rule_name>`, `cargo test --test lint`,
     `cargo test --test rule_docs` (checks the description/examples requirement,
     that each example triggers, the per-rule snapshot, and that the committed
     page matches a fresh render — fails if you forgot `docgen`).
   - CLI smoke check on a scratch fixture:
     `cargo run -q -- lint /tmp/f.tex`, then if it has a Safe fix
     `cargo run -q -- lint --fix /tmp/f.tex` (or `--unsafe-fixes` for an Unsafe
     one) and inspect the file.
   - Full: `cargo test`,
     `cargo clippy --all-targets --all-features -- -D warnings`,
     `cargo fmt` (the rustfmt git hook aborts the commit on unformatted files).

## Dos and don'ts

- **Do** keep spans tight and construct them from `text_range()`, never by
  counting bytes in `input`.
- **Do** make cross-file rules inert without a project view (early-return on
  `None` resolution/citations) and conservative over incomplete namespaces.
- **Do** put rule logic in the rule module; shared structural accessors belong in
  `src/ast.rs`, not duplicated across rules.
- **Do** respect `% badness-ignore` implicitly—the driver filters suppressed
  ranges after the fact, so the rule emits unconditionally.
- **Don't** run the formatter, compute layout, or worry about line width in a
  fix. Fix decides *what* to rewrite; the formatter owns *how it's laid out*.
- **Don't** ship a fix whose result might not parse or might change meaning
  silently. Mark meaning-changing fixes `Unsafe`; withhold ambiguous ones.
- **Don't** paper over a parser bug in a rule. Fix it in the parser (with corpus
  + snapshot tests and a losslessness assertion) per tenet 3.
- **Don't** rename an existing rule id to fix a typo without a migration note—the
  id is user-facing config surface.

## Report-back format

When done, report:

1. Rule id, severity, node-shape vs whole-file, and whether it ships a
   Safe/Unsafe fix (or none).
2. Gating (interests, or which `RuleContext` resolution it requires).
3. New files (rule module, new `tests/snapshots/rule_docs__<name>.snap`) and
   edited files (`rules.rs` three lists, the regenerated `linter-rules.md`, and
   `tests/lint.rs`/`guide/linting.md` if touched).
4. Targeted test names run, including `rule_docs`, that `docgen` was run, and the
   CLI `--fix` smoke-test outcome.
5. Full-suite results (`cargo test`, clippy, fmt).
