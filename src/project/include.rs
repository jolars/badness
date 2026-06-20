//! Static extraction of file-inclusion edges from a file's CST.
//!
//! LaTeX wires documents together with a small set of inclusion commands —
//! `\input`, `\include`, `\import`/`\subimport`, `\subfile` — whose target is a
//! literal path argument. We model only what is statically knowable: literal
//! brace-group targets. A command whose target is missing or not a flat literal
//! (e.g. built from another macro) becomes [`IncludeTarget::Dynamic`] so the
//! cross-file graph stays conservative.
//!
//! This is the LaTeX analog of arity's `project/source.rs` (`source("file.R")`
//! extraction); keep it close so the eventual shared-crate extraction stays a
//! mechanical lift. Resolution here is **pure path arithmetic** — `.tex`
//! extension defaulting and `base_dir` joining — and never touches the disk; the
//! resolved-vs-unresolved decision against the analyzed file set happens in
//! [`crate::project::graph::IncludeGraph::build`].
//!
//! **Out of scope** (not source includes): `\includegraphics`, `\graphicspath`,
//! `\bibliography`/`\addbibresource`, `\usepackage`/`\RequirePackage`,
//! `\documentclass` — these pull in non-`.tex` assets or packages.
//!
//! **Known limitations** (safe, conservative — both degrade to `Dynamic` or
//! omission): bare plain-TeX `\input foo` (no braces) leaves `foo` as sibling
//! text the greedy argument grammar never attaches, so it is not seen as an
//! edge; `\include`'s main-document-relative base directory and `\includeonly`
//! filtering are deferred (we resolve `\include` like `\input`).

use std::path::{Path, PathBuf};

use rowan::TextRange;

use crate::ast::{command_name, nth_group_text};
use crate::syntax::{SyntaxKind, SyntaxNode};

/// Which inclusion command produced an edge. Kept distinct even where resolution
/// is currently identical, so later passes can honor the semantic differences
/// (`\include`'s `\clearpage` + main-dir base, `\includeonly` gating).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum IncludeKind {
    Input,
    Include,
    Import,
    SubImport,
    SubFile,
}

/// The target file of an inclusion command.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum IncludeTarget {
    /// A statically-resolved path: the literal argument with a `.tex` extension
    /// defaulted in and joined onto the including file's directory when relative.
    Path(PathBuf),
    /// A missing or non-literal argument we cannot resolve without expanding TeX.
    Dynamic,
}

/// An inclusion edge stripped of its byte range — the part the cross-file graph
/// depends on. Carries no positional data, so a body edit that merely shifts a
/// command's offset leaves it unchanged and the project-graph memo holds (the
/// firewall this feeds). It also satisfies `salsa::Update`, which [`IncludeEdge`]
/// cannot because of its `TextRange` field.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct IncludeEdgeKey {
    pub kind: IncludeKind,
    pub target: IncludeTarget,
}

/// A file-inclusion dependency edge extracted from a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludeEdge {
    pub kind: IncludeKind,
    pub target: IncludeTarget,
    /// Range of the command, for diagnostics.
    pub range: TextRange,
}

impl IncludeEdge {
    /// Project this edge onto its range-free [`IncludeEdgeKey`].
    pub fn key(&self) -> IncludeEdgeKey {
        IncludeEdgeKey {
            kind: self.kind,
            target: self.target.clone(),
        }
    }
}

/// Collect inclusion-command edges in `root`. `base_dir` is the directory of the
/// file being scanned; relative literal targets are resolved against it.
///
/// Unlike arity's top-level-only `source()` scan, this walks the whole tree:
/// `\input` is valid anywhere (inside a group, an environment, …), so any
/// recognized command in the CST is a candidate edge.
pub fn collect_include_edges(root: &SyntaxNode, base_dir: Option<&Path>) -> Vec<IncludeEdge> {
    root.descendants()
        .filter(|node| node.kind() == SyntaxKind::COMMAND)
        .filter_map(|node| include_edge(&node, base_dir))
        .collect()
}

/// Like [`collect_include_edges`] but projected onto range-free
/// [`IncludeEdgeKey`]s — the form the cross-file graph query consumes.
pub fn collect_include_edge_keys(
    root: &SyntaxNode,
    base_dir: Option<&Path>,
) -> Vec<IncludeEdgeKey> {
    root.descendants()
        .filter(|node| node.kind() == SyntaxKind::COMMAND)
        .filter_map(|node| include_edge(&node, base_dir))
        .map(|edge| edge.key())
        .collect()
}

/// Build an [`IncludeEdge`] from a `COMMAND` node, or `None` if it is not a
/// recognized inclusion command.
fn include_edge(command: &SyntaxNode, base_dir: Option<&Path>) -> Option<IncludeEdge> {
    let kind = include_kind(&command_name(command)?)?;
    let target = include_target(command, kind, base_dir);
    Some(IncludeEdge {
        kind,
        target,
        range: command.text_range(),
    })
}

