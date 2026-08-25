//! Delimiter-bearing math-fragment reparse.
//!
//! The token tier handles edits whose existing virtual-atom partition remains
//! valid. This tier handles the local shape changes it refuses by reparsing the
//! outermost enclosing math construct that carries the delimiters establishing
//! math mode: `INLINE_MATH`, `DISPLAY_MATH`, or a math `ENVIRONMENT`.
//!
//! The old fragment must reproduce itself when parsed in isolation under the
//! base parse's exact context. The edit may then change only state-neutral math
//! syntax; control sequences, comments, environment names, definitions, and dtx
//! are refused. Finally, the edited fragment must still parse as one same-kind
//! container spanning all of its bytes. Together, those checks prove the edit
//! cannot change lexing or grammar decisions outside the replaced node.

use rowan::{TextRange, TextSize};

use crate::parser::core::parse_fragment_with_ctx;
use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

use super::leaf::context_admits;
use super::region::replaced_errors;
use super::{Edit, ReparseBase, ReparseTier, Reparsed, finish};

/// Reparse the outermost delimiter-bearing math container holding `edit`.
pub(super) fn reparse_math(
    base: &ReparseBase<'_>,
    edit: &Edit,
    new_text: &str,
) -> Option<Reparsed> {
    // Docstrip's line/column modes are whole-file state. A delimiter-bearing
    // fragment is not enough to reproduce them, even when its old bytes happen to
    // parse faithfully in isolation.
    if base.config.dtx || !errors_are_ordered(base.errors) || !surface_is_local(base, edit) {
        return None;
    }

    let edit_range = TextRange::new(
        TextSize::try_from(edit.range.start).ok()?,
        TextSize::try_from(edit.range.end).ok()?,
    );
    let root = base.syntax();
    let target = math_target(&root, edit_range)?;
    let body = target
        .children()
        .find(|node| node.kind() == SyntaxKind::MATH)?;
    if !body.text_range().contains_range(edit_range) {
        return None;
    }
    if !position_is_local(&target, edit_range) {
        return None;
    }

    let target_range = target.text_range();
    let start = usize::from(target_range.start());
    let end = usize::from(target_range.end());
    let old = base.text.get(start..end)?;
    let local = Edit {
        range: edit.range.start.checked_sub(start)?..edit.range.end.checked_sub(start)?,
        insert: edit.insert.clone(),
    };
    let new = local.apply(old);

    // Faithfulness proves that the target does not depend on lexer or parser
    // entry state absent from the base context supplied to the fragment parse.
    let old_parsed = parse_fragment_with_ctx(old, base.ctx, base.config, base.implicit_expl);
    if !old_parsed.errors.is_empty() {
        return None;
    }
    let old_target = full_span_container(&old_parsed.syntax(), target.kind(), old.len())?;
    if old_target.green() != target.green() {
        return None;
    }

    let parsed = parse_fragment_with_ctx(&new, base.ctx, base.config, base.implicit_expl);
    let replacement = full_span_container(&parsed.syntax(), target.kind(), new.len())?;
    if !right_boundary_holds(base, &target, &new, &replacement) {
        return None;
    }
    let errors = replaced_errors(base.errors, target_range, &parsed.errors, edit)?;
    let green = target.replace_with(replacement.green().to_owned());
    finish(green, errors, ReparseTier::Math, base, new_text)
}

/// Choose the outermost delimiter-bearing math node that contains the edit.
///
/// An edit inside a nested math environment can change a gate opened by an outer
/// `\[`/`$` (an unmatched `}` is the minimal counterexample). Replacing only the
/// inner environment would preserve a node the full parse demotes. Taking the
/// enclosing closure makes every affected math gate part of the faithfulness and
/// full-span checks.
fn math_target(root: &SyntaxNode, edit: TextRange) -> Option<SyntaxNode> {
    let starts: Vec<SyntaxNode> = if edit.is_empty() {
        root.token_at_offset(edit.start())
            .filter_map(|token| token.parent())
            .collect()
    } else {
        match root.covering_element(edit) {
            rowan::NodeOrToken::Node(node) => vec![node],
            rowan::NodeOrToken::Token(token) => token.parent().into_iter().collect(),
        }
    };

    starts
        .into_iter()
        .flat_map(|node| node.ancestors())
        .filter(is_math_container)
        .filter(|node| node.text_range().contains_range(edit))
        .max_by_key(|node| node.text_range().len())
}

