//! `abbreviation-spacing`: TeX's sentence-vs-interword spacing goes wrong around
//! abbreviations and acronyms (ChkTeX 12/13, lacheck, textidote sh:010/011).
//!
//! Outside `\frenchspacing`, TeX widens the space after `.`/`?`/`!` into
//! *inter-sentence* space, but suppresses that widening when the punctuation
//! directly follows an uppercase letter (assuming an abbreviation). Two shapes
//! defeat that heuristic:
//!
//! - **Interword after a lowercase abbreviation (ChkTeX 12).** `e.g.`, `i.e.`,
//!   `etc.`, `et al.`, ... end in a lowercase letter, so TeX reads the trailing
//!   period as a sentence end and sets a *wide* space. Mid-sentence that is wrong;
//!   the fix forces an interword space with `\ ` (`e.g.\ foo`). To stay
//!   conservative the rule fires only when a **lowercase** word follows, the
//!   strong signal that the sentence continues (so an `etc.` that genuinely ends a
//!   sentence, followed by a capital, is left alone).
//! - **Intersentence after an uppercase acronym (ChkTeX 13).** `USA.`, `UFO.`,
//!   `FBI.` end in an uppercase letter, so TeX assumes an abbreviation and sets a
//!   *narrow* space. At a sentence end that is wrong; the fix restores
//!   inter-sentence space with `\@` before the period (`USA\@.`). To stay
//!   conservative the rule fires only for a run of **two or more** uppercase
//!   letters before the punctuation (so single-letter initials like `J.` and dotted
//!   forms like `U.S.A.` are left alone) and only when an **uppercase** word
//!   follows, the signal of a new sentence.
//!
//! Both fixes are `Unsafe` (like the sibling spacing rules
//! `missing-nonbreaking-space` and `swallowed-space`): they change the typeset
//! spacing, which is exactly what `Applicability::Unsafe` is for, so `--fix`
//! leaves them alone while `--unsafe-fixes` and the editor code action apply them.
//! Each is still correct by construction (tenet 1): `\ ` is valid wherever the
//! space stood and `\@` is valid before the period, so the result re-parses and
//! stays lossless.
//!
//! `\frenchspacing` removes the sentence/interword distinction entirely, so the
//! rule tracks the toggle in document order and stays silent once
//! `\frenchspacing` is seen (until a later `\nonfrenchspacing`). This makes the
//! rule whole-file rather than node-shape: the finding depends on preceding
//! toggles, not just the local word. Group scoping of the toggle is *not* modeled
//! -- a `\frenchspacing` inside a group suppresses to end of file, a false
//! negative, which the conservative direction prefers.
//!
//! The rule reads only `WORD` tokens, so comments, `\verb`, and verbatim (which
//! never lex as `WORD`) are untouched, and math is skipped (a `.` there is not
//! sentence punctuation).

use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxToken};

use super::{Example, Rule, RuleContext, StreamVisitor};

const EXAMPLES: &[Example] = &[
    Example {
        caption: "A lowercase abbreviation followed by more text takes an interword space:",
        source: "We tried several methods, e.g. gradient descent.\n",
    },
    Example {
        caption: "An acronym ending a sentence takes intersentence spacing:",
        source: "The rover reached the USA. Then it stopped.\n",
    },
];

/// Lowercase-ending abbreviations whose trailing period TeX mis-reads as a
/// sentence end. `et al.` is handled separately (it spans two words).
const INTERWORD_ABBREVS: &[&str] = &["e.g.", "i.e.", "etc.", "cf.", "vs.", "viz."];

pub struct AbbreviationSpacing;