/// The recognized inclusion command for a control-word name (sans backslash).
fn include_kind(name: &str) -> Option<IncludeKind> {
    Some(match name {
        "input" => IncludeKind::Input,
        "include" => IncludeKind::Include,
        "import" => IncludeKind::Import,
        "subimport" => IncludeKind::SubImport,
        "subfile" => IncludeKind::SubFile,
        _ => return None,
    })
}

/// Resolve the literal argument(s) of `command` to a target path. `\import` and
/// `\subimport` take `{dir}{file}` (joined); the rest take a single `{file}`.
fn include_target(
    command: &SyntaxNode,
    kind: IncludeKind,
    base_dir: Option<&Path>,
) -> IncludeTarget {
    let raw = match kind {
        IncludeKind::Import | IncludeKind::SubImport => {
            match (nth_group_text(command, 0), nth_group_text(command, 1)) {
                (Some(dir), Some(file)) => PathBuf::from(dir).join(file),
                _ => return IncludeTarget::Dynamic,
            }
        }
        _ => match nth_group_text(command, 0) {
            Some(file) => PathBuf::from(file),
            None => return IncludeTarget::Dynamic,
        },
    };

    let with_ext = default_tex_extension(raw);
    let resolved = match base_dir {
        Some(dir) if with_ext.is_relative() => dir.join(with_ext),
        _ => with_ext,
    };
    IncludeTarget::Path(resolved)
}

/// Default the `.tex` extension when the path has none — TeX appends it to a
/// bare inclusion target.
fn default_tex_extension(path: PathBuf) -> PathBuf {
    if path.extension().is_none() {
        path.with_extension("tex")
    } else {
        path
    }
}

/// The target of a bibliography-resource command (`\bibliography`,
/// `\addbibresource`). Mirrors [`IncludeTarget`]: a statically-resolved `.bib`
/// path, or [`BibTarget::Dynamic`] for a missing or non-literal argument.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum BibTarget {
    /// A resolved path with a `.bib` extension defaulted in and joined onto the
    /// including file's directory when relative.
    Path(PathBuf),
    /// A missing or non-literal argument we cannot resolve without expanding TeX.
    Dynamic,
}

/// Collect the bibliography-resource targets declared in `root`: `\bibliography{a,b}`
/// (a comma-separated list, each defaulting `.bib`) and `\addbibresource{a.bib}`
/// (a single resource). Relative targets resolve against `base_dir`. The
/// bibliography analog of [`collect_include_edge_keys`] — the cross-file citation
/// resolver consumes these. Out of scope (per the include-module docs): these are
/// *not* source includes.
pub fn collect_bib_resource_targets(root: &SyntaxNode, base_dir: Option<&Path>) -> Vec<BibTarget> {
    let mut targets = Vec::new();
    for command in root
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::COMMAND)
    {
        let Some(name) = command_name(&command) else {
            continue;
        };
        match name.as_str() {
            // `\bibliography{a,b}`: a comma-separated list of `.bib` basenames.
            "bibliography" => match nth_group_text(&command, 0) {
                Some(list) => {
                    for entry in list.split(',').map(str::trim).filter(|e| !e.is_empty()) {
                        targets.push(resolve_bib(PathBuf::from(entry), base_dir));
                    }
                    if list.split(',').all(|e| e.trim().is_empty()) {
                        targets.push(BibTarget::Dynamic);
                    }
                }
                None => targets.push(BibTarget::Dynamic),
            },
            // `\addbibresource{refs.bib}`: a single resource (usually with `.bib`).
            "addbibresource" => match nth_group_text(&command, 0) {
                Some(file) => targets.push(resolve_bib(PathBuf::from(file), base_dir)),
                None => targets.push(BibTarget::Dynamic),
            },
            _ => {}
        }
    }
    targets
}

