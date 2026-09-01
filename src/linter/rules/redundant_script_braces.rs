//! `redundant-script-braces`: braces around a single-token sub/superscript
//! argument that can be dropped without changing meaning (`x^{2}` -> `x^2`,
//! `y_{\alpha}` -> `y_\alpha`).
//!
//! In math mode `^`/`_` bind a single following token, so a one-token braced
//! argument (`{2}`, `{\alpha}`) is redundant. The autofix ([`strip_braces_fix`])
//! is a `Safe` deletion of the two brace tokens, leaving the inner token
//! untouched — correct by construction (parses + lossless, tenet 1).
//!
//! **The fix owes correctness as a raw edit, not layout** (tenet 1): it runs
//! without the formatter, so it may only strip when the braces are redundant in
//! the *unspaced* text. badness's catcode-faithful lexer globs a run of word
//! characters into one `WORD` (`is_word_char`), and a bare `^`/`_` binds that
//! whole `WORD`, so removing the braces is meaning-preserving only when the
//! following character cannot glue onto the inner token:
//!
//! - `x^{2}$` -> `x^2$` (a `$` cannot extend the `WORD`) — safe.
//! - `x^{2}-3` stays braced: unspaced `x^2-3` re-lexes `2-3` as one `WORD`, so `^`
//!   would bind `2-3` — a meaning change. (The formatter *used* to strip this
//!   because it also inserts operator spacing, `x^2 - 3`; a raw fix has no such
//!   guarantee, so it withholds.)
//! - `y_{\alpha}b` stays braced: `\alphab` is one control word.
//!
//! So the rule strips only when the inner token is a single-character `WORD` or a
//! lone control word *and* the next character is absent or not a word character
//! (for a control word, not an ASCII letter). Standard named math operators are
//! excluded: commands such as `\max` expand to `\mathop`, which TeX cannot consume
//! as an unbraced script field. This is the guard that formerly lived in the
//! formatter (`strippable_script_arg`), minus the spacing-dependent widening.

use std::path::PathBuf;

use crate::parser::lexer::is_word_char;
use crate::semantic::math::NAMED_MATH_OPERATORS;
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use crate::linter::diagnostic::{Diagnostic, Edit, Fix, Severity};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "Redundant braces around a single-token script argument:",
    source: "$x^{2}$ and $y_{\\alpha}$\n",
}];

pub struct RedundantScriptBraces;

impl Rule for RedundantScriptBraces {
    fn id(&self) -> &'static str {
        "redundant-script-braces"
    }

    fn emits_fix(&self) -> bool {
        true
    }

    fn default_severity(&self) -> Severity {
        // Cosmetic and extremely common (`x^{2}`); the lowest on-by-default level
        // so it drives `--fix`/code-actions without flooding math-heavy documents.
        Severity::Hint
    }

    fn description(&self) -> &'static str {
        "Flag braces around a single-token sub/superscript argument, which `^`/`_` \
         bind without them (`x^{2}` is `x^2`). The autofix deletes the two braces \
         and leaves the inner token untouched. It is withheld when dropping the \
         braces would let the following character glue onto the argument and change \
         meaning (`x^{2}-3` stays braced — unspaced `x^2-3` would re-lex `2-3` as one \
         token; `y_{\\alpha}b` stays braced — `\\alphab` is one control word). It \
         also leaves standard named math operators braced because commands such as \
         `\\max` are not valid unbraced script fields."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::SUBSCRIPT, SyntaxKind::SUPERSCRIPT]
    }

    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(script) = el.as_node() else {
            return;
        };
        if !ctx.in_math(usize::from(script.text_range().start())) {
            return;
        }
        let Some(group) = script
            .children()
            .find(|n| n.kind() == SyntaxKind::GROUP)
            .filter(strippable_script_arg)
        else {
            return;
        };
        let Some(fix) = strip_braces_fix(&group) else {
            return;
        };
        let range = group.text_range();
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start: usize::from(range.start()),
            end: usize::from(range.end()),
            message: "redundant braces around a single-token script argument".to_owned(),
            fix: Some(fix),
            related: Vec::new(),
        });
    }
}

/// Build the `{X}` -> `X` autofix: one atomic [`Fix`] deleting the opening and
/// closing brace tokens, leaving the inner token bytes untouched. `Safe` — a
/// single-token script argument is identical with or without braces, and
/// [`strippable_script_arg`] has already ruled out a gluing neighbor. Returns
/// `None` if the group is missing a brace (a malformed group — nothing to strip).
fn strip_braces_fix(group: &SyntaxNode) -> Option<Fix> {
    let mut lbrace = None;
    let mut rbrace = None;
    for el in group.children_with_tokens() {
        if let SyntaxElement::Token(t) = el {
            match t.kind() {
                SyntaxKind::L_BRACE => lbrace = Some(t.text_range()),
                SyntaxKind::R_BRACE => rbrace = Some(t.text_range()),
                _ => {}
            }
        }
    }
    let (l, r) = (lbrace?, rbrace?);
    Some(Fix::safe_edits(
        vec![
            Edit::new(l.start().into(), l.end().into(), ""),
            Edit::new(r.start().into(), r.end().into(), ""),
        ],
        "Remove redundant braces",
    ))
}

