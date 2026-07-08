//! `straight-quotes`: a literal ASCII double quote (`"`) used for quotation.
//! Mirrors ChkTeX rules 18/32-34 and `lacheck`.
//!
//! In LaTeX a straight `"` always sets a *closing* double quote (`''`) regardless
//! of where it appears, so an opening `"` comes out backwards. The correct forms
//! are the ligatures `` `` `` (two backticks) to open and `''` (two apostrophes)
//! to close. This rule flags every ASCII `"` in text and offers a fix.
//!
//! **Direction is inferred from context**, so the fix is `Unsafe`: an opening
//! quote is one preceded by whitespace, a line break, an opening delimiter
//! (`(`, `[`, `{`), a backtick, or the start of the document; anything else reads
//! as a closing quote. The guess can be wrong (and flips the typeset glyph), so
//! `--fix` leaves it alone; `--unsafe-fixes` and the editor code action apply it.
//! The rewrite is still correct by construction (tenet 1): `` `` `` and `''` both
//! parse and the edit stays lossless.
//!
//! Only ASCII `"` is flagged -- the Unicode curly quotes and the `` `` ``/`''`
//! ligatures are already correct. Single straight quotes (`'`) are left alone:
//! they are legitimately apostrophes and closing quotes, so flagging them would
//! be a false-positive minefield. The rule reads only `WORD` tokens, so comments,
//! `\verb`, and verbatim environments (which never lex as `WORD`) are untouched,
//! and math is skipped (a `"` there is not a quotation mark).

use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxToken};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[
    Example {
        caption: "Straight ASCII double quotes around a phrase:",
        source: "He said \"hello world\" to me.\n",
    },
    Example {
        caption: "An opening quote after a parenthesis:",
        source: "(\"quoted\")\n",
    },
];

pub struct StraightQuotes;

impl Rule for StraightQuotes {
    fn id(&self) -> &'static str {
        "straight-quotes"
    }

    fn emits_fix(&self) -> bool {
        true
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a literal ASCII double quote (`\"`) used for quotation. In LaTeX a \
         straight `\"` always sets a *closing* double quote, so an opening one \
         comes out backwards; the correct forms are `` `` `` (two backticks) to \
         open and `''` (two apostrophes) to close. The fix is **unsafe**: it \
         infers direction from context -- a quote preceded by whitespace, a line \
         break, an opening delimiter (`(`, `[`, `{`), a backtick, or the start of \
         the document opens, anything else closes -- and applies only under \
         `--unsafe-fixes` or as an editor code action, since the guess can flip \
         the typeset glyph. Single straight quotes (`'`) are left alone (they are \
         legitimately apostrophes), and comments, verbatim, and math are never \
         touched."
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
        let text = tok.text();
        // Cheap reject: most words hold no straight quote at all.
        if !text.contains('"') {
            return;
        }
        // A straight `"` in math is not a quotation mark; leave it alone.
        if ctx.in_math(usize::from(tok.text_range().start())) {
            return;
        }
        let base = usize::from(tok.text_range().start());

        for (offset, _) in text.match_indices('"') {
            let opening = opens_here(tok, text, offset);
            let (replacement, kind) = if opening {
                ("``", "opening")
            } else {
                ("''", "closing")
            };
            let start = base + offset;
            let end = start + 1;
            let fix = Fix::unsafe_(
                start,
                end,
                replacement,
                format!("Replace `\"` with `{replacement}` ({kind} quote)"),
            );
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: PathBuf::new(),
                start,
                end,
                message: format!(
                    "straight double quote; use `` `` `` (opening) or `''` (closing) -- inferred {kind} here"
                ),
                fix: Some(fix),
            });
        }
    }
}

/// Guess whether the `"` at byte `offset` in `text` is an *opening* quote. A quote
/// preceded by whitespace, an opening delimiter (`(`, `[`, `{`), a backtick, or
/// nothing (start of document) opens; anything else closes. The character before
/// is read in-token when there is one, otherwise off the immediately preceding
/// token (trivia included, so whitespace and newlines are seen as such).
fn opens_here(tok: &SyntaxToken, text: &str, offset: usize) -> bool {
    let before = if offset > 0 {
        text[..offset].chars().next_back()
    } else {
        tok.prev_token().and_then(|t| t.text().chars().next_back())
    };
    match before {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '(' | '[' | '{' | '`'),
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
            if StraightQuotes.interests().contains(&el.kind()) {
                StraightQuotes.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_open_and_close_with_unsafe_fixes() {
        let src = "He said \"hello world\" to me.\n";
        let out = findings(src);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|d| d.rule == "straight-quotes"));
        // Opening quote after the space.
        let open = out[0].fix.as_ref().expect("a fix");
        assert_eq!(open.applicability, Applicability::Unsafe);
        assert_eq!(open.content, "``");
        // Closing quote after the `d` of `world`.
        assert_eq!(out[1].fix.as_ref().unwrap().content, "''");
        // Unsafe fixes are skipped without the opt-in, applied with it.
        let fixes: Vec<_> = out.iter().map(|d| d.fix.clone().unwrap()).collect();
        assert_eq!(apply_fixes(src, &fixes, false).applied, 0);
        assert_eq!(
            apply_fixes(src, &fixes, true).output,
            "He said ``hello world'' to me.\n"
        );
    }

    #[test]
    fn opening_after_paren_opens() {
        // `("quoted")` lexes as one WORD; the in-token `(` before the first quote
        // reads as an opening context, the `d` before the second as closing.
        let out = findings("(\"quoted\")\n");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].fix.as_ref().unwrap().content, "``");
        assert_eq!(out[1].fix.as_ref().unwrap().content, "''");
    }

    #[test]
    fn quote_at_document_start_opens() {
        let out = findings("\"Start.\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].fix.as_ref().unwrap().content, "``");
        assert_eq!((out[0].start, out[0].end), (0, 1));
    }

    #[test]
    fn single_quotes_are_not_flagged() {
        assert!(findings("don't say it's fine\n").is_empty());
    }

    #[test]
    fn correct_ligatures_are_clean() {
        assert!(findings("``already correct''\n").is_empty());
    }

    #[test]
    fn math_is_skipped() {
        assert!(findings("$x = \"y\"$\n").is_empty());
    }

    #[test]
    fn tight_span_on_each_quote() {
        // Span is exactly the one `"` byte, never the whole word.
        let out = findings("a\"b\n");
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].start, out[0].end), (1, 2));
    }
}
