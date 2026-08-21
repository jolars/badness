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
//! **A quotation is one finding, not two.** An inferred opening quote is held
//! until its closer arrives, and the pair is reported once, spanning the whole
//! quotation, with a *single* fix carrying both edits. Fix edits are atomic
//! (`linter/fix.rs`), so `` `` `` and `''` can never half-apply, and the editor
//! offers one code action that repairs the pair from either end -- the two-finding
//! shape made a reader fix each quote separately. Pairing rides the driver's
//! shared walk as a [`StreamVisitor`], since a closer's partner is a fact about
//! the element *sequence* that a stateless per-token check cannot carry.
//!
//! An **unpaired** quote still reports on its own, with the single-edit fix its
//! inferred direction gives: an opening quote whose closer never arrives (or that
//! a second opening quote supersedes) flushes as a solo finding, as does a closing
//! quote with nothing pending. A **blank line ends the quotation** -- a `"` left
//! open at a paragraph break is unterminated, not a partner for the next
//! paragraph's quote -- which bounds how far a wrong pairing can reach.
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

use crate::linter::diagnostic::{Diagnostic, Edit, Fix, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxToken};

use super::{Example, Rule, RuleContext, StreamVisitor};

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
         open and `''` (two apostrophes) to close. A quotation is reported **once**, \
         spanning both quotes, and its fix rewrites the pair in one atomic edit -- \
         so a single editor code action repairs it from either end. A quote left \
         unpaired (no closer before the paragraph ends) reports on its own. The fix \
         is **unsafe**: it infers direction from context -- a quote preceded by \
         whitespace, a line break, an opening delimiter (`(`, `[`, `{`), a backtick, \
         or the start of the document opens, anything else closes -- and applies \
         only under `--unsafe-fixes` or as an editor code action, since the guess \
         can flip the typeset glyph. Single straight quotes (`'`) are left alone \
         (they are legitimately apostrophes), and comments, verbatim, math, TeX hex \
         constants (`\"2D`), and `\\pdfmapline` font maps are never touched."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn stream(&self) -> Option<Box<dyn StreamVisitor>> {
        Some(Box::new(StraightQuotesVisitor::default()))
    }
}

/// Carries an inferred *opening* quote across the shared walk until its closer
/// arrives, so a quotation reports as one finding with one paired fix.
#[derive(Default)]
struct StraightQuotesVisitor {
    /// Byte offset of an opening `"` still waiting for its partner.
    pending: Option<usize>,
    /// Consecutive `NEWLINE` tokens since the last content token. Two of them is a
    /// `\par`, which ends any pending quotation. `WHITESPACE` does not break the
    /// run (a blank line may carry indentation); a `COMMENT` does, since a comment
    /// line is not a paragraph break.
    newlines: usize,
}

impl StreamVisitor for StraightQuotesVisitor {
    fn visit(&mut self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(tok) = el.as_token() else {
            return;
        };
        match tok.kind() {
            SyntaxKind::NEWLINE => {
                self.newlines += 1;
                if self.newlines >= BLANK_LINE_NEWLINES {
                    self.flush(sink);
                }
                return;
            }
            SyntaxKind::WHITESPACE => return,
            _ => self.newlines = 0,
        }
        if tok.kind() != SyntaxKind::WORD {
            return;
        }
        let text = tok.text();
        // Cheap reject: most words hold no straight quote at all.
        if !text.contains('"') {
            return;
        }
        // A straight `"` in math is not a quotation mark; leave it alone.
        if !ctx.in_text(usize::from(tok.text_range().start())) {
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
            self.record(base + offset, opens_here(tok, text, offset), sink);
        }
    }

    fn finish(&mut self, _ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        self.flush(sink);
    }
}

/// A blank line is two consecutive newlines, matching the parser's own threshold.
const BLANK_LINE_NEWLINES: usize = 2;

