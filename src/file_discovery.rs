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

use crate::formatter::WrapMode;
use crate::parser::LatexFlavor;

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

/// Which pipeline a lintable file feeds: the LaTeX layer (`.tex`, plus the
/// package/class sources `.sty`/`.cls`) or the BibTeX layer (`.bib`). `Ord` so a
/// `(PathBuf, FileKind)` list sorts/dedups by path; `Hash` so it can tag a
/// [`crate::project::ProjectMember`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileKind {
    /// A `.tex` document.
    Tex,
    /// A `.sty` package source.
    Sty,
    /// A `.cls` class source.
    Cls,
    /// A `.bib` bibliography database.
    Bib,
}

impl FileKind {
    /// Whether this kind feeds the LaTeX pipeline (`.tex`/`.sty`/`.cls`), as
    /// opposed to the BibTeX one. The LaTeX kinds share a parser, formatter, and
    /// linter, differing only in [`latex_flavor`](Self::latex_flavor) and
    /// [`default_wrap`](Self::default_wrap).
    pub fn is_latex(self) -> bool {
        matches!(self, FileKind::Tex | FileKind::Sty | FileKind::Cls)
    }

    /// The [`LatexFlavor`] to parse this kind with: `.sty`/`.cls` are loaded under
    /// an implicit `\makeatletter` ([`LatexFlavor::Package`]); everything else is a
    /// plain [`LatexFlavor::Document`].
    pub fn latex_flavor(self) -> LatexFlavor {
        match self {
            FileKind::Sty | FileKind::Cls => LatexFlavor::Package,
            _ => LatexFlavor::Document,
        }
    }

    /// The default paragraph [`WrapMode`] for this kind when the caller gives no
    /// explicit override: a package/class body is code, not prose, so it defaults
    /// to [`WrapMode::Preserve`]; a document reflows ([`WrapMode::Reflow`]).
    pub fn default_wrap(self) -> WrapMode {
        match self {
            FileKind::Sty | FileKind::Cls => WrapMode::Preserve,
            _ => WrapMode::Reflow,
        }
    }
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
    } else if ext.eq_ignore_ascii_case("sty") {
        Some(FileKind::Sty)
    } else if ext.eq_ignore_ascii_case("cls") {
        Some(FileKind::Cls)
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
    fn collect_lint_files_keeps_all_kinds_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("b.tex"), "b").unwrap();
        fs::write(root.join("a.bib"), "a").unwrap();
        fs::write(root.join("note.sty"), "x").unwrap();
        fs::write(root.join("base.cls"), "x").unwrap();
        fs::write(root.join("readme.md"), "x").unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub").join("c.bib"), "c").unwrap();

        let files = collect_lint_files(&[root.to_path_buf()]).unwrap();
        assert_eq!(
            files,
            vec![
                (root.join("a.bib"), FileKind::Bib),
                (root.join("b.tex"), FileKind::Tex),
                (root.join("base.cls"), FileKind::Cls),
                (root.join("note.sty"), FileKind::Sty),
                (root.join("sub").join("c.bib"), FileKind::Bib),
            ],
            "the `.md` file is ignored; `.sty`/`.cls` are collected as LaTeX kinds"
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
        let path = dir.path().join("readme.md");
        fs::write(&path, "x").unwrap();
        assert_eq!(
            collect_lint_files(std::slice::from_ref(&path)),
            Err(FileDiscoveryError::UnsupportedLintFilePath { path })
        );
    }

    #[test]
    fn file_kind_or_tex_dispatches_by_extension() {
        // The permissive resolver behind the LSP and `--stdin-filepath`: `.bib`
        // (any case) → Bib; `.sty`/`.cls` → their own kinds; `.tex`, an unknown
        // extension, and no extension all default to Tex. It reads only the name,
        // so the file need not exist.
        assert_eq!(file_kind_or_tex(Path::new("refs.bib")), FileKind::Bib);
        assert_eq!(file_kind_or_tex(Path::new("refs.BIB")), FileKind::Bib);
        assert_eq!(file_kind_or_tex(Path::new("doc.tex")), FileKind::Tex);
        assert_eq!(file_kind_or_tex(Path::new("pkg.sty")), FileKind::Sty);
        assert_eq!(file_kind_or_tex(Path::new("Pkg.STY")), FileKind::Sty);
        assert_eq!(file_kind_or_tex(Path::new("base.cls")), FileKind::Cls);
        assert_eq!(file_kind_or_tex(Path::new("Base.CLS")), FileKind::Cls);
        assert_eq!(file_kind_or_tex(Path::new("buffer")), FileKind::Tex);
    }

    #[test]
    fn collect_lint_files_accepts_explicit_package_and_class() {
        let dir = tempfile::tempdir().unwrap();
        let sty = dir.path().join("pkg.sty");
        let cls = dir.path().join("base.cls");
        fs::write(&sty, "x").unwrap();
        fs::write(&cls, "x").unwrap();
        assert_eq!(
            collect_lint_files(std::slice::from_ref(&sty)).unwrap(),
            vec![(sty, FileKind::Sty)]
        );
        assert_eq!(
            collect_lint_files(std::slice::from_ref(&cls)).unwrap(),
            vec![(cls, FileKind::Cls)]
        );
    }

    #[test]
    fn package_and_class_kinds_are_latex_with_preserve_default() {
        // `.sty`/`.cls` feed the LaTeX pipeline under the `Package` flavor and
        // default to code-not-prose wrapping; `.tex` stays a reflowed document.
        for kind in [FileKind::Sty, FileKind::Cls] {
            assert!(kind.is_latex());
            assert_eq!(kind.latex_flavor(), LatexFlavor::Package);
            assert_eq!(kind.default_wrap(), WrapMode::Preserve);
        }
        assert!(FileKind::Tex.is_latex());
        assert_eq!(FileKind::Tex.latex_flavor(), LatexFlavor::Document);
        assert_eq!(FileKind::Tex.default_wrap(), WrapMode::Reflow);
        assert!(!FileKind::Bib.is_latex());
    }
}
