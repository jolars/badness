//! `textDocument/documentLink` computation: a single-file CST walk turning
//! file-referencing commands into clickable links.
//!
//! We surface the same include edges the cross-file graph tracks, plus the
//! package/class loads, bibliography resources, and graphics inclusions the graph
//! deliberately leaves out — every command whose literal argument names a file on
//! disk:
//!
//! - **Includes** — `\input`/`\include`/`\subfile` (`{file}`, default `.tex`) and
//!   `\import`/`\subimport` (`{dir}{file}`, joined; the `{file}` argument is the
//!   link).
//! - **Packages/classes** — `\usepackage`/`\RequirePackage` (`.sty`,
//!   comma-list) and `\documentclass`/`\LoadClass`/`\LoadClassWithOptions`
//!   (`.cls`), each with a `.dtx` literate-source fallback.
//! - **Bibliography** — `\bibliography` (`.bib`, comma-list) and
//!   `\addbibresource` (`.bib`).
//! - **Graphics** — `\includegraphics`, whose extension is guessed against the
//!   image types [`FileArgKind::Graphics`] completes.
//!
//! Unlike [`crate::project::include`], resolution here is **disk-aware**: a link
//! is emitted only when the resolved target actually exists (the first existing
//! candidate wins for a multi-extension guess). The local-only package resolver
//! has no kpsewhich/TEXMF search, so a system `\usepackage{amsmath}` resolves to a
//! nonexistent `./amsmath.sty` and correctly yields no link, while a
//! project-local `mypkg.sty` does. Comma-separated names each get their own
//! precise span (the [`nth_group_inner`] byte-slice technique the semantic builder
//! uses for `\cref{a,b}`), so each underlines independently.
//!
//! Known limitations: `\graphicspath` is unsupported (graphics resolve against
//! `base_dir` only), and a braceless `\input foo` is never attached as an argument
//! by the grammar, so it produces no link — both shared with `project::include`.

use std::path::{Path, PathBuf};

use rowan::{TextRange, TextSize};

use crate::ast::{command_name, nth_group_inner, nth_group_text};
use crate::completion::FileArgKind;
use crate::project::package::dtx_source_of;
use crate::project::texmf::TexmfIndex;
use crate::syntax::{SyntaxKind, SyntaxNode};

/// A resolved, on-disk-existing link target paired with the source span that
/// should underline. Kept free of LSP/URI types so the walk stays unit-testable;
/// the caller ([`super::compute_document_link`]) maps it to an
/// [`lsp_types::DocumentLink`] via the shared `lsp_range` + `path_to_uri`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkTarget {
    /// Byte range of the path argument (per comma-separated name) in the source.
    pub range: TextRange,
    /// The resolved path that exists on disk.
    pub target: PathBuf,
}

/// Collect the document links in `root`. `base_dir` is the directory of the file
/// being scanned; relative targets resolve against it, and existence is checked
/// there. Walks all descendants (a `\input` is valid anywhere in the tree).
///
/// `texmf` is the installed-tree index: a package/class name that resolves nowhere
/// under `base_dir` (a system `\usepackage{amsmath}`) falls back to its installed
/// source, so such loads become clickable too. Pass an empty [`TexmfIndex`] to keep
/// resolution local-only (the pre-index behavior).
pub(crate) fn document_links(
    root: &SyntaxNode,
    base_dir: Option<&Path>,
    texmf: &TexmfIndex,
) -> Vec<LinkTarget> {
    let mut links = Vec::new();
    for command in root
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::COMMAND)
    {
        let Some(name) = command_name(&command) else {
            continue;
        };
        let Some(class) = classify(&name) else {
            continue;
        };
        collect_command(&command, class, base_dir, texmf, &mut links);
    }
    links
}

