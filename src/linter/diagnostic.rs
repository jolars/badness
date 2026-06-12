//! The diagnostic model shared by every lint finding.
//!
//! Mirrors arity's `linter/diagnostic.rs`, but byte ranges are stored as plain
//! `usize` offsets (matching the parser's [`SyntaxError`] and the salsa layer's
//! `ParseDiagnosticData`) rather than a rowan `TextRange`.

use std::path::PathBuf;

use crate::parser::SyntaxError;

/// Severity of a diagnostic. Ordered to mirror LSP's `DiagnosticSeverity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

/// A single lint finding, keyed to a byte range in one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Stable identifier for the rule that produced this finding. Parse
    /// diagnostics use `"parse"`.
    pub rule: &'static str,
    pub severity: Severity,
    pub path: PathBuf,
    /// Start byte offset into the file text.
    pub start: usize,
    /// End byte offset into the file text (exclusive).
    pub end: usize,
    pub message: String,
}

impl Diagnostic {
    /// Lift a parser [`SyntaxError`] into a `Diagnostic` for `path`. Parse
    /// errors are always [`Severity::Error`].
    pub fn from_parse(path: PathBuf, error: &SyntaxError) -> Self {
        Self {
            rule: "parse",
            severity: Severity::Error,
            path,
            start: error.start,
            end: error.end,
            message: error.message.clone(),
        }
    }
}
