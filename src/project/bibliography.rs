//! Filesystem resolution for bibliography resources.
//!
//! Static extraction stays in [`super::include`]: it turns literal
//! `\bibliography`/`\addbibresource` arguments into project-local candidate
//! paths without reading the environment or disk. This module is the root-crate
//! boundary that may resolve a missing candidate through BibTeX's search path.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Resolve an extracted bibliography candidate to a readable file.
///
/// A project-local file wins. Otherwise, plain `BIBINPUTS`/`TEXBIB` path
/// elements provide a no-subprocess fallback, and `kpsewhich` handles Kpathsea's
/// full grammar (`//`, brace/default expansion, and `texmf.cnf`). Failures are
/// inert: callers keep the citation namespace open rather than reporting a
/// possibly false `undefined-citation`.
pub fn resolve_bibliography_file(requested: &Path, base_dir: Option<&Path>) -> Option<PathBuf> {
    if requested.is_file() {
        return Some(requested.to_path_buf());
    }

    let query = search_query(requested, base_dir);
    for variable in ["BIBINPUTS", "TEXBIB"] {
        if let Some(value) = std::env::var_os(variable)
            && let Some(path) = resolve_plain_search_path(query, base_dir, &value)
        {
            return Some(path);
        }
    }

    resolve_with_kpsewhich(query, base_dir)
}

/// Recover the literal relative spelling from the project-local candidate that
/// static extraction produced. An absolute spelling outside `base_dir` remains
/// absolute; Kpathsea will reject it if it does not exist.
fn search_query<'a>(requested: &'a Path, base_dir: Option<&Path>) -> &'a Path {
    base_dir
        .and_then(|base| requested.strip_prefix(base).ok())
        .unwrap_or(requested)
}

/// Search only ordinary path elements. Kpathsea-specific elements are left for
/// `kpsewhich`; treating them as ordinary directories can silently choose a
/// different file from the one BibTeX would use.
fn resolve_plain_search_path(
    query: &Path,
    base_dir: Option<&Path>,
    value: &OsStr,
) -> Option<PathBuf> {
    for entry in std::env::split_paths(value) {
        if entry.as_os_str().is_empty() || has_kpathsea_syntax(&entry) {
            continue;
        }
        let root = if entry.is_absolute() {
            entry
        } else {
            base_dir.unwrap_or_else(|| Path::new(".")).join(entry)
        };
        let candidate = root.join(query);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn has_kpathsea_syntax(path: &Path) -> bool {
    let text = path.as_os_str().to_string_lossy();
    // Kpathsea expands only a leading tilde; internal tildes occur in Windows short names.
    text.contains("//")
        || text.contains('{')
        || text.contains('}')
        || text.contains('$')
        || text.starts_with('~')
        || text.starts_with("!!")
}

fn resolve_with_kpsewhich(query: &Path, base_dir: Option<&Path>) -> Option<PathBuf> {
    let mut command = Command::new("kpsewhich");
    command
        .arg("--progname=bibtex")
        .arg("--format=bib")
        .arg("--")
        .arg(query);
    if let Some(base) = base_dir {
        command.current_dir(base);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let found = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if found.is_empty() {
        return None;
    }
    let path = PathBuf::from(found);
    let path = if path.is_absolute() {
        path
    } else {
        base_dir.unwrap_or_else(|| Path::new(".")).join(path)
    };
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_search_path_resolves_relative_to_document_directory() {
        let dir = tempfile::tempdir().unwrap();
        let bib_dir = dir.path().join("bibliographies");
        std::fs::create_dir(&bib_dir).unwrap();
        std::fs::write(bib_dir.join("shared.bib"), "@article{key}\n").unwrap();

        assert_eq!(
            resolve_plain_search_path(
                Path::new("shared.bib"),
                Some(dir.path()),
                OsStr::new("bibliographies"),
            ),
            Some(bib_dir.join("shared.bib"))
        );
    }

    #[test]
    fn plain_search_path_allows_nonleading_tilde() {
        let dir = tempfile::tempdir().unwrap();
        let bib_dir = dir.path().join("RUNNER~1");
        std::fs::create_dir(&bib_dir).unwrap();
        std::fs::write(bib_dir.join("shared.bib"), "@article{key}\n").unwrap();

        assert_eq!(
            resolve_plain_search_path(Path::new("shared.bib"), None, bib_dir.as_os_str()),
            Some(bib_dir.join("shared.bib"))
        );
    }

    #[test]
    fn kpathsea_specific_elements_are_not_guessed() {
        assert!(has_kpathsea_syntax(Path::new("/texmf/bibtex//")));
        assert!(has_kpathsea_syntax(Path::new("$TEXMF/bibtex/bib")));
        assert!(!has_kpathsea_syntax(Path::new("/data/bibliographies")));
    }
}