fn is_math_container(node: &SyntaxNode) -> bool {
    match node.kind() {
        SyntaxKind::INLINE_MATH | SyntaxKind::DISPLAY_MATH => true,
        SyntaxKind::ENVIRONMENT => node
            .children()
            .any(|child| child.kind() == SyntaxKind::MATH),
        _ => false,
    }
}

/// Find the one same-kind container that spans an isolated fragment exactly.
fn full_span_container(root: &SyntaxNode, kind: SyntaxKind, len: usize) -> Option<SyntaxNode> {
    let end = TextSize::try_from(len).ok()?;
    let mut matches = root.descendants().filter(|node| {
        node.kind() == kind && node.text_range() == TextRange::new(TextSize::from(0), end)
    });
    let node = matches.next()?;
    matches.next().is_none().then_some(node)
}

/// Prove malformed local structure cannot make the replacement consume an
/// unchanged token to its right. A closing delimiter normally makes this
/// immediate; the probe matters for recovery shapes such as an inserted `{` in a
/// math environment, where an isolated parse can end honestly at EOF while the
/// full parse continues the open group past the old environment boundary.
fn right_boundary_holds(
    base: &ReparseBase<'_>,
    target: &SyntaxNode,
    replacement_text: &str,
    replacement: &SyntaxNode,
) -> bool {
    const MAX_BOUNDARY_BYTES: usize = 1024;

    let Some(next) = target.last_token().and_then(|token| token.next_token()) else {
        return true;
    };
    if next.text().len() > MAX_BOUNDARY_BYTES {
        return false;
    }
    let mut probe = String::with_capacity(replacement_text.len() + next.text().len());
    probe.push_str(replacement_text);
    probe.push_str(next.text());
    let parsed = parse_fragment_with_ctx(&probe, base.ctx, base.config, base.implicit_expl);
    parsed.syntax().descendants().any(|node| {
        node.kind() == target.kind()
            && usize::from(node.text_range().start()) == 0
            && usize::from(node.text_range().end()) == replacement_text.len()
            && node.green() == replacement.green()
    })
}

/// The changed bytes may not introduce or remove lexer-stateful spelling.
fn surface_is_local(base: &ReparseBase<'_>, edit: &Edit) -> bool {
    let removed = &base.text[edit.range.clone()];
    !edit
        .insert
        .chars()
        .chain(removed.chars())
        .any(|character| matches!(character, '\\' | '%' | '@' | ':'))
}

/// Reject touched or boundary-adjacent tokens whose spelling has non-local
/// meaning. Empty edits inspect both boundary candidates because adding a letter
/// after a control word would rename that control word.
fn position_is_local(root: &SyntaxNode, edit: TextRange) -> bool {
    let tokens: Vec<SyntaxToken> = if edit.is_empty() {
        root.token_at_offset(edit.start()).collect()
    } else {
        root.descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .filter(|token| ranges_overlap(token.text_range(), edit))
            .collect()
    };

    tokens.iter().all(|token| {
        matches!(
            token.kind(),
            SyntaxKind::WORD
                | SyntaxKind::WHITESPACE
                | SyntaxKind::NEWLINE
                | SyntaxKind::L_BRACE
                | SyntaxKind::R_BRACE
                | SyntaxKind::L_BRACKET
                | SyntaxKind::R_BRACKET
                | SyntaxKind::DOLLAR
                | SyntaxKind::AMPERSAND
                | SyntaxKind::HASH
                | SyntaxKind::CARET
                | SyntaxKind::UNDERSCORE
                | SyntaxKind::TILDE
        ) && context_admits(token, token).is_some()
    })
}

fn ranges_overlap(left: TextRange, right: TextRange) -> bool {
    left.start() < right.end() && left.end() > right.start()
}

/// Diagnostic splicing preserves vector order. Some deeply malformed parses emit
/// recovery diagnostics in stack order rather than source order; an otherwise
/// local edit can reorder that stack during a full parse, so those bases must fall
/// back rather than inheriting an order the fragment cannot reproduce.
fn errors_are_ordered(errors: &[crate::parser::core::SyntaxError]) -> bool {
    errors.windows(2).all(|pair| pair[0].start < pair[1].start)
}

