//! The TikZ/pgf statement *unit model*: which gaps inside a picture-body
//! `STATEMENT` are unit-internal and must never become width-break
//! opportunities.
//!
//! The parser's `STATEMENT` node owns a statement's *extent* (everything up to
//! the top-level `;`), which is all the boundary layout and the continuation
//! hang need. What extent cannot give is good breaks *inside* a statement: the
//! interior is an undifferentiated atom stream, so a width fill would happily
//! break a coordinate from its operation (`\draw (6,6)` / `circle (3);`) or an
//! `at` from its coordinate. These are vocabulary-dependent relationships, so
//! they belong in the semantic layer rather than the grammar. A missed or
//! incorrect relationship can affect line breaking, but never the syntax tree.
//!
//! The model is deliberately a **glue map, not a grammar**: for each authored
//! gap between a statement's top-level elements, one verdict — unit-internal
//! (render as a single space, never break) or neutral (an ordinary break
//! opportunity). Everything unrecognized is neutral. All
//! reads are non-trivia token text, so the verdicts are Tier 1 by
//! construction: no trivia predicate is consulted, and a width wrap re-derives
//! the same units on every pass.

use crate::syntax::{SyntaxElement, SyntaxKind};

/// Path operators: a break lands *before* one (the idiomatic continuation
/// lead), and the operator binds **forward** to whatever it connects.
fn is_path_operator(text: &str) -> bool {
    matches!(text, "--" | "|-" | "-|" | "..")
}

/// Path operation and connective keywords that bind **forward** to their
/// argument (`circle (1)`, `node {A}`, `to [out=90]`, `controls (a)`), and
/// which a preceding coordinate binds **to** (`(6,6) circle` never splits).
///
/// Curated, and deliberately small: these are pgf's core path vocabulary, the
/// words whose split from their neighbors reads as broken TikZ. A library verb
/// not listed here degrades to a neutral gap — today's layout — which is the
/// admission bargain of keeping the model semantic-side.
fn is_operation_keyword(text: &str) -> bool {
    matches!(
        text,
        "circle"
            | "rectangle"
            | "ellipse"
            | "arc"
            | "grid"
            | "parabola"
            | "sin"
            | "cos"
            | "plot"
            | "coordinate"
            | "node"
            | "pic"
            | "edge"
            | "to"
            | "controls"
            | "and"
    )
}

/// A coordinate-shaped word: `(0,0)`, `(a.north)`, `(3);` (the terminator rides
/// the word), the relative forms `+(1,0)` / `++(1,0)`, or a coordinate's
/// closing tail — a word ending in `)`, which is how a multi-token coordinate
/// (`(\point)`, where `\point` lexes as its own `CONTROL_WORD`) presents its
/// last element. A cheap shape test, not a coordinate parser — it exists only
/// to decide whether a following operation keyword belongs to this word, so
/// only the *preceding* side of a gap ever consults it.
fn is_coordinate_shaped(text: &str) -> bool {
    let bare = text
        .strip_prefix("++")
        .or_else(|| text.strip_prefix('+'))
        .unwrap_or(text);
    bare.starts_with('(') || bare.ends_with(')')
}

/// What the glue rules need to know about one non-trivia element.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UnitPart {
    /// `--`, `|-`, `-|`, `..` — binds forward.
    Operator,
    /// `at` — binds both sides (`\node (D) at (0,0)` never splits around it).
    At,
    /// A curated operation/connective keyword — binds forward, and a
    /// coordinate before it binds to it.
    Operation,
    /// A coordinate-shaped `WORD`.
    Coordinate,
    /// A `%` comment: never glued on either side — a comment must end its
    /// line, so a glue verdict across one would be a promise the layout
    /// cannot keep.
    Comment,
    /// Everything else: commands, groups, brackets, ordinary words.
    Other,
}

fn classify(element: &SyntaxElement) -> UnitPart {
    match element {
        SyntaxElement::Token(token) => match token.kind() {
            SyntaxKind::COMMENT => UnitPart::Comment,
            SyntaxKind::WORD => {
                let text = token.text();
                if is_path_operator(text) {
                    UnitPart::Operator
                } else if text == "at" {
                    UnitPart::At
                } else if is_operation_keyword(text) {
                    UnitPart::Operation
                } else if is_coordinate_shaped(text) {
                    UnitPart::Coordinate
                } else {
                    UnitPart::Other
                }
            }
            _ => UnitPart::Other,
        },
        SyntaxElement::Node(_) => UnitPart::Other,
    }
}