/// How a recognized command's argument(s) name a file.
#[derive(Debug, Clone, Copy)]
enum LinkClass {
    /// A single `{file}` argument at brace-group `group`, defaulting `ext` when the
    /// literal has no extension. `dtx` adds a `.dtx` literate-source fallback.
    Single {
        group: usize,
        ext: &'static str,
        dtx: bool,
    },
    /// A comma-separated list at group 0, each name defaulting `ext`. `dtx` adds
    /// the `.dtx` fallback per name (package/class loads).
    List { ext: &'static str, dtx: bool },
    /// `\import{dir}{file}`: `{file}` (group 1) is the link, resolved under `{dir}`
    /// (group 0); default `.tex`.
    ImportPair,
    /// `\includegraphics{file}` at group 0: the extension is guessed against the
    /// graphics image types when the literal has none.
    Graphics,
}

/// The link classification of a control-word name, or `None` if it references no
/// file. Mirrors the recognized sets of `project::{include, package}` and
/// `completion::file_arg`.
fn classify(name: &str) -> Option<LinkClass> {
    Some(match name {
        "input" | "include" | "subfile" => LinkClass::Single {
            group: 0,
            ext: "tex",
            dtx: false,
        },
        "import" | "subimport" => LinkClass::ImportPair,
        "usepackage" | "RequirePackage" => LinkClass::List {
            ext: "sty",
            dtx: true,
        },
        "documentclass" | "LoadClass" | "LoadClassWithOptions" => LinkClass::Single {
            group: 0,
            ext: "cls",
            dtx: true,
        },
        "bibliography" => LinkClass::List {
            ext: "bib",
            dtx: false,
        },
        "addbibresource" => LinkClass::Single {
            group: 0,
            ext: "bib",
            dtx: false,
        },
        "includegraphics" => LinkClass::Graphics,
        _ => return None,
    })
}

/// Emit the link(s) for one recognized command, pushing an entry per name that
/// resolves to an existing file.
fn collect_command(
    command: &SyntaxNode,
    class: LinkClass,
    base_dir: Option<&Path>,
    texmf: &TexmfIndex,
    out: &mut Vec<LinkTarget>,
) {
    match class {
        LinkClass::Single { group, ext, dtx } => {
            let Some((range, raw)) = nth_group_inner(command, group) else {
                return;
            };
            let name = raw.trim();
            if name.is_empty() {
                return;
            }
            if let Some(target) = resolve_existing(name, &[ext], dtx, base_dir, texmf) {
                out.push(LinkTarget { range, target });
            }
        }
        LinkClass::List { ext, dtx } => {
            let Some((inner_range, inner)) = nth_group_inner(command, 0) else {
                return;
            };
            for (name, range) in comma_spans(&inner, inner_range) {
                if let Some(target) = resolve_existing(name, &[ext], dtx, base_dir, texmf) {
                    out.push(LinkTarget { range, target });
                }
            }
        }
        LinkClass::ImportPair => {
            // `{dir}` is the base (group 0); `{file}` (group 1) is the link.
            let (Some(dir), Some((range, file))) =
                (nth_group_text(command, 0), nth_group_inner(command, 1))
            else {
                return;
            };
            let file = file.trim();
            if file.is_empty() {
                return;
            }
            let joined = PathBuf::from(dir.trim()).join(file);
            let raw = joined.to_string_lossy();
            if let Some(target) = resolve_existing(&raw, &["tex"], false, base_dir, texmf) {
                out.push(LinkTarget { range, target });
            }
        }
        LinkClass::Graphics => {
            let Some((range, raw)) = nth_group_inner(command, 0) else {
                return;
            };
            let name = raw.trim();
            if name.is_empty() {
                return;
            }
            let exts = FileArgKind::Graphics.extensions();
            if let Some(target) = resolve_existing(name, exts, false, base_dir, texmf) {
                out.push(LinkTarget { range, target });
            }
        }
    }
}

/// Resolve `raw` to the first candidate path that exists on disk, or `None`.
///
/// When `raw` already carries an extension, the sole candidate is `raw` itself;
/// otherwise each of `exts` is appended in order (the graphics guess tries several,
/// the deterministic kinds pass a single default). `dtx` adds a trailing `.dtx`
/// literate-source candidate for `.sty`/`.cls` targets. Each candidate is joined
/// onto `base_dir` when relative before the existence check.
///
/// When nothing exists under `base_dir`, a bare name (no directory) is looked up in
/// the installed-tree `texmf` index — this is what makes a system `\usepackage{amsmath}`
/// resolve to its installed source. An empty index skips this fallback, leaving the
/// pre-index local-only behavior intact.
fn resolve_existing(
    raw: &str,
    exts: &[&str],
    dtx: bool,
    base_dir: Option<&Path>,
    texmf: &TexmfIndex,
) -> Option<PathBuf> {
    let raw = PathBuf::from(raw);
    let mut candidates: Vec<PathBuf> = if raw.extension().is_some() {
        vec![raw.clone()]
    } else {
        exts.iter().map(|ext| raw.with_extension(ext)).collect()
    };
    if dtx {
        // Fall back to the `.dtx` a `.sty`/`.cls` would be generated from.
        let dtx_of: Vec<PathBuf> = candidates.iter().filter_map(|c| dtx_source_of(c)).collect();
        candidates.extend(dtx_of);
    }
    let local = candidates.into_iter().find_map(|candidate| {
        let resolved = match base_dir {
            Some(dir) if candidate.is_relative() => dir.join(candidate),
            _ => candidate,
        };
        resolved.is_file().then_some(resolved)
    });
    local.or_else(|| resolve_in_texmf(&raw, exts, dtx, texmf))
}

/// Resolve a bare package/class/include *name* against the installed TEXMF index.
/// Only a name with no directory component qualifies (a system package is referenced
/// by name, never a relative path); an explicit extension pins the lookup, otherwise
/// `exts` (plus a trailing `dtx` when `dtx`) are tried in order.
fn resolve_in_texmf(raw: &Path, exts: &[&str], dtx: bool, texmf: &TexmfIndex) -> Option<PathBuf> {
    if texmf.is_empty() {
        return None;
    }
    // Reject anything carrying a directory: only bare names live in the tree by name.
    if raw.parent().is_some_and(|p| !p.as_os_str().is_empty()) {
        return None;
    }
    let stem = raw.file_stem()?.to_str()?;
    let mut try_exts: Vec<&str> = match raw.extension().and_then(|e| e.to_str()) {
        Some(ext) => vec![ext],
        None => exts.to_vec(),
    };
    if dtx {
        try_exts.push("dtx");
    }
    texmf.resolve(stem, &try_exts).map(Path::to_path_buf)
}

/// Split a group's inner text into comma-separated names paired with their precise
/// source ranges, dropping empties. The document-link analog of the semantic
/// builder's `key_spans`: each name's range is sliced off `inner_range` by byte
/// offset (exact because trimming removes only single-byte ASCII whitespace).
///
/// Shared with [`super::hover`]'s package-name hover (`pub(super)`), which picks the
/// single segment covering the cursor.
pub(super) fn comma_spans(inner: &str, inner_range: TextRange) -> Vec<(&str, TextRange)> {
    let base = inner_range.start();
    let mut out = Vec::new();
    let mut seg_off = 0usize;
    for segment in inner.split(',') {
        let name = segment.trim();
        if !name.is_empty() {
            let lo = segment.len() - segment.trim_start().len();
            let start = base + TextSize::from((seg_off + lo) as u32);
            let end = start + TextSize::from(name.len() as u32);
            out.push((name, TextRange::new(start, end)));
        }
        seg_off += segment.len() + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    /// Parse `src` and collect links resolved against `base_dir` (no TEXMF tier).
    fn links(src: &str, base_dir: &Path) -> Vec<LinkTarget> {
        links_with_texmf(src, base_dir, &TexmfIndex::default())
    }

    /// [`links`] with an explicit installed-tree index for the system-package tier.
    fn links_with_texmf(src: &str, base_dir: &Path, texmf: &TexmfIndex) -> Vec<LinkTarget> {
        let root = SyntaxNode::new_root(parse(src).green);
        document_links(&root, Some(base_dir), texmf)
    }

    /// The source substring a link underlines.
    fn underlined<'a>(src: &'a str, link: &LinkTarget) -> &'a str {
        &src[link.range]
    }