#[cfg(test)]
mod tests {
    use crate::declarations::ResolvedDeclarations;
    use crate::parser::core::parse_with_declarations_resolved;
    use crate::parser::lexer::{LatexFlavor, LexConfig};

    use super::*;

    fn with_base<R>(text: &str, f: impl FnOnce(&ReparseBase<'_>) -> R) -> R {
        with_config(text, LatexFlavor::Document.into(), f)
    }

    fn with_config<R>(text: &str, config: LexConfig, f: impl FnOnce(&ReparseBase<'_>) -> R) -> R {
        let declared = ResolvedDeclarations::default();
        let (parse, ctx) = parse_with_declarations_resolved(text, config, &declared);
        f(&ReparseBase::from_parts(
            text,
            &parse.green,
            &parse.errors,
            &ctx,
            config,
            &declared,
        ))
    }

    fn edit(range: std::ops::Range<usize>, insert: &str) -> Edit {
        Edit {
            range,
            insert: insert.to_owned(),
        }
    }

    fn insert_after(text: &str, needle: &str, insert: &str) -> Edit {
        let at = text.find(needle).expect("fixture") + needle.len();
        edit(at..at, insert)
    }

    fn assert_math(text: &str, edit: Edit) {
        with_base(text, |base| {
            let out = reparse_math(base, &edit, &edit.apply(text)).expect("expected math splice");
            assert_eq!(out.tier, ReparseTier::Math);
        });
    }

    fn assert_refuses(text: &str, edit: Edit) {
        with_base(text, |base| {
            assert!(reparse_math(base, &edit, &edit.apply(text)).is_none());
        });
    }

    #[test]
    fn reparses_state_neutral_local_math_syntax() {
        assert_math(
            "before $ab$ after\n",
            insert_after("before $ab$ after\n", "a", "{x}"),
        );
        assert_math(
            "before $a b$ after\n",
            insert_after("before $a b$ after\n", "a", "\n"),
        );
        assert_math(
            "before \\[ab\\] after\n",
            insert_after("before \\[ab\\] after\n", "a", "$x$"),
        );
        assert_math(
            "before $ab$ after\n",
            insert_after("before $ab$ after\n", "a", "^2_3"),
        );
    }

    #[test]
    fn replaces_local_diagnostics_and_shifts_the_suffix() {
        let text = "$x$\n\n\\begin{itemize}\n";
        with_base(text, |base| {
            assert_eq!(base.errors.len(), 1, "fixture has one suffix error");
            let edit = insert_after(text, "x", "^");
            let out = reparse_math(base, &edit, &edit.apply(text)).expect("expected math splice");
            assert_eq!(out.errors.len(), 2);
            assert_eq!(out.errors[0].message, "missing argument after `^`/`_`");
            assert_eq!(out.errors[1].start, base.errors[0].start + 1);
        });
    }

    #[test]
    fn refuses_edits_that_can_escape_or_change_lexer_state() {
        let text = "before $ab$ after\n";
        assert_refuses(text, insert_after(text, "a", "{"));
        assert_refuses(text, insert_after(text, "a", "\\foo"));
        assert_refuses(text, insert_after(text, "a", "% note"));
        assert_refuses(text, edit(7..8, "")); // opening `$`

        let nested = "\\[\n\\begin{matrix}\n0_a\n\\end{matrix}\n\\]\n";
        assert_refuses(nested, insert_after(nested, "a", "}"));
    }

    #[test]
    fn refuses_environment_names_and_definition_bodies() {
        let environment = "\\begin{align}\nx+y\n\\end{align}\n";
        assert_refuses(environment, edit(8..9, "z"));

        let definition = "$\\newcommand{\\foo}{bar} + x$\n";
        let start = definition.find("bar").expect("fixture");
        assert_refuses(definition, edit(start..start + 1, "z"));
    }

    #[test]
    fn refuses_dtx_even_when_the_fragment_is_otherwise_faithful() {
        let config = LexConfig {
            flavor: LatexFlavor::Document,
            dtx: true,
        };
        let text = "% $x^23_i$\n";
        with_config(text, config, |base| {
            let edit = insert_after(text, "3", "z");
            assert!(reparse_math(base, &edit, &edit.apply(text)).is_none());
        });
    }
}
