//! `math-operator-name`: a bare log-like function name (`sin`, `cos`, `log`,
//! `lim`, …) written in math mode without its backslash, so TeX sets it as a
//! run of italic variables (`s`, `i`, `n`) instead of the upright operator with
//! its proper spacing. Mirrors ChkTeX rule 35 ("You should put a `\ ` in front
//! of the function name").
//!
//! LaTeX (and amsmath) define a fixed set of these operators — `\sin`, `\log`,
//! `\lim`, and friends. Writing them bare (`$sin x$`) both looks wrong (italic,
//! and glued to the argument) and reads wrong. The rule flags such a name when it
//! appears at the start of a `WORD` in math mode, ending at a word boundary (the
//! end of the word or a non-letter such as `(`). That catches the two common
//! shapes — `$sin x$` (whole word) and `$sin(x)$` (one glued `WORD`) — while
//! leaving ordinary words that merely *begin* with a function name alone
//! (`since`, `cosine`), and preferring the longest match (`sinh` over `sin`).
//!
//! **Conservative gating.** Two guards keep false positives down:
//!   - Only inside math mode (an ancestor `MATH`); a bare `sin` in text is just
//!     the English word.
//!   - Never inside a `SUBSCRIPT`/`SUPERSCRIPT`, where a name like `max` in
//!     `x_{max}` is almost always a *label* ("the maximum"), not the operator.
//!     The base of `\lim_{n}` is still flagged: `lim` there is the WORD base, not
//!     inside the script.
//!
//! The fix inserts the backslash in front of the matched prefix (`sin` →
//! `\sin`), a single contiguous splice that re-parses and stays lossless (tenet
//! 1): the letters become a `CONTROL_WORD` and any trailing `(x)` is untouched.
//! It is **`Unsafe`**, not Safe: it changes the typeset output (upright glyph and
//! operator spacing), and a bare `sin` is *usually* the operator but occasionally
//! a genuine product `s·i·n`. So `--fix` leaves it alone; `--unsafe-fixes` and the
//! editor code action apply it — the same classification as the sibling
//! `times-variable` rule.
//!
//! The operator table lives here, not in `data/signatures.json`: "this bare name
//! should have been an operator" is a lint judgment, not the structural
//! arity/verbatim fact the signature DB carries (AGENTS.md core decision #2).

use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[
    Example {
        caption: "A bare function name typesets as italic variables:",
        source: "$sin x + cos x = 1$\n",
    },
    Example {
        caption: "It fires through the glued `f(x)` form too:",
        source: "The limit $lim(x)$ diverges.\n",
    },
];

/// The LaTeX/amsmath log-like function operators. Each is defined as an upright
/// `\mathop`; written bare it degrades to italic variables. Sorted longest-first
/// is unnecessary — [`match_operator_prefix`] picks the longest match explicitly.
const OPERATORS: &[&str] = &[
    "arccos", "arcsin", "arctan", "arg", "cos", "cosh", "cot", "coth", "csc", "deg", "det", "dim",
    "exp", "gcd", "hom", "inf", "ker", "lg", "lim", "liminf", "limsup", "ln", "log", "max", "min",
    "Pr", "sec", "sin", "sinh", "sup", "tan", "tanh",
];

pub struct MathOperatorName;

impl Rule for MathOperatorName {
    fn id(&self) -> &'static str {
        "math-operator-name"
    }

    fn emits_fix(&self) -> bool {
        true
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a bare log-like function name (`sin`, `cos`, `log`, `lim`, and the \
         rest of the LaTeX/amsmath set) written in math mode without its \
         backslash, so TeX sets it as italic variables instead of the upright \
         `\\sin` operator with correct spacing (ChkTeX 35). It fires when the name \
         starts a `WORD` and ends at a word boundary, catching both `$sin x$` and \
         the glued `$sin(x)$`, while leaving words that merely begin with one \
         (`since`) alone and preferring the longest match (`sinh` over `sin`). To \
         stay conservative it only fires inside math mode and never inside a \
         subscript or superscript, where `max` in `x_{max}` is almost always a \
         label rather than the operator. The fix inserts the backslash \
         (`sin` -> `\\sin`); it is **unsafe** because it changes the typeset output \
         (upright glyph and operator spacing) and a bare `sin` is occasionally a \
         real product, so `--fix` leaves it alone while `--unsafe-fixes` and the \
         editor code action apply it."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::WORD]
    }

    fn check(&self, el: &SyntaxElement, _ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(tok) = el.as_token() else {
            return;
        };
        // Only in math mode, and never inside a sub/superscript (label position).
        let mut in_math = false;
        for node in tok.parent_ancestors() {
            match node.kind() {
                SyntaxKind::SUBSCRIPT | SyntaxKind::SUPERSCRIPT => return,
                SyntaxKind::MATH => {
                    in_math = true;
                    break;
                }
                _ => {}
            }
        }
        if !in_math {
            return;
        }

        let Some(name) = match_operator_prefix(tok.text()) else {
            return;
        };
        let base = usize::from(tok.text_range().start());
        let start = base;
        let end = base + name.len();
        let content = format!("\\{name}");

        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start,
            end,
            message: format!("bare `{name}` in math typesets as italic variables; use `\\{name}`"),
            fix: Some(Fix::unsafe_(
                start,
                end,
                content,
                format!("Replace `{name}` with `\\{name}`"),
            )),
        });
    }
}

