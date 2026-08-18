//! Conservative top-level region reparse.
//!
//! The first useful region is one top-level prose paragraph. A faithfulness parse
//! must reproduce the old paragraph under the base parse's exact [`ParseCtx`]. The
//! edit may then touch only direct prose leaves and insert lexer-state-invariant
//! characters, so unchanged commands and their state transitions remain unchanged.
//! The edited fragment must still parse to exactly one paragraph. This deliberately
//! refuses edits to commands, groups, math, comments, catcode-sensitive punctuation,
//! and paragraph seams. Those need the later general-region proof; treating blank
//! lines alone as a reset would be unsound.

use crate::parser::core::parse_fragment_with_ctx;
use rowan::{GreenNode, GreenToken, NodeOrToken};

use crate::parser::core::SyntaxError;
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use super::leaf::shifted_errors;
use super::{Edit, ReparseBase, ReparseTier, Reparsed, finish};

type GreenElement = NodeOrToken<GreenNode, GreenToken>;

/// Reparse an edit spanning multiple leaves inside one inert top-level paragraph.
pub(super) fn reparse_region(
    base: &ReparseBase<'_>,
    edit: &Edit,
    new_text: &str,
) -> Option<Reparsed> {
    reparse_paragraph_seam(base, edit, new_text)
        .or_else(|| reparse_one_paragraph(base, edit, new_text))
}

fn reparse_one_paragraph(base: &ReparseBase<'_>, edit: &Edit, new_text: &str) -> Option<Reparsed> {
    let root = base.syntax();
    let paragraph = root.children().find(|node| {
        if node.kind() != SyntaxKind::PARAGRAPH {
            return false;
        }
        let range = node.text_range();
        let start = usize::from(range.start());
        let end = usize::from(range.end());
        edit.range.start >= start && edit.range.end <= end
    })?;

    let range = paragraph.text_range();
    let start = usize::from(range.start());
    let end = usize::from(range.end());

    // This tier exists for edits a one-leaf tier cannot express. Every touched
    // token must be a direct prose child: crossing a nested command or group could
    // alter a definition scan or a forward gate, even if the inserted bytes look
    // harmless in isolation.
    let touched: Vec<_> = paragraph
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| {
            let token = token.text_range();
            usize::from(token.start()) < edit.range.end
                && usize::from(token.end()) > edit.range.start
        })
        .collect();
    if edit.range.is_empty()
        || touched.len() < 2
        || touched.iter().any(|token| {
            token.parent().as_ref() != Some(&paragraph)
                || !matches!(
                    token.kind(),
                    SyntaxKind::WORD | SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE
                )
        })
    {
        return None;
    }

    let old = base.text.get(start..end)?;
    let local = Edit {
        range: edit.range.start.checked_sub(start)?..edit.range.end.checked_sub(start)?,
        insert: edit.insert.clone(),
    };
    let new = local.apply(old);

    // These are precisely the inserted surface characters whose classification
    // may depend on TeX/expl3/package lexer state or which can open structure.
    if !inert_insert(&edit.insert) {
        return None;
    }

    // Faithfulness is the entry-context proof: if a mode established before this
    // paragraph matters to its bytes, a fresh fragment parse will not reproduce
    // the paragraph and the tier refuses. Unchanged commands inside the fragment
    // then make the lexer exit state change by exactly the same transitions.
    let old_parsed = parse_fragment_with_ctx(old, base.ctx, base.config, base.implicit_expl);
    if !old_parsed.errors.is_empty() {
        return None;
    }
    let old_fragment = only_paragraph(&old_parsed.syntax())?;
    if old_fragment.green() != paragraph.green() {
        return None;
    }

    let parsed = parse_fragment_with_ctx(&new, base.ctx, base.config, base.implicit_expl);
    if !parsed.errors.is_empty() {
        return None;
    }
    let replacement = only_paragraph(&parsed.syntax())?;

    let errors = shifted_errors(base.errors, range, edit)?;
    let green = paragraph.replace_with(replacement.green().to_owned());
    finish(green, errors, ReparseTier::Region, base, new_text)
}

