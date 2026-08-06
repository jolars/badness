//! Compatibility wrapper over the BibTeX pipeline, split across the workspace:
//! parsing and semantics live in `badness-parser`, the formatter in
//! `badness-formatter`, and the CLI-side layers (linter and LSP integration)
//! here.

pub use badness_formatter::bib::*;

pub mod completion;
pub mod document_link;
pub mod linter;
pub mod outline;
