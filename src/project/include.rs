//! Static extraction of file-inclusion edges from a file's CST.
//!
//! LaTeX wires documents together with a small set of inclusion commands —
//! `\input`, `\include`, `\import`/`\subimport`, `\subfile`/`\subfileinclude` —
//! whose target is a literal path argument. We model only what is statically
//! knowable: literal brace-group targets. A command whose target is missing or
//! not a flat literal (e.g. built from another macro) becomes
//! [`IncludeTarget::Dynamic`] so the cross-file graph stays conservative.
//!
//! Resolution here is **pure path arithmetic** — `.tex`
//! extension defaulting and `base_dir` joining — and never touches the disk; the
//! resolved-vs-unresolved decision against the analyzed file set happens in
//! [`crate::project::graph::IncludeGraph::build`].
//!
//! **Out of scope** (not source includes): `\includegraphics`, `\graphicspath`,
//! `\bibliography`/`\addbibresource`, `\usepackage`/`\RequirePackage` — these
//! pull in non-`.tex` assets or packages.
//!
//! **The one shape-gated exception is `\documentclass`.** A `subfiles` subfile
//! names its parent in the *class option* (`\documentclass[../main.tex]{subfiles}`),
//! and that declaration is the only thing tying the two files together when the
//! parent's `\subfile{…}` call is absent or out of the analyzed set — without it
//! a subfile is its own closed, rooted label namespace and every cross-file
//! `\ref`/`\cite` in it reads as undefined (issue #112). The gate is a static
//! lexical fact, never meaning: the edge fires only when the mandatory group
//! reads exactly `subfiles`, so an ordinary `\documentclass[a4paper]{article}`
//! carries no include edge. See [`subfiles_parent_arg`].
//!
//! **Known limitations** (safe, conservative — both degrade to `Dynamic` or
//! omission): bare plain-TeX `\input foo` (no braces) leaves `foo` as sibling
//! text the greedy argument grammar never attaches, so it is not seen as an
//! edge; `\include`'s main-document-relative base directory and `\includeonly`
//! filtering are deferred (we resolve `\include` like `\input`).

use std::path::{Path, PathBuf};

use rowan::TextRange;

use crate::ast::{command_name, nth_group_text};
use crate::project::package::{OptionArg, load_option_args};
use crate::syntax::{SyntaxKind, SyntaxNode};

/// Which inclusion command produced an edge. Kept distinct even where resolution
/// is currently identical, so later passes can honor the semantic differences
/// (`\include`'s `\clearpage` + main-dir base, `\includeonly` gating).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::SalsaValue)]
pub enum IncludeKind {
    Input,
    Include,
    Import,
    SubImport,
    SubFile,
    /// `\subfileinclude{file}` (subfiles): `\subfile`'s `\include`-flavored
    /// sibling (it wraps the body in `\include`, so the target starts a new page).
    /// Resolution is identical; kept distinct like `\include` is from `\input`.
    SubFileInclude,
    /// `\documentclass[parent]{subfiles}`: the *reverse* edge a subfile declares
    /// to the main document whose preamble it borrows. Unlike every other kind
    /// this is not a document-body inclusion — it exists so a subfile and its
    /// parent share one label/citation namespace — so
    /// [`crate::project::graph::IncludeGraph::build`] keeps it out of the
    /// reachability and cycle adjacency.
    SubFilesParent,
    /// `\loadglsentries[type]{file}` (glossaries): loads a file of
    /// `\newglossaryentry`/`\newacronym` definitions. Resolution is `\input`-like
    /// (single target, `.tex` defaulted); kept distinct so later passes can honor
    /// the difference (its body is preamble-only definitions, never text).
    GlsEntries,
}

/// The target file of an inclusion command.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::SalsaValue)]
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
/// firewall this feeds). It also satisfies `salsa::SalsaValue`, which [`IncludeEdge`]
/// cannot because of its `TextRange` field.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::SalsaValue)]
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
/// This walks the whole tree rather than only the top level:
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
    let name = command_name(command)?;
    // `\documentclass` is recognized by *shape*, not by name alone (see the module
    // docs), so it is dispatched before the name-only table.
    let kind = match name.as_str() {
        "documentclass" => return subfiles_parent_edge(command, base_dir),
        _ => include_kind(&name)?,
    };
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
        "subfileinclude" => IncludeKind::SubFileInclude,
        "loadglsentries" => IncludeKind::GlsEntries,
        _ => return None,
    })
}

