//! `textDocument/foldingRange` computation: a pure single-file CST walk producing
//! the foldable regions of a LaTeX document — environments (`\begin…\end`),
//! sectioning spans (`\section` … `\subparagraph`), and runs of standalone comment
//! lines. No semantic model and no workspace lookup, so it runs straight on the read
//! pool like the document-symbol outline.
//!
//! Three sources, each emitting only multi-line spans (a single-line construct never
//! folds):
//!
//! - **Sectioning spans** reuse [`outline`]: an [`OutlineSymbol::Section`] item's
//!   `range` is already stretched by the outline's level-stack nesting to where the
//!   section closes (the next heading of equal/shallower level, or the end of the
//!   enclosing scope), which is exactly a section fold. We recurse the tree so nested
//!   subsections fold within their parent.
//! - **Environments** fold every `ENVIRONMENT` node from its `\begin` line to its
//!   `\end` line.
//! - **Comment runs** group maximal runs of consecutive *standalone* comment lines (a
//!   comment that is the only non-trivia token on its line; a trailing `code % x`
//!   comment is excluded) and fold each run of two or more lines under
//!   [`FoldingRangeKind::Comment`].

use lsp_types::{FoldingRange, FoldingRangeKind};

use crate::semantic::{OutlineItem, OutlineSymbol, outline};
use crate::syntax::{SyntaxKind, SyntaxNode};
use crate::text::LineIndex;

/// The foldable regions of an already-parsed LaTeX `root`. `idx`/`text` must index
/// the same buffer `root` was parsed from.
pub(crate) fn folding_ranges(root: &SyntaxNode, idx: &LineIndex, text: &str) -> Vec<FoldingRange> {
    let line_of = |offset: usize| idx.utf16_position(text, offset).0;
    let mut ranges = Vec::new();

    // 1. Sectioning spans — reuse the outline's stretched section ranges. A section's
    //    `range.end()` is exclusive (the next heading's start, or the scope end), so
    //    the last line that belongs to it is `end - 1`; using `end` would fold the
    //    following heading's line into the previous section.
    collect_section_folds(&outline(root), &line_of, &mut ranges);

    // 2. Environments — every `\begin…\end`. Fold from the `\begin` line (the BEGIN
    //    child's start, *not* the node's: a run of leading `%` comments binds into the
    //    ENVIRONMENT node, so its own start can sit lines earlier — those fold as a
    //    comment run instead). The node end is just past the `\end` group's closing
    //    brace, on the `\end` line, so no off-by-one is needed there.
    for node in root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::ENVIRONMENT)
    {
        let begin = node
            .children()
            .find(|c| c.kind() == SyntaxKind::BEGIN)
            .unwrap_or_else(|| node.clone());
        emit(
            &mut ranges,
            line_of(begin.text_range().start().into()),
            line_of(node.text_range().end().into()),
            None,
        );
    }

    // 3. Comment runs — collect the line of every standalone comment, then fold each
    //    maximal run of consecutive lines (length >= 2).
    let mut standalone: Vec<u32> = Vec::new();
    let mut code_on_line = false;
    for token in root
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
    {
        match token.kind() {
            SyntaxKind::NEWLINE => code_on_line = false,
            SyntaxKind::WHITESPACE => {}
            SyntaxKind::COMMENT => {
                if !code_on_line {
                    standalone.push(line_of(token.text_range().start().into()));
                }
            }
            _ => code_on_line = true,
        }
    }
    let mut i = 0;
    while i < standalone.len() {
        let mut j = i + 1;
        while j < standalone.len() && standalone[j] == standalone[j - 1] + 1 {
            j += 1;
        }
        if j - i >= 2 {
            ranges.push(FoldingRange {
                start_line: standalone[i],
                end_line: standalone[j - 1],
                kind: Some(FoldingRangeKind::Comment),
                ..Default::default()
            });
        }
        i = j;
    }

    ranges
}

/// Emit a fold per `Section` item, recursing into children so nested subsections fold
/// within their parent. Non-section items (floats/theorems are folded by the
/// environment walk; labels are not foldable) are skipped but still recursed through.
fn collect_section_folds(
    items: &[OutlineItem],
    line_of: &impl Fn(usize) -> u32,
    ranges: &mut Vec<FoldingRange>,
) {
    for item in items {
        if item.kind == OutlineSymbol::Section {
            let range = item.range;
            if range.end() > range.start() {
                // Start at the heading line via `selection_range` (the title group,
                // always on the `\section` line) rather than `range.start()`: a run of
                // leading `%` comments binds into the COMMAND node, so its own start
                // can sit lines earlier (those fold as a comment run instead).
                emit(
                    ranges,
                    line_of(item.selection_range.start().into()),
                    line_of(usize::from(range.end()) - 1),
                    None,
                );
            }
        }
        collect_section_folds(&item.children, line_of, ranges);
    }
}

