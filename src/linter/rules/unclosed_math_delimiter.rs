//! `unclosed-math-delimiter`: a math opener the parser's shape gates *silently
//! demoted* because no closer was reachable — a `$` with no matching `$`, a `\[`
//! or `\(` with no `\]`/`\)`, or a `\left` with no `\right` — flagged as a likely
//! authoring typo.
//!
//! The parser treats input as generic TeX surface syntax, so an opener with no
//! reachable closer is *not* an error: macro code routinely passes bare
//! delimiters around as data (`>{$}` array columns, `\expandafter\@tempa\[\@nil`,
//! `\char_set_catcode_letter:N \)`). Its shape gates (`dollar_closes`,
//! `delim_math_closes`, `left_right_closes` in `grammar.rs`) therefore demote such
//! an opener to a plain token with **no diagnostic** (the formatter gates on any
//! parser diagnostic, so an Error there would re-block the file). In *prose*,
//! though, an unclosed opener is almost always a dropped closer. This rule
//! re-derives that demotion from the CST shape (a `$`/`\[`/`\(` not wrapped in its
//! math node, a `\left` that is a `COMMAND` rather than a `LEFT_RIGHT` marker) and
//! reports it — the RA-faithful split, where the untiered parser tolerates and the
//! linter judges (see `TODO.md`).
//!
//! **Context heuristic (prose vs. macro-code data).** The same shape is data in
//! macro code and a typo in prose, and the two are genuinely indistinguishable in
//! the general case (`>{$}` vs a dropped `$`), so the rule is conservative — it
//! reports only a delimiter in *document prose*, staying silent when the opener
//! sits in any of the macro-code contexts the parser gate exists for:
//!
//! - inside a brace group / optional / argument (`\newcommand{…}{$}`, `\def\x{$}`,
//!   the array `>{$}` column spec, any macro-argument data);
//! - inside an expl3 code region (shared with the formatter's `expl3_regions`);
//! - inside a `macrocode`/`macrocode*` body (a `.dtx` code chunk).
//!
//! This trades recall for precision: a genuine typo *inside* a group goes
//! unreported (a false negative), which the tenets prefer over warning on the
//! `>{$}` that is the whole reason the gate is silent. No autofix: the fix could
//! be inserting a closer anywhere, or deleting a stray opener — ambiguous by
//! nature (mirrors `mismatched-delimiter`).

use std::path::PathBuf;

use crate::ast::{command_name, control_word_range, environment_name};
use crate::linter::diagnostic::{Diagnostic, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[
    Example {
        caption: "An inline-math `$` with no matching `$`:",
        source: "Let $x = 1 be the base case.\n",
    },
    Example {
        caption: "A display-math `\\[` with no matching `\\]`:",
        source: "The bound \\[ x + y follows immediately.\n",
    },
    Example {
        caption: "A `\\left` with no matching `\\right`:",
        source: "$a + \\left( b + c$\n",
    },
];

pub struct UnclosedMathDelimiter;

impl Rule for UnclosedMathDelimiter {
    fn id(&self) -> &'static str {
        "unclosed-math-delimiter"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a math opener the parser silently demoted to a plain token because no \
         closer was reachable -- a `$` with no matching `$`, a `\\[`/`\\(` with no \
         `\\]`/`\\)`, or a `\\left` with no `\\right`. Such a shape is routine data \
         in macro code (`>{$}` array columns, `\\expandafter\\@tempa\\[\\@nil`), so \
         the parser tolerates it without a diagnostic; in prose it is almost always \
         a dropped closer. To stay clear of the macro-code cases the rule is \
         conservative: it reports only an opener in document prose, staying silent \
         when it sits inside a brace group or optional argument (`\\newcommand{...}{$}`, \
         the `>{$}` column spec), an expl3 region, or a `macrocode` body. No autofix: \
         the correction (insert a closer, or delete a stray opener) is ambiguous."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[
            SyntaxKind::DOLLAR,
            SyntaxKind::CONTROL_SYMBOL,
            SyntaxKind::COMMAND,
        ]
    }

    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        match el {
            // A `$` not wrapped in its `INLINE_MATH`/`DISPLAY_MATH` node was
            // demoted (`dollar_closes` found no reachable closer).
            SyntaxElement::Token(tok) if tok.kind() == SyntaxKind::DOLLAR => {
                let Some(parent) = tok.parent() else { return };
                if is_math_wrapper(parent.kind()) {
                    return; // a balanced delimiter of real math
                }
                let range = tok.text_range();
                let offset = usize::from(range.start());
                if suppressed(&parent, ctx, offset) {
                    return;
                }
                push(
                    self,
                    sink,
                    range,
                    "`$` has no matching `$` (unclosed inline math)",
                );
            }
            // A `\[` / `\(` not wrapped in its math node was demoted
            // (`delim_math_closes` found no reachable `\]`/`\)`).
            SyntaxElement::Token(tok) if tok.kind() == SyntaxKind::CONTROL_SYMBOL => {
                let (closer, kind) = match tok.text() {
                    "\\[" => ("\\]", "display"),
                    "\\(" => ("\\)", "inline"),
                    _ => return,
                };
                let Some(parent) = tok.parent() else { return };
                if is_math_wrapper(parent.kind()) {
                    return;
                }
                let range = tok.text_range();
                let offset = usize::from(range.start());
                if suppressed(&parent, ctx, offset) {
                    return;
                }
                push(
                    self,
                    sink,
                    range,
                    &format!(
                        "`{}` has no matching `{closer}` (unclosed {kind} math)",
                        tok.text()
                    ),
                );
            }
            // A demoted `\left` is a plain `COMMAND` (a balanced one is a bare
            // marker token under a `LEFT_RIGHT`). Gate on `in_math`: only a
            // `\left` the gate demoted *inside* real math is a dropped `\right`;
            // a bare `\left` in text is a different (non-delimiter) shape.
            SyntaxElement::Node(node) if node.kind() == SyntaxKind::COMMAND => {
                if command_name(node).as_deref() != Some("left") {
                    return;
                }
                let range = control_word_range(node).unwrap_or_else(|| node.text_range());
                let offset = usize::from(range.start());
                if !ctx.in_math(offset) || suppressed(node, ctx, offset) {
                    return;
                }
                push(self, sink, range, "`\\left` has no matching `\\right`");
            }
            _ => {}
        }
    }
}