/// The longest operator name that is a prefix of `text` ending at a word boundary
/// (end of `text`, or a following byte that is not an ASCII letter). Returns
/// `None` when no operator matches, keeping ordinary words that merely begin with
/// a function name (`since`, `cosine`) out of scope.
fn match_operator_prefix(text: &str) -> Option<&'static str> {
    let bytes = text.as_bytes();
    OPERATORS
        .iter()
        .copied()
        .filter(|op| {
            let n = op.len();
            bytes.len() >= n
                && &bytes[..n] == op.as_bytes()
                && bytes.get(n).is_none_or(|b| !b.is_ascii_alphabetic())
        })
        .max_by_key(|op| op.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linter::diagnostic::Applicability;
    use crate::linter::fix::apply_fixes;
    use crate::parser::parse;
    use crate::semantic::SemanticModel;
    use crate::syntax::SyntaxNode;

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
            if MathOperatorName.interests().contains(&el.kind()) {
                MathOperatorName.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_bare_operator_with_unsafe_fix() {
        let src = "$sin x$\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "math-operator-name");
        // Caret on just `sin` (bytes 1..4), not the trailing ` x`.
        assert_eq!((out[0].start, out[0].end), (1, 4));
        let fix = out[0].fix.as_ref().expect("a fix");
        assert_eq!(fix.applicability, Applicability::Unsafe);
        assert_eq!(fix.content, "\\sin");
        // Unsafe: skipped without opt-in, applied with it.
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).applied,
            0
        );
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), true).output,
            "$\\sin x$\n"
        );
    }

    #[test]
    fn flags_glued_paren_form_fixing_only_the_name() {
        // `sin(x)` lexes as one WORD; the fix rewrites just the `sin` prefix.
        let src = "$sin(x)$\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].start, out[0].end), (1, 4));
        let fix = out[0].fix.as_ref().expect("a fix");
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), true).output,
            "$\\sin(x)$\n"
        );
    }

    #[test]
    fn flags_each_operator_in_a_relation() {
        // `$sin x + cos x = 1$` -> both `sin` and `cos` fire.
        let out = findings("$sin x + cos x = 1$\n");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn command_form_is_fine() {
        assert!(findings("$\\sin x + \\cos x$\n").is_empty());
    }

    #[test]
    fn outside_math_is_left_alone() {
        // Plain prose: `sin` is the English word, not the operator.
        assert!(findings("It was a sin to log this.\n").is_empty());
    }

    #[test]
    fn word_that_only_starts_with_operator_is_left_alone() {
        // `since` begins with `sin` but the boundary char is a letter.
        assert!(findings("$since$\n").is_empty());
    }

    #[test]
    fn prefers_longest_operator() {
        // `sinh` must win over `sin`; the fix is `\sinh`, not `\sin`h.
        let src = "$sinh x$\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].start, out[0].end), (1, 5));
        let fix = out[0].fix.as_ref().expect("a fix");
        assert_eq!(fix.content, "\\sinh");
    }

    #[test]
    fn subscript_label_is_left_alone() {
        // `x_{max}` — `max` is a label inside the subscript, not the operator.
        assert!(findings("$x_{max}$\n").is_empty());
    }

    #[test]
    fn lim_base_before_subscript_is_flagged() {
        // The `lim` base sits outside the subscript, so it still fires.
        let out = findings("$lim_{n} a_n$\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("lim"));
    }
}
