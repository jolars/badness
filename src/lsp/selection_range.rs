//! `textDocument/selectionRange` computation: a pure single-file CST walk producing,
//! for each cursor position, the nested "expand selection" chain that grows outward
//! through the syntax hierarchy (token -> group -> argument -> command -> environment
//! -> ... -> root). No semantic model and no workspace lookup, so it runs straight on
//! the read pool like the folding outline.
//!
//! The chain is exactly the CST ancestor stack: the leaf token at the cursor, then
//! every enclosing node up to and including `ROOT`. Because the enclosing-environment
//! stack is a subsequence of those ancestors, this subsumes texlab's `findEnvironments`
//! command for free (AGENTS.md / TODO.md). We add no kind filtering—the tree's own
//! nesting is the intended granularity—and only collapse consecutive equal ranges
//! (a `COMMAND` and its sole `ARGUMENT` often share a span) so each step strictly
//! widens, as clients expect.

use lsp_types::{Position, Range, SelectionRange};
use rowan::{TextRange, TextSize, TokenAtOffset};

use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};
use crate::text::LineIndex;

/// The expand-selection chain for each cursor in `positions`, one [`SelectionRange`]
/// per input position (in order). `idx`/`text` must index the same buffer `root` was
/// parsed from.
pub(crate) fn selection_ranges(
    root: &SyntaxNode,
    idx: &LineIndex,
    text: &str,
    positions: &[Position],
) -> Vec<SelectionRange> {
    positions
        .iter()
        .map(|pos| selection_range_at(root, idx, text, *pos))
        .collect()
}

/// The single expand-selection chain at `pos`: the leaf token's range followed by
/// every ancestor's range up to `ROOT`, deduplicated to strictly-widening steps and
/// folded into a `SelectionRange` linked from innermost to outermost.
fn selection_range_at(
    root: &SyntaxNode,
    idx: &LineIndex,
    text: &str,
    pos: Position,
) -> SelectionRange {
    let offset = idx.offset_at(text, pos.line, pos.character);
    let at = TextSize::new(offset.min(u32::MAX as usize) as u32);

    // Collect the innermost-first stack of byte ranges. A cursor inside a token starts
    // from that token; a cursor at a token boundary prefers the non-trivia side so it
    // expands into real syntax first; a cursor past EOF has only the root.
    let mut ranges: Vec<TextRange> = Vec::new();
    match root.token_at_offset(at) {
        TokenAtOffset::Single(tok) => push_token_chain(&tok, &mut ranges),
        TokenAtOffset::Between(left, right) => {
            push_token_chain(&prefer_nontrivia(left, right), &mut ranges)
        }
        TokenAtOffset::None => ranges.push(root.text_range()),
    }
    // A node and its sole child can share a span; collapse so each level grows.
    ranges.dedup();

    let to_range = |r: TextRange| {
        let (sl, sc) = idx.position(text, r.start().into());
        let (el, ec) = idx.position(text, r.end().into());
        Range {
            start: Position::new(sl, sc),
            end: Position::new(el, ec),
        }
    };

    // Fold outermost -> innermost so each node's `parent` points at the wider range.
    // `ranges` is never empty (the root always covers `at`), so `current` is `Some`.
    let mut current: Option<Box<SelectionRange>> = None;
    for &r in ranges.iter().rev() {
        current = Some(Box::new(SelectionRange {
            range: to_range(r),
            parent: current,
        }));
    }
    *current.expect("chain always contains the root range")
}

/// Push `tok`'s range, then every ancestor's range up to and including `ROOT`.
fn push_token_chain(tok: &SyntaxToken, ranges: &mut Vec<TextRange>) {
    ranges.push(tok.text_range());
    ranges.extend(tok.parent_ancestors().map(|n| n.text_range()));
}

