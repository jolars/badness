//! `badness format --check`: format each file and report which ones would change,
//! without writing anything.
//!
//! Adapted from arity's `src/formatter/check.rs`: the input paths are resolved to
//! the concrete `.tex` files via [`collect_tex_files`] (explicit files and/or
//! recursively-walked directories) before checking.

use std::fmt;
use std::fs;
use std::path::PathBuf;

use super::{FormatError, FormatStyle, format_with_style};
use crate::file_discovery::{FileDiscoveryError, collect_tex_files};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub checked_files: usize,
    pub changed_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    MissingPaths,
    NoTexFiles,
    NonTexFilePath { path: PathBuf },
    WalkError { path: PathBuf, message: String },
    ReadError { path: PathBuf, source: String },
    FormatError { path: PathBuf, source: FormatError },
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
            Self::NoTexFiles => {
                write!(f, "no .tex files found under the provided input paths")
            }
            Self::NonTexFilePath { path } => {
                write!(
                    f,
                    "input file {} is not a .tex file; --check only supports .tex files",
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
        }
    }
}

impl std::error::Error for CheckError {}

impl From<FileDiscoveryError> for CheckError {
    fn from(value: FileDiscoveryError) -> Self {
        match value {
            FileDiscoveryError::NonTexFilePath { path }
            | FileDiscoveryError::UnsupportedLintFilePath { path } => Self::NonTexFilePath { path },
            FileDiscoveryError::WalkError { path, message } => Self::WalkError { path, message },
        }
    }
}

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

    let files = collect_tex_files(paths)?;
    if files.is_empty() {
        return Err(CheckError::NoTexFiles);
    }

    let checked_files = files.len();
    let mut changed_files = Vec::new();

    for path in files {
        let content = fs::read_to_string(&path).map_err(|err| CheckError::ReadError {
            path: path.clone(),
            source: err.to_string(),
        })?;

        let formatted =
            format_with_style(&content, style).map_err(|err| CheckError::FormatError {
                path: path.clone(),
                source: err,
            })?;
        if formatted != content {
            changed_files.push(path);
        }
    }

    Ok(CheckResult {
        checked_files,
        changed_files,
    })
}