impl Rule for AbbreviationSpacing {
    fn id(&self) -> &'static str {
        "abbreviation-spacing"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag TeX's sentence-vs-interword spacing going wrong around abbreviations \
         and acronyms (ChkTeX 12/13). Outside `\\frenchspacing`, TeX widens the \
         space after `.`/`?`/`!` unless the punctuation follows an uppercase \
         letter. Two shapes defeat that: a lowercase abbreviation (`e.g.`, `i.e.`, \
         `etc.`, `et al.`) gets a too-wide space, fixed with `\\ ` (`e.g.\\ foo`); \
         and an uppercase acronym ending a sentence (`USA.`) gets a too-narrow \
         space, fixed with `\\@` (`USA\\@.`). To stay conservative the first fires \
         only before a lowercase word (the sentence clearly continues) and the \
         second only for a run of two or more capitals before the period and \
         before an uppercase word (a new sentence), so initials (`J.`), dotted \
         forms (`U.S.A.`), and mid-sentence acronyms are left alone. Both fixes are \
         **unsafe** -- they change the typeset spacing -- so `--fix` leaves them \
         alone while `--unsafe-fixes` and the editor code action apply them. The \
         rule is silent under `\\frenchspacing`, and never touches comments, \
         verbatim, or math."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    // Streaming rather than node-shape: the finding depends on the running
    // `\frenchspacing` toggle, not just the local word, so we track it across the
    // driver's one shared walk in document order.
    fn stream(&self) -> Option<Box<dyn StreamVisitor>> {
        Some(Box::new(AbbreviationSpacingVisitor { french: false }))
    }

    fn emits_fix(&self) -> bool {
        true
    }
}

/// Carries the `\frenchspacing` toggle across the shared walk; the rule stays
/// silent between a `\frenchspacing` and the next `\nonfrenchspacing`.
struct AbbreviationSpacingVisitor {
    french: bool,
}

impl StreamVisitor for AbbreviationSpacingVisitor {
    fn visit(&mut self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(tok) = el.as_token() else {
            return;
        };
        match tok.kind() {
            SyntaxKind::CONTROL_WORD => match tok.text() {
                "\\frenchspacing" => self.french = true,
                "\\nonfrenchspacing" => self.french = false,
                _ => {}
            },
            SyntaxKind::WORD if !self.french => {
                // A `.` in math is not sentence punctuation; skip.
                if ctx.in_math(usize::from(tok.text_range().start())) {
                    return;
                }
                check_word(tok, sink);
            }
            _ => {}
        }
    }
}

/// Flag the two mis-spacing shapes on a single `WORD` token (see the module doc).
/// Free-standing so the streaming visitor can call it without a rule instance.
fn check_word(word: &SyntaxToken, sink: &mut Vec<Diagnostic>) {
    let text = word.text();
    let base = usize::from(word.text_range().start());

    // --- Interword after a lowercase abbreviation (ChkTeX 12). ---
    if let Some(abbrev) = matched_interword_abbrev(word)
        && let Some(ws) = word.next_token()
        && ws.kind() == SyntaxKind::WHITESPACE
        && ws
            .next_token()
            .is_some_and(|next| starts_with_ascii_lower(&next))
    {
        let start = usize::from(ws.text_range().start());
        let end = usize::from(ws.text_range().end());
        // Replace the (possibly multi-space) gap with a single `\ ` interword
        // space. Correct by construction (tenet 1): `\ ` is a valid space here,
        // the splice is contiguous, so the result parses and stays lossless.
        // Unsafe because it changes the typeset spacing.
        let fix = Fix::unsafe_(
            start,
            end,
            "\\ ",
            format!("Replace the space after `{abbrev}` with an interword space `\\ `"),
        );
        sink.push(Diagnostic {
                rule: "abbreviation-spacing",
                severity: Severity::Warning,
                path: PathBuf::new(),
                start,
                end,
                message: format!(
                    "`{abbrev}` is an abbreviation, not a sentence end; use an interword space `\\ ` (`{abbrev}\\ `) so TeX does not widen the gap"
                ),
                fix: Some(fix),
            });
        return;
    }

    // --- Intersentence after an uppercase acronym (ChkTeX 13). ---
    if ends_with_acronym_punct(text)
        && let Some(ws) = word.next_token()
        && ws.kind() == SyntaxKind::WHITESPACE
        && ws
            .next_token()
            .is_some_and(|next| starts_with_ascii_upper(&next))
    {
        // The punctuation is the final ASCII byte of the word; `\@` goes
        // immediately before it.
        let punct = base + text.len() - 1;
        let start = punct;
        let end = base + text.len();
        // Zero-width insertion of `\@` before the period. Correct by
        // construction (tenet 1): `\@` is valid before the punctuation, so the
        // result parses and stays lossless. Unsafe because it changes spacing.
        let fix = Fix::unsafe_(
            punct,
            punct,
            "\\@",
            "Insert `\\@` before the sentence-ending punctuation so TeX widens the gap",
        );
        sink.push(Diagnostic {
                rule: "abbreviation-spacing",
                severity: Severity::Warning,
                path: PathBuf::new(),
                start,
                end,
                message:
                    "capital before sentence-ending punctuation suppresses intersentence spacing; use `\\@` (`Word\\@.`) to restore it"
                        .to_owned(),
                fix: Some(fix),
            });
    }
}