/// At a token boundary, expand into the non-trivia side when exactly one side is
/// trivia; otherwise default to the right token (rust-analyzer's convention).
fn prefer_nontrivia(left: SyntaxToken, right: SyntaxToken) -> SyntaxToken {
    let is_trivia = |k| {
        matches!(
            k,
            SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
        )
    };
    if is_trivia(right.kind()) && !is_trivia(left.kind()) {
        left
    } else {
        right
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    /// A `((start_line, start_col), (end_line, end_col))` range pair for terse
    /// assertions.
    type Pair = ((u32, u32), (u32, u32));

    /// The expand-selection chain at `(line, character)`, flattened innermost ->
    /// outermost as `(start, end)` LSP position pairs for terse assertions.
    fn chain(src: &str, line: u32, character: u32) -> Vec<Pair> {
        let root = SyntaxNode::new_root(parse(src).green);
        let idx = LineIndex::new(src);
        let sr = selection_range_at(&root, &idx, src, Position::new(line, character));
        let mut out = Vec::new();
        let mut cur = Some(Box::new(sr));
        while let Some(node) = cur {
            let r = node.range;
            out.push((
                (r.start.line, r.start.character),
                (r.end.line, r.end.character),
            ));
            cur = node.parent;
        }
        out
    }

    /// Assert the chain strictly widens: each range contains the previous, and no two
    /// consecutive ranges are equal (dedup should have collapsed those).
    fn assert_strictly_nested(chain: &[Pair]) {
        for pair in chain.windows(2) {
            let (inner, outer) = (pair[0], pair[1]);
            assert!(
                outer.0 <= inner.0 && inner.1 <= outer.1 && outer != inner,
                "each step must strictly widen: {inner:?} inside {outer:?}",
            );
        }
    }

    #[test]
    fn command_argument_expands_group_to_command_to_root() {
        // \section{Intro}\n — cursor on the 'n' of Intro (byte 10).
        let src = "\\section{Intro}\n";
        let c = chain(src, 0, 10);
        assert_strictly_nested(&c);
        // Innermost is the WORD "Intro" (chars 9..14).
        assert_eq!(
            c[0],
            ((0, 9), (0, 14)),
            "innermost is the title word: {c:?}"
        );
        // A GROUP {Intro} (8..15) and the whole \section command (0..15) sit between.
        assert!(
            c.contains(&((0, 8), (0, 15))),
            "the {{Intro}} group is a level: {c:?}"
        );
        assert!(
            c.contains(&((0, 0), (0, 15))),
            "the \\section command is a level: {c:?}"
        );
        // Outermost is the whole document (through the trailing newline).
        assert_eq!(
            *c.last().unwrap(),
            ((0, 0), (1, 0)),
            "root spans the doc: {c:?}"
        );
    }

    #[test]
    fn cursor_in_environment_body_has_environment_ancestor() {
        // The enclosing \begin..\end must appear as a widening step (findEnvironments).
        let src = "\\begin{itemize}\n\\item hello\n\\end{itemize}\n";
        // Cursor on "hello" (line 1).
        let c = chain(src, 1, 8);
        assert_strictly_nested(&c);
        // Some level spans from the \begin line to just past \end (the ENVIRONMENT).
        assert!(
            c.iter().any(|&(s, e)| s == (0, 0) && e == (2, 13)),
            "an ENVIRONMENT level spans \\begin..\\end: {c:?}"
        );
        assert_eq!(
            *c.last().unwrap(),
            ((0, 0), (3, 0)),
            "root spans the doc: {c:?}"
        );
    }

    #[test]
    fn math_script_expands_outward() {
        // $x^2$ — cursor on the script '2'. Just assert a well-formed widening chain
        // ending at the document root; the exact math node kinds are the parser's.
        let src = "$x^2$\n";
        let c = chain(src, 0, 3);
        assert_strictly_nested(&c);
        assert!(c.len() >= 2, "at least token + root: {c:?}");
        assert_eq!(
            *c.last().unwrap(),
            ((0, 0), (1, 0)),
            "root spans the doc: {c:?}"
        );
    }

    #[test]
    fn bare_word_in_paragraph_expands_to_root() {
        let src = "hello world\n";
        let c = chain(src, 0, 2);
        assert_strictly_nested(&c);
        // Innermost covers the "hello" word only, not the whole line.
        assert_eq!(c[0], ((0, 0), (0, 5)), "innermost is the word: {c:?}");
        assert_eq!(
            *c.last().unwrap(),
            ((0, 0), (1, 0)),
            "root spans the doc: {c:?}"
        );
    }

    #[test]
    fn out_of_range_position_clamps_and_ends_at_root() {
        let src = "\\section{A}\n";
        // Far past EOF: `offset_at` clamps to the buffer end, so we still return a
        // well-formed chain (from the last token) terminating at the document root.
        let c = chain(src, 99, 0);
        assert_strictly_nested(&c);
        assert_eq!(
            *c.last().unwrap(),
            ((0, 0), (1, 0)),
            "root spans the doc: {c:?}"
        );
    }

    #[test]
    fn empty_document_yields_a_single_empty_range() {
        let c = chain("", 0, 0);
        assert_eq!(c.len(), 1, "only the root range: {c:?}");
        assert_eq!(c[0], ((0, 0), (0, 0)), "empty root: {c:?}");
    }

    #[test]
    fn multiple_positions_map_one_to_one() {
        let src = "\\section{A}\n\\section{B}\n";
        let root = SyntaxNode::new_root(parse(src).green);
        let idx = LineIndex::new(src);
        let out = selection_ranges(
            &root,
            &idx,
            src,
            &[Position::new(0, 9), Position::new(1, 9)],
        );
        assert_eq!(out.len(), 2, "one chain per input position");
    }
}