impl StraightQuotesVisitor {
    /// Take the quote at `start` into the pairing state. An opening quote is held
    /// for its closer; a second opening quote supersedes the one it finds pending,
    /// flushing that as unpaired (an unterminated quotation, not a partner). A
    /// closing quote consumes a pending opener into one paired finding, or reports
    /// alone when there is none.
    fn record(&mut self, start: usize, opening: bool, sink: &mut Vec<Diagnostic>) {
        if opening {
            if let Some(stale) = self.pending.replace(start) {
                sink.push(solo(stale, true));
            }
        } else if let Some(open) = self.pending.take() {
            sink.push(pair(open, start));
        } else {
            sink.push(solo(start, false));
        }
    }

    /// Report a still-pending opening quote as unpaired. Called at a paragraph
    /// break and at end of file.
    fn flush(&mut self, sink: &mut Vec<Diagnostic>) {
        if let Some(open) = self.pending.take() {
            sink.push(solo(open, true));
        }
    }
}

/// The finding for a matched quotation: one diagnostic spanning both quotes, whose
/// single fix carries both edits. Fix edits apply atomically, so the pair is
/// rewritten together or not at all, and the span covers the quotation so the
/// editor offers the action with the caret at either end.
fn pair(open: usize, close: usize) -> Diagnostic {
    let fix = Fix::unsafe_edits(
        vec![
            Edit::new(open, open + 1, "``"),
            Edit::new(close, close + 1, "''"),
        ],
        "Replace the straight quotes with `` `` `` and `''`",
    );
    Diagnostic {
        rule: StraightQuotes.id(),
        severity: StraightQuotes.default_severity(),
        path: PathBuf::new(),
        start: open,
        end: close + 1,
        message: "straight double quotes; use `` `` `` (opening) and `''` (closing)".to_owned(),
        fix: Some(fix),
        related: Vec::new(),
    }
}

