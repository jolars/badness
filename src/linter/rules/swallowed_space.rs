//! `swallowed-space`: a text-producing control word directly followed by a
//! space that TeX eats, gluing the macro's output to the next word
//! (`\LaTeX is` renders "LaTeXis") (ChkTeX 1).
//!
//! When TeX tokenizes a *control word* (a backslash plus letters) it discards
//! any spaces that follow, so the space in `\LaTeX is` never reaches the output
//! and the two run together. The fix inserts `{}` right after the control word
//! (`\LaTeX{} is`): the empty group ends the control word, and the following
//! space is then an ordinary interword space.
//!
//! Scope is deliberately narrow: only a curated set of **argument-less,
//! text-producing** macros (the TeX-family logos) is flagged. For those, a
//! swallowed space before a following word is essentially always a mistake. A
//! macro that takes an argument is a different shape (`\foo bar` grabs `b` as the
//! argument, not "a swallowed space before text"), and we would need meaning we
//! do not model to tell the two apart, so those stay out of scope (a false
//! negative, which tenet-style conservatism prefers to a false positive). The
//! rule also fires only in text mode — in math the inter-token space is
//! insignificant, so nothing visible changes.
//!
//! The following token must be a word beginning with an alphanumeric character:
//! only then does the glue produce visibly wrong output. A following punctuation
//! mark (`\LaTeX .` -> "LaTeX.") is exactly what the author wanted, so it is left
//! alone.
//!
//! The fix is `Unsafe` (like the sibling spacing rules `missing-nonbreaking-space`
//! and `math-operator-name`): inserting `{}` changes the typeset output — that is
//! the whole point — so `--fix` leaves it alone while `--unsafe-fixes` and the
//! editor code action apply it. It is still correct by construction (tenet 1): an
//! empty `{}` is valid wherever a control word ends, so the result re-parses and
//! stays lossless.
//!
//! The macro table lives here, not in `data/signatures.json`: "this logo swallows
//! a following space" is a lint judgment, not the structural arity/verbatim fact
//! the signature DB carries (AGENTS.md core decision #2).

use std::path::PathBuf;

use crate::ast::{AstToken, ControlWord, child_token, command_name};
use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "A logo swallows the following space, gluing it to the next word:",
    source: "We used \\LaTeX to typeset this.\n",
}];

/// Argument-less, text-producing macros whose trailing space TeX eats. Restricted
/// to the well-known TeX-family logos, where a swallowed space before a following
/// word is essentially always a mistake.
const TEXT_MACROS: &[&str] = &[
    "TeX", "LaTeX", "LaTeXe", "eTeX", "XeTeX", "XeLaTeX", "LuaTeX", "LuaLaTeX", "pdfTeX",
    "pdfLaTeX", "BibTeX", "ConTeXt", "MetaPost", "MetaFont", "SliTeX", "PiCTeX", "AmS", "AmSTeX",
    "plainTeX",
];

pub struct SwallowedSpace;

