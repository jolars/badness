---
paths:
  - "crates/badness-parser/**/*.rs"
  - "crates/badness-parser/data/*.json"
  - "src/parser.rs"
  - "src/semantic.rs"
  - "src/semantic/**/*.rs"
  - "src/incremental.rs"
  - "src/syntax.rs"
  - "src/ast.rs"
---

# Parser rules

Narrative rationale lives in `docs/src/development/architecture.md` § *The
parser*. Keep this file operational.

## Hard invariants

- **Lossless always:** `reconstruct(text) == text` byte-for-byte.
- **Tree purity:** parse shape is a function of source text plus explicit
  project declarations only.
- **No parser abort:** recover and continue; make progress on malformed input.
- **Generic degradation:** unresolved shapes become generic nodes, never crash
  and never silent corruption.
- **Incremental reparse is refusal-first:** success must match a full parse's tree
  and errors exactly. Every tier returns through the shared oracle/length check;
  an unproved edit returns `None` and full-parses.

## Purity and semantic admission

- Do not use ambient package scope/CWL/signature DB to direct attachment.
- The only non-text parser input is explicit declarations (`badness.toml`
  through `Declarations` in `ParseCtx`).
- A declaration names a spelling; it must not force impossible pairing.
- Parser-side semantic facts must be curated/declarative and falsifiable by
  source shape (i.e., demotable by a gate when wrong).

## Gates and diagnostics

- Gate mismatches demote to generic syntax; do not emit low-confidence parser
  diagnostics for routine macro patterns.
- A shape gate must mirror the parse path it guards; test both permissive and
  rejecting directions when changing a gate.
- Prefer false negatives over false positives in lexer-mode and gate admission.

## Attachment and grouping

- Greedy grouping is the default where text carries no arity protocol.
- Do not use signature arity to attach parser arguments.
- Keep bracket/optional attachment shape-driven, not meaning-driven.
- expl3 argspec-driven grouping is a sanctioned dialect-specific exception;
  keep fallback to greedy for underivable heads.

## Cross-subsystem contracts

- Keep expl3 toggle name recognition shared with formatter
  (`parser::lexer::expl_toggle`); formatter owns positional layout gating.
- Environment, conditional, and math pairing rules must preserve formatter
  safety and not overpromise closers.
- Environment aliases from self-definition scan and declarations are allowed;
  cross-file/package inference is not.

## Incremental reparse

- Tiers sit on ordinary `parse`/`lex`: leaf or node splices, or fixed-context
  fragment parses with explicit locality proofs. Do not checkpoint lexer state,
  reuse token streams, or restart the grammar at an offset without recording a
  new architecture decision.
- Relex fragments under the base parse's `ParseCtx` and full-file `.dtx` facts.
- The previous-parse side channel is not a salsa input and may not become one.
- Forward-gate effects outside a fragment require an explicit proof; otherwise
  decline. A debug-oracle divergence is repaired by adding a bail.
- The token tier's text-read classification is a hand-maintained soundness
  enumeration backed by a source-scanning test. Adding a grammar text comparison
  means classifying it there, with a positional guard if it can observe a `WORD`.
- Guard text reads where they are reachable, not by spelling alone; otherwise a
  math/expl3-only read can reject ordinary prose and erase the tier's workload.
- Lexer readers count too: keep following-text/one-shot lookahead recognition in
  the shared lexer predicates rather than restating it in a reparse guard.

## Data and maintenance

- Generated parser data in `crates/badness-parser/data/` is regenerated via
  existing sync tasks/scripts, not hand-edited.
- When behavior changes, update:
  1. Parser tests/snapshots and losslessness checks.
  2. This rule file (briefly).
  3. Architecture narrative for rationale.
