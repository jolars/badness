# Badness hotspot leads

Read after locating the hot phase. Historical percentages in
`benches/README.md` describe an earlier run, not the current ranking.

| Measured cost | Candidate explanation and next check |
| --- | --- |
| Rowan cursor traversal | Repeated child scans or materialization in lowering and lint dispatch. Confirm the same wide node is visited repeatedly before consolidating passes. |
| Allocation or collection growth | Trace to the allocation site. Borrow, remove an intermediate collection, or reuse local capacity when its lifetime fits. A size pre-count adds a scan and can cost more than growth. |
| Lexer scanning or string operations | ASCII delimiter scans may use bytes, but token contents are arbitrary UTF-8. Preserve Unicode whitespace and character boundaries wherever the existing semantics require them. |
| Green-tree construction | Check lexer/events/tree-builder work and token sizes before changing CST shape or interning. Investigate the nested-tree rehashing note in `TODO.md` if cost grows with depth. |
| Formatter lowering or printing | Check repeated IR construction and width measurement. Width depends on current column, gaps, and print mode; memoizing by text alone can invalidate flat-fit proofs. |
| Lint dispatch or rules | Attribute work to a rule or shared index. Keep shared node-shape, whole-file, and streaming dispatch; do not introduce a tree walk per rule. |
| Salsa or text updates | Check unnecessary writes, line-table rebuilds, declaration publication, and invalidation. Preserve paired text/index storage, edit staging, and ordered writes. |
| Growing LSP heap | Distinguish current project data, retained query/history data, and allocator pages using the two memory harnesses. A smaller single-shot parse does not establish bounded retention. |

Do not pool rowan `NodeCache` or `GreenNodeBuilder` across independent parses
as a benchmark shortcut. Retained green nodes change memory behavior, and a
warm cache across iterations does not model fresh CLI processes.

The previous `has_verbatim_body` scan consolidation measured within noise.
Treat it as a hypothesis to revisit only with evidence of repeated expensive
walks. Likewise, replacing `chars().count()` with a byte loop is not inherently
faster; removing repeated counts may be the actual opportunity.

## Source map

- LaTeX parser: `crates/badness-parser/src/parser/`, especially `lexer.rs`,
  `grammar.rs`, `events.rs`, `tree_builder.rs`, and `reparse.rs`.
- LaTeX formatter: `crates/badness-formatter/src/formatter/`, especially
  `core.rs`, `ir.rs`, and `printer.rs`.
- Linter: `src/linter/check.rs`, `rules/`, `fix.rs`, and `render.rs`.
- BibTeX: `crates/badness-parser/src/bib/`,
  `crates/badness-formatter/src/bib/`, and `src/bib/linter/`.
- Incremental analysis and LSP: `src/incremental.rs`, `src/text/`, and
  `src/lsp/`. Follow the measured request to its handler before editing.