/// The finding for a quote with no partner: the span is the one `"` byte and the
/// fix rewrites it alone, in the direction [`opens_here`] inferred.
fn solo(start: usize, opening: bool) -> Diagnostic {
    let (replacement, kind) = if opening {
        ("``", "opening")
    } else {
        ("''", "closing")
    };
    let end = start + 1;
    Diagnostic {
        rule: StraightQuotes.id(),
        severity: StraightQuotes.default_severity(),
        path: PathBuf::new(),
        start,
        end,
        message: format!(
            "straight double quote; use `` `` `` (opening) or `''` (closing) -- inferred {kind} here"
        ),
        fix: Some(Fix::unsafe_(
            start,
            end,
            replacement,
            format!("Replace `\"` with `{replacement}` ({kind} quote)"),
        )),
        related: Vec::new(),
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
        let mut visitor = StraightQuotes.stream().expect("a stream visitor");
        for el in root.descendants_with_tokens() {
            visitor.visit(&el, &ctx, &mut out);
        }
        visitor.finish(&ctx, &mut out);
        // The driver sorts findings by position; do the same so the tests read in
        // document order even when a deferred flush lands last.
        out.sort_by_key(|d| (d.start, d.end));
        out
    }

    /// The `(start, end, content)` triples of a finding's fix, in edit order.
    fn edits(d: &Diagnostic) -> Vec<(usize, usize, &str)> {
        d.fix
            .as_ref()
            .expect("a fix")
            .edits
            .iter()
            .map(|e| (e.start, e.end, e.content.as_str()))
            .collect()
    }

    #[test]
    fn a_quotation_is_one_finding_with_one_paired_fix() {
        let src = "He said \"hello world\" to me.\n";
        let out = findings(src);
        assert_eq!(out.len(), 1, "the pair is one finding, not two: {out:?}");
        assert_eq!(out[0].rule, "straight-quotes");
        // The span covers the whole quotation, so an editor offers the action with
        // the caret at either quote.
        assert_eq!((out[0].start, out[0].end), (8, 21));
        // One fix, both edits.
        let fix = out[0].fix.as_ref().expect("a fix");
        assert_eq!(fix.applicability, Applicability::Unsafe);
        assert_eq!(edits(&out[0]), [(8, 9, "``"), (20, 21, "''")]);
        // Unsafe fixes are skipped without the opt-in, applied with it.
        let fixes = vec![fix.clone()];
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
        assert_eq!(out.len(), 1);
        assert_eq!(edits(&out[0]), [(1, 2, "``"), (8, 9, "''")]);
    }

    #[test]
    fn a_pair_spanning_lines_still_reports_once() {
        // A single newline is not a paragraph break, so the quotation stays open
        // across it and still pairs.
        let src = "He said \"hello\nworld\" to me.\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(
            apply_fixes(src, &[out[0].fix.clone().unwrap()], true).output,
            "He said ``hello\nworld'' to me.\n"
        );
    }

    #[test]
    fn quotations_pair_independently() {
        let out = findings("say \"one\" then \"two\" now\n");
        assert_eq!(out.len(), 2);
        assert_eq!(edits(&out[0]), [(4, 5, "``"), (8, 9, "''")]);
        assert_eq!(edits(&out[1]), [(15, 16, "``"), (19, 20, "''")]);
    }

    #[test]
    fn an_unpaired_opening_quote_reports_alone() {
        // No closer before the file ends: the pending opener flushes as a solo
        // finding with the single-edit fix its inferred direction gives.
        let out = findings("he said \"hello\n");
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].start, out[0].end), (8, 9));
        assert_eq!(edits(&out[0]), [(8, 9, "``")]);
        assert!(out[0].message.contains("inferred opening here"));
    }

    #[test]
    fn a_blank_line_ends_a_pending_quotation() {
        // The unterminated quote in the first paragraph must not pair with the
        // second paragraph's quote; each reports alone.
        let out = findings("open \"here\n\nand \"there\n");
        assert_eq!(out.len(), 2);
        assert_eq!(edits(&out[0]), [(5, 6, "``")]);
        assert_eq!(edits(&out[1]), [(16, 17, "``")]);
    }

    #[test]
    fn a_comment_line_does_not_end_a_quotation() {
        // A comment line is not a paragraph break, so the quotation still pairs.
        let out = findings("say \"hello\n% a note\nworld\" now\n");
        assert_eq!(out.len(), 1);
        assert_eq!(edits(&out[0]), [(4, 5, "``"), (25, 26, "''")]);
    }

    #[test]
    fn a_second_opening_quote_supersedes_the_pending_one() {
        // `"a "b" ` -- the first quote never closes; it reports alone and the
        // second pairs with the closer.
        let out = findings("\"a \"b\" c\n");
        assert_eq!(out.len(), 2);
        assert_eq!(edits(&out[0]), [(0, 1, "``")]);
        assert_eq!(edits(&out[1]), [(3, 4, "``"), (5, 6, "''")]);
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
        // A `"` in ordinary text right next to such a command still flags, and the
        // skipped Lua quotes never enter the pairing state.
        let out = findings("say \"hi\" \\directlua{f(\"z\")}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(edits(&out[0]), [(4, 5, "``"), (7, 8, "''")]);
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
        // letter `l`, so it is prose, not a hex constant: the pair still flags.
        let out = findings("He said \"Alpha\" today.\n");
        assert_eq!(out.len(), 1);
        assert_eq!(edits(&out[0]), [(8, 9, "``"), (14, 15, "''")]);
    }

    #[test]
    fn all_hex_acronym_loses_only_opening_quote() {
        // Accepted false negative: `"CAFE"` reads as a hex run (`CAFE`) terminated
        // by `"`, so the opening quote is skipped; the closing one has no partner
        // to pair with and still flags alone.
        let out = findings("the \"CAFE\" run\n");
        assert_eq!(out.len(), 1);
        assert_eq!(edits(&out[0]), [(9, 10, "''")]);
    }

    #[test]
    fn tight_span_on_an_unpaired_quote() {
        // Span is exactly the one `"` byte, never the whole word.
        let out = findings("a\"b\n");
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].start, out[0].end), (1, 2));
    }
}
