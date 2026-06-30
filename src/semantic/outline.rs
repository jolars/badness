//! Build a document-symbol outline from the CST: the sectioning hierarchy
//! (`\part` … `\subparagraph`), with float/theorem environments, `\label`s, and a
//! `.dtx`'s documented macros/environments (via [`doc_associations`]) as leaves.
//! LSP-agnostic by design (byte ranges, no `lsp_types`) so it is
//! unit-testable without the language server; the `lsp` module converts the
//! [`OutlineItem`] tree into `lsp_types::DocumentSymbol`.
//!
//! Classification reads the built-in [`signature`] DB: a command's
//! [`sectioning`](signature::CommandSig::sectioning) level and an environment's
//! [`outline`](signature::EnvironmentSig::outline) category. User-defined
//! sectioning commands are out of scope here — sectioning is a static, standard
//! set — so we consult [`signature::builtin`] directly rather than the scanned
//! two-tier [`signature::Signatures`].
//!
//! Two passes. [`collect`] walks the CST in document order into a flat list of
//! `(level, item)` pairs: sectioning commands carry their level, while floats,
//! theorems, and labels carry `None`. Non-outline environments (`document`,
//! `itemize`, …) are *transparent* — their contents splice into the parent stream
//! so a `\label` inside `itemize` still surfaces. [`nest_sections`] then folds the
//! flat list into the hierarchy with a level stack (sectioning commands are CST
//! siblings; nesting is implied by level, not tree shape), attaching each
//! non-section item to the deepest open section and stretching each section's
//! range to where it closes.

use rowan::{TextRange, TextSize};

use crate::ast::{
    command_name, environment_name, first_group_range, group_inner_source, nth_group,
    nth_group_text,
};
use crate::semantic::doc::{DocAssociation, DocKind, doc_associations};
use crate::semantic::signature::{self, OutlineKind};
use crate::syntax::{SyntaxKind, SyntaxNode};

/// The kind of an outline entry, driving the LSP `SymbolKind` mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineSymbol {
    /// A sectioning command (`\section`, `\subsection`, …).
    Section,
    /// A float environment (`figure`, `table`).
    Float,
    /// A theorem-like environment (`theorem`, `lemma`, `proof`, …).
    Theorem,
    /// A `\label{…}` definition.
    Label,
    /// A documented `.dtx` macro: a `macro` environment or `\DescribeMacro`.
    Macro,
    /// A documented `.dtx` environment: an `environment` environment or
    /// `\DescribeEnv`.
    Environment,
}

/// One node in the document-symbol outline tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineItem {
    /// Display name: a section title, an environment name, or a label key.
    pub name: String,
    pub kind: OutlineSymbol,
    /// The full extent of the symbol (a section spans to where it closes).
    pub range: TextRange,
    /// The identifier sub-range to highlight on selection (always `⊆ range`).
    pub selection_range: TextRange,
    pub children: Vec<OutlineItem>,
}

/// Build the outline tree for `root`.
///
/// The sectioning/float/label stream from [`collect`] is merged with the `.dtx`
/// documentation constructs from [`doc_associations`], re-sorted into document
/// order, then folded into the hierarchy by [`nest_sections`] — so a documented
/// macro nests under its enclosing section like any other leaf. (A doc construct
/// nested *inside* a float surfaces as a top-level sibling rather than under that
/// float, since [`collect_environment`] pre-nests float children from its own walk;
/// real `.dtx` files don't put doc constructs inside floats.)
pub fn outline(root: &SyntaxNode) -> Vec<OutlineItem> {
    let mut raws = collect(root);
    raws.extend(doc_associations(root).into_iter().map(doc_raw));
    // Stable sort keeps co-located items (e.g. a section and a label at the same
    // offset) in their original relative order.
    raws.sort_by_key(|raw| raw.item.range.start());
    nest_sections(raws, root.text_range().end())
}