impl Rule for SwallowedSpace {
    fn id(&self) -> &'static str {
        "swallowed-space"
    }

    fn emits_fix(&self) -> bool {
        true
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a text-producing control word directly followed by a space that TeX \
         eats, gluing the macro's output to the next word (`\\LaTeX is` renders \
         \"LaTeXis\") (ChkTeX 1). When TeX tokenizes a control word it discards \
         following spaces, so the space never reaches the output. To stay \
         conservative the rule fires only for a curated set of argument-less \
         TeX-family logos (`\\LaTeX`, `\\TeX`, `\\BibTeX`, ...), only in text mode, \
         and only when the next token is a word beginning with an alphanumeric \
         character -- a following period (`\\LaTeX .` -> \"LaTeX.\") is what the \
         author wanted. The fix inserts `{}` after the control word \
         (`\\LaTeX{} is`), ending the macro name so the space survives; it is \
         **unsafe** because it changes the typeset output, so `--fix` leaves it \
         alone while `--unsafe-fixes` and the editor code action apply it."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::COMMAND]
    }

    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(command) = el.as_node() else {
            return;
        };
        // `command_name` returns `None` for a control *symbol* (`\,`, `\%`),
        // which never swallows a space, so those fall through here.
        let Some(name) = command_name(command) else {
            return;
        };
        if !TEXT_MACROS.contains(&name.as_str()) {
            return;
        }
        // In math the inter-token space is insignificant, so a swallowed space
        // changes nothing visible: stay quiet.
        if ctx.in_math(usize::from(command.text_range().start())) {
            return;
        }
        // The `CONTROL_WORD` token is the command's first token; the token
        // directly after it must be a same-line space. Anything else — a `{`
        // (already `\LaTeX{}`), a `\ ` control symbol, a `~`, or a newline —
        // means the space is not swallowed, so we skip.
        let Some(control_word) = child_token::<ControlWord>(command) else {
            return;
        };
        let Some(space) = control_word.syntax().next_token() else {
            return;
        };
        if space.kind() != SyntaxKind::WHITESPACE {
            return;
        }
        // Only a following word that begins with an alphanumeric character glues
        // into visibly wrong output. Leading punctuation (`\LaTeX .`) is desired.
        let Some(next) = space.next_token() else {
            return;
        };
        if next.kind() != SyntaxKind::WORD
            || !next
                .text()
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric())
        {
            return;
        }

        let range = space.text_range();
        let start = usize::from(range.start());
        let end = usize::from(range.end());
        // Insert `{}` at the control word's end (== the space's start): a
        // zero-width splice ending the macro name so the following space becomes
        // an ordinary interword space. Correct by construction (tenet 1): an empty
        // group is valid here, so the result re-parses and stays lossless. Unsafe
        // because it changes the typeset output.
        let fix = Fix::unsafe_(
            start,
            start,
            "{}",
            format!("Insert `{{}}` after `\\{name}` so the space is not swallowed"),
        );
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start,
            end,
            message: format!(
                "`\\{name}` swallows the following space; add `{{}}` (`\\{name}{{}}`) or `\\ ` so it prints"
            ),
            fix: Some(fix),
        });
    }
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
        let ctx = RuleContext::new(std::path::Path::new("x.tex"), &root, &model, None, None);
        let mut out = Vec::new();
        for el in root.descendants_with_tokens() {
            if SwallowedSpace.interests().contains(&el.kind()) {
                SwallowedSpace.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_logo_before_word_with_unsafe_fix() {
        let src = "\\LaTeX is nice\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "swallowed-space");
        // Caret on the swallowed space (byte 6..7), not the command.
        assert_eq!((out[0].start, out[0].end), (6, 7));
        let fix = out[0].fix.as_ref().expect("a fix");
        assert_eq!(fix.applicability, Applicability::Unsafe);
        // A zero-width insertion of `{}` right after `\LaTeX`.
        assert_eq!((fix.start, fix.end), (6, 6));
        assert_eq!(fix.content, "{}");
        // Unsafe: skipped without opt-in, applied with it.
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).applied,
            0
        );
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), true).output,
            "\\LaTeX{} is nice\n"
        );
    }

    #[test]
    fn already_braced_is_clean() {
        assert!(findings("\\LaTeX{} is nice\n").is_empty());
    }

    #[test]
    fn escaped_space_is_clean() {
        assert!(findings("\\LaTeX\\ is nice\n").is_empty());
    }

    #[test]
    fn tie_is_clean() {
        assert!(findings("\\LaTeX~is nice\n").is_empty());
    }

    #[test]
    fn following_punctuation_is_clean() {
        // `\LaTeX .` renders "LaTeX." — the swallowed space is what the author
        // wanted, so do not flag.
        assert!(findings("\\LaTeX .\n").is_empty());
    }

    #[test]
    fn following_digit_is_flagged() {
        // `\LaTeX 2e` glues to "LaTeX2e"; a leading digit still counts as text.
        assert_eq!(findings("\\LaTeX 2e\n").len(), 1);
    }

    #[test]
    fn newline_after_logo_is_out_of_scope() {
        // A source line break (not a plain space) is left for a later pass, like
        // `missing-nonbreaking-space`.
        assert!(findings("\\LaTeX\nis nice\n").is_empty());
    }

    #[test]
    fn trailing_space_at_line_end_is_clean() {
        // Space then newline: nothing glues, so no finding.
        assert!(findings("\\LaTeX \n").is_empty());
    }

    #[test]
    fn non_logo_command_is_left_alone() {
        // Ordinary commands are out of scope: we cannot tell an argument-grab
        // (`\foo bar`) from a swallowed space without meaning we do not model.
        assert!(findings("\\foo bar\n").is_empty());
    }

    #[test]
    fn control_symbol_is_left_alone() {
        // A control symbol (`\%`) does not swallow a following space.
        assert!(findings("100\\% is high\n").is_empty());
    }

    #[test]
    fn math_mode_is_left_alone() {
        // In math the inter-token space is insignificant.
        assert!(findings("$\\TeX x$\n").is_empty());
    }

    #[test]
    fn flags_each_occurrence() {
        assert_eq!(findings("\\TeX is old and \\LaTeX is new\n").len(), 2);
    }
}
