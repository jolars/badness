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

/// Whether a [`Fix`] preserves the document's meaning. `Safe` fixes are applied
/// by `lint --fix`; `Unsafe` fixes (those that could change typeset output, e.g.
/// a rewrite that alters spacing) require `--unsafe-fixes` or an explicit editor
/// action. Mirrors arity's `Applicability`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applicability {
    Safe,
    Unsafe,
}

/// A code edit that, if applied, fixes the diagnostic in question. A fix is a
/// single contiguous replacement: substitute `content` for the source bytes in
/// `start..end`. Mirrors arity's `Fix`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    /// Replacement text to substitute in.
    pub content: String,
    /// Byte offset of the start of the replacement.
    pub start: usize,
    /// Byte offset of the end of the replacement (exclusive).
    pub end: usize,
    /// Whether applying the fix preserves meaning.
    pub applicability: Applicability,
    /// Human-readable title (e.g. for an LSP code action).
    pub description: String,
}

impl Fix {
    /// A meaning-preserving fix.
    pub fn safe(
        start: usize,
        end: usize,
        content: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            content: content.into(),
            start,
            end,
            applicability: Applicability::Safe,
            description: description.into(),
        }
    }

    /// A fix that may change meaning; applied only on explicit opt-in.
    pub fn unsafe_(
        start: usize,
        end: usize,
        content: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            content: content.into(),
            start,
            end,
            applicability: Applicability::Unsafe,
            description: description.into(),
        }
    }
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
    /// An autofix for this finding, if one is available. `lint --fix` applies
    /// these; a finding can exist without one.
    pub fix: Option<Fix>,
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
            fix: None,
        }
    }
}
