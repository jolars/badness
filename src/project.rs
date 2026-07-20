//! Cross-file / project-level analysis.
//!
//! Where the parser and (future) per-file semantic layer are strictly
//! single-file, this module models how files relate: the `\input`/`\include`/
//! `\import` inclusion graph. Extraction is **purely syntactic** — it reads the
//! generic CST with small local helpers and needs no signature DB or semantic
//! model — so it lands ahead of those Phase 3 items (AGENTS.md decision #2:
//! meaning never leaks into the syntactic layer).
//!
//! `include.rs` handles inclusion extraction; `graph.rs` builds the cross-file
//! graph.

// The file is named `auxfile.rs` rather than `aux.rs` because `aux` is a
// reserved device name on Windows and git refuses to check out such a path.
#[path = "project/auxfile.rs"]
pub mod aux;
pub mod citations;
pub mod graph;
pub mod include;
pub mod labels;
pub mod options;
pub mod package;
pub mod texmf;

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
pub use options::{
    PackageOptionFacts, ResolvedPackageOptions, package_option_facts, resolved_package_options,
};
pub use package::{
    OptionArg, PackageEdge, PackageEdgeKey, PackageKind, PackageTarget, collect_package_edge_keys,
    collect_package_edges, dtx_source_of, load_option_args, resolve_load_target,
};
pub use texmf::{TexmfConfig, TexmfIndex, global_index};