    #[test]
    fn input_links_only_when_the_tex_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("chap1.tex"), "").unwrap();

        let src = "\\input{chap1}\n\\input{missing}\n";
        let got = links(src, dir.path());
        assert_eq!(got.len(), 1);
        assert_eq!(underlined(src, &got[0]), "chap1");
        assert_eq!(got[0].target, dir.path().join("chap1.tex"));
    }

    #[test]
    fn explicit_extension_is_kept_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.ltx"), "").unwrap();

        let src = "\\include{notes.ltx}\n";
        let got = links(src, dir.path());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].target, dir.path().join("notes.ltx"));
    }

    #[test]
    fn usepackage_list_links_each_local_sty_separately() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mypkg.sty"), "").unwrap();
        // `amsmath` is a system package with no local file: no link.

        let src = "\\usepackage{mypkg,amsmath}\n";
        let got = links(src, dir.path());
        assert_eq!(got.len(), 1);
        assert_eq!(underlined(src, &got[0]), "mypkg");
        assert_eq!(got[0].target, dir.path().join("mypkg.sty"));
    }

    #[test]
    fn documentclass_falls_back_to_dtx() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("myclass.dtx"), "").unwrap();

        let src = "\\documentclass{myclass}\n";
        let got = links(src, dir.path());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].target, dir.path().join("myclass.dtx"));
    }

    #[test]
    fn bibliography_defaults_bib_extension() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("refs.bib"), "").unwrap();

        let src = "\\bibliography{refs}\n\\addbibresource{refs.bib}\n";
        let got = links(src, dir.path());
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|l| l.target == dir.path().join("refs.bib")));
    }

    #[test]
    fn includegraphics_guesses_the_first_existing_extension() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("fig.png"), "").unwrap();

        let src = "\\includegraphics{fig}\n";
        let got = links(src, dir.path());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].target, dir.path().join("fig.png"));
    }

    #[test]
    fn import_joins_dir_and_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/part.tex"), "").unwrap();

        let src = "\\import{sub}{part}\n";
        let got = links(src, dir.path());
        assert_eq!(got.len(), 1);
        // The link underlines only the `{file}` argument.
        assert_eq!(underlined(src, &got[0]), "part");
        assert_eq!(got[0].target, dir.path().join("sub/part.tex"));
    }

    #[test]
    fn nested_macro_argument_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let src = "\\input{\\foo}\n";
        assert!(links(src, dir.path()).is_empty());
    }

    #[test]
    fn system_package_resolves_through_the_texmf_index() {
        // No local `amsmath.sty`, but the installed tree has one: the load links to it.
        let base = tempfile::tempdir().unwrap();
        let tree = tempfile::tempdir().unwrap();
        let installed = tree.path().join("tex/latex/amsmath/amsmath.sty");
        std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
        std::fs::write(&installed, "").unwrap();
        let texmf = TexmfIndex::build_from_roots(&[tree.path().to_path_buf()]);

        let src = "\\usepackage{amsmath}\n";
        // Local-only (empty index): no link, as before.
        assert!(links(src, base.path()).is_empty());
        // With the index: the system package resolves to its installed source.
        let got = links_with_texmf(src, base.path(), &texmf);
        assert_eq!(got.len(), 1);
        assert_eq!(underlined(src, &got[0]), "amsmath");
        assert_eq!(got[0].target, installed);
    }

    #[test]
    fn local_package_wins_over_the_texmf_index() {
        // A project-local `mypkg.sty` resolves locally even when the tree also has one;
        // the `base_dir` hit is returned before the TEXMF fallback is consulted.
        let base = tempfile::tempdir().unwrap();
        std::fs::write(base.path().join("mypkg.sty"), "").unwrap();
        let tree = tempfile::tempdir().unwrap();
        let installed = tree.path().join("tex/latex/mypkg/mypkg.sty");
        std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
        std::fs::write(&installed, "").unwrap();
        let texmf = TexmfIndex::build_from_roots(&[tree.path().to_path_buf()]);

        let got = links_with_texmf("\\usepackage{mypkg}\n", base.path(), &texmf);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].target, base.path().join("mypkg.sty"));
    }
}
