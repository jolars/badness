//! `badness format --check`: format each file and report which ones would change,
//! without writing anything.
//!
//! Adapted from arity's `src/formatter/check.rs`, minus the `file_discovery`
//! dependency (badness has none yet): the MVP operates on an explicit list of
//! file paths. Directory-walking discovery is a later Phase 2 item.

use std::fmt;
use std::fs;
use std::path::PathBuf;

use super::{FormatError, FormatStyle, format_with_style};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub checked_files: usize,
    pub changed_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    MissingPaths,
    ReadError { path: PathBuf, source: String },
    FormatError { path: PathBuf, source: FormatError },
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPaths => {
                write!(f, "--check requires at least one input file path")
            }
            Self::ReadError { path, source } => {
                write!(f, "failed to read {}: {source}", path.display())
            }
            Self::FormatError { path, source } => {
                write!(f, "failed to format {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for CheckError {}

pub fn check_paths(paths: &[PathBuf]) -> Result<CheckResult, CheckError> {
    check_paths_with_style(paths, FormatStyle::default())
}

pub fn check_paths_with_style(
    paths: &[PathBuf],
    style: FormatStyle,
) -> Result<CheckResult, CheckError> {
    if paths.is_empty() {
        return Err(CheckError::MissingPaths);
    }

    let checked_files = paths.len();
    let mut changed_files = Vec::new();

    for path in paths {
        let content = fs::read_to_string(path).map_err(|err| CheckError::ReadError {
            path: path.clone(),
            source: err.to_string(),
        })?;

        let formatted =
            format_with_style(&content, style).map_err(|err| CheckError::FormatError {
                path: path.clone(),
                source: err,
            })?;
        if formatted != content {
            changed_files.push(path.clone());
        }
    }

    Ok(CheckResult {
        checked_files,
        changed_files,
    })
}
