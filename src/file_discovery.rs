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
    NonTexFilePath { path: PathBuf },
    WalkError { path: PathBuf, message: String },
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
}
