//! `dollar-display-math`: plain-TeX `$$…$$` display math, superseded in LaTeX by
//! `\[…\]`.
//!
//! `$$` is a TeX primitive; in LaTeX it bypasses `amsmath` spacing hooks and
//! breaks `fleqn`/`\everydisplay`, so the LaTeX team and l2tabu steer users to
//! `\[…\]`. The replacement is a pure delimiter swap, so the message names it —
//! the seed of a later (safe) autofix (this slice reports only; see Tenet 5).
//!
//! The parser builds a `DISPLAY_MATH` node for *both* `$$…$$` and `\[…\]`
//! (`grammar.rs`, `dollar_math` vs `delim_math`); the two are told apart by the
//! kind of the opening delimiter token — a `DOLLAR` for the `$$` form.

use std::path::PathBuf;

use rowan::NodeOrToken;

use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use crate::linter::diagnostic::{Diagnostic, Severity};

use super::{Rule, RuleContext};

pub struct DollarDisplayMath;

impl Rule for DollarDisplayMath {
    fn id(&self) -> &'static str {
        "dollar-display-math"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
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
        });
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::semantic::SemanticModel;

    fn findings(src: &str) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse(src).green);
        let model = SemanticModel::build(&root);
        let ctx = RuleContext {
            path: std::path::Path::new("x.tex"),
            root: &root,
            model: &model,
            resolution: None,
        };
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
    fn bracket_display_is_fine() {
        assert!(findings("\\[x = y\\]\n").is_empty());
    }

    #[test]
    fn inline_dollar_is_fine() {
        assert!(findings("$x = y$\n").is_empty());
    }
}