/// Resolve one bibliography target: default the `.bib` extension, then join onto
/// `base_dir` when relative.
fn resolve_bib(raw: PathBuf, base_dir: Option<&Path>) -> BibTarget {
    let with_ext = if raw.extension().is_none() {
        raw.with_extension("bib")
    } else {
        raw
    };
    let resolved = match base_dir {
        Some(dir) if with_ext.is_relative() => dir.join(with_ext),
        _ => with_ext,
    };
    BibTarget::Path(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn edges(src: &str, base_dir: Option<&Path>) -> Vec<IncludeEdge> {
        let root = SyntaxNode::new_root(parse(src).green);
        collect_include_edges(&root, base_dir)
    }

    #[test]
    fn input_appends_tex_and_resolves_against_base_dir() {
        let base = PathBuf::from("/proj");
        let e = edges("\\input{chapters/intro}\n", Some(&base));
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].kind, IncludeKind::Input);
        assert_eq!(
            e[0].target,
            IncludeTarget::Path(PathBuf::from("/proj/chapters/intro.tex"))
        );
    }

    #[test]
    fn explicit_extension_is_kept() {
        let e = edges("\\input{logo.pdf_tex}\n", None);
        assert_eq!(
            e[0].target,
            IncludeTarget::Path(PathBuf::from("logo.pdf_tex"))
        );
    }

    #[test]
    fn include_is_recognized_as_its_own_kind() {
        let e = edges("\\include{body}\n", None);
        assert_eq!(e[0].kind, IncludeKind::Include);
        assert_eq!(e[0].target, IncludeTarget::Path(PathBuf::from("body.tex")));
    }

    #[test]
    fn underscores_and_slashes_in_path_reassemble() {
        let e = edges("\\input{parts/my_section}\n", None);
        assert_eq!(
            e[0].target,
            IncludeTarget::Path(PathBuf::from("parts/my_section.tex"))
        );
    }

    #[test]
    fn import_joins_directory_and_file() {
        let base = PathBuf::from("/proj");
        let e = edges("\\import{sub/dir/}{chapter}\n", Some(&base));
        assert_eq!(e[0].kind, IncludeKind::Import);
        assert_eq!(
            e[0].target,
            IncludeTarget::Path(PathBuf::from("/proj/sub/dir/chapter.tex"))
        );
    }

    #[test]
    fn subimport_and_subfile_are_recognized() {
        let si = edges("\\subimport{d}{f}\n", None);
        assert_eq!(si[0].kind, IncludeKind::SubImport);
        assert_eq!(si[0].target, IncludeTarget::Path(PathBuf::from("d/f.tex")));

        let sf = edges("\\subfile{sections/one}\n", None);
        assert_eq!(sf[0].kind, IncludeKind::SubFile);
        assert_eq!(
            sf[0].target,
            IncludeTarget::Path(PathBuf::from("sections/one.tex"))
        );
    }

    #[test]
    fn absolute_target_ignores_base_dir() {
        let base = PathBuf::from("/proj");
        let e = edges("\\input{/abs/preamble}\n", Some(&base));
        assert_eq!(
            e[0].target,
            IncludeTarget::Path(PathBuf::from("/abs/preamble.tex"))
        );
    }

    #[test]
    fn missing_argument_is_dynamic() {
        let e = edges("\\input\n", None);
        assert_eq!(e[0].target, IncludeTarget::Dynamic);
    }

    #[test]
    fn import_with_one_group_is_dynamic() {
        let e = edges("\\import{onlydir}\n", None);
        assert_eq!(e[0].target, IncludeTarget::Dynamic);
    }

    #[test]
    fn nested_macro_argument_is_dynamic() {
        let e = edges("\\input{\\jobname}\n", None);
        assert_eq!(e[0].target, IncludeTarget::Dynamic);
    }

    #[test]
    fn bare_input_without_braces_is_not_an_edge() {
        // The greedy argument grammar only attaches `{…}`/`[…]`; a space-delimited
        // plain-TeX filename stays sibling text, so no group → Dynamic, but the
        // command is still recognized as an `\input`.
        let e = edges("\\input foo.tex\n", None);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].target, IncludeTarget::Dynamic);
    }

    #[test]
    fn non_inclusion_commands_are_ignored() {
        let e = edges(
            "\\includegraphics{logo}\n\\usepackage{amsmath}\n\\section{Hi}\n",
            None,
        );
        assert!(e.is_empty());
    }

    #[test]
    fn multiple_edges_are_collected_in_source_order() {
        let e = edges("\\input{a}\n\\include{b}\n", None);
        let names: Vec<_> = e
            .iter()
            .map(|edge| match &edge.target {
                IncludeTarget::Path(p) => p.clone(),
                IncludeTarget::Dynamic => PathBuf::from("<dyn>"),
            })
            .collect();
        assert_eq!(names, vec![PathBuf::from("a.tex"), PathBuf::from("b.tex")]);
    }

    fn bib_targets(src: &str, base_dir: Option<&Path>) -> Vec<BibTarget> {
        let root = SyntaxNode::new_root(parse(src).green);
        collect_bib_resource_targets(&root, base_dir)
    }

    #[test]
    fn bibliography_splits_comma_list_and_defaults_bib() {
        let base = PathBuf::from("/proj");
        let t = bib_targets("\\bibliography{refs,extra}\n", Some(&base));
        assert_eq!(
            t,
            vec![
                BibTarget::Path(PathBuf::from("/proj/refs.bib")),
                BibTarget::Path(PathBuf::from("/proj/extra.bib")),
            ]
        );
    }

    #[test]
    fn addbibresource_keeps_explicit_extension() {
        let t = bib_targets("\\addbibresource{refs.bib}\n", None);
        assert_eq!(t, vec![BibTarget::Path(PathBuf::from("refs.bib"))]);
    }

    #[test]
    fn addbibresource_without_extension_defaults_bib() {
        let t = bib_targets("\\addbibresource{refs}\n", None);
        assert_eq!(t, vec![BibTarget::Path(PathBuf::from("refs.bib"))]);
    }

    #[test]
    fn bibliography_missing_argument_is_dynamic() {
        let t = bib_targets("\\bibliography\n", None);
        assert_eq!(t, vec![BibTarget::Dynamic]);
    }

    #[test]
    fn non_bibliography_commands_are_ignored() {
        assert!(bib_targets("\\input{a}\n\\cite{k}\n", None).is_empty());
    }
}
