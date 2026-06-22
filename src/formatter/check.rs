//! `badness format --check`: format each file and report which ones would change,
//! without writing anything.
//!
//! Adapted from arity's `src/formatter/check.rs`: the input paths are resolved to
//! the concrete `.tex`/`.bib` files via [`collect_lint_files`] (explicit files
//! and/or recursively-walked directories) before checking, then each file is
//! checked through its own formatter (LaTeX or BibTeX) by [`FileKind`].

use std::fmt;
use std::fs;
use std::path::PathBuf;

use super::{FormatError, FormatStyle, WrapMode, format_file_with_packages};
use crate::file_discovery::{ExcludeFilter, FileDiscoveryError, FileKind, collect_lint_files};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub checked_files: usize,
    pub changed_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    MissingPaths,
    NoFiles,
    UnsupportedFilePath {
        path: PathBuf,
    },
    WalkError {
        path: PathBuf,
        message: String,
    },
    ReadError {
        path: PathBuf,
        source: String,
    },
    FormatError {
        path: PathBuf,
        source: FormatError,
    },
    BibFormatError {
        path: PathBuf,
        source: crate::bib::FormatError,
    },
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPaths => {
                write!(
                    f,
                    "--check requires at least one input path (file or directory)"
                )
            }
            Self::NoFiles => {
                write!(
                    f,
                    "no .tex, .sty, .cls, or .bib files found under the provided input paths"
                )
            }
            Self::UnsupportedFilePath { path } => {
                write!(
                    f,
                    "input file {} is not a .tex, .sty, .cls, or .bib file",
                    path.display()
                )
            }
            Self::WalkError { path, message } => {
                write!(f, "failed while scanning {}: {message}", path.display())
            }
            Self::ReadError { path, source } => {
                write!(f, "failed to read {}: {source}", path.display())
            }
            Self::FormatError { path, source } => {
                write!(f, "failed to format {}: {source}", path.display())
            }
            Self::BibFormatError { path, source } => {
                write!(f, "failed to format {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for CheckError {}

impl From<FileDiscoveryError> for CheckError {
    fn from(value: FileDiscoveryError) -> Self {
        match value {
            FileDiscoveryError::NonTexFilePath { path }
            | FileDiscoveryError::UnsupportedLintFilePath { path } => {
                Self::UnsupportedFilePath { path }
            }
            FileDiscoveryError::WalkError { path, message } => Self::WalkError { path, message },
        }
    }
}

pub fn check_paths(paths: &[PathBuf]) -> Result<CheckResult, CheckError> {
    check_paths_with_style(paths, FormatStyle::default(), None, &ExcludeFilter::none())
}

/// Check `paths` under `style`. `wrap_override` is the global `--wrap` value: when
/// `None`, each file uses its kind's default wrap ([`FileKind::default_wrap`], so
/// `.sty`/`.cls` default to `Preserve`), resolved per file below. `exclude` prunes
/// directory discovery (explicitly-named files are never pruned).
pub fn check_paths_with_style(
    paths: &[PathBuf],
    mut style: FormatStyle,
    wrap_override: Option<WrapMode>,
    exclude: &ExcludeFilter,
) -> Result<CheckResult, CheckError> {
    if paths.is_empty() {
        return Err(CheckError::MissingPaths);
    }

    let files = collect_lint_files(paths, exclude)?;
    if files.is_empty() {
        return Err(CheckError::NoFiles);
    }

    let checked_files = files.len();
    let mut changed_files = Vec::new();

    for (path, kind) in files {
        let content = fs::read_to_string(&path).map_err(|err| CheckError::ReadError {
            path: path.clone(),
            source: err.to_string(),
        })?;

        style.wrap = wrap_override.unwrap_or(kind.default_wrap());
        let formatted = match kind {
            FileKind::Tex | FileKind::Sty | FileKind::Cls | FileKind::Dtx | FileKind::Ins => {
                format_file_with_packages(&content, &path, style, kind.lex_config()).map_err(
                    |err| CheckError::FormatError {
                        path: path.clone(),
                        source: err,
                    },
                )?
            }
            FileKind::Bib => crate::bib::format_with_style(&content, style).map_err(|err| {
                CheckError::BibFormatError {
                    path: path.clone(),
                    source: err,
                }
            })?,
        };
        if formatted != content {
            changed_files.push(path);
        }
    }

    Ok(CheckResult {
        checked_files,
        changed_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn check_flags_unformatted_bib() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refs.bib");
        // Lowercase entry type, no padding: the bib formatter would rewrite this.
        fs::write(&path, "@article{k,title={T}}\n").unwrap();

        let result = check_paths(std::slice::from_ref(&path)).unwrap();
        assert_eq!(result.checked_files, 1);
        assert_eq!(result.changed_files, vec![path]);
    }

    #[test]
    fn check_passes_formatted_bib() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refs.bib");
        // Pre-format so a second pass is a no-op (idempotence).
        let formatted = crate::bib::format("@article{k,title={T}}\n").unwrap();
        fs::write(&path, &formatted).unwrap();

        let result = check_paths(std::slice::from_ref(&path)).unwrap();
        assert!(
            result.changed_files.is_empty(),
            "got: {:?}",
            result.changed_files
        );
    }

    #[test]
    fn check_mixes_tex_and_bib() {
        let dir = tempfile::tempdir().unwrap();
        let bib = dir.path().join("refs.bib");
        let tex = dir.path().join("doc.tex");
        fs::write(&bib, "@misc{k,title={T}}\n").unwrap();
        fs::write(&tex, "\\section{Hi}\n").unwrap();

        let result = check_paths(&[dir.path().to_path_buf()]).unwrap();
        assert_eq!(result.checked_files, 2);
        // Only the unformatted bib should be flagged.
        assert_eq!(result.changed_files, vec![bib]);
    }
}