/// The parent-document argument of a `subfiles` class declaration
/// (`\documentclass[../main.tex]{subfiles}`), or `None` when `command` is any
/// other `\documentclass`.
///
/// The gate is the mandatory group reading exactly `subfiles` — a static lexical
/// fact, so an ordinary `\documentclass[a4paper,12pt]{article}` is never mistaken
/// for a subfile and its options are never mistaken for a path. Reuses
/// [`load_option_args`], which already strips the brackets, keeps exact byte
/// ranges, and returns `None` for non-literal bracket content (a macro or group).
/// Exactly one segment is required: the argument is a single path, so a
/// comma-split bracket is not one and degrades to "no literal parent".
///
/// Returns the [`OptionArg`] rather than a bare path so
/// [`crate::lsp::document_link`] can underline the same span this gate accepts,
/// keeping the edge extractor and the clickable link from ever disagreeing.
pub fn subfiles_parent_arg(command: &SyntaxNode) -> Option<OptionArg> {
    if !is_subfiles_class(command) {
        return None;
    }
    match load_option_args(command)?.as_slice() {
        [arg] => Some(arg.clone()),
        _ => None,
    }
}

/// Whether `command`'s mandatory group names the `subfiles` class — the whole
/// gate, and a purely lexical one.
fn is_subfiles_class(command: &SyntaxNode) -> bool {
    nth_group_text(command, 0).is_some_and(|name| name.trim() == "subfiles")
}

