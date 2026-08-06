//! Compatibility wrapper over the BibTeX pipeline, split across the workspace:
//! parsing and semantics live in `badness-parser`, and the CLI-side layers
//! (formatter — pending its move to `badness-formatter` — linter, and LSP
//! integration) live here.

pub use badness_parser::bib::*;

pub mod completion;
pub mod document_link;
pub mod formatter;
pub mod linter;
pub mod outline;

pub use formatter::{FormatError, format, format_node, format_with_style};
