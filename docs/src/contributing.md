# Contributing

Badness is an open-source project. The authoritative guide for working in the
codebase (architecture, tenets, and conventions) lives in
[`CONTRIBUTING.md`](https://github.com/jolars/badness/blob/main/CONTRIBUTING.md)
at the repo root.

## Architecture

Badness follows the rust-analyzer model:

- A hand-written, error-tolerant **lexer and parser** turn LaTeX into a flat
  token stream, then an **event stream** (`Start`/`Tok`/`Finish`), which a tree
  builder re-attaches trivia to and feeds into
  [rowan](https://github.com/rust-analyzer/rowan) to produce a **lossless
  concrete syntax tree**.
- A **semantic layer**: a signature database—assigns meaning (arity,
  verbatim-ness, sectioning) on top of the generic tree. Meaning never leaks
  into the parser.
- The **formatter** lowers the tree into a Wadler-style `Doc` IR, which a
  printer lays out under a flat/break fit model.
- Incremental recomputation is **salsa**-first: green nodes are stored in salsa
  and red cursors are materialized on demand.

## Ground rules

- Keep the syntactic layer free of semantic knowledge.
- New parser features need corpus and snapshot tests *and* a losslessness
  assertion.
- Keep code `rustfmt`-clean; `clippy` warnings are errors.

See
[`CONTRIBUTING.md`](https://github.com/jolars/badness/blob/main/CONTRIBUTING.md)
for more details on the architecture, tenets, and conventions.