/// The [`IncludeKind::SubFilesParent`] edge of a `\documentclass`, or `None` when
/// it is not a `subfiles` declaration at all.
///
/// A `subfiles` declaration whose parent we cannot read — no bracket
/// (`\documentclass{subfiles}`), or a non-literal one — still yields an edge, a
/// [`IncludeTarget::Dynamic`] one: we know the file is a fragment of *some*
/// document we cannot see, and an open namespace is exactly the conservative
/// answer that keeps `undefined-ref` quiet.
fn subfiles_parent_edge(command: &SyntaxNode, base_dir: Option<&Path>) -> Option<IncludeEdge> {
    if !is_subfiles_class(command) {
        return None;
    }
    let target = match subfiles_parent_arg(command) {
        Some(arg) => IncludeTarget::Path(resolve_tex(PathBuf::from(arg.text.as_str()), base_dir)),
        None => IncludeTarget::Dynamic,
    };
    Some(IncludeEdge {
        kind: IncludeKind::SubFilesParent,
        target,
        range: command.text_range(),
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

    IncludeTarget::Path(resolve_tex(raw, base_dir))
}

/// Resolve one inclusion target: default the `.tex` extension when the path has
/// none (TeX appends it to a bare inclusion target), then join onto `base_dir`
/// when the result is relative. Pure path arithmetic, mirroring
/// [`super::package`]'s resolver.
fn resolve_tex(raw: PathBuf, base_dir: Option<&Path>) -> PathBuf {
    let with_ext = if raw.extension().is_none() {
        raw.with_extension("tex")
    } else {
        raw
    };
    resolve_against(with_ext, base_dir)
}

/// Join `path` onto `base_dir` when relative, then collapse the `.`/`..`
/// segments the join introduces.
///
/// The graph resolves an edge by comparing this path against the analyzed member
/// set *component-wise*, so an uncollapsed `chapters/../main.tex` never matches
/// the discovered `main.tex`. That bites hardest on `subfiles`, whose parent
/// declaration is idiomatically `\documentclass[../main.tex]{subfiles}`, but the
/// same arithmetic serves every `\input{../shared}`.
fn resolve_against(path: PathBuf, base_dir: Option<&Path>) -> PathBuf {
    let joined = match base_dir {
        Some(dir) if path.is_relative() => dir.join(path),
        _ => path,
    };
    lexically_normalize(joined)
}

/// Collapse `.` and `..` segments textually — no symlink resolution, no
/// existence check, so it stays pure and never blocks on I/O (the disk-aware
/// mirror of this lives in the language server, not here).
///
/// A `..` is dropped together with the named segment it undoes; a *leading* `..`
/// has nothing to undo and is preserved, so a member set discovered under a
/// `../proj` root still matches. The salsa file registry normalizes the same way
/// ([`crate::incremental::normalize_path`]), but absolutizes first — which this
/// must not do, since graph paths are compared to whatever spelling discovery
/// handed us.
fn lexically_normalize(path: PathBuf) -> PathBuf {
    use std::path::Component;

    if !path
        .components()
        .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
    {
        return path;
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir
                if matches!(out.components().next_back(), Some(Component::Normal(_))) =>
            {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The target of a bibliography-resource command (`\bibliography`,
/// `\addbibresource`). Mirrors [`IncludeTarget`]: a statically-resolved `.bib`
/// path, or [`BibTarget::Dynamic`] for a missing or non-literal argument.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::SalsaValue)]
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
    BibTarget::Path(resolve_against(with_ext, base_dir))
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
    fn subfileinclude_is_recognized() {
        let e = edges("\\subfileinclude{sections/one}\n", None);
        assert_eq!(e[0].kind, IncludeKind::SubFileInclude);
        assert_eq!(
            e[0].target,
            IncludeTarget::Path(PathBuf::from("sections/one.tex"))
        );
    }

    #[test]
    fn subfiles_class_option_is_a_parent_edge() {
        // Issue #112: the class option is the only thing tying a subfile to its
        // main document when the parent's `\subfile{…}` call is out of view.
        let base = PathBuf::from("/proj/chapters");
        let e = edges("\\documentclass[../main.tex]{subfiles}\n", Some(&base));
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].kind, IncludeKind::SubFilesParent);
        // The `..` collapses, so this matches the discovered `/proj/main.tex`.
        assert_eq!(
            e[0].target,
            IncludeTarget::Path(PathBuf::from("/proj/main.tex"))
        );
    }

    #[test]
    fn subfiles_parent_defaults_the_tex_extension() {
        let e = edges("\\documentclass[main]{subfiles}\n", None);
        assert_eq!(e[0].target, IncludeTarget::Path(PathBuf::from("main.tex")));
    }

    #[test]
    fn ordinary_documentclass_is_not_an_include() {
        // The gate is the class *name*: real options must never read as a path.
        assert!(edges("\\documentclass[a4paper,12pt]{article}\n", None).is_empty());
        assert!(edges("\\documentclass{article}\n", None).is_empty());
    }

    #[test]
    fn subfiles_without_a_readable_parent_is_dynamic() {
        // No bracket at all, a macro-built parent, and a comma-split bracket
        // (not a single path) all mean "a fragment of some document we cannot
        // see" — an open namespace, not a resolved edge.
        for src in [
            "\\documentclass{subfiles}\n",
            "\\documentclass[\\parentfile]{subfiles}\n",
            "\\documentclass[a,b]{subfiles}\n",
        ] {
            let e = edges(src, None);
            assert_eq!(e.len(), 1, "expected one edge for {src:?}");
            assert_eq!(e[0].kind, IncludeKind::SubFilesParent);
            assert_eq!(e[0].target, IncludeTarget::Dynamic, "for {src:?}");
        }
    }

    #[test]
    fn loadglsentries_is_recognized_with_optional_arg() {
        // `\loadglsentries[type]{file}`: the OPTIONAL never shifts the target
        // group, and the `.tex` extension defaults in like `\input`.
        let e = edges("\\loadglsentries[main]{glossary/entries}\n", None);
        assert_eq!(e[0].kind, IncludeKind::GlsEntries);
        assert_eq!(
            e[0].target,
            IncludeTarget::Path(PathBuf::from("glossary/entries.tex"))
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
    fn parent_segments_collapse_against_the_base_dir() {
        let base = PathBuf::from("/proj/chapters");
        let e = edges("\\input{../shared/preamble}\n", Some(&base));
        assert_eq!(
            e[0].target,
            IncludeTarget::Path(PathBuf::from("/proj/shared/preamble.tex"))
        );
    }

    #[test]
    fn a_leading_parent_segment_is_preserved() {
        // Nothing to undo, and the member set may well be spelled this way (a
        // `badness lint ../proj` run) — collapsing it would break the match.
        let e = edges("\\input{../main}\n", None);
        assert_eq!(
            e[0].target,
            IncludeTarget::Path(PathBuf::from("../main.tex"))
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
    fn parameter_argument_is_dynamic() {
        // `\input{#1}` in a definition body: the target exists only at expansion
        // time, the canonical dynamic include.
        let e = edges("\\input{#1}\n", None);
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