/// The interword abbreviation `word` ends with, if any: a single-token entry from
/// [`INTERWORD_ABBREVS`], or `et al.` (recognized as an `al.` word directly
/// preceded by the word `et`). The abbreviation must sit at a word boundary (start
/// of word or after a non-alphanumeric like `(`), so `cal.` never matches `al.`.
fn matched_interword_abbrev(word: &SyntaxToken) -> Option<&'static str> {
    let text = word.text();
    for &abbrev in INTERWORD_ABBREVS {
        if let Some(prefix) = text.strip_suffix(abbrev)
            && prefix
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric())
        {
            return Some(abbrev);
        }
    }
    // `et al.`: an `al.` word (at a boundary) whose preceding content word is `et`.
    if let Some(prefix) = text.strip_suffix("al.")
        && prefix
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric())
        && prev_content_word_is(word, "et")
    {
        return Some("et al.");
    }
    None
}

/// Whether the content token before `word` (skipping one whitespace gap) is a
/// `WORD` with exactly `expected` text.
fn prev_content_word_is(word: &SyntaxToken, expected: &str) -> bool {
    let Some(ws) = word.prev_token() else {
        return false;
    };
    if ws.kind() != SyntaxKind::WHITESPACE {
        return false;
    }
    ws.prev_token()
        .is_some_and(|prev| prev.kind() == SyntaxKind::WORD && prev.text() == expected)
}

/// Whether `text` ends with a sentence-final `.`/`?`/`!` immediately preceded by a
/// run of two or more uppercase ASCII letters (an acronym like `USA.`). The
/// two-letter floor excludes single initials (`J.`) and dotted forms (`U.S.A.`).
fn ends_with_acronym_punct(text: &str) -> bool {
    let Some(body) = text
        .strip_suffix('.')
        .or_else(|| text.strip_suffix('?'))
        .or_else(|| text.strip_suffix('!'))
    else {
        return false;
    };
    let uppercase_run = body
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_uppercase())
        .count();
    uppercase_run >= 2
}

/// Whether `tok` is a `WORD` beginning with a lowercase ASCII letter.
fn starts_with_ascii_lower(tok: &SyntaxToken) -> bool {
    tok.kind() == SyntaxKind::WORD
        && tok
            .text()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase())
}

