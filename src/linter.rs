//! Linter: diagnostics over the lossless CST.
//!
//! This first cut only surfaces **parse diagnostics** (the parser's byte-range
//! error side channel) to the CLI; it owns no rules of its own yet. Lint rules,
//! comment suppression (`% badness-ignore` style), and autofix are the next
//! Phase 5 items (see `TODO.md`). The module is kept close to ravel's
//! `src/linter/` shape so the eventual shared-crate extraction stays a
//! mechanical lift (see `AGENTS.md`). **[copy shape]**

pub mod diagnostic;
pub mod render;

pub use diagnostic::{Diagnostic, Severity};
pub use render::{OutputMode, render_findings};
