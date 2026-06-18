//! badness — a formatter, linter, and language server for LaTeX, built on a
//! lossless rowan CST. See `AGENTS.md` for the architecture and `TODO.md` for
//! the roadmap.

pub mod ast;
pub mod completion;
pub mod file_discovery;
pub mod formatter;
pub mod incremental;
pub mod linter;
pub mod lsp;
pub mod parser;
pub mod project;
pub mod semantic;
pub mod syntax;
pub mod text;
