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
//!
//! Two more contexts where a `"` is not quotation are skipped. A **TeX hex
//! constant** (`\mathchardef\mdash="2D`, `\DeclareMathSymbol{...}{"AC}`): a `"`
//! before uppercase hex digits introduces a hexadecimal number, and rewriting it
//! breaks the constant ([`is_hex_constant`]). And a **font-map line**
//! (`\pdfmapline{... " .167 SlantFont"}`): the `"` there delimits a PostScript
//! transform, not a quotation ([`super::in_pdfmap_argument`]).

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
         legitimately apostrophes), and comments, verbatim, math, TeX hex \
         constants (`\"2D`), and `\\pdfmapline` font maps are never touched."
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
        // Inside `\directlua{…}` and kin the body is Lua source, so a `"` is a
        // string delimiter, not quotation (issue: cvd's embedded Lua). Skip it.
        if super::in_code_argument(tok) {
            return;
        }
        // Inside `\pdfmapline{…}` the `"` delimits a PostScript transform in a
        // font-map entry, not quotation; skip the whole argument.
        if super::in_pdfmap_argument(tok) {
            return;
        }
        let base = usize::from(tok.text_range().start());

        for (offset, _) in text.match_indices('"') {
            // A `"` introducing a TeX hex constant (`"2D`, `"AC`) is a number, not
            // a quotation mark; rewriting it would break the constant.
            if is_hex_constant(&text[offset + 1..]) {
                continue;
            }
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
                related: Vec::new(),
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

/// Whether the text immediately after a `"` reads as a TeX **hex constant**
/// (`"2D`, `"AC`) rather than the start of a quotation. TeX's `"` scans a
/// hexadecimal number whose digits are `0-9` and *uppercase* `A-F` only (lowercase
/// `a-f` are not hex digits), so a real constant is one-or-more such digits
/// terminated by a non-letter boundary — end of the token, or a non-alphabetic
/// character (`}`, `=`, space, `\`). A prose quote, by contrast, is followed by a
/// letter (`"Alpha"`) or a non-hex character, so it is not skipped. Command-
/// agnostic on purpose: this catches `\mathchardef`, `\mathchar`, `\chardef`,
/// `\char`, `\mathcode`, `\DeclareMathSymbol`, … uniformly, and the *bare*
/// assignment form (`\mathchardef\mdash="2D`, no brace group) as well. Since word
/// characters glue, `="2D` lexes as one `WORD`, so the hex run and its boundary are
/// always in-token. A false negative (a quoted all-hex acronym like `"CAFE"` loses
/// its opening-quote finding) is the safe direction (AGENTS.md linter posture): no
/// fix means no corruption.
fn is_hex_constant(after: &str) -> bool {
    let run = after
        .bytes()
        .take_while(|b| matches!(b, b'0'..=b'9' | b'A'..=b'F'))
        .count();
    run > 0
        && after[run..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphabetic())
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
        assert_eq!(open.edits[0].content, "``");
        // Closing quote after the `d` of `world`.
        assert_eq!(out[1].fix.as_ref().unwrap().edits[0].content, "''");
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
        assert_eq!(out[0].fix.as_ref().unwrap().edits[0].content, "``");
        assert_eq!(out[1].fix.as_ref().unwrap().edits[0].content, "''");
    }

    #[test]
    fn quote_at_document_start_opens() {
        let out = findings("\"Start.\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].fix.as_ref().unwrap().edits[0].content, "``");
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
    fn lua_string_literals_are_skipped() {
        // The `"` inside `\directlua{…}` are Lua string delimiters, not quotation.
        assert!(findings("\\directlua{lfs = require(\"lfs\")}\n").is_empty());
        assert!(findings("\\luadirect{token.set_macro(\"x\", \"y\")}\n").is_empty());
        // A `"` in ordinary text right next to such a command still flags.
        assert_eq!(findings("say \"hi\" \\directlua{f(\"z\")}\n").len(), 2);
    }

    #[test]
    fn hex_constant_bare_assignment_is_skipped() {
        // `\mathchardef\mdash="2D` — the `"2D` is a hex number, not quotation.
        assert!(findings("\\mathchardef\\mdash=\"2D\n").is_empty());
        // Other `"`-hex primitives are covered command-agnostically.
        assert!(findings("\\mathcode`\\-=\"2D\n").is_empty());
        assert!(findings("\\chardef\\x=\"7F\n").is_empty());
    }

    #[test]
    fn hex_constant_in_braced_slot_is_skipped() {
        let src = "\\DeclareMathSymbol{\\mdash}{\\mathalpha}{operators}{\"2D}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn pdfmapline_delimiters_are_skipped() {
        // The `"` there delimit a PostScript transform, not quotation.
        let src = "\\pdfmapline{+font <font.pfb \" -.25 SlantFont \" <font2.pfb}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn prose_quote_before_hex_letter_word_still_flags() {
        // `"Alpha"` — the opening `"` is before `A`, but `A` is followed by the
        // letter `l`, so it is prose, not a hex constant: both quotes still flag.
        let out = findings("He said \"Alpha\" today.\n");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].fix.as_ref().unwrap().edits[0].content, "``");
        assert_eq!(out[1].fix.as_ref().unwrap().edits[0].content, "''");
    }

    #[test]
    fn all_hex_acronym_loses_only_opening_quote() {
        // Accepted false negative: `"CAFE"` reads as a hex run (`CAFE`) terminated
        // by `"`, so the opening quote is skipped; the closing one still flags.
        let out = findings("the \"CAFE\" run\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].fix.as_ref().unwrap().edits[0].content, "''");
    }

    #[test]
    fn tight_span_on_each_quote() {
        // Span is exactly the one `"` byte, never the whole word.
        let out = findings("a\"b\n");
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].start, out[0].end), (1, 2));
    }
}
