---
name: add-code-action
description: Add or change a Badness language-server code action, editor quick fix, or syntax-aware refactoring. Use when asked to expose a linter fix in editors; add a `textDocument/codeAction` for LaTeX or BibTeX; implement a standalone refactor such as changing table structure; add another action kind; or modify code-action request, filtering, workspace-edit, or test plumbing.
---

# Add a Badness code action

Keep code-action behavior pure and atomic. Classify the action before editing:

1. **Diagnostic quick fix:** attach a `Fix` to the linter diagnostic. The shared
   LSP path exposes it automatically; do not add rule-specific LSP logic.
2. **Standalone refactoring:** build a syntax-aware `CodeAction` in
   `src/lsp/code_action.rs` and compose it in `run_code_action`.
3. **Command rather than action:** keep an interaction that needs parameters or
   a follow-up request on the existing `workspace/executeCommand` path. Do not
   disguise it as a fully built code action.

## Contracts

- Use `CodeActionKind::QUICKFIX` only for diagnostic fixes. Use
  `REFACTOR_REWRITE` for a deliberate structural rewrite; choose another
  standard LSP kind only when its semantics fit.
- Honor `CodeActionContext.only`. A requested parent kind such as `refactor`
  admits `refactor.rewrite`; an unrelated kind must exclude the action. Keep
  filtering centralized through `code_action_kind_requested`, and avoid
  expensive action-specific work early when the requested kinds rule it out.
- Emit a complete `WorkspaceEdit`, or emit no action. Decline malformed,
  unresolved, redefined, or ambiguous shapes rather than applying a partial
  transformation.
- When several diagnostics can carry the same edit, prove fix-all cannot apply
  a zero-width insertion once per finding. Emit one owning diagnostic, or use
  edits whose conflict behavior prevents duplication, and test the fixpoint.
- Construct ranges from CST `TextRange`s and convert them with the shared
  `byte_range_to_lsp`/`LineIndex` helpers. Never count source bytes by hand.
- A code action decides what to rewrite, not final layout. Do not run the
  formatter inside the action or rely on later formatting for safety.
- Parse production buffers with the file's exact `FileKind::lex_config()` and
  project `ResolvedDeclarations`. Do not use default parsing in production.
  Unit tests may use default parsing for ordinary `.tex` fixtures.
- Preserve parser and formatter boundaries. Fix a missing or incorrect CST
  shape in the parser rather than scanning around it in the action.
- Treat LSP action titles, kinds, edits, and applicability as user-facing
  behavior. Keep titles imperative and specific.

## Key files

- `src/lsp/code_action.rs`—pure builders. `code_actions_for_range` converts
  fix-carrying diagnostics into quick fixes; standalone syntax-aware builders
  live beside it with focused unit tests.
- `src/lsp.rs`—server capability, `on_code_action`, `WorkerJob::CodeAction`,
  worker dispatch, and `run_code_action`. Thread new request context through
  these sites only when the pure builder genuinely needs it.
- `src/linter/diagnostic.rs`—`Diagnostic`, `Fix`, `Edit`, and
  `Applicability`. A `Safe` fix is preferred; an `Unsafe` fix is still offered
  explicitly in editors but is not applied by ordinary `lint --fix`.
- `src/linter/rules/`—diagnostic-specific fixes. Use the `add-lint-rule` skill
  as well when adding a new rule rather than merely adding a fix to one. When
  an existing rule gains or loses fixes, keep `Rule::emits_fix()` in sync;
  fix-loop scheduling depends on it.
- `tests/lsp.rs`—in-process LSP transcript tests. Use
  `lsp_code_action_quickfix` and `lsp_code_action_adds_a_table_column` as the two
  route examples.
- `docs/src/guide/editor-setup.md`—document standalone user-facing editor
  actions. Linter-rule fixes belong primarily in the generated rule reference.

## Workflow

1. **State the transformation and its refusal gates.** Identify the cursor or
   diagnostic target, every edited site, the action kind, and shapes where
   author intent or syntax cannot be proven. Decide whether a structural action
   operates at the request's start position or over the whole selected range.

2. **Choose the route.**
   - For one unambiguous correction to a diagnostic, add a `Fix`. Use `Safe`
     only when meaning is preserved; use `Unsafe` when the explicit rewrite may
     change output. Withhold a fix when several corrections are equally
     plausible unless the requested action intentionally names one choice.
     Exercise the edit through `apply_fixes`, including multiple findings that
     target the same construct.
   - `Diagnostic` currently carries one `Fix`. If the feature needs several
     alternative actions, do not squeeze them into that field; add a standalone
     provider or deliberately extend the diagnostic model.
   - For a refactoring available without a diagnostic, use a standalone pure
     builder.

3. **Write the failing pure test first.**
   - Quick fix: test the rule's `Fix`, then verify
     `code_actions_for_range` only when shared conversion behavior changes.
   - Standalone action: test the builder in `src/lsp/code_action.rs`. Apply its
     returned edits to the fixture and assert the entire result, the title, and
     the kind.
   - Cover at least one positive case, cursor outside the target, malformed or
     ambiguous input, user redefinition/declaration behavior when relevant,
     and every structural feature the action claims to support. Add Unicode or
     CRLF coverage when position conversion is part of the change.

4. **Implement the pure transformation.** Prefer typed AST wrappers. Find the
   smallest enclosing target when constructs may nest. Gather every edit before
   constructing the action; return no action if any required target fails a
   gate. Keep edits disjoint and keyed by the correct URI. Resolve every foreign
   file before emitting a cross-file edit. When local CST gates cannot prove the
   rewritten shape, speculatively parse the candidate with the same lex config
   and declarations and require the intended localized structure.

5. **Wire standalone actions once.** Reuse the parsed tree and request context
   in `run_code_action`; do not parse separately for each provider. The server
   already advertises fully built code actions with `Simple(true)`, so most new
   actions do not change capabilities. If new request data is required, update
   `on_code_action` → `WorkerJob::CodeAction` → worker dispatch →
   `run_code_action` in lockstep.

6. **Add an end-to-end transcript test.** In `tests/lsp.rs`, open a live buffer,
   request `textDocument/codeAction`, deserialize `Vec<CodeActionOrCommand>`,
   and assert the action's kind and exact `WorkspaceEdit`. For a new action kind
   or filtering path, also request an incompatible `context.only` and assert the
   action is absent.

7. **Update user-facing documentation.** Document a standalone refactoring in
   the editor guide, including its scope and refusal gates. For a linter fix,
   update the rule description/example and regenerate the rules reference with
   `task docs:rules`; never hand-edit the generated page.

8. **Validate.** Run focused unit and LSP transcript tests while developing,
   then `cargo fmt --all` and `task check`. If the concurrent scaling test alone
   misses its timing bound, rerun `cargo test -p badness --test scaling` in
   isolation and report both results rather than hiding the first failure.

## Report back

Report the action title and kind, whether it is diagnostic-backed or standalone,
the supported and declined shapes, the edited files, and the focused and full
checks run.