/// A node kind that wraps a *balanced* math delimiter: a demoted opener is never a
/// direct child of one, so this tells a real delimiter from a demoted token.
fn is_math_wrapper(kind: SyntaxKind) -> bool {
    matches!(kind, SyntaxKind::INLINE_MATH | SyntaxKind::DISPLAY_MATH)
}

/// Whether a demoted delimiter at `offset`, whose nearest enclosing node is
/// `start`, sits in a macro-code context the parser's silent demotion exists for —
/// so the rule stays quiet (see the module docs). Mirrors the parser's
/// `in_macro_code` split (definition body + expl3 region) plus the group/argument
/// data cases the two shapes are genuinely ambiguous with.
fn suppressed(start: &SyntaxNode, ctx: &RuleContext<'_>, offset: usize) -> bool {
    if ctx.in_expl3(offset) {
        return true;
    }
    start.ancestors().any(|n| match n.kind() {
        // Any braced argument: definition bodies (`\newcommand{…}{$}`, `\def\x{$}`),
        // the array `>{$}` column spec, and macro-argument data all live here, and
        // none can be told from a typo by shape alone.
        SyntaxKind::GROUP
        | SyntaxKind::OPTIONAL
        | SyntaxKind::ARGUMENT
        | SyntaxKind::NAME_GROUP => true,
        // A `.dtx` `macrocode`/`macrocode*` chunk is package code, not prose.
        SyntaxKind::ENVIRONMENT => is_macrocode_environment(&n),
        _ => false,
    })
}

/// Whether an `ENVIRONMENT` node is a `macrocode`/`macrocode*` chunk.
fn is_macrocode_environment(env: &SyntaxNode) -> bool {
    env.children()
        .find(|c| c.kind() == SyntaxKind::BEGIN)
        .and_then(|begin| environment_name(&begin))
        .is_some_and(|name| name == "macrocode" || name == "macrocode*")
}

