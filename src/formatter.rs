//! CLI/LSP bridge over the [`badness_formatter`] engine.
//!
//! The formatting engine lives in the `badness-formatter` crate; this module
//! re-exports it and hosts the CLI-side concerns that do not belong in the
//! published engine: the batch path-walking check API ([`check`]) and the
//! disk-backed package-aware entries ([`format_file_with_packages`]), which
//! pull local `.sty`/`.cls` signatures in via [`crate::semantic::load`].

pub mod check;

pub use badness_formatter::formatter::*;

pub use check::{ChangedFile, CheckError, CheckResult, check_paths, check_paths_with_style};

use crate::declarations::ResolvedDeclarations;
use crate::parser::{LexConfig, parse_with_declarations};
use crate::semantic::disk_scope_signatures;

/// Format an on-disk file's `content` (located at `path`, parsed under `config`),
/// pulling the signatures of its local loaded packages in from disk so calls to
/// package-defined macros are shaped correctly. The shared CLI entry for both
/// `format` and `format --check` — using one entry keeps the two consistent, so a
/// formatted file checks clean. `path`'s directory anchors local `.sty`/`.cls`
/// resolution; stdin (no path) uses [`format_with_style_flavored`] instead.
pub fn format_file_with_packages(
    content: &str,
    path: &std::path::Path,
    style: FormatStyle,
    config: impl Into<LexConfig>,
    declared: &ResolvedDeclarations,
) -> Result<String, FormatError> {
    format_file_with_packages_sentence(
        content,
        path,
        style,
        config,
        SentenceOptions::default(),
        declared,
    )
}

/// Like [`format_file_with_packages`] but with explicit [`SentenceOptions`]
/// for the `sentence`/`semantic` wrap modes.
pub fn format_file_with_packages_sentence(
    content: &str,
    path: &std::path::Path,
    style: FormatStyle,
    config: impl Into<LexConfig>,
    sentence: SentenceOptions<'_>,
    declared: &ResolvedDeclarations,
) -> Result<String, FormatError> {
    let parsed = parse_with_declarations(content, config, declared);
    if !parsed.errors.is_empty() {
        return Err(FormatError::ParseErrors {
            count: parsed.errors.len(),
        });
    }
    let root = parsed.syntax();
    let external = disk_scope_signatures(&root, path, declared);
    format_node_with_signatures_sentence(&root, style, &external, sentence)
}

/// Format `content` read from **stdin**, which has no path to anchor local
/// `.sty`/`.cls` resolution against, so no package scope is folded in.
///
/// This exists rather than calling the engine's
/// [`format_with_style_flavored_sentence`] because that entry parses with
/// `parse_with_flavor` and so cannot see the project's declarations — and a
/// formatter that honors `[environments.…]` for `badness format file.tex` but
/// not for `badness format < file.tex` would be a trap. The declarations reach
/// both the parse and the signature scope here, exactly as they do above.
pub fn format_stdin_sentence(
    content: &str,
    style: FormatStyle,
    config: impl Into<LexConfig>,
    sentence: SentenceOptions<'_>,
    declared: &ResolvedDeclarations,
) -> Result<String, FormatError> {
    let parsed = parse_with_declarations(content, config, declared);
    if !parsed.errors.is_empty() {
        return Err(FormatError::ParseErrors {
            count: parsed.errors.len(),
        });
    }
    let mut scope = crate::semantic::SignatureDb::default();
    scope.merge_declarations(declared);
    format_node_with_signatures_sentence(&parsed.syntax(), style, &scope, sentence)
}
