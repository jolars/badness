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
//!     always a *label* ("the maximum"), not the operator. Curated math-domain
//!     arguments build the same `SUBSCRIPT`/`SUPERSCRIPT` structure as explicit
//!     math, which covers `\frac{x_{exp}}{n}` (issue #37).
//!   - Never in an argument whose positional domain is text or unknown. This
//!     covers keys, prose islands, and uncurated commands without name carve-outs.
//!   - Never inside a math alphabet or `\operatorname`: `\mathrm{exp}` already
//!     sets `exp` upright, so flagging it is wrong. Text escapes are excluded by
//!     the shared mode index.
//!   - Never a pgfmath function call inside a TikZ `calc` coordinate
//!     ([`in_calc_coordinate`]): in `\draw ($sin(x)$)` the `calc` library
//!     repurposes `$…$` as coordinate arithmetic where `sin` is a
//!     backslash-less pgfmath function, so the `$` is not math shift and the
//!     `\sin` rewrite would break the pgfmath parser.
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
//! The operator vocabulary comes from the shared static math classifier, not
//! `data/signatures.json`: it is a math-class fact, not structural arity data.

use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
use crate::semantic::NAMED_MATH_OPERATORS;
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxToken};

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
         or superscript, where `max` in `x_{max}` is almost always a label, and \
         never inside a text-domain or unknown argument. The fix inserts the backslash \
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

    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(tok) = el.as_token() else {
            return;
        };
        if !ctx.in_math(usize::from(tok.text_range().start())) {
            return;
        }
        for node in tok.parent_ancestors() {
            match node.kind() {
                SyntaxKind::SUBSCRIPT | SyntaxKind::SUPERSCRIPT => return,
                _ => {}
            }
        }
        // Math alphabets and `\operatorname` remain mathematical, but already
        // establish the intentional upright/letter treatment this rule asks for.
        if super::in_math_alphabet_or_operator_argument(tok) {
            return;
        }

        // A script after a lexer `WORD` binds only its final input character, so
        // `lim_{n}` parses as the sibling `li` followed by a `SCRIPTED` node whose
        // base is `m`. Rejoin that source-glued prefix for this lexical lint; the
        // CST split is the exact TeX structure, while the operator spelling still
        // spans both leaves.
        let mut spelling = tok.text().to_owned();
        let mut start = usize::from(tok.text_range().start());
        if let Some(parent) = tok.parent()
            && parent.kind() == SyntaxKind::SCRIPTED
            && parent.first_token().as_ref() == Some(tok)
            && let Some(prev) = parent
                .prev_sibling_or_token()
                .and_then(|el| el.into_token())
            && prev.kind() == SyntaxKind::WORD
            && prev.text_range().end() == tok.text_range().start()
        {
            spelling.insert_str(0, prev.text());
            start = usize::from(prev.text_range().start());
        }
        let Some(name) = match_operator_prefix(&spelling) else {
            return;
        };
        // A pgfmath function call inside a TikZ `calc` coordinate `($sin(x)$)`:
        // the `$` is not math shift there and the `\sin` rewrite would break the
        // pgfmath parser.
        if in_calc_coordinate(tok, &spelling, name) {
            return;
        }
        let end = start + name.len();
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

/// True when the matched operator is a pgfmath function call inside a TikZ `calc`
/// coordinate `($…$)`. The `calc` library repurposes `$…$` as coordinate
/// arithmetic, where `sin`/`cos` are backslash-less pgfmath functions; reading the
/// `$` as math shift and flagging them is a false positive, and the `\sin` rewrite
/// would break the pgfmath parser. Two static shape facts gate it, so ordinary
/// math still flags:
///   - the name is *glued* to `(` (a pgfmath call `sin(…)`), so a spaced operator
///     even in parenthesized prose math (`($sin x$)`) still flags, and
///   - the enclosing inline math is a parenthesized coordinate — a `(` directly
///     before the opening `$` and a `)` directly after the closing `$` — so
///     ordinary inline math (`$lim(x)$`) still flags.
fn in_calc_coordinate(tok: &SyntaxToken, spelling: &str, name: &str) -> bool {
    // A glued pgfmath call: the byte right after the operator name is `(`.
    if spelling.as_bytes().get(name.len()) != Some(&b'(') {
        return false;
    }
    let Some(math) = tok
        .parent_ancestors()
        .find(|n| n.kind() == SyntaxKind::INLINE_MATH)
    else {
        return false;
    };
    flank_char(math.prev_sibling_or_token(), false) == Some('(')
        && flank_char(math.next_sibling_or_token(), true) == Some(')')
}