/// Whether `tok` is a `WORD` beginning with an uppercase ASCII letter.
fn starts_with_ascii_upper(tok: &SyntaxToken) -> bool {
    tok.kind() == SyntaxKind::WORD
        && tok
            .text()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase())
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
        let mut visitor = AbbreviationSpacing.stream().expect("streaming rule");
        for el in root.descendants_with_tokens() {
            visitor.visit(&el, &ctx, &mut out);
        }
        visitor.finish(&ctx, &mut out);
        out
    }

    #[test]
    fn flags_interword_abbrev_with_unsafe_fix() {
        let src = "see e.g. foo\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "abbreviation-spacing");
        // Caret on the space after `e.g.` (byte 8..9).
        assert_eq!((out[0].start, out[0].end), (8, 9));
        let fix = out[0].fix.as_ref().expect("a fix");
        assert_eq!(fix.applicability, Applicability::Unsafe);
        assert_eq!(fix.content, "\\ ");
        // Unsafe: skipped without the opt-in, applied with it.
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).applied,
            0
        );
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), true).output,
            "see e.g.\\ foo\n"
        );
    }

    #[test]
    fn flags_ie_and_etc() {
        assert_eq!(findings("so i.e. bar\n").len(), 1);
        assert_eq!(findings("apples, etc. and more\n").len(), 1);
    }

    #[test]
    fn flags_et_al() {
        let out = findings("Smith et al. showed this\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("et al."));
    }

    #[test]
    fn abbrev_before_capital_is_left_alone() {
        // A following capital signals a possible sentence end (e.g. `etc.` ending a
        // sentence), so we stay conservative and do not flag.
        assert!(findings("and so on, etc. The next\n").is_empty());
    }

    #[test]
    fn abbrev_in_parens_is_flagged() {
        // `(e.g.` lexes as one WORD; the `(` boundary still matches.
        assert_eq!(findings("methods (e.g. foo) work\n").len(), 1);
    }

    #[test]
    fn word_ending_in_al_without_et_is_clean() {
        // `cal.` must not be read as `al.`; the boundary before `al.` is a letter.
        assert!(findings("the cal. value here\n").is_empty());
        // `al.` with a non-`et` predecessor stays quiet too.
        assert!(findings("the al. value here\n").is_empty());
    }

    #[test]
    fn already_interword_is_clean() {
        // `e.g.\ foo`: the following token is a control symbol, not whitespace.
        assert!(findings("see e.g.\\ foo\n").is_empty());
    }

    #[test]
    fn flags_acronym_intersentence_with_unsafe_fix() {
        let src = "the USA. Then we left\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        // Caret on the period after `USA` (byte 7..8).
        assert_eq!((out[0].start, out[0].end), (7, 8));
        let fix = out[0].fix.as_ref().expect("a fix");
        assert_eq!(fix.applicability, Applicability::Unsafe);
        assert_eq!(fix.content, "\\@");
        // Zero-width insertion just before the period.
        assert_eq!((fix.start, fix.end), (7, 7));
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), true).output,
            "the USA\\@. Then we left\n"
        );
    }

    #[test]
    fn flags_acronym_with_question_mark() {
        assert_eq!(findings("Was it the FBI? They wondered\n").len(), 1);
    }

    #[test]
    fn single_capital_initial_is_left_alone() {
        // `J. Smith`: one capital before the period, so no acronym.
        assert!(findings("From J. Smith we heard\n").is_empty());
    }

    #[test]
    fn dotted_acronym_is_left_alone() {
        // `U.S.A.`: only one capital immediately before the final period.
        assert!(findings("the U.S.A. Then home\n").is_empty());
    }

    #[test]
    fn acronym_before_lowercase_is_left_alone() {
        // A following lowercase word signals a mid-sentence abbreviation, not a
        // sentence end.
        assert!(findings("the USA. government spends\n").is_empty());
    }

    #[test]
    fn frenchspacing_suppresses_the_rule() {
        // Under `\frenchspacing` the sentence/interword distinction is gone.
        assert!(findings("\\frenchspacing see e.g. foo\n").is_empty());
        assert!(findings("\\frenchspacing the USA. Then home\n").is_empty());
    }

    #[test]
    fn nonfrenchspacing_reenables_the_rule() {
        assert_eq!(
            findings("\\frenchspacing off \\nonfrenchspacing see e.g. foo\n").len(),
            1
        );
    }

    #[test]
    fn math_is_left_alone() {
        assert!(findings("$e.g. x$\n").is_empty());
    }

    #[test]
    fn flags_each_occurrence() {
        let out = findings("see e.g. foo and i.e. bar\n");
        assert_eq!(out.len(), 2);
    }
}
