//! The diagnostic model shared by every lint finding.
//!
//! Byte ranges are stored as plain
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
/// action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applicability {
    Safe,
    Unsafe,
}

/// One contiguous replacement inside a [`Fix`]: substitute `content` for the
/// source bytes in `start..end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// Replacement text to substitute in.
    pub content: String,
    /// Byte offset of the start of the replacement.
    pub start: usize,
    /// Byte offset of the end of the replacement (exclusive).
    pub end: usize,
}

impl Edit {
    pub fn new(start: usize, end: usize, content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            start,
            end,
        }
    }
}

/// A code edit that, if applied, fixes the diagnostic in question. A fix is a
/// set of disjoint replacements in the diagnostic's own file, applied
/// **atomically**: `lint --fix` and the LSP code action apply all of a fix's
/// edits or none, so a rename that must touch two sites (e.g. a `\begin`/`\end`
/// pair) can never half-apply. Most fixes carry a single edit. Cross-file fixes
/// are not expressible (see TODO.md); edits always target the diagnostic's file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    /// The replacements, each in the diagnostic's own file. Must be mutually
    /// disjoint (the apply engine drops a fix whose edits overlap each other).
    pub edits: Vec<Edit>,
    /// Whether applying the fix preserves meaning.
    pub applicability: Applicability,
    /// Human-readable title (e.g. for an LSP code action).
    pub description: String,
}

impl Fix {
    /// A meaning-preserving fix with a single contiguous replacement.
    pub fn safe(
        start: usize,
        end: usize,
        content: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self::safe_edits(vec![Edit::new(start, end, content)], description)
    }

    /// A single-replacement fix that may change meaning; applied only on
    /// explicit opt-in.
    pub fn unsafe_(
        start: usize,
        end: usize,
        content: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self::unsafe_edits(vec![Edit::new(start, end, content)], description)
    }

    /// A meaning-preserving fix touching one or more disjoint spans.
    pub fn safe_edits(edits: Vec<Edit>, description: impl Into<String>) -> Self {
        Self {
            edits,
            applicability: Applicability::Safe,
            description: description.into(),
        }
    }

    /// A multi-span fix that may change meaning; applied only on explicit
    /// opt-in.
    pub fn unsafe_edits(edits: Vec<Edit>, description: impl Into<String>) -> Self {
        Self {
            edits,
            applicability: Applicability::Unsafe,
            description: description.into(),
        }
    }
}

/// A secondary location attached to a [`Diagnostic`]: the "see also" span that
/// rust-analyzer models with `DiagnosticRelatedInformation` (e.g. the *first*
/// definition behind a `duplicate-label`). Rendered as an annotate-snippets
/// context annotation in the CLI and as `DiagnosticRelatedInformation` in LSP.
///
/// Unlike the primary [`Diagnostic::path`] (left empty and stamped later), a
/// related location's `path` is the *real* file it lives in, filled at rule
/// time — it may differ from the diagnostic's own file (the cross-file case). A
/// `0..0` range is a deliberate file-level link (the target's exact byte range
/// is unknown), pointing at the file's start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelatedInfo {
    /// File the secondary location lives in.
    pub path: PathBuf,
    /// Start byte offset into that file's text.
    pub start: usize,
    /// End byte offset into that file's text (exclusive). Equal to `start` for a
    /// file-level link.
    pub end: usize,
    /// Human-readable note (e.g. "first definition of `x`").
    pub message: String,
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
    /// Secondary "see also" locations for this finding (possibly in other
    /// files). Usually empty; `duplicate-label` points at the first definition.
    pub related: Vec<RelatedInfo>,
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
            related: Vec::new(),
        }
    }
}