/// Reparse two paragraphs and the blank-line seam between them.
///
/// The unchanged outer seams keep paragraph-anchored gates outside the fragment
/// unaffected. Gates without paragraph anchors cannot change because the edit may
/// touch only root trivia and inserts no structural spelling.
fn reparse_paragraph_seam(base: &ReparseBase<'_>, edit: &Edit, new_text: &str) -> Option<Reparsed> {
    if base.config.dtx || edit.range.is_empty() || !inert_insert(&edit.insert) {
        return None;
    }

    let root = base.syntax();
    let elements: Vec<SyntaxElement> = root.children_with_tokens().collect();
    let touched: Vec<usize> = elements
        .iter()
        .enumerate()
        .filter_map(|(i, element)| {
            let range = element.text_range();
            (usize::from(range.start()) < edit.range.end
                && usize::from(range.end()) > edit.range.start)
                .then_some(i)
        })
        .collect();
    if touched.len() < 2
        || touched.iter().any(|&i| {
            !matches!(
                elements[i].kind(),
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE
            )
        })
    {
        return None;
    }

    let seam_first = *touched.first()?;
    let seam_last = *touched.last()?;
    let first = (0..seam_first)
        .rev()
        .find(|&i| elements[i].kind() == SyntaxKind::PARAGRAPH)?;
    let last =
        (seam_last + 1..elements.len()).find(|&i| elements[i].kind() == SyntaxKind::PARAGRAPH)?;
    if elements[first + 1..last]
        .iter()
        .any(|element| !matches!(element.kind(), SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE))
    {
        return None;
    }

    let range = rowan::TextRange::new(
        elements[first].text_range().start(),
        elements[last].text_range().end(),
    );
    let start = usize::from(range.start());
    let end = usize::from(range.end());
    let old = base.text.get(start..end)?;
    let local = Edit {
        range: edit.range.start.checked_sub(start)?..edit.range.end.checked_sub(start)?,
        insert: edit.insert.clone(),
    };
    let new = local.apply(old);

    let old_green: Vec<GreenElement> = base
        .green
        .children()
        .skip(first)
        .take(last - first + 1)
        .map(|element| element.to_owned())
        .collect();
    let expected = GreenNode::new(SyntaxKind::ROOT.into(), old_green);
    let old_parsed = parse_fragment_with_ctx(old, base.ctx, base.config, base.implicit_expl);
    if old_parsed.green != expected || !old_parsed.errors.is_empty() {
        return None;
    }

    let parsed = parse_fragment_with_ctx(&new, base.ctx, base.config, base.implicit_expl);
    let errors = replaced_errors(base.errors, range, &parsed.errors, edit)?;
    let replacement: Vec<GreenElement> = parsed
        .green
        .children()
        .map(|element| element.to_owned())
        .collect();
    let mut children =
        Vec::with_capacity(base.green.children().len() - (last - first + 1) + replacement.len());
    children.extend(
        base.green
            .children()
            .take(first)
            .map(|element| element.to_owned()),
    );
    children.extend(replacement);
    children.extend(
        base.green
            .children()
            .skip(last + 1)
            .map(|element| element.to_owned()),
    );
    let green = GreenNode::new(SyntaxKind::ROOT.into(), children);
    finish(green, errors, ReparseTier::Region, base, new_text)
}

fn replaced_errors(
    errors: &[SyntaxError],
    old_range: rowan::TextRange,
    replacement: &[SyntaxError],
    edit: &Edit,
) -> Option<Vec<SyntaxError>> {
    let start = usize::from(old_range.start());
    let end = usize::from(old_range.end());
    let mut out = Vec::with_capacity(errors.len() + replacement.len());
    for error in errors {
        if error.end <= start {
            out.push(error.clone());
        } else if error.start < end {
            return None;
        }
    }
    out.extend(replacement.iter().map(|error| SyntaxError {
        message: error.message.clone(),
        start: error.start + start,
        end: error.end + start,
    }));
    for error in errors.iter().filter(|error| error.start >= end) {
        out.push(SyntaxError {
            message: error.message.clone(),
            start: error.start.checked_add_signed(edit.delta())?,
            end: error.end.checked_add_signed(edit.delta())?,
        });
    }
    Some(out)
}

