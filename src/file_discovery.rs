//! Resolving CLI input paths into the concrete `.tex` files to process.
//!
//! Ported from arity's `src/file_discovery.rs` (an EXTRACTION CANDIDATE),
//! swapping `.R` for `.tex`: explicit file arguments must be `.tex` files, while
//! directories are walked recursively via the `ignore` crate (respecting
//! `.gitignore`) to collect every `.tex` file beneath them. Keep this close to
//! arity's version so the eventual shared-crate extraction stays a mechanical
//! lift.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileDiscoveryError {
    NonTexFilePath {
        path: PathBuf,
    },
    /// An explicit lint input that is neither `.tex` nor `.bib`.
    UnsupportedLintFilePath {
        path: PathBuf,
    },
    WalkError {
        path: PathBuf,
        message: String,
    },
}

/// Which pipeline a lintable file feeds: the LaTeX layer (`.tex`) or the BibTeX
/// layer (`.bib`). `Ord` so a `(PathBuf, FileKind)` list sorts/dedups by path;
/// `Hash` so it can tag a [`crate::project::ProjectMember`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileKind {
    Tex,
    Bib,
}

/// Resolve `paths` (files and/or directories) into a sorted, de-duplicated list
/// of `.tex` files. Explicit file paths must be `.tex` files; directories are
/// walked recursively, keeping only `.tex` files and honoring `.gitignore`.
pub fn collect_tex_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>, FileDiscoveryError> {
    let mut files = Vec::new();

    for path in paths {
        if path.is_file() {
            if !is_tex_file(path) {
                return Err(FileDiscoveryError::NonTexFilePath { path: path.clone() });
            }
            files.push(path.clone());
            continue;
        }

        if path.is_dir() {
            let mut builder = WalkBuilder::new(path);
            builder.standard_filters(true);
            builder.hidden(false);
            for entry in builder.build() {
                match entry {
                    Ok(entry) => {
                        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                            continue;
                        }
                        let entry_path = entry.path().to_path_buf();
                        if is_tex_file(&entry_path) {
                            files.push(entry_path);
                        }
                    }
                    Err(err) => {
                        return Err(FileDiscoveryError::WalkError {
                            path: path.clone(),
                            message: err.to_string(),
                        });
                    }
                }
            }
            continue;
        }

        return Err(FileDiscoveryError::WalkError {
            path: path.clone(),
            message: "path does not exist".to_string(),
        });
    }

    files.sort();
    files.dedup();
    Ok(files)
}

fn is_tex_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("tex"))
}

/// The lint [`FileKind`] of `path` by extension (`.tex`/`.bib`), or `None` for any
/// other file.
fn lint_file_kind(path: &Path) -> Option<FileKind> {
    let ext = path.extension().and_then(|ext| ext.to_str())?;
    if ext.eq_ignore_ascii_case("tex") {
        Some(FileKind::Tex)
    } else if ext.eq_ignore_ascii_case("bib") {
        Some(FileKind::Bib)
    } else {
        None
    }
}

/// The [`FileKind`] of `path` by extension, defaulting to [`FileKind::Tex`] for any
/// non-`.bib` extension (including none). The permissive resolver used where a
/// pipeline must be picked for content that has no real file on disk — the LSP
/// (buffers named only by URI) and the CLI's `--stdin-filepath`. Contrast
/// [`collect_lint_files`], which *rejects* an unsupported explicit path rather than
/// defaulting it.
pub fn file_kind_or_tex(path: &Path) -> FileKind {
    lint_file_kind(path).unwrap_or(FileKind::Tex)
}

