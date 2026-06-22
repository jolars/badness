//! Cross-file / project-level analysis.
//!
//! Where the parser and (future) per-file semantic layer are strictly
//! single-file, this module models how files relate: the `\input`/`\include`/
//! `\import` inclusion graph. Extraction is **purely syntactic** — it reads the
//! generic CST with small local helpers and needs no signature DB or semantic
//! model — so it lands ahead of those Phase 3 items (AGENTS.md decision #2:
//! meaning never leaks into the syntactic layer).
//!
//! This is the LaTeX analog of arity's `project/` module; the file shapes track
//! arity's (`source.rs` ↔ `include.rs`, `scope.rs`/`graph.rs` ↔ `graph.rs`) so a
//! later shared-crate extraction stays a mechanical lift.

pub mod citations;
pub mod graph;
pub mod include;
pub mod labels;
pub mod package;

pub use citations::{CiteFileFacts, ResolvedCitations, document_cite_names, resolved_citations};
pub use graph::{
    FileFacts, IncludeGraph, PackageFileFacts, PackageGraph, Project, ProjectMember,
    ResolvedInclude, ResolvedLoad, UnresolvedInclude, UnresolvedLoad, package_graph, project_graph,
};
pub use include::{
    BibTarget, IncludeEdge, IncludeEdgeKey, IncludeKind, IncludeTarget,
    collect_bib_resource_targets, collect_include_edge_keys, collect_include_edges,
};
pub use labels::{ResolvedLabels, resolved_labels};
pub use package::{
    PackageEdge, PackageEdgeKey, PackageKind, PackageTarget, collect_package_edge_keys,
    collect_package_edges,
};
