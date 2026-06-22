//! Static extraction of package/class *load* edges from a file's CST.
//!
//! LaTeX assembles a document's macro vocabulary by loading a class and packages:
//! `\usepackage`/`\RequirePackage` pull in a `.sty`; `\documentclass`/`\LoadClass`/
//! `\LoadClassWithOptions` pull in a `.cls`. This is the load-graph analog of the
//! source-include extraction in [`super::include`] — the same shape, with different
//! commands and default extensions — and it resolves **local** `.sty`/`.cls` only.
//! There is no kpathsea/TEXMF search: a name like `amsmath` simply stays unresolved
//! unless a sibling `amsmath.sty` is itself an analyzed member.
//!
//! As with includes, resolution here is **pure path arithmetic** (`.sty`/`.cls`
//! extension defaulting + `base_dir` joining) and never touches the disk; the
//! resolved-vs-unresolved decision against the analyzed file set happens in
//! [`crate::project::graph::PackageGraph::build`]. Options
//! (`\usepackage[opt]{name}`) are not modeled here — `nth_group_text` reads the
//! `{name}` group and skips the `[opt]` optional, and the load graph never needs
//! them.

use std::path::{Path, PathBuf};

use rowan::TextRange;

use crate::ast::{command_name, nth_group_text};
use crate::syntax::{SyntaxKind, SyntaxNode};

/// Which load command produced an edge. Kept distinct even where resolution is
/// currently identical, so later passes can honor the differences
/// (`\LoadClassWithOptions` forwarding the current class options; `\documentclass`
/// being the unique class root).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum PackageKind {
    UsePackage,
    RequirePackage,
    DocumentClass,
    LoadClass,
    LoadClassWithOptions,
}

impl PackageKind {
    /// The default file extension for this load kind: `.sty` for package loads,
    /// `.cls` for class loads.
    fn extension(self) -> &'static str {
        match self {
            PackageKind::UsePackage | PackageKind::RequirePackage => "sty",
            PackageKind::DocumentClass
            | PackageKind::LoadClass
            | PackageKind::LoadClassWithOptions => "cls",
        }
    }

    /// Whether this kind takes a comma-separated *list* of names
    /// (`\usepackage{a,b}` loads two packages). Class loads take a single name.
    fn is_list(self) -> bool {
        matches!(self, PackageKind::UsePackage | PackageKind::RequirePackage)
    }
}

/// The target file of a load command. Mirrors [`super::include::IncludeTarget`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum PackageTarget {
    /// A statically-resolved path: the literal name with a `.sty`/`.cls` extension
    /// defaulted in and joined onto the loading file's directory when relative.
    Path(PathBuf),
    /// A missing or non-literal argument we cannot resolve without expanding TeX.
    Dynamic,
}

/// A load edge stripped of its byte range — the part the cross-file graph depends
/// on. Carries no positional data, so a body edit that merely shifts a command's
/// offset leaves it unchanged and the package-graph memo holds (the firewall this
/// feeds). It also satisfies `salsa::Update`, which [`PackageEdge`] cannot because
/// of its `TextRange` field. Mirrors [`super::include::IncludeEdgeKey`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct PackageEdgeKey {
    pub kind: PackageKind,
    pub target: PackageTarget,
}

/// A package/class load dependency edge extracted from a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageEdge {
    pub kind: PackageKind,
    pub target: PackageTarget,
    /// Range of the command, for diagnostics. A comma list (`\usepackage{a,b}`)
    /// yields one edge per name, all sharing the single command range.
    pub range: TextRange,
}

impl PackageEdge {
    /// Project this edge onto its range-free [`PackageEdgeKey`].
    pub fn key(&self) -> PackageEdgeKey {
        PackageEdgeKey {
            kind: self.kind,
            target: self.target.clone(),
        }
    }
}

/// Collect package/class load edges in `root`. `base_dir` is the directory of the
/// file being scanned; relative literal targets are resolved against it.
///
/// Like [`super::include::collect_include_edges`] this walks the whole tree — a
/// `\RequirePackage` is valid anywhere in a `.sty`/`.cls`, not just the preamble —
/// so any recognized command in the CST is a candidate edge.
pub fn collect_package_edges(root: &SyntaxNode, base_dir: Option<&Path>) -> Vec<PackageEdge> {
    root.descendants()
        .filter(|node| node.kind() == SyntaxKind::COMMAND)
        .flat_map(|node| package_edges_of(&node, base_dir))
        .collect()
}