/// The nearest non-trivia character flanking a node on one side: the first char of
/// the next sibling token (`next`), or the last char of the previous sibling token,
/// skipping whitespace and newlines. `None` at the edge of the containing node.
fn flank_char(mut el: Option<SyntaxElement>, next: bool) -> Option<char> {
    while let Some(e) = el {
        match e.kind() {
            SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE => {
                el = if next {
                    e.next_sibling_or_token()
                } else {
                    e.prev_sibling_or_token()
                };
            }
            _ => {
                let text = e.as_token()?.text().to_owned();
                return if next {
                    text.chars().next()
                } else {
                    text.chars().next_back()
                };
            }
        }
    }
    None
}

/// The longest operator name that is a prefix of `text` ending at a word boundary
/// (end of `text`, or a following byte that is not an ASCII letter). Returns
/// `None` when no operator matches, keeping ordinary words that merely begin with
/// a function name (`since`, `cosine`) out of scope.
fn match_operator_prefix(text: &str) -> Option<&'static str> {
    let bytes = text.as_bytes();
    NAMED_MATH_OPERATORS
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
        // Issue #37: `\frac`'s curated math domain builds the same structural
        // subscript nodes as explicit math.
        assert!(findings("$\\frac{x_{exp}}{n}$\n").is_empty());
        assert!(findings("$\\frac{x^{max}}{n}$\n").is_empty());
    }

    #[test]
    fn bare_subscript_inside_argument_group_is_left_alone() {
        // The unbraced form is structurally a subscript too.
        assert!(findings("$\\frac{x_exp}{n}$\n").is_empty());
    }

    #[test]
    fn argument_group_after_operator_word_is_still_flagged() {
        // A bare operator elsewhere in the same math argument still fires.
        assert_eq!(findings("$\\frac{x_{a} exp y}{n}$\n").len(), 1);
    }

    #[test]
    fn unknown_arguments_inside_math_are_left_alone() {
        assert!(findings("$\\unknown{sin x}$\n").is_empty());
    }

    #[test]
    fn upright_font_argument_is_left_alone() {
        // `\mathrm{exp}` already sets `exp` upright; flagging it (and the broken
        // `\mathrm{\exp}` rewrite) is wrong.
        assert!(findings("$\\mathrm{exp}(x)$\n").is_empty());
        assert!(findings("$\\mathbf{sin}$\n").is_empty());
        assert!(findings("$\\operatorname{arg}$\n").is_empty());
    }

    #[test]
    fn text_escape_is_left_alone() {
        // Prose inside a text escape is not math.
        assert!(findings("$\\text{the gcd is}\\gcd(x)$\n").is_empty());
        assert!(findings("$\\intertext{where max is}$\n").is_empty());
    }

    #[test]
    fn calc_coordinate_is_left_alone() {
        // TikZ `calc`: `($sin(x)$)` is coordinate arithmetic where `sin` is a
        // pgfmath function; the `\sin` rewrite would break the pgfmath parser.
        assert!(findings("\\draw ($sin(x)$);\n").is_empty());
        assert!(
            findings("\\begin{tikzpicture}\n\\draw ($cos(x)$);\n\\end{tikzpicture}\n").is_empty()
        );
    }

    #[test]
    fn parenthesized_prose_math_is_still_flagged() {
        // The calc gate needs both the glued `(` and the `($…$)` wrapper. A spaced
        // operator in parenthesized prose math (`($sin x$)`) is not a pgfmath call,
        // so it still flags.
        assert_eq!(findings("($sin x$)\n").len(), 1);
        // And a glued call that is not paren-wrapped (`$sin(x)$`) still flags.
        assert_eq!(findings("$sin(x)$\n").len(), 1);
    }

    #[test]
    fn lim_base_before_subscript_is_flagged() {
        // The `lim` base sits outside the subscript, so it still fires.
        let src = "$lim_{n} a_n$\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("lim"));
        assert_eq!((out[0].start, out[0].end), (1, 4));
        let fix = out[0].fix.as_ref().expect("a fix");
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), true).output,
            "$\\lim_{n} a_n$\n"
        );
    }
}