/// Resolve `paths` (files and/or directories) into a sorted, de-duplicated list of
/// lintable files tagged by [`FileKind`]. Explicit file paths must be `.tex` or
/// `.bib`; directories are walked recursively, keeping both kinds and honoring
/// `.gitignore`. The lint analog of [`collect_tex_files`] (which stays `.tex`-only
/// for `format`).
pub fn collect_lint_files(
    paths: &[PathBuf],
) -> Result<Vec<(PathBuf, FileKind)>, FileDiscoveryError> {
    let mut files = Vec::new();

    for path in paths {
        if path.is_file() {
            match lint_file_kind(path) {
                Some(kind) => files.push((path.clone(), kind)),
                None => {
                    return Err(FileDiscoveryError::UnsupportedLintFilePath { path: path.clone() });
                }
            }
            continue;
        }

        if path.is_dir() {
            let mut builder = WalkBuilder::new(path);
            builder.standard_filters(true);
            builder.hidden(false);
            for entry in builder.build() {
                match entry {
                    Ok(entry) => {
                        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                            continue;
                        }
                        let entry_path = entry.path().to_path_buf();
                        if let Some(kind) = lint_file_kind(&entry_path) {
                            files.push((entry_path, kind));
                        }
                    }
                    Err(err) => {
                        return Err(FileDiscoveryError::WalkError {
                            path: path.clone(),
                            message: err.to_string(),
                        });
                    }
                }
            }
            continue;
        }

        return Err(FileDiscoveryError::WalkError {
            path: path.clone(),
            message: "path does not exist".to_string(),
        });
    }

    files.sort();
    files.dedup();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn collects_tex_files_recursively_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("b.tex"), "b").unwrap();
        fs::write(root.join("a.tex"), "a").unwrap();
        fs::write(root.join("note.sty"), "x").unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub").join("c.tex"), "c").unwrap();

        let files = collect_tex_files(&[root.to_path_buf()]).unwrap();
        assert_eq!(
            files,
            vec![
                root.join("a.tex"),
                root.join("b.tex"),
                root.join("sub").join("c.tex"),
            ]
        );
    }

    #[test]
    fn explicit_tex_file_is_accepted_uppercase_too() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Doc.TEX");
        fs::write(&path, "x").unwrap();
        let files = collect_tex_files(std::slice::from_ref(&path)).unwrap();
        assert_eq!(files, vec![path]);
    }

    #[test]
    fn explicit_non_tex_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pkg.sty");
        fs::write(&path, "x").unwrap();
        assert_eq!(
            collect_tex_files(std::slice::from_ref(&path)),
            Err(FileDiscoveryError::NonTexFilePath { path })
        );
    }

    #[test]
    fn missing_path_is_a_walk_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope");
        assert!(matches!(
            collect_tex_files(&[path]),
            Err(FileDiscoveryError::WalkError { .. })
        ));
    }

    #[test]
    fn duplicate_paths_are_deduplicated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.tex");
        fs::write(&path, "a").unwrap();
        let files = collect_tex_files(&[path.clone(), path.clone()]).unwrap();
        assert_eq!(files, vec![path]);
    }

    #[test]
    fn collect_lint_files_keeps_both_kinds_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("b.tex"), "b").unwrap();
        fs::write(root.join("a.bib"), "a").unwrap();
        fs::write(root.join("note.sty"), "x").unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub").join("c.bib"), "c").unwrap();

        let files = collect_lint_files(&[root.to_path_buf()]).unwrap();
        assert_eq!(
            files,
            vec![
                (root.join("a.bib"), FileKind::Bib),
                (root.join("b.tex"), FileKind::Tex),
                (root.join("sub").join("c.bib"), FileKind::Bib),
            ]
        );
    }

    #[test]
    fn collect_lint_files_accepts_explicit_bib() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refs.BIB");
        fs::write(&path, "x").unwrap();
        let files = collect_lint_files(std::slice::from_ref(&path)).unwrap();
        assert_eq!(files, vec![(path, FileKind::Bib)]);
    }

    #[test]
    fn collect_lint_files_rejects_unsupported_explicit_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pkg.sty");
        fs::write(&path, "x").unwrap();
        assert_eq!(
            collect_lint_files(std::slice::from_ref(&path)),
            Err(FileDiscoveryError::UnsupportedLintFilePath { path })
        );
    }

    #[test]
    fn file_kind_or_tex_dispatches_by_extension() {
        // The permissive resolver behind the LSP and `--stdin-filepath`: `.bib`
        // (any case) → Bib; `.tex`, an unknown extension, and no extension all
        // default to Tex. It reads only the name, so the file need not exist.
        assert_eq!(file_kind_or_tex(Path::new("refs.bib")), FileKind::Bib);
        assert_eq!(file_kind_or_tex(Path::new("refs.BIB")), FileKind::Bib);
        assert_eq!(file_kind_or_tex(Path::new("doc.tex")), FileKind::Tex);
        assert_eq!(file_kind_or_tex(Path::new("pkg.sty")), FileKind::Tex);
        assert_eq!(file_kind_or_tex(Path::new("buffer")), FileKind::Tex);
    }
}