/// Like [`collect_package_edges`] but projected onto range-free
/// [`PackageEdgeKey`]s — the form the cross-file graph query consumes.
pub fn collect_package_edge_keys(
    root: &SyntaxNode,
    base_dir: Option<&Path>,
) -> Vec<PackageEdgeKey> {
    root.descendants()
        .filter(|node| node.kind() == SyntaxKind::COMMAND)
        .flat_map(|node| package_edges_of(&node, base_dir))
        .map(|edge| edge.key())
        .collect()
}

/// Build the load edge(s) for a `COMMAND` node, or an empty vector if it is not a
/// recognized load command. Package loads (`\usepackage`/`\RequirePackage`) expand
/// a comma list into one edge per name; class loads produce a single edge. A
/// missing or non-literal name argument yields one [`PackageTarget::Dynamic`] edge.
fn package_edges_of(command: &SyntaxNode, base_dir: Option<&Path>) -> Vec<PackageEdge> {
    let Some(kind) = command_name(command).and_then(|name| package_kind(&name)) else {
        return Vec::new();
    };
    let range = command.text_range();
    let ext = kind.extension();

    let dynamic = || {
        vec![PackageEdge {
            kind,
            target: PackageTarget::Dynamic,
            range,
        }]
    };

    let Some(text) = nth_group_text(command, 0) else {
        return dynamic();
    };

    if kind.is_list() {
        let edges: Vec<PackageEdge> = text
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| PackageEdge {
                kind,
                target: PackageTarget::Path(resolve(PathBuf::from(name), ext, base_dir)),
                range,
            })
            .collect();
        // An all-blank list (`\usepackage{}` / `\usepackage{ , }`) resolves to
        // nothing literal; report it as one dynamic edge, mirroring `\bibliography`.
        if edges.is_empty() { dynamic() } else { edges }
    } else {
        let name = text.trim();
        if name.is_empty() {
            return dynamic();
        }
        vec![PackageEdge {
            kind,
            target: PackageTarget::Path(resolve(PathBuf::from(name), ext, base_dir)),
            range,
        }]
    }
}

/// The recognized load command for a control-word name (sans backslash).
fn package_kind(name: &str) -> Option<PackageKind> {
    Some(match name {
        "usepackage" => PackageKind::UsePackage,
        "RequirePackage" => PackageKind::RequirePackage,
        "documentclass" => PackageKind::DocumentClass,
        "LoadClass" => PackageKind::LoadClass,
        "LoadClassWithOptions" => PackageKind::LoadClassWithOptions,
        _ => return None,
    })
}

