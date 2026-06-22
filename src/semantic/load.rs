//! Merging a document's *loaded-package* signatures into one scope, db-less.
//!
//! This is the pure, salsa-free counterpart of the
//! [`scope_signatures`](crate::incremental::scope_signatures) query: the CLI
//! formatter has no salsa database, so it walks the package-load graph directly,
//! reading each local `.sty`/`.cls` through a [`PackageSource`] (disk for the CLI,
//! an in-memory map for tests), scanning it with the existing
//! [`scan_definitions`], and folding the results into one [`SignatureDb`].
//!
//! Precedence matches the salsa query: a package's own dependencies are merged
//! before the package (so the package overrides them), later loads override
//! earlier ones, and the document's own definitions are overlaid last so they win
//! over every package. Resolution is **local only** — a [`PackageSource`] that
//! returns `None` (e.g. a TEXMF package like `amsmath` with no sibling file)
//! simply contributes nothing.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::file_discovery::file_kind_or_tex;
use crate::parser::parse_with_flavor;
use crate::project::{PackageTarget, collect_package_edge_keys};
use crate::semantic::{SignatureDb, scan_definitions};
use crate::syntax::SyntaxNode;

/// A provider of parsed local package sources, abstracting *how* a resolved
/// `.sty`/`.cls` path is obtained. Returns the package's parsed CST root plus its
/// own base directory (used to resolve that package's nested loads), or `None`
/// when the path is not a local file we can read.
pub trait PackageSource {
    fn load(&self, path: &Path) -> Option<(SyntaxNode, PathBuf)>;
}

/// Collect the merged signature scope for a document `root`: the scanned
/// definitions of every package transitively loaded via `src`, with the
/// document's own definitions overlaid last. `base_dir` is the document's
/// directory (relative load targets resolve against it). See the module docs for
/// the precedence rules; mirrors [`crate::incremental::scope_signatures`].
pub fn collect_package_signatures(
    root: &SyntaxNode,
    base_dir: Option<&Path>,
    src: &impl PackageSource,
) -> SignatureDb {
    let mut merged = SignatureDb::default();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    collect_loaded(root, base_dir, src, &mut visited, &mut merged);
    // The document's own definitions are applied last, so they win over packages.
    merged.merge_from(&scan_definitions(root));
    merged
}

/// Fold the definitions of the packages loaded by `root` into `merged`, recursing
/// into each package's own loads first (post-order: a package overrides its
/// dependencies).
fn collect_loaded(
    root: &SyntaxNode,
    base_dir: Option<&Path>,
    src: &impl PackageSource,
    visited: &mut HashSet<PathBuf>,
    merged: &mut SignatureDb,
) {
    for edge in collect_package_edge_keys(root, base_dir) {
        let PackageTarget::Path(path) = edge.target else {
            continue;
        };
        if !visited.insert(path.clone()) {
            continue;
        }
        if let Some((pkg_root, pkg_base)) = src.load(&path) {
            collect_loaded(&pkg_root, Some(&pkg_base), src, visited, merged);
            merged.merge_from(&scan_definitions(&pkg_root));
        }
    }
}

/// A [`PackageSource`] that reads local `.sty`/`.cls` files from disk, parsing
/// each under its file-kind flavor (so a `.sty` lexes with `@` as a letter). Used
/// by the CLI formatter, which has no salsa database. A path that does not exist
/// or cannot be read simply yields `None` (local-only resolution).
pub struct DiskPackageSource;

impl PackageSource for DiskPackageSource {
    fn load(&self, path: &Path) -> Option<(SyntaxNode, PathBuf)> {
        let text = std::fs::read_to_string(path).ok()?;
        let parsed = parse_with_flavor(&text, file_kind_or_tex(path).lex_config());
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        Some((parsed.syntax(), base))
    }
}

/// The merged package-signature scope for a document with parsed `root` located at
/// `path`, reading its local `.sty`/`.cls` loads from disk. The CLI's db-less
/// equivalent of [`crate::incremental::scope_signatures`].
pub fn disk_scope_signatures(root: &SyntaxNode, path: &Path) -> SignatureDb {
    collect_package_signatures(root, path.parent(), &DiskPackageSource)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use std::collections::HashMap;

    /// An in-memory [`PackageSource`]: a path → source-text map, parsed on demand.
    struct MapSource {
        files: HashMap<PathBuf, String>,
    }

    impl MapSource {
        fn new(files: &[(&str, &str)]) -> Self {
            Self {
                files: files
                    .iter()
                    .map(|(p, s)| (PathBuf::from(p), s.to_string()))
                    .collect(),
            }
        }
    }

    impl PackageSource for MapSource {
        fn load(&self, path: &Path) -> Option<(SyntaxNode, PathBuf)> {
            let text = self.files.get(path)?;
            let root = SyntaxNode::new_root(parse(text).green);
            let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
            Some((root, base))
        }
    }

    fn scope(doc: &str, base: &str, files: &[(&str, &str)]) -> SignatureDb {
        let root = SyntaxNode::new_root(parse(doc).green);
        collect_package_signatures(&root, Some(Path::new(base)), &MapSource::new(files))
    }

    #[test]
    fn pulls_in_a_local_package_definition() {
        let db = scope(
            "\\usepackage{mypkg}\n\\myfoo{a}{b}\n",
            "/proj",
            &[("/proj/mypkg.sty", "\\newcommand{\\myfoo}[2]{#1#2}\n")],
        );
        let sig = db.command("myfoo").expect("package command in scope");
        assert_eq!(sig.args.len(), 2);
    }

    #[test]
    fn unresolved_package_contributes_nothing() {
        // No sibling `amsmath.sty` in the source map → nothing pulled in.
        let db = scope("\\usepackage{amsmath}\n", "/proj", &[]);
        assert!(db.command("amsmath").is_none());
        assert_eq!(db.command_names().count(), 0);
    }

    #[test]
    fn transitive_load_is_followed() {
        let db = scope(
            "\\usepackage{a}\n",
            "/proj",
            &[
                (
                    "/proj/a.sty",
                    "\\RequirePackage{b}\n\\newcommand{\\fa}{x}\n",
                ),
                ("/proj/b.sty", "\\newcommand{\\fb}[1]{#1}\n"),
            ],
        );
        assert!(db.command("fa").is_some());
        assert!(db.command("fb").is_some());
    }

    #[test]
    fn document_definition_wins_over_package() {
        let db = scope(
            "\\usepackage{mypkg}\n\\newcommand{\\dup}[2]{#1#2}\n",
            "/proj",
            &[("/proj/mypkg.sty", "\\newcommand{\\dup}[1]{#1}\n")],
        );
        // The document's 2-arg \dup overrides the package's 1-arg one.
        assert_eq!(db.command("dup").unwrap().args.len(), 2);
    }

    #[test]
    fn load_cycle_terminates() {
        // a requires b, b requires a — the visited set breaks the cycle.
        let db = scope(
            "\\usepackage{a}\n",
            "/proj",
            &[
                (
                    "/proj/a.sty",
                    "\\RequirePackage{b}\n\\newcommand{\\fa}{x}\n",
                ),
                (
                    "/proj/b.sty",
                    "\\RequirePackage{a}\n\\newcommand{\\fb}{y}\n",
                ),
            ],
        );
        assert!(db.command("fa").is_some());
        assert!(db.command("fb").is_some());
    }
}