fn only_paragraph(root: &SyntaxNode) -> Option<SyntaxNode> {
    let mut elements = root.children_with_tokens();
    let paragraph = elements.next()?.into_node()?;
    (paragraph.kind() == SyntaxKind::PARAGRAPH && elements.next().is_none()).then_some(paragraph)
}

fn inert_insert(text: &str) -> bool {
    !text.chars().any(|c| {
        matches!(
            c,
            '\\' | '{' | '}' | '[' | ']' | '$' | '&' | '#' | '^' | '_' | '%' | '~' | ':' | '@'
        )
    })
}

#[cfg(test)]
mod tests {
    use crate::declarations::ResolvedDeclarations;
    use crate::parser::core::parse_with_declarations_resolved;
    use crate::parser::lexer::LatexFlavor;

    use super::*;

    fn assert_region(text: &str, range: std::ops::Range<usize>, insert: &str) {
        let declared = ResolvedDeclarations::default();
        let (parse, ctx) = parse_with_declarations_resolved(text, LatexFlavor::Document, &declared);
        let base = ReparseBase::from_parts(
            text,
            &parse.green,
            &parse.errors,
            &ctx,
            LatexFlavor::Document.into(),
            &declared,
        );
        let edit = Edit {
            range,
            insert: insert.into(),
        };
        let new_text = edit.apply(text);
        let out = reparse_region(&base, &edit, &new_text).expect("region should splice");
        assert_eq!(out.tier, ReparseTier::Region);
    }

    #[test]
    fn replaces_a_multi_token_span_in_one_paragraph() {
        assert_region(
            "alpha beta gamma.\n\nnext paragraph.\n",
            6..16,
            "better words",
        );
    }

    #[test]
    fn handles_crlf_around_the_paragraph_without_inspecting_literal_seams() {
        assert_region(
            "alpha beta gamma.\r\n\r\nnext paragraph.\r\n",
            6..16,
            "better words",
        );
    }

    #[test]
    fn unchanged_commands_may_share_the_paragraph() {
        assert_region(
            "alpha beta and \\emph{nested words} after gamma delta.\n",
            6..14,
            "better prose",
        );
    }

    #[test]
    fn deleting_a_blank_line_merges_adjacent_paragraphs() {
        assert_region("alpha beta.\n\ngamma delta.\n", 11..13, " ");
    }

    #[test]
    fn refuses_stateful_or_structural_text() {
        let declared = ResolvedDeclarations::default();
        let text = "alpha \\emph{beta} gamma.\n";
        let (parse, ctx) = parse_with_declarations_resolved(text, LatexFlavor::Document, &declared);
        let base = ReparseBase::from_parts(
            text,
            &parse.green,
            &parse.errors,
            &ctx,
            LatexFlavor::Document.into(),
            &declared,
        );
        let edit = Edit {
            range: 3..20,
            insert: "plain".into(),
        };
        assert!(reparse_region(&base, &edit, &edit.apply(text)).is_none());
    }

    #[test]
    fn faithfulness_refuses_a_paragraph_whose_entry_mode_is_not_reproduced() {
        let declared = ResolvedDeclarations::default();
        let text = "\\ExplSyntaxOn\n\nalpha_beta gamma delta.\n";
        let (parse, ctx) = parse_with_declarations_resolved(text, LatexFlavor::Document, &declared);
        let base = ReparseBase::from_parts(
            text,
            &parse.green,
            &parse.errors,
            &ctx,
            LatexFlavor::Document.into(),
            &declared,
        );
        let edit = Edit {
            range: 26..37,
            insert: "better prose".into(),
        };
        assert!(reparse_region(&base, &edit, &edit.apply(text)).is_none());
    }
}