/// Convert a `.dtx` [`DocAssociation`] into a leaf `Raw` (never a section, so
/// `level: None`); [`nest_sections`] then attaches it to the deepest open section.
fn doc_raw(assoc: DocAssociation) -> Raw {
    Raw {
        level: None,
        item: OutlineItem {
            name: assoc.name,
            kind: match assoc.kind {
                DocKind::Macro | DocKind::DescribeMacro => OutlineSymbol::Macro,
                DocKind::Environment | DocKind::DescribeEnv => OutlineSymbol::Environment,
            },
            range: assoc.range,
            selection_range: assoc.name_range,
            children: Vec::new(),
        },
    }
}

/// A collected item plus, for a sectioning command, its nesting level.
struct Raw {
    level: Option<u8>,
    item: OutlineItem,
}

/// Walk `node`'s children in document order, producing the flat `(level, item)`
/// stream. Recurses into transparent environments and other container nodes so
/// nested labels/floats/sections surface; outline environments (float/theorem)
/// become a single item whose own children are nested independently.
fn collect(node: &SyntaxNode) -> Vec<Raw> {
    let mut out = Vec::new();
    for child in node.children() {
        match child.kind() {
            SyntaxKind::COMMAND => collect_command(&child, &mut out),
            SyntaxKind::ENVIRONMENT => collect_environment(&child, &mut out),
            // Any other container (PARAGRAPH, GROUP, BEGIN, END, …): recurse so a
            // command or environment nested inside still reaches the stream.
            _ => out.extend(collect(&child)),
        }
    }
    out
}

/// Emit a sectioning or `\label` item for a `COMMAND` node, if it is one.
fn collect_command(command: &SyntaxNode, out: &mut Vec<Raw>) {
    let Some(name) = command_name(command) else {
        return;
    };

    // Curated [`signature::builtin`] only — never the bulk CWL tier: the symbol
    // outline is a curated judgment, and CWL's sectioning classifications are not
    // trustworthy enough to drive it (the CWL tier carries no `sectioning` anyway).
    if let Some(level) = signature::builtin()
        .command(&name)
        .and_then(|c| c.sectioning)
    {
        let selection = nth_group(command, 0)
            .map(|g| g.text_range())
            .unwrap_or_else(|| command.text_range());
        out.push(Raw {
            level: Some(level),
            item: OutlineItem {
                name: section_title(command).unwrap_or(name),
                kind: OutlineSymbol::Section,
                range: command.text_range(),
                selection_range: selection,
                children: Vec::new(),
            },
        });
    } else if name == "label" {
        // Skip an empty or nested-macro key (`\label{\foo}`), matching the
        // semantic model's conservative collection.
        if let Some(key) = nth_group_text(command, 0) {
            let key = key.trim();
            if !key.is_empty() {
                let range = first_group_range(command);
                out.push(Raw {
                    level: None,
                    item: OutlineItem {
                        name: key.to_owned(),
                        kind: OutlineSymbol::Label,
                        range,
                        selection_range: range,
                        children: Vec::new(),
                    },
                });
            }
        }
    }
}

/// Emit a float/theorem item for an `ENVIRONMENT` node, or splice its contents in
/// transparently when it is not outline-worthy.
fn collect_environment(env: &SyntaxNode, out: &mut Vec<Raw>) {
    // The name lives in the `\begin{name}` (`BEGIN` node), not on `ENVIRONMENT`.
    let begin = env.children().find(|c| c.kind() == SyntaxKind::BEGIN);
    let name = begin.as_ref().and_then(environment_name);
    let kind = name.as_deref().and_then(|name| {
        signature::builtin()
            .environment(name)
            .and_then(|e| e.outline)
    });

    let Some(kind) = kind else {
        // Transparent: hoist any inner sections/floats/labels into the parent.
        out.extend(collect(env));
        return;
    };

    let name = name.unwrap_or_default();
    let selection = begin
        .as_ref()
        .map(|b| b.text_range())
        .unwrap_or_else(|| env.text_range());
    out.push(Raw {
        level: None,
        item: OutlineItem {
            name,
            kind: match kind {
                OutlineKind::Float => OutlineSymbol::Float,
                OutlineKind::Theorem => OutlineSymbol::Theorem,
            },
            range: env.text_range(),
            selection_range: selection,
            // An inner `\label`/nested environment nests under the float; nest the
            // body independently against the environment's end.
            children: nest_sections(collect(env), env.text_range().end()),
        },
    });
}