fn push(
    rule: &UnclosedMathDelimiter,
    sink: &mut Vec<Diagnostic>,
    range: rowan::TextRange,
    message: &str,
) {
    sink.push(Diagnostic {
        rule: rule.id(),
        severity: rule.default_severity(),
        path: PathBuf::new(),
        start: usize::from(range.start()),
        end: usize::from(range.end()),
        message: message.to_owned(),
        fix: None,
        related: Vec::new(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::lexer::{LatexFlavor, LexConfig};
    use crate::parser::{parse, parse_with_flavor};
    use crate::semantic::SemanticModel;

    fn findings_of(root: &SyntaxNode) -> Vec<Diagnostic> {
        let model = SemanticModel::build(root);
        let ctx = RuleContext::new(
            std::path::Path::new("x.tex"),
            root,
            &model,
            None,
            None,
            None,
        );
        let mut out = Vec::new();
        for el in root.descendants_with_tokens() {
            if UnclosedMathDelimiter.interests().contains(&el.kind()) {
                UnclosedMathDelimiter.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    fn findings(src: &str) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse(src).green);
        findings_of(&root)
    }

    fn findings_dtx(src: &str) -> Vec<Diagnostic> {
        let cfg = LexConfig {
            flavor: LatexFlavor::Package,
            dtx: true,
        };
        let root = SyntaxNode::new_root(parse_with_flavor(src, cfg).green);
        findings_of(&root)
    }

    #[test]
    fn flags_unclosed_dollar_in_prose() {
        let out = findings("Let $x = 1 be the base.\n");
        assert_eq!(out.len(), 1, "got: {out:?}");
        assert_eq!(out[0].rule, "unclosed-math-delimiter");
        assert!(out[0].message.contains('$'), "got: {}", out[0].message);
        // Caret sits on just the `$` (byte 4..5).
        assert_eq!((out[0].start, out[0].end), (4, 5));
    }

    #[test]
    fn flags_unclosed_display_opener() {
        let out = findings("The bound \\[ x + y follows.\n");
        assert_eq!(out.len(), 1, "got: {out:?}");
        assert!(out[0].message.contains("\\["), "got: {}", out[0].message);
        assert!(out[0].message.contains("\\]"), "got: {}", out[0].message);
    }

    #[test]
    fn flags_unclosed_inline_paren_opener() {
        let out = findings("A value \\( x + y here.\n");
        assert_eq!(out.len(), 1, "got: {out:?}");
        assert!(out[0].message.contains("\\("), "got: {}", out[0].message);
    }

    #[test]
    fn flags_unclosed_left_inside_real_math() {
        let out = findings("$a + \\left( b + c$\n");
        assert_eq!(out.len(), 1, "got: {out:?}");
        assert!(out[0].message.contains("\\left"), "got: {}", out[0].message);
        assert!(
            out[0].message.contains("\\right"),
            "got: {}",
            out[0].message
        );
    }

    #[test]
    fn balanced_delimiters_are_fine() {
        assert!(findings("$x$\n").is_empty());
        assert!(findings("\\[ x + y \\]\n").is_empty());
        assert!(findings("\\( x + y \\)\n").is_empty());
        assert!(findings("$\\left( x \\right)$\n").is_empty());
        assert!(findings("$$x$$\n").is_empty());
    }

    #[test]
    fn array_column_dollar_is_not_flagged() {
        // `>{$}` / `<{$}` array column specs demote `$` to data — the canonical
        // false-positive the rule must avoid.
        assert!(findings("\\begin{tabular}{>{$}c<{$}}\na & b\\end{tabular}\n").is_empty());
    }

    #[test]
    fn definition_body_delimiter_is_not_flagged() {
        assert!(findings("\\newcommand{\\x}{$}\n").is_empty());
        assert!(findings("\\def\\x{$}\n").is_empty());
        assert!(findings("\\newcommand{\\x}{\\[}\n").is_empty());
    }

    #[test]
    fn expl3_region_delimiter_is_not_flagged() {
        // A `\[` at top level inside an expl3 region is data, not prose display
        // math — suppressed via the shared `expl3_regions`.
        assert!(findings("\\ExplSyntaxOn\n\\[\n\\ExplSyntaxOff\n").is_empty());
    }

    #[test]
    fn macrocode_body_delimiter_is_not_flagged() {
        // A `\[` inside a `.dtx` `macrocode` chunk is package code.
        let out = findings_dtx("%    \\begin{macrocode}\n\\[\n%    \\end{macrocode}\n");
        assert!(out.is_empty(), "got: {out:?}");
    }

    #[test]
    fn bare_left_in_text_is_not_flagged() {
        // A `\left` outside any math region is a different shape (not a demoted
        // delimiter); the `in_math` gate keeps it out of scope.
        assert!(findings("plain \\left text\n").is_empty());
    }

    #[test]
    fn carries_no_fix() {
        let out = findings("Let $x = 1 be the base.\n");
        assert!(out[0].fix.is_none());
    }
}
