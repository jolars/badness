//! `dollar-display-math`: plain-TeX `$$…$$` display math, superseded in LaTeX by
//! `\[…\]`.
//!
//! `$$` is a TeX primitive; in LaTeX it bypasses `amsmath` spacing hooks and
//! breaks `fleqn`/`\everydisplay`, so the LaTeX team and l2tabu steer users to
//! `\[…\]`. The replacement is a pure delimiter swap, carried as a `Safe`
//! autofix ([`delimiter_swap_fix`]) that `lint --fix` applies: two edits in one
//! atomic [`Fix`], rewriting only the delimiters and leaving the body bytes
//! untouched, so it is correct by construction (parses + lossless, tenet 1).
//! Withheld when the display math is unclosed.
//!
//! The parser builds a `DISPLAY_MATH` node for *both* `$$…$$` and `\[…\]`
//! (`grammar.rs`, `dollar_math` vs `delim_math`); the two are told apart by the
//! kind of the opening delimiter token — a `DOLLAR` for the `$$` form.

use std::path::PathBuf;

use rowan::NodeOrToken;

use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use crate::linter::diagnostic::{Diagnostic, Edit, Fix, Severity};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "Plain-TeX display math:",
    source: "$$a + b = c$$\n",
}];

pub struct DollarDisplayMath;

impl Rule for DollarDisplayMath {
    fn id(&self) -> &'static str {
        "dollar-display-math"
    }

    fn emits_fix(&self) -> bool {
        true
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag plain-TeX `$$...$$` display math. `$$` is a TeX primitive that \
         bypasses `amsmath` spacing hooks and breaks `fleqn`/`\\everydisplay`, so \
         LaTeX steers users to `\\[...\\]`. The autofix swaps the delimiters in \
         place and leaves the body untouched, so it parses and stays lossless; \
         it is withheld when the display math is unclosed."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::DISPLAY_MATH]
    }

    fn check(&self, el: &SyntaxElement, _ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(math) = el.as_node() else {
            return;
        };
        let Some(range) = opening_dollars_range(math) else {
            // The `\[…\]` form opens with a CONTROL_SYMBOL, not `$$` — fine.
            return;
        };
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start: usize::from(range.start()),
            end: usize::from(range.end()),
            message: "`$$…$$` is plain-TeX display math; use `\\[…\\]`".to_owned(),
            fix: delimiter_swap_fix(math, range),
            related: Vec::new(),
        });
    }
}

/// Build the `$$…$$` → `\[…\]` autofix: one atomic [`Fix`] carrying two edits
/// that swap the opening and closing delimiters in place; the math body is not
/// touched at all. Returns `None` (report-only) when the node has no closing
/// `$$` (unclosed display math / a parse error) — there is no closer to swap.
///
/// Each `$$`→`\[`/`$$`→`\]` is a 2-byte→2-byte glyph swap, so the fix is
/// correct by construction (tenet 1). It is `Safe`: the swap is the
/// almost-always-wanted LaTeX form.
fn delimiter_swap_fix(math: &SyntaxNode, opening: rowan::TextRange) -> Option<Fix> {
    let closing = closing_dollars_range(math)?;
    Some(Fix::safe_edits(
        vec![
            Edit::new(opening.start().into(), opening.end().into(), "\\["),
            Edit::new(closing.start().into(), closing.end().into(), "\\]"),
        ],
        "Replace `$$…$$` with `\\[…\\]`",
    ))
}

/// The range spanning the leading `$$` of a `DISPLAY_MATH` node, or `None` when
/// the node opens with `\[` (a `CONTROL_SYMBOL`) instead. The grammar bumps the
/// two `$` as the node's first two tokens.
fn opening_dollars_range(math: &SyntaxNode) -> Option<rowan::TextRange> {
    let mut dollars = math
        .children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .take_while(|t| t.kind() == SyntaxKind::DOLLAR);
    let first = dollars.next()?;
    let second = dollars.next()?;
    Some(rowan::TextRange::new(
        first.text_range().start(),
        second.text_range().end(),
    ))
}

/// The range spanning the trailing `$$` of a `DISPLAY_MATH` node, or `None` when
/// the node is unclosed (fewer than two trailing `$` tokens). The grammar bumps
/// the closing `$$` as the node's last two tokens, after the `MATH` body.
fn closing_dollars_range(math: &SyntaxNode) -> Option<rowan::TextRange> {
    let mut dollars = math
        .children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .filter(|t| t.kind() == SyntaxKind::DOLLAR);
    // Of the (up to) four `$` tokens, the last two are the closer.
    let trailing: Vec<_> = dollars.by_ref().collect();
    let [.., second_last, last] = trailing.as_slice() else {
        return None;
    };
    // Guard against `$$` with no body and no closer: the opener's two `$` must
    // not be mistaken for the closer. A closed `$$…$$` has four `$` tokens; an
    // unclosed `$$…` has only the two openers.
    if trailing.len() < 4 {
        return None;
    }
    Some(rowan::TextRange::new(
        second_last.text_range().start(),
        last.text_range().end(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::semantic::SemanticModel;

    fn findings(src: &str) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse(src).green);
        let model = SemanticModel::build(&root);
        let ctx = RuleContext::new(
            std::path::Path::new("x.tex"),
            &root,
            &model,
            None,
            None,
            None,
        );
        let mut out = Vec::new();
        for el in root.descendants_with_tokens() {
            if DollarDisplayMath.interests().contains(&el.kind()) {
                DollarDisplayMath.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_dollar_dollar() {
        let out = findings("$$x = y$$\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "dollar-display-math");
        // Caret covers just the opening `$$` (bytes 0..2).
        assert_eq!((out[0].start, out[0].end), (0, 2));
    }

    #[test]
    fn carries_safe_delimiter_swap_fix() {
        use crate::linter::diagnostic::Applicability;
        use crate::linter::fix::apply_fixes;

        let src = "$$x = y$$\n";
        let out = findings(src);
        let fix = out[0].fix.as_ref().expect("should carry a fix");
        assert_eq!(fix.applicability, Applicability::Safe);
        // One atomic fix, two edits: each `$$` is swapped in place and the
        // body bytes are never touched.
        assert_eq!(
            fix.edits,
            vec![Edit::new(0, 2, "\\["), Edit::new(7, 9, "\\]")]
        );
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).output,
            "\\[x = y\\]\n"
        );
    }

    #[test]
    fn unclosed_dollar_dollar_is_not_display_math() {
        // With no reachable closer the dollars are macro-code data, not math
        // delimiters (the parser's `$` shape gate): no `DISPLAY_MATH` node is
        // built, so there is nothing to flag — and nothing to fix.
        assert!(findings("$$x = y\n").is_empty());
    }

    #[test]
    fn unclosed_display_math_with_late_closer_reports_without_a_fix() {
        // A closer reachable only past a lone `$` still parses as `$$` math,
        // but the rule withholds the delimiter swap when the shape is not the
        // clean `$$ … $$` pair — report only (tenet 1).
        let out = findings("$$x = y$ z$$w\n");
        assert!(!out.is_empty());
    }

    #[test]
    fn bracket_display_is_fine() {
        assert!(findings("\\[x = y\\]\n").is_empty());
    }

    #[test]
    fn inline_dollar_is_fine() {
        assert!(findings("$x = y$\n").is_empty());
    }
}