/// Push a fold spanning `[start_line, end_line]`, dropping single-line spans.
fn emit(
    ranges: &mut Vec<FoldingRange>,
    start_line: u32,
    end_line: u32,
    kind: Option<FoldingRangeKind>,
) {
    if end_line > start_line {
        ranges.push(FoldingRange {
            start_line,
            end_line,
            kind,
            ..Default::default()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn folds(src: &str) -> Vec<FoldingRange> {
        let root = SyntaxNode::new_root(parse(src).green);
        let idx = LineIndex::new(src);
        folding_ranges(&root, &idx, src)
    }

    /// A `(start_line, end_line, kind)` triple for terse assertions.
    fn triples(ranges: &[FoldingRange]) -> Vec<(u32, u32, Option<FoldingRangeKind>)> {
        ranges
            .iter()
            .map(|r| (r.start_line, r.end_line, r.kind.clone()))
            .collect()
    }

    #[test]
    fn sibling_sections_fold_each_to_before_next() {
        // line 0: \section{A}, 1: text, 2: \section{B}, 3: more
        let src = "\\section{A}\ntext\n\\section{B}\nmore\n";
        let t = triples(&folds(src));
        assert!(t.contains(&(0, 1, None)), "section A folds 0..1, got {t:?}");
        assert!(t.contains(&(2, 3, None)), "section B folds 2..3, got {t:?}");
    }

    #[test]
    fn nested_subsection_folds_within_section() {
        // 0: \section{A}, 1: \subsection{B}, 2: body, 3: \section{C}
        let src = "\\section{A}\n\\subsection{B}\nbody\n\\section{C}\nx\n";
        let t = triples(&folds(src));
        // A spans to just before C (line 2), B spans to the same body line.
        assert!(t.contains(&(0, 2, None)), "section A, got {t:?}");
        assert!(t.contains(&(1, 2, None)), "subsection B, got {t:?}");
    }

    #[test]
    fn single_line_section_does_not_fold() {
        let src = "\\section{A}\n";
        assert!(folds(src).is_empty());
    }

    #[test]
    fn last_section_runs_to_eof() {
        let src = "\\section{A}\nl1\nl2\n";
        let t = triples(&folds(src));
        assert!(t.contains(&(0, 2, None)), "got {t:?}");
    }

    #[test]
    fn multiline_environment_folds() {
        let src = "\\begin{itemize}\n\\item x\n\\end{itemize}\n";
        let t = triples(&folds(src));
        assert!(t.contains(&(0, 2, None)), "itemize folds 0..2, got {t:?}");
    }

    #[test]
    fn single_line_environment_does_not_fold() {
        let src = "\\begin{a}x\\end{a}\n";
        assert!(folds(src).is_empty());
    }

    #[test]
    fn comment_run_folds_as_comment() {
        // 0,1,2 are standalone comments; 3 is code.
        let src = "% a\n% b\n% c\ntext\n";
        let t = triples(&folds(src));
        assert_eq!(t, vec![(0, 2, Some(FoldingRangeKind::Comment))]);
    }

    #[test]
    fn single_comment_does_not_fold() {
        let src = "% lonely\ntext\n";
        assert!(folds(src).is_empty());
    }

    #[test]
    fn leading_comments_fold_separately_from_their_construct() {
        // The `%` run binds into the following ENVIRONMENT/COMMAND node, but each
        // construct must fold from its own `\begin`/`\section` line, leaving the
        // comment run as its own fold.
        let src = "% a\n% b\n\\begin{itemize}\n\\item x\n\\end{itemize}\n";
        let t = triples(&folds(src));
        assert!(
            t.contains(&(0, 1, Some(FoldingRangeKind::Comment))),
            "comment run, got {t:?}"
        );
        assert!(t.contains(&(2, 4, None)), "itemize from \\begin, got {t:?}");

        let src = "% a\n% b\n\\section{A}\nbody\nmore\n";
        let t = triples(&folds(src));
        assert!(
            t.contains(&(0, 1, Some(FoldingRangeKind::Comment))),
            "comment run, got {t:?}"
        );
        assert!(t.contains(&(2, 4, None)), "section from heading, got {t:?}");
    }

    #[test]
    fn trailing_comment_does_not_join_a_run() {
        // A trailing comment after code is not standalone, so the two `%` lines do
        // not form a run.
        let src = "code % a\n% b\nmore\n";
        assert!(folds(src).is_empty(), "got {:?}", triples(&folds(src)));
    }
}