/// Whether a script-argument brace group `{X}` may safely drop its braces as a
/// *raw* edit: `X` is a single TeX token (a one-character `WORD` or a lone
/// control word) and the token following the group cannot glue onto `X` once the
/// braces are gone. See the module doc for why the spacing-dependent widening the
/// formatter used is deliberately omitted.
fn strippable_script_arg(group: &SyntaxNode) -> bool {
    let mut inner = group.children_with_tokens().filter(|el| {
        !matches!(
            el.kind(),
            SyntaxKind::L_BRACE
                | SyntaxKind::R_BRACE
                | SyntaxKind::WHITESPACE
                | SyntaxKind::NEWLINE
        )
    });
    let Some(only) = inner.next() else {
        return false; // empty `{}` — nothing to strip
    };
    if inner.next().is_some() {
        return false; // more than one token between the braces
    }
    match only {
        SyntaxElement::Token(t)
            if t.kind() == SyntaxKind::WORD && t.text().chars().count() == 1 =>
        {
            next_char_safe_after(group, false)
        }
        SyntaxElement::Node(n) if is_strippable_control_word(&n) => {
            next_char_safe_after(group, true)
        }
        _ => false,
    }
}

/// A lone control word that TeX may consume as an unbraced script field.
///
/// Named operators are syntactically lone commands in the CST, but LaTeX
/// implements them through `\mathop`; that expansion requires a braced field.
fn is_strippable_control_word(node: &SyntaxNode) -> bool {
    if node.kind() != SyntaxKind::COMMAND {
        return false;
    }
    let mut children = node.children_with_tokens();
    let first_is_control_word = matches!(
        children.next(),
        Some(SyntaxElement::Token(t)) if t.kind() == SyntaxKind::CONTROL_WORD
    );
    if !first_is_control_word || children.next().is_some() {
        return false;
    }
    crate::ast::command_name(node)
        .is_some_and(|name| !NAMED_MATH_OPERATORS.contains(&name.as_str()))
}

/// True if the character following `group` cannot glue onto a stripped
/// single-token argument. `letter_only` (for a control-word argument) forbids
/// only a following ASCII letter (`\alphab` is one control word); a `WORD`
/// argument forbids any following word character (`2-3`/`2y` re-lex as one
/// `WORD`).
fn next_char_safe_after(group: &SyntaxNode, letter_only: bool) -> bool {
    let Some(next) = group.last_token().and_then(|t| t.next_token()) else {
        return true;
    };
    match next.text().chars().next() {
        None => true,
        Some(c) if letter_only => !c.is_ascii_alphabetic(),
        Some(c) => !is_word_char(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linter::diagnostic::Applicability;
    use crate::linter::fix::apply_fixes;
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
            if RedundantScriptBraces.interests().contains(&el.kind()) {
                RedundantScriptBraces.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    fn fixed(src: &str) -> String {
        let out = findings(src);
        let fixes: Vec<_> = out.iter().filter_map(|d| d.fix.clone()).collect();
        apply_fixes(src, &fixes, false).output
    }

    #[test]
    fn strips_single_word_and_control_word() {
        let out = findings("$x^{2}$ and $y_{\\alpha}$\n");
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|d| d.rule == "redundant-script-braces"));
        assert!(
            out.iter()
                .all(|d| d.fix.as_ref().unwrap().applicability == Applicability::Safe)
        );
        assert_eq!(
            fixed("$x^{2}$ and $y_{\\alpha}$\n"),
            "$x^2$ and $y_\\alpha$\n"
        );
    }

    #[test]
    fn keeps_multichar_argument() {
        assert!(findings("$x^{ab}$\n").is_empty());
        assert!(findings("$x^{n+1}$\n").is_empty());
    }

    #[test]
    fn keeps_when_a_following_word_char_would_glue() {
        // Unspaced `x^2y` / `x^2-3` re-lex the argument as one `WORD`, so `^`
        // would bind more than the single token — the strip is withheld.
        assert!(findings("$x^{2}y$\n").is_empty());
        assert!(findings("$x^{2}-3$\n").is_empty());
        assert!(findings("$y_{i}<z$\n").is_empty());
    }

    #[test]
    fn keeps_control_word_before_letter() {
        // `\alphab` is one control word.
        assert!(findings("$y_{\\alpha}b$\n").is_empty());
    }

    #[test]
    fn keeps_named_math_operators() {
        // LaTeX's named operators expand to `\mathop`, which TeX cannot consume
        // as an unbraced script field.
        assert!(findings("$t_{\\max}(x)$ and $x^{\\lim}$\n").is_empty());
    }

    #[test]
    fn strips_control_word_before_nonletter() {
        // `\alpha` ends at the `2`; the strip is safe.
        assert_eq!(fixed("$y_{\\alpha}2$\n"), "$y_\\alpha2$\n");
    }

    #[test]
    fn keeps_empty_group() {
        assert!(findings("$x^{}$\n").is_empty());
    }

    #[test]
    fn safe_before_math_close_and_command() {
        assert_eq!(fixed("$x^{2}\\cdot y$\n"), "$x^2\\cdot y$\n");
    }

    #[test]
    fn works_in_known_math_slots_outside_explicit_math() {
        assert_eq!(fixed("\\frac{x^{2}}{y_{i}}\n"), "\\frac{x^2}{y_i}\n");
    }

    #[test]
    fn stays_out_of_text_and_unknown_arguments() {
        assert!(findings("$\\text{x^{2}} + \\unknown{y_{i}}$\n").is_empty());
    }
}
