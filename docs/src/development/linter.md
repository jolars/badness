# Linter

The linter reports diagnostics over the same lossless CST the formatter
consumes, and like the formatter it is a pure function of the input plus shipped
data. Each diagnostic may carry an autofix, which is a textual edit that never
invokes the formatter.

The user-facing catalog of shipped rules lives in the reference section ([Linter
Rules](../reference/linter-rules.md), [BibTeX Linter
Rules](../reference/bib-linter-rules.md)). Those pages are generated from each
rule's own description and examples, so they are never hand-edited.

## The `Rule` trait

Every lint implements `Rule` (`src/linter/rules.rs`), which is `Send + Sync` so
the registry can be shared across the LSP's read pool. A rule declares:

- `id`, the stable kebab-case identifier reported as the diagnostic's `rule` and
  targeted by `% badness-ignore <id>`
- `default_severity`, one of `Error`, `Warning` (the default), `Info`, or `Hint`
- `description`, `examples`, `example_path`, and `example_companions`, the copy
  and worked snippets that generate the rule reference
- `emits_fix`, whether the rule can ever set `Diagnostic::fix`

The docs tests require a non-empty description and at least one triggering
example per shipped rule, and each example is linted live when the reference is
rendered. `emits_fix` must be true for any rule that produces a fix, since the
`--fix` fixpoint loop runs only the fix-emitting rules each round; a test checks
it against reality.

No rule walks the tree on its own. Each participates in the driver's single
shared traversal one of three ways.

Node-shape rules name the `SyntaxKind`s they care about in `interests()` and
implement `check()`. The driver calls `check` once per visited element, node or
token, whose kind they subscribed to.

Whole-file rules leave `interests()` empty and implement `check_file()`, which
runs once after the walk. This suits rules driven by the semantic model or by
cross-file resolution rather than by node shape.

Streaming rules return a `StreamVisitor` from `stream()` when the finding
depends on the sequence of elements, such as a running toggle or the previous
heading's level, which a stateless per-element `check` cannot carry. The driver
builds one visitor per file, feeds it every element in document order, then
calls `finish()`.

## The rule context

Each rule reads a `RuleContext` assembled once per file. Besides the syntax root
and the `SemanticModel`, it carries the cross-file resolution a project view
provides: `resolution` for labels, `citations` for cite keys, and `packages` for
package options. Each is `None` when there is no project view, as with stdin or
an LSP context that has not assembled one, which makes the corresponding
cross-file rules inert rather than wrong.

It also precomputes two read-only side indexes shared by every rule, mirroring
the formatter's expl3 region set as a range set derived purely from the tree.
`math_regions` holds the disjoint byte ranges covered by `MATH` nodes, so the
many rules that must ignore math (`e.g.` inside `$…$` is not sentence
punctuation, a `-` there is not a dash) share one membership test instead of
each climbing the ancestor chain per token. `conditionals` holds the
`\if…\else…\fi` branch path per byte offset, so the duplicate-detection rules
share one tracker instead of each interpreting `\else` and `\fi` themselves.

## The registry

`all_rules()` lists every built-in rule in registry order. `RuleRegistry::build`
turns that list into a `by_kind` dispatch table indexed by the `SyntaxKind`
discriminant, so node dispatch is a slice index, plus an `any_node_rules` flag
that lets the driver skip the walk entirely when only whole-file or streaming
rules are active. The table is identical for every file, so `lint_document`
borrows a cached registry rather than rebuilding it, and the registry is shared
by reference across the CLI's rayon lint phase.

Configuration (`[lint]` `select` and `ignore`, and the matching CLI flags)
narrows the active set through `RuleSelection`, applied as a post-filter so the
shared driver stays config-unaware.

## Autofixes

A `Diagnostic` may carry a `Fix`: one or more `Edit`s applied atomically, so a
paired insertion can never half-apply. Each `Edit` names its target file.
`path: None`, the common case, means the diagnostic's own file, so a single-file
fix never has to know its own path; `path: Some(p)` targets another file. A fix
whose edits reach into other files is cross-file, and atomicity then spans
files: every edit in every file lands, or none does.

A fix decides what to rewrite, never how to lay it out. It owes correctness, so
the result still parses and is still lossless, but not line width. When a fix
cannot meet that bar for some shape, make it correct by construction or withhold
it for that shape while still reporting the finding. The pipeline is
fix-then-format, and the formatter never runs inside `--fix`.

This is where meaning-preserving content rewrites live, mirroring the
[whitespace-only formatter](formatter.md#the-formatter-is-whitespace-only).
Rewriting `$$…$$` → `\[…\]` (`dollar-display-math`) and stripping redundant
braces around a single-token script `x^{2}` → `x^2` (`redundant-script-braces`)
are both `Safe` autofixes here, not layout. Because a fix owes correctness as a
raw edit, with no formatter spacing to lean on, such a rule can be strictly more
conservative than a layout pass would be: `redundant-script-braces` withholds
the strip when a following character would re-glue the argument, so `x^{2}-3`
stays braced because unspaced `x^2-3` re-lexes `2-3` as one token.

Each fix declares an `Applicability`. `Safe` fixes preserve meaning and are
applied by `lint --fix`. `Unsafe` fixes, those that could change typeset output
such as a spacing rewrite, require `--unsafe-fixes` or an explicit editor code
action.

`apply_fixes` (`src/linter/fix.rs`) is a pure function over source, fixes, and
an include-unsafe flag. It never touches files and is shared by the CLI and the
LSP code-action path. It considers fixes in source order, applies each
atomically, and drops any fix that is malformed (no edits, a backwards or
out-of-bounds span, internally overlapping edits) or that overlaps an
already-accepted fix, so the output stays well-formed. Accepted edits are
spliced right to left so earlier byte offsets stay valid. `lint --fix` runs this
to a fixpoint, re-linting between rounds.

A cross-file fix cannot apply against one source, so `apply_fixes` skips it. The
sibling `apply_fixes_multi` is the same engine keyed by path, dropping the whole
fix if any edit in any file is malformed or conflicts. Both apply paths honor
cross-file fixes atomically. The LSP groups a fix's edits into one
`WorkspaceEdit` keyed by URI, resolving each foreign file's text from the
snapshot to compute its ranges, and drops the whole action if a target file is
unavailable. `lint --fix` runs the fast per-file pass first, in parallel, then a
project-level pass that builds cross-file resolution over the whole set,
collects the fixes that reach across files, applies them through
`apply_fixes_multi`, and writes every changed file back. That pass is gated to
multi-file projects, so single-file `--fix` pays nothing.

No built-in rule emits a cross-file fix yet; the cross-file rules are
report-only. The model and both apply paths are in place for when one does.

## Suppression

Findings are suppressed inline with the `% badness-ignore` directive family.
`% badness-ignore <rule>: <reason>` suppresses a rule on the next meaningful
sibling, and `% badness-ignore-file <rule>: <reason>` suppresses it file-wide.

## Adding a rule

New rules follow a fixed shape, which the `add-lint-rule` workflow automates.

1. Implement `Rule` in a new `src/linter/rules/<name>.rs`, choosing node-shape,
   whole-file, or streaming dispatch, with `id`, `default_severity`,
   `description`, and at least one triggering example. Emit a losslessness-safe
   fix where one is warranted, and set `emits_fix` accordingly.
2. Register it in the three lockstep lists in `src/linter/rules.rs`: the module
   declaration, the re-export, and the entry in `all_rules()`.
3. Ship unit tests next to the rule and an integration test, plus a losslessness
   assertion on any fixture.
4. Regenerate the rules reference with `task docs:rules`. Do not edit the
   rendered page by hand.