/// Per-element glue verdicts for a `STATEMENT`'s top-level element stream.
///
/// `glue_before[i]` is `true` when the authored gap immediately before
/// `elements[i]` is unit-internal: the formatter renders it as a single space
/// and never breaks there. Entries for trivia elements, and for elements with
/// no gap before them (glued in the source — adjacency already forms one
/// atom), are `false` and meaningless.
///
/// The rules, each backed by the corpus survey:
///
/// - a path **operator binds forward** (`-- (1,1)` is one unit; the break
///   point is *before* the operator);
/// - **`at` binds both sides** (`at` is essentially never split from its
///   coordinate in the wild);
/// - an **operation keyword binds forward** to its argument
///   (`circle (1)`, `node {A}`, `controls (a)`);
/// - a **coordinate binds to a following operation keyword**
///   (`(6,6) circle (3)` — the split the model exists to forbid);
/// - inside a **loose bracket run** (a statement-level `[`…`]`, an options
///   list) a gap glues unless it follows a comma: `edge [loop above]` never
///   splits an option mid-phrase, while a long keyval run still breaks at its
///   entry boundaries — the comma convention every keyval layout here uses.
///
/// A comment on either side of a gap suppresses every rule.
pub fn statement_glue(elements: &[SyntaxElement]) -> Vec<bool> {
    let mut glue = vec![false; elements.len()];
    let mut prev: Option<UnitPart> = None;
    let mut prev_ends_comma = false;
    let mut saw_gap = false;
    let mut bracket_depth = 0usize;
    for (idx, element) in elements.iter().enumerate() {
        if matches!(element.kind(), SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE) {
            saw_gap = true;
            continue;
        }
        let part = classify(element);
        if let Some(prev) = prev
            && saw_gap
            && prev != UnitPart::Comment
            && part != UnitPart::Comment
        {
            glue[idx] = (bracket_depth > 0 && !prev_ends_comma)
                || matches!(
                    prev,
                    UnitPart::Operator | UnitPart::At | UnitPart::Operation
                )
                || part == UnitPart::At
                || (prev == UnitPart::Coordinate && part == UnitPart::Operation);
        }
        match element.kind() {
            SyntaxKind::L_BRACKET => bracket_depth += 1,
            SyntaxKind::R_BRACKET => bracket_depth = bracket_depth.saturating_sub(1),
            _ => {}
        }
        prev_ends_comma = element
            .as_token()
            .is_some_and(|token| token.text().ends_with(','));
        prev = Some(part);
        saw_gap = false;
    }
    glue
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::syntax::SyntaxNode;

    /// The statement's element stream, with each glued gap rendered as `·`
    /// and each neutral gap as `|` — a direct projection of the verdicts.
    fn units(picture_body: &str) -> String {
        let input = format!("\\begin{{tikzpicture}}\n{picture_body}\n\\end{{tikzpicture}}\n");
        let parsed = parse(&input);
        assert_eq!(parsed.syntax().to_string(), input, "losslessness");
        let stmt: SyntaxNode = parsed
            .syntax()
            .descendants()
            .find(|n| n.kind() == SyntaxKind::STATEMENT)
            .expect("a STATEMENT node");
        let elements: Vec<SyntaxElement> = stmt.children_with_tokens().collect();
        let glue = statement_glue(&elements);
        let mut out = String::new();
        let mut pending_gap = false;
        for (idx, element) in elements.iter().enumerate() {
            if matches!(element.kind(), SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE) {
                pending_gap = true;
                continue;
            }
            if pending_gap {
                out.push(if glue[idx] { '·' } else { '|' });
                pending_gap = false;
            }
            out.push_str(&element.to_string().replace('\n', "␤"));
        }
        out
    }

    #[test]
    fn an_operator_binds_forward_and_a_break_lands_before_it() {
        assert_eq!(
            units(r"\draw (0,0) -- (1,1) -- cycle;"),
            r"\draw|(0,0)|--·(1,1)|--·cycle;"
        );
    }

    #[test]
    fn a_coordinate_binds_its_operation_and_the_operation_its_argument() {
        assert_eq!(
            units(r"\draw (6,6) circle (3);"),
            r"\draw|(6,6)·circle·(3);"
        );
    }

    #[test]
    fn at_binds_both_sides() {
        assert_eq!(
            units(r"\node (D) at (0,0) {A};"),
            r"\node|(D)·at·(0,0)|{A};"
        );
    }

    #[test]
    fn a_mid_path_node_and_its_label_are_one_unit() {
        assert_eq!(
            units(r"\draw (0,0) -- (2,2) node {above};"),
            r"\draw|(0,0)|--·(2,2)·node·{above};"
        );
    }

    #[test]
    fn a_controls_clause_chains_and_the_break_stays_before_the_operator() {
        assert_eq!(
            units(r"\draw (0,0) .. controls (1,1) and (2,0) .. (3,0);"),
            r"\draw|(0,0)|..·controls·(1,1)·and·(2,0)|..·(3,0);"
        );
    }

    #[test]
    fn relative_coordinates_are_coordinate_shaped() {
        assert_eq!(
            units(r"\draw (0,0) -- ++(1,0) circle (2pt);"),
            r"\draw|(0,0)|--·++(1,0)·circle·(2pt);"
        );
    }

    #[test]
    fn a_comment_suppresses_glue_on_both_sides() {
        // A comment must end its line, so no unit may claim to span one.
        assert_eq!(
            units("\\draw (0,0) -- % note\n(1,1);"),
            r"\draw|(0,0)|--|% note|(1,1);"
        );
    }

    #[test]
    fn unrecognized_vocabulary_stays_neutral() {
        // An axis-body statement with prose-ish words: no rule fires except
        // the curated `and`, and a wrong guess is only a bigger atom.
        assert_eq!(
            units(r"\legend{a} extra words here;"),
            r"\legend{a}|extra|words|here;"
        );
    }

    #[test]
    fn an_options_bracket_run_is_one_unit() {
        // `[loop above]` is an options list: a break inside it splits an
        // option mid-phrase (vassar.tex's automaton `edge [loop above]`).
        assert_eq!(
            units(r"\path (A) edge [loop above, red] node {x} (B);"),
            r"\path|(A)·edge·[loop·above,|red]|node·{x}|(B);"
        );
    }

    #[test]
    fn a_multi_token_coordinate_tail_still_binds_its_operation() {
        // `(\point)` lexes as three tokens; the closing `)` word is the
        // coordinate's tail, and `circle` still belongs to it (Euclid's
        // `\fill [black] (\point) circle [radius=2pt];`).
        assert_eq!(
            units(r"\fill (\point) circle [radius=2pt];"),
            r"\fill|(\point)·circle·[radius=2pt];"
        );
    }

    #[test]
    fn a_source_glued_pair_needs_no_verdict() {
        // `(0,0)--(1,1)` lexes as one WORD: no gap, nothing to decide.
        assert_eq!(units(r"\draw (0,0)--(1,1);"), r"\draw|(0,0)--(1,1);");
    }
}
