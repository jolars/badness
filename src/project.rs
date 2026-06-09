//! Cross-file / project-level analysis.
//!
//! Where the parser and (future) per-file semantic layer are strictly
//! single-file, this module models how files relate: the `\input`/`\include`/
//! `\import` inclusion graph. Extraction is **purely syntactic** — it reads the
//! generic CST with small local helpers and needs no signature DB or semantic
//! model — so it lands ahead of those Phase 3 items (AGENTS.md decision #2:
//! meaning never leaks into the syntactic layer).
//!
//! This is the LaTeX analog of ravel's `project/` module; the file shapes track
//! ravel's (`source.rs` ↔ `include.rs`, `scope.rs`/`graph.rs` ↔ `graph.rs`) so a
//! later shared-crate extraction stays a mechanical lift.

pub mod graph;
pub mod include;

pub use graph::{
    FileFacts, IncludeGraph, Project, ProjectMember, ResolvedInclude, UnresolvedInclude,
    project_graph,
};
pub use include::{
    IncludeEdge, IncludeEdgeKey, IncludeKind, IncludeTarget, collect_include_edge_keys,
    collect_include_edges,
};