/// Fold the flat `(level, item)` stream into the sectioning hierarchy. `end_bound`
/// is the closing offset of the enclosing scope (document or environment body),
/// used to stretch sections still open at the end.
fn nest_sections(raws: Vec<Raw>, end_bound: TextSize) -> Vec<OutlineItem> {
    let mut roots: Vec<OutlineItem> = Vec::new();
    // (level, the section being accumulated) — innermost on top.
    let mut stack: Vec<(u8, OutlineItem)> = Vec::new();

    for raw in raws {
        match raw.level {
            Some(level) => {
                let start = raw.item.range.start();
                // A section at this level (or a shallower one) closes every open
                // section it is not nested under; each ends where this one begins.
                while stack.last().is_some_and(|(open, _)| *open >= level) {
                    let (_, mut section) = stack.pop().unwrap();
                    section.range = TextRange::new(section.range.start(), start);
                    attach(&mut roots, &mut stack, section);
                }
                stack.push((level, raw.item));
            }
            // A float/theorem/label sits inside the deepest open section.
            None => attach(&mut roots, &mut stack, raw.item),
        }
    }

    // Drain still-open sections; they run to the end of the enclosing scope.
    while let Some((_, mut section)) = stack.pop() {
        section.range = TextRange::new(section.range.start(), end_bound);
        attach(&mut roots, &mut stack, section);
    }
    roots
}

/// Attach `item` to the deepest open section, or to the roots when none is open.
fn attach(roots: &mut Vec<OutlineItem>, stack: &mut [(u8, OutlineItem)], item: OutlineItem) {
    match stack.last_mut() {
        Some((_, parent)) => parent.children.push(item),
        None => roots.push(item),
    }
}

