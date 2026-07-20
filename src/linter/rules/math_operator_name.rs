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
//! **Conservative gating.** Three guards keep false positives down:
//!   - Only inside math mode (an ancestor `MATH`); a bare `sin` in text is just
//!     the English word.
//!   - Never in script position, where a name like `max` in `x_{max}` is almost
//!     always a *label* ("the maximum"), not the operator. That means no
//!     `SUBSCRIPT`/`SUPERSCRIPT` ancestor, and also not the *raw* script shape —
//!     a word or group directly preceded by a `_`/`^` token — which is what a
//!     command's argument body produces (`\frac{x_{exp}}{n}`, issue #37):
//!     argument mode is macro meaning the parser never resolves, so those bodies
//!     parse in text mode and build no script nodes. The base of `\lim_{n}` is
//!     still flagged: `lim` there is the WORD base, not inside the script.
//!   - Never inside a key argument (`super::in_key_argument`): the `max` in
//!     `$\label{eq:thing_max}$` or `\eqref{eq:max}` is part of an opaque
//!     identifier, not typeset math. Math-content arguments (`\frac{sin x}{2}`)
//!     are still in scope — the gate is a curated name-family list
//!     (`semantic::builder::key_argument_command`), not a blanket
//!     argument skip.
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
         stay conservative it only fires inside math mode, never in a subscript \
         or superscript, where `max` in `x_{max}` is almost always a label \
         rather than the operator (including inside a command argument such as \
         `\\frac{x_{max}}{n}`, whose body carries no script nodes), and never \
         inside a key argument such as \
         `\\label{eq:thing_max}` or `\\eqref{eq:max}`, whose content is an opaque \
         identifier rather than typeset math. The fix inserts the backslash \
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
        // Inside a command's argument group (`\frac{x_{exp}}{n}`) the body is
        // parsed in text mode — argument mode is macro meaning the parser never
        // resolves — so a `_`/`^` there never builds a `SUBSCRIPT` node (issue
        // #37). Recognize the raw script shape too: the word itself, or an
        // enclosing `GROUP`, directly preceded by a `_`/`^` token.
        if follows_script_operator(tok.prev_sibling_or_token()) {
            return;
        }
        let mut in_math = false;
        for node in tok.parent_ancestors() {
            match node.kind() {
                SyntaxKind::SUBSCRIPT | SyntaxKind::SUPERSCRIPT => return,
                SyntaxKind::GROUP if follows_script_operator(node.prev_sibling_or_token()) => {
                    return;
                }
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
        // A key argument (`\label{eq:thing_max}`, `\eqref`, `\cite`, …) holds an
        // opaque identifier, not typeset math.
        if super::in_key_argument(tok) {
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
            related: Vec::new(),
        });
    }
}

/// True when the element at `prev` (skipping whitespace and newlines, never a
/// comment) is a `_`/`^` token — the raw script shape left by a text-mode parse,
/// where no `SUBSCRIPT`/`SUPERSCRIPT` node wraps the script argument.
fn follows_script_operator(mut prev: Option<SyntaxElement>) -> bool {
    while let Some(el) = prev {
        match el.kind() {
            SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE => prev = el.prev_sibling_or_token(),
            SyntaxKind::UNDERSCORE | SyntaxKind::CARET => return true,
            _ => return false,
        }
    }
    false
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
        assert_eq!(fix.edits[0].content, "\\sin");
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
        assert_eq!(fix.edits[0].content, "\\sinh");
    }

    #[test]
    fn label_key_in_math_is_left_alone() {
        // Issue #25: `max` in the label key is an opaque identifier, not math.
        assert!(findings("$\\label{eq:thing_max}$\n").is_empty());
    }

    #[test]
    fn ref_and_cite_keys_in_math_are_left_alone() {
        assert!(findings("$\\eqref{max_norm}$\n").is_empty());
        assert!(findings("$x \\cite{max2000}$\n").is_empty());
    }

    #[test]
    fn math_content_argument_is_still_flagged() {
        // The key gate is a name-family list, not a blanket argument skip:
        // `\frac`'s arguments are math content, so a bare `sin` there fires.
        assert_eq!(findings("$\\frac{sin x}{2}$\n").len(), 1);
    }

    #[test]
    fn subscript_label_is_left_alone() {
        // `x_{max}` — `max` is a label inside the subscript, not the operator.
        assert!(findings("$x_{max}$\n").is_empty());
    }

    #[test]
    fn subscript_label_inside_argument_group_is_left_alone() {
        // Issue #37: `\frac`'s argument body parses in text mode, so `x_{exp}`
        // there has no SUBSCRIPT node — the raw `_`-before-group shape must
        // still read as script position.
        assert!(findings("$\\frac{x_{exp}}{n}$\n").is_empty());
        assert!(findings("$\\frac{x^{max}}{n}$\n").is_empty());
    }

    #[test]
    fn bare_subscript_inside_argument_group_is_left_alone() {
        // The unbraced form: `exp` directly follows the `_` token.
        assert!(findings("$\\frac{x_exp}{n}$\n").is_empty());
    }

    #[test]
    fn argument_group_after_operator_word_is_still_flagged() {
        // The raw-shape guard keys on `_`/`^` only: a bare operator elsewhere
        // in the same argument still fires.
        assert_eq!(findings("$\\frac{x_{a} exp y}{n}$\n").len(), 1);
    }

    #[test]
    fn lim_base_before_subscript_is_flagged() {
        // The `lim` base sits outside the subscript, so it still fires.
        let out = findings("$lim_{n} a_n$\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("lim"));
    }
}