/// Resolve one load target: default the `.sty`/`.cls` extension when the name has
/// none, then join onto `base_dir` when the result is relative. Pure path
/// arithmetic, like [`super::include`]'s resolver.
fn resolve(raw: PathBuf, ext: &str, base_dir: Option<&Path>) -> PathBuf {
    let with_ext = if raw.extension().is_none() {
        raw.with_extension(ext)
    } else {
        raw
    };
    match base_dir {
        Some(dir) if with_ext.is_relative() => dir.join(with_ext),
        _ => with_ext,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn edges(src: &str, base_dir: Option<&Path>) -> Vec<PackageEdge> {
        let root = SyntaxNode::new_root(parse(src).green);
        collect_package_edges(&root, base_dir)
    }

    fn targets(src: &str, base_dir: Option<&Path>) -> Vec<PackageTarget> {
        edges(src, base_dir).into_iter().map(|e| e.target).collect()
    }

    #[test]
    fn usepackage_appends_sty_and_resolves_against_base_dir() {
        let base = PathBuf::from("/proj");
        let e = edges("\\usepackage{mypkg}\n", Some(&base));
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].kind, PackageKind::UsePackage);
        assert_eq!(
            e[0].target,
            PackageTarget::Path(PathBuf::from("/proj/mypkg.sty"))
        );
    }

    #[test]
    fn documentclass_appends_cls() {
        let e = edges("\\documentclass{myclass}\n", None);
        assert_eq!(e[0].kind, PackageKind::DocumentClass);
        assert_eq!(
            e[0].target,
            PackageTarget::Path(PathBuf::from("myclass.cls"))
        );
    }

    #[test]
    fn require_package_and_load_class_recognized() {
        let rp = edges("\\RequirePackage{tools}\n", None);
        assert_eq!(rp[0].kind, PackageKind::RequirePackage);
        assert_eq!(
            rp[0].target,
            PackageTarget::Path(PathBuf::from("tools.sty"))
        );

        let lc = edges("\\LoadClass{base}\n", None);
        assert_eq!(lc[0].kind, PackageKind::LoadClass);
        assert_eq!(lc[0].target, PackageTarget::Path(PathBuf::from("base.cls")));

        let lco = edges("\\LoadClassWithOptions{base}\n", None);
        assert_eq!(lco[0].kind, PackageKind::LoadClassWithOptions);
        assert_eq!(
            lco[0].target,
            PackageTarget::Path(PathBuf::from("base.cls"))
        );
    }

    #[test]
    fn usepackage_splits_comma_list_into_one_edge_each() {
        let base = PathBuf::from("/proj");
        let t = targets("\\usepackage{amsmath, amssymb}\n", Some(&base));
        assert_eq!(
            t,
            vec![
                PackageTarget::Path(PathBuf::from("/proj/amsmath.sty")),
                PackageTarget::Path(PathBuf::from("/proj/amssymb.sty")),
            ]
        );
    }

    #[test]
    fn options_are_skipped() {
        // The `[utf8]` optional is not a `GROUP`, so `nth_group_text(_, 0)` reads
        // `{inputenc}`.
        let e = edges("\\usepackage[utf8]{inputenc}\n", None);
        assert_eq!(
            e[0].target,
            PackageTarget::Path(PathBuf::from("inputenc.sty"))
        );
    }

    #[test]
    fn explicit_extension_is_kept() {
        let e = edges("\\usepackage{local.sty}\n", None);
        assert_eq!(e[0].target, PackageTarget::Path(PathBuf::from("local.sty")));
    }

    #[test]
    fn subdirectory_name_resolves() {
        let e = edges("\\usepackage{styles/mypkg}\n", None);
        assert_eq!(
            e[0].target,
            PackageTarget::Path(PathBuf::from("styles/mypkg.sty"))
        );
    }

    #[test]
    fn absolute_target_ignores_base_dir() {
        let base = PathBuf::from("/proj");
        let e = edges("\\documentclass{/abs/myclass}\n", Some(&base));
        assert_eq!(
            e[0].target,
            PackageTarget::Path(PathBuf::from("/abs/myclass.cls"))
        );
    }

    #[test]
    fn missing_argument_is_dynamic() {
        let e = edges("\\usepackage\n", None);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].target, PackageTarget::Dynamic);
    }

    #[test]
    fn nested_macro_argument_is_dynamic() {
        let e = edges("\\usepackage{\\mypkgname}\n", None);
        assert_eq!(e[0].target, PackageTarget::Dynamic);
    }

    #[test]
    fn empty_list_is_dynamic() {
        let e = edges("\\usepackage{}\n", None);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].target, PackageTarget::Dynamic);
    }

    #[test]
    fn non_load_commands_are_ignored() {
        let e = edges("\\input{a}\n\\section{Hi}\n\\includegraphics{logo}\n", None);
        assert!(e.is_empty());
    }

    #[test]
    fn multiple_edges_are_collected_in_source_order() {
        let t = targets("\\documentclass{cls}\n\\usepackage{a,b}\n", None);
        assert_eq!(
            t,
            vec![
                PackageTarget::Path(PathBuf::from("cls.cls")),
                PackageTarget::Path(PathBuf::from("a.sty")),
                PackageTarget::Path(PathBuf::from("b.sty")),
            ]
        );
    }

    #[test]
    fn comma_list_shares_one_range() {
        let e = edges("\\usepackage{a,b}\n", None);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].range, e[1].range);
    }
}