/// The display title of a sectioning command: the first `{…}` argument (the
/// `[opt]` short title is skipped by [`nth_group`]). Falls back to the raw inner
/// source when the title holds nested macros, and to `None` when there is no title
/// group at all (the caller substitutes the command name).
fn section_title(command: &SyntaxNode) -> Option<String> {
    let group = nth_group(command, 0)?;
    let text = nth_group_text(command, 0)
        .unwrap_or_else(|| group_inner_source(&group))
        .trim()
        .to_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{LatexFlavor, LexConfig, parse, parse_with_flavor};

    fn outline_of(src: &str) -> Vec<OutlineItem> {
        outline(&SyntaxNode::new_root(parse(src).green))
    }

    /// Parse in `.dtx` (docstrip) mode so the leading-`%` ltxdoc lines become real
    /// `macro`/`environment`/`\DescribeMacro` constructs, then build the outline.
    fn outline_of_dtx(src: &str) -> Vec<OutlineItem> {
        let config = LexConfig {
            flavor: LatexFlavor::Document,
            dtx: true,
        };
        let parsed = parse_with_flavor(src, config);
        assert_eq!(parsed.syntax().to_string(), src, "losslessness violated");
        outline(&parsed.syntax())
    }

    #[test]
    fn sibling_sections_are_roots() {
        let items = outline_of("\\section{A}\ntext\n\\section{B}\n");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "A");
        assert_eq!(items[0].kind, OutlineSymbol::Section);
        assert_eq!(items[1].name, "B");
        assert!(items[0].children.is_empty());
    }

    #[test]
    fn deeper_levels_nest() {
        let items = outline_of("\\section{A}\n\\subsection{B}\n\\subsubsection{C}\n");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "A");
        assert_eq!(items[0].children.len(), 1);
        let b = &items[0].children[0];
        assert_eq!(b.name, "B");
        assert_eq!(b.children.len(), 1);
        assert_eq!(b.children[0].name, "C");
    }

    #[test]
    fn shallower_section_pops_back_to_root() {
        let items = outline_of("\\section{A}\n\\subsection{B}\n\\section{C}\n");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "A");
        assert_eq!(items[0].children[0].name, "B");
        assert_eq!(items[1].name, "C");
        assert!(items[1].children.is_empty());
    }

    #[test]
    fn figure_with_label_nests_label() {
        let items = outline_of("\\begin{figure}\n\\label{fig:x}\n\\end{figure}\n");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, OutlineSymbol::Float);
        assert_eq!(items[0].name, "figure");
        assert_eq!(items[0].children.len(), 1);
        assert_eq!(items[0].children[0].kind, OutlineSymbol::Label);
        assert_eq!(items[0].children[0].name, "fig:x");
    }

    #[test]
    fn theorem_is_theorem_kind() {
        let items = outline_of("\\begin{theorem}\nx\n\\end{theorem}\n");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, OutlineSymbol::Theorem);
        assert_eq!(items[0].name, "theorem");
    }

    #[test]
    fn label_after_section_nests_under_it() {
        let items = outline_of("\\section{A}\n\\label{sec:a}\n");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].children.len(), 1);
        assert_eq!(items[0].children[0].kind, OutlineSymbol::Label);
        assert_eq!(items[0].children[0].name, "sec:a");
    }

    #[test]
    fn float_inside_section_nests() {
        let items = outline_of("\\section{A}\n\\begin{table}\nx\n\\end{table}\n");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].children.len(), 1);
        assert_eq!(items[0].children[0].kind, OutlineSymbol::Float);
        assert_eq!(items[0].children[0].name, "table");
    }

    #[test]
    fn label_inside_itemize_is_hoisted_to_section() {
        let items =
            outline_of("\\section{A}\n\\begin{itemize}\n\\item \\label{x}\n\\end{itemize}\n");
        assert_eq!(items.len(), 1);
        // `itemize` is transparent — the label surfaces under the section, not
        // under an itemize node.
        assert_eq!(items[0].children.len(), 1);
        assert_eq!(items[0].children[0].name, "x");
    }

    #[test]
    fn section_extent_ends_at_next_sibling_start() {
        let src = "\\section{A}\ntext\n\\section{B}\n";
        let items = outline_of(src);
        let next = src.rfind("\\section").unwrap();
        assert_eq!(usize::from(items[0].range.end()), next);
    }

    #[test]
    fn nested_macro_title_falls_back_to_source() {
        let items = outline_of("\\section{\\textsc{Intro}}\n");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "\\textsc{Intro}");
    }

    #[test]
    fn dtx_macro_env_is_a_macro_symbol() {
        let items = outline_of_dtx("% \\begin{macro}{\\foo}\n% docs.\n% \\end{macro}\n");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "\\foo");
        assert_eq!(items[0].kind, OutlineSymbol::Macro);
    }

    #[test]
    fn dtx_describe_macro_braced_and_braceless() {
        let braced = outline_of_dtx("% \\DescribeMacro{\\foo} does foo.\n");
        assert_eq!(braced.len(), 1);
        assert_eq!(braced[0].name, "\\foo");
        assert_eq!(braced[0].kind, OutlineSymbol::Macro);

        let braceless = outline_of_dtx("% \\DescribeMacro\\foo does foo.\n");
        assert_eq!(braceless.len(), 1);
        assert_eq!(braceless[0].name, "\\foo");
        assert_eq!(braceless[0].kind, OutlineSymbol::Macro);
    }

    #[test]
    fn dtx_environment_constructs_are_environment_symbols() {
        let env = outline_of_dtx("% \\begin{environment}{myenv}\n% docs.\n% \\end{environment}\n");
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].name, "myenv");
        assert_eq!(env[0].kind, OutlineSymbol::Environment);

        let describe = outline_of_dtx("% \\DescribeEnv{myenv} is an env.\n");
        assert_eq!(describe.len(), 1);
        assert_eq!(describe[0].name, "myenv");
        assert_eq!(describe[0].kind, OutlineSymbol::Environment);
    }

    #[test]
    fn dtx_macro_nests_under_preceding_section() {
        let items =
            outline_of_dtx("\\section{Impl}\n% \\begin{macro}{\\foo}\n% docs.\n% \\end{macro}\n");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "Impl");
        assert_eq!(items[0].children.len(), 1);
        assert_eq!(items[0].children[0].name, "\\foo");
        assert_eq!(items[0].children[0].kind, OutlineSymbol::Macro);
    }

    #[test]
    fn dtx_constructs_without_sections_are_roots_in_order() {
        let items =
            outline_of_dtx("% \\DescribeMacro\\foo\n% \\begin{macro}{\\bar}\n% \\end{macro}\n");
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["\\foo", "\\bar"]);
    }
}
