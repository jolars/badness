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
//! [`crate::project::graph::PackageGraph::build`]. When no generated `.sty`/`.cls`
//! is a member, resolution falls back to the package's literate `.dtx` source (see
//! [`dtx_source_of`]); the generated file is preferred when both exist. Options
//! (`\usepackage[opt]{name}`) are not modeled here — `nth_group_text` reads the
//! `{name}` group and skips the `[opt]` optional, and the load graph never needs
//! them.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use rowan::TextRange;
use smol_str::SmolStr;

use crate::ast::{AstNode, Optional, child, command_name, nth_group_text};
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

/// One literal option inside a load command's `[...]`: its whitespace-trimmed
/// text and the tight byte range of exactly that trimmed text (the span a
/// per-option diagnostic underlines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionArg {
    pub text: SmolStr,
    pub range: TextRange,
}

/// The literal options of `command`'s first `[...]`, split on commas. `None`
/// when there is no optional at all, or when it holds non-literal content (any
/// child node — a macro or group makes the whole bracket dynamic, matching
/// [`nth_group_text`]'s posture). Empty segments (`[,]`, `[ ]`) are dropped, so
/// `Some(vec![])` means "an options bracket with nothing usable in it".
///
/// Commas glob into `WORD` tokens under catcode lexing, so splitting happens on
/// the accumulated inner *text*; ranges stay exact because the skipped outer
/// brackets bound a contiguous token run inside the `OPTIONAL` node.
pub fn load_option_args(command: &SyntaxNode) -> Option<Vec<OptionArg>> {
    let optional = child::<Optional>(command)?;
    let mut tokens = Vec::new();
    for element in optional.syntax().children_with_tokens() {
        match element {
            rowan::NodeOrToken::Token(token) => tokens.push(token),
            rowan::NodeOrToken::Node(_) => return None,
        }
    }
    // Strip only the delimiting brackets; an interior bracket (rare, but legal
    // text) stays part of the accumulated text so byte offsets keep lining up.
    let mut tokens: &[_] = &tokens;
    if let Some((first, rest)) = tokens.split_first()
        && first.kind() == SyntaxKind::L_BRACKET
    {
        tokens = rest;
    }
    if let Some((last, rest)) = tokens.split_last()
        && last.kind() == SyntaxKind::R_BRACKET
    {
        tokens = rest;
    }

    let Some(first) = tokens.first() else {
        return Some(Vec::new());
    };
    let base = usize::from(first.text_range().start());
    let mut text = String::new();
    for token in tokens {
        text.push_str(token.text());
    }

    let mut args = Vec::new();
    let mut offset = 0usize;
    for segment in text.split(',') {
        let trimmed = segment.trim();
        if !trimmed.is_empty() {
            let lead = segment.len() - segment.trim_start().len();
            let start = base + offset + lead;
            args.push(OptionArg {
                text: SmolStr::from(trimmed),
                range: TextRange::new(
                    (start as u32).into(),
                    ((start + trimmed.len()) as u32).into(),
                ),
            });
        }
        offset += segment.len() + 1;
    }
    Some(args)
}

/// Resolve one load-target name exactly as the edge extractor does: default the
/// kind's `.sty`/`.cls` extension, then join a relative result onto `base_dir`.
/// The lint layer shares this with [`collect_package_edges`] so the two can
/// never disagree on which member file a load points at.
pub fn resolve_load_target(name: &str, kind: PackageKind, base_dir: Option<&Path>) -> PathBuf {
    resolve(PathBuf::from(name), kind.extension(), base_dir)
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

/// The `.dtx` literate source a `.sty`/`.cls` target would be generated from, used
/// as a resolution fallback when the generated file is absent from the analyzed set.
/// Returns `None` unless `target` ends in a `.sty`/`.cls` extension (an explicit
/// other extension, or an already-`.dtx` target, has no fallback). Pure path
/// arithmetic; membership is decided by the caller.
pub fn dtx_source_of(target: &Path) -> Option<PathBuf> {
    matches!(
        target.extension().and_then(OsStr::to_str),
        Some("sty" | "cls")
    )
    .then(|| target.with_extension("dtx"))
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

    fn option_args(src: &str) -> Option<Vec<OptionArg>> {
        let root = SyntaxNode::new_root(parse(src).green);
        let command = root
            .descendants()
            .find(|n| n.kind() == SyntaxKind::COMMAND)
            .expect("a command");
        load_option_args(&command)
    }

    fn spans(src: &str) -> Vec<(String, usize, usize)> {
        option_args(src)
            .expect("literal options")
            .into_iter()
            .map(|o| {
                (
                    o.text.to_string(),
                    usize::from(o.range.start()),
                    usize::from(o.range.end()),
                )
            })
            .collect()
    }

    #[test]
    fn single_option_has_tight_range() {
        // `\usepackage[draft]{mypkg}`: `draft` spans bytes 12..17.
        let out = spans("\\usepackage[draft]{mypkg}\n");
        assert_eq!(out, vec![("draft".to_string(), 12, 17)]);
    }

    #[test]
    fn glued_comma_list_splits_with_per_segment_ranges() {
        let out = spans("\\usepackage[a,b]{mypkg}\n");
        assert_eq!(
            out,
            vec![("a".to_string(), 12, 13), ("b".to_string(), 14, 15)]
        );
    }

    #[test]
    fn whitespace_and_newlines_trim_and_shrink_ranges() {
        // `[ draft ,\n final ]`: ranges cover the trimmed words only.
        let src = "\\usepackage[ draft ,\n final ]{mypkg}\n";
        let out = spans(src);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, "draft");
        assert_eq!(&src[out[0].1..out[0].2], "draft");
        assert_eq!(out[1].0, "final");
        assert_eq!(&src[out[1].1..out[1].2], "final");
    }

    #[test]
    fn empty_segments_are_dropped() {
        assert_eq!(option_args("\\usepackage[,]{mypkg}\n"), Some(Vec::new()));
        assert_eq!(option_args("\\usepackage[ ]{mypkg}\n"), Some(Vec::new()));
        assert_eq!(option_args("\\usepackage[]{mypkg}\n"), Some(Vec::new()));
    }

    #[test]
    fn dynamic_bracket_content_is_none() {
        assert_eq!(option_args("\\usepackage[\\opt]{mypkg}\n"), None);
        assert_eq!(option_args("\\usepackage[a={b,c}]{mypkg}\n"), None);
    }

    #[test]
    fn no_bracket_is_none() {
        assert_eq!(option_args("\\usepackage{mypkg}\n"), None);
    }

    #[test]
    fn resolve_load_target_matches_edge_resolution() {
        let base = PathBuf::from("/proj");
        assert_eq!(
            resolve_load_target("mypkg", PackageKind::UsePackage, Some(&base)),
            PathBuf::from("/proj/mypkg.sty")
        );
        let e = edges("\\usepackage{mypkg}\n", Some(&base));
        assert_eq!(
            e[0].target,
            PackageTarget::Path(resolve_load_target(
                "mypkg",
                PackageKind::UsePackage,
                Some(&base)
            ))
        );
    }
}
