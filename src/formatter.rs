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

use crate::parser::{LexConfig, parse_with_flavor};
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
) -> Result<String, FormatError> {
    format_file_with_packages_sentence(content, path, style, config, SentenceOptions::default())
}

/// Like [`format_file_with_packages`] but with explicit [`SentenceOptions`]
/// for the `sentence`/`semantic` wrap modes.
pub fn format_file_with_packages_sentence(
    content: &str,
    path: &std::path::Path,
    style: FormatStyle,
    config: impl Into<LexConfig>,
    sentence: SentenceOptions<'_>,
) -> Result<String, FormatError> {
    let parsed = parse_with_flavor(content, config);
    if !parsed.errors.is_empty() {
        return Err(FormatError::ParseErrors {
            count: parsed.errors.len(),
        });
    }
    let root = parsed.syntax();
    let external = disk_scope_signatures(&root, path);
    format_node_with_signatures_sentence(&root, style, &external, sentence)
}
