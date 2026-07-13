//! `hard-coded-reference`: a literal cross-reference like `Figure 3`, `Table~1`,
//! or `Section 2` written in prose instead of `\ref`/`\cref` to a `\label`
//! (textidote sh:hcfig/hctab/hcsec).
//!
//! Hard-coding the number defeats LaTeX's automatic numbering: renumbering a
//! float or reordering sections silently breaks the reference, and the reader
//! loses the hyperlink. The convention is `\cref{fig:x}` (or `Figure~\ref{fig:x}`)
//! so the number tracks the target.
//!
//! **Report-only, heuristic.** The rule ships no autofix: the correct rewrite
//! needs the *label* the number refers to, which is nowhere in the text, so
//! there is nothing to synthesize (tenet 1 -- a fix owes correctness by
//! construction, and we cannot meet that here). It only reports the finding.
//!
//! To stay out of false positives the shape is deliberately narrow:
//!
//! - The reference word is a **capitalized** member of a curated list
//!   ([`REFERENCE_WORDS`]) matched exactly as a whole `WORD` token, so plurals
//!   (`Figures`), lowercase (`figure`), and glued forms (`(Figure`) are left
//!   alone -- conservative by design (false negatives beat false positives).
//! - It must be followed, across a single space or a tie `~` (never a paragraph
//!   break), by a `WORD` that is an **arabic number** (leading digit, no ASCII
//!   letters), so `Figure~\ref{x}` (a command follows) and `Figure three` (a
//!   word) never match.
//!
//! The rule reads only `WORD` tokens and skips math, so comments, `\verb`, and
//! verbatim (which never lex as `WORD`) are untouched.

use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[
    Example {
        caption: "A hard-coded figure number instead of a cross-reference:",
        source: "See Figure 3 for the results.\n",
    },
    Example {
        caption: "Even tied with `~`, the number is still hard-coded:",
        source: "Table~1 lists the parameters.\n",
    },
];

/// Capitalized reference words (and dotted abbreviations) that, when followed by
/// a bare number, signal a hard-coded cross-reference. Matched exactly against a
/// whole `WORD` token, so only these spellings fire.
const REFERENCE_WORDS: &[&str] = &[
    "Figure",
    "Fig.",
    "Table",
    "Tab.",
    "Section",
    "Sec.",
    "Chapter",
    "Chap.",
    "Equation",
    "Eq.",
    "Appendix",
    "Part",
    "Algorithm",
    "Alg.",
    "Listing",
    "Theorem",
    "Lemma",
    "Definition",
    "Corollary",
    "Proposition",
];

pub struct HardCodedReference;

impl Rule for HardCodedReference {
    fn id(&self) -> &'static str {
        "hard-coded-reference"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a literal cross-reference written in prose -- `Figure 3`, `Table~1`, \
         `Section 2` -- instead of `\\ref`/`\\cref` to a `\\label` (textidote \
         sh:hcfig/hctab/hcsec). Hard-coding the number defeats LaTeX's automatic \
         numbering: renumbering a float or reordering sections silently breaks the \
         reference and drops the hyperlink. The rule is **report-only** -- the \
         correct rewrite needs the label the number refers to, which is not in the \
         text, so no autofix is offered. To stay conservative it fires only for a \
         capitalized reference word (`Figure`, `Table`, `Section`, `Eq.`, ...) \
         matched as a whole word and directly followed, across one space or a tie \
         `~`, by an arabic number; plurals, lowercase, `Figure~\\ref{x}`, and \
         `Figure three` are left alone. It never touches math, comments, or \
         verbatim."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::WORD]
    }

    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(word) = el.as_token() else {
            return;
        };
        let kw = word.text();
        if !REFERENCE_WORDS.contains(&kw) {
            return;
        }
        // A `.` in math is not sentence punctuation, and a reference word there is
        // not prose; skip math entirely.
        if ctx.in_math(usize::from(word.text_range().start())) {
            return;
        }
        // The separator between the word and the number: either a tie `~`, or a
        // run of trivia (spaces/tabs/newlines) with no paragraph break. A blank
        // line (a second `NEWLINE`) means the number starts a new paragraph, so it
        // never reaches the numeric token below and is rejected.
        let Some(first) = word.next_token() else {
            return;
        };
        let (number, tie) = if first.kind() == SyntaxKind::TILDE {
            (first.next_token(), true)
        } else {
            // Skip a same-paragraph gap: one WHITESPACE and/or a single NEWLINE.
            let mut tok = Some(first);
            let mut newlines = 0;
            while let Some(t) = &tok {
                match t.kind() {
                    SyntaxKind::WHITESPACE => {}
                    SyntaxKind::NEWLINE => newlines += 1,
                    _ => break,
                }
                if newlines >= 2 {
                    return; // paragraph break
                }
                tok = t.next_token();
            }
            (tok, false)
        };
        let Some(number) = number else {
            return;
        };
        if number.kind() != SyntaxKind::WORD {
            return;
        }
        let Some(num_len) = numeric_ref_len(number.text()) else {
            return;
        };

        // Span the whole phrase, from the reference word through the number's
        // leading digit run (trailing punctuation like a comma or sentence period
        // is left out).
        let start = usize::from(word.text_range().start());
        let end = usize::from(number.text_range().start()) + num_len;
        let sep_display = if tie { "~" } else { " " };
        let phrase = format!("{kw}{sep_display}{}", &number.text()[..num_len]);
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start,
            end,
            message: format!(
                "hard-coded reference `{phrase}`; use `\\ref`/`\\cref` to a `\\label` so the number stays in sync"
            ),
            fix: None,
            related: Vec::new(),
        });
    }
}

/// Byte length of the numeric reference at the start of `text`, or `None` if the
/// token is not a bare arabic number. Qualifies when the first character is an
/// ASCII digit and the token contains no ASCII letters (so `3rd`, `v3`, and `3x3`
/// are rejected); the returned length covers the leading run of digits and
/// interior dots (`3.2`), trimming any trailing dot so a sentence period stays
/// out of the span.
fn numeric_ref_len(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    if !bytes.first().is_some_and(u8::is_ascii_digit) {
        return None;
    }
    if bytes.iter().any(u8::is_ascii_alphabetic) {
        return None;
    }
    let run = bytes
        .iter()
        .take_while(|&&b| b.is_ascii_digit() || b == b'.')
        .count();
    Some(text[..run].trim_end_matches('.').len())
}

#[cfg(test)]
mod tests {
    use super::*;
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
            if HardCodedReference.interests().contains(&el.kind()) {
                HardCodedReference.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_figure_number() {
        let src = "See Figure 3 here\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "hard-coded-reference");
        // Span covers `Figure 3` (bytes 4..12), not the trailing text.
        assert_eq!(&src[out[0].start..out[0].end], "Figure 3");
        // No autofix: report-only.
        assert!(out[0].fix.is_none());
    }

    #[test]
    fn flags_tie_separated_number() {
        let src = "Table~1 lists them\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(&src[out[0].start..out[0].end], "Table~1");
    }

    #[test]
    fn flags_dotted_abbreviation() {
        assert_eq!(findings("as in Fig. 3 above\n").len(), 1);
        assert_eq!(findings("see Eq.~2 for this\n").len(), 1);
    }

    #[test]
    fn ref_command_is_left_alone() {
        assert!(findings("Figure~\\ref{fig:x} shows\n").is_empty());
        assert!(findings("see Figure \\ref{fig:x}\n").is_empty());
    }

    #[test]
    fn lowercase_word_is_left_alone() {
        // Capitalization is the convention for a named reference; stay conservative.
        assert!(findings("in figure 3 we plot\n").is_empty());
    }

    #[test]
    fn plural_and_glued_forms_are_left_alone() {
        assert!(findings("Figures 3 and 4\n").is_empty());
        assert!(findings("(Figure 3)\n").is_empty());
    }

    #[test]
    fn spelled_out_number_is_left_alone() {
        assert!(findings("Figure three shows\n").is_empty());
    }

    #[test]
    fn bare_reference_word_is_left_alone() {
        assert!(findings("the Figure shows a plot\n").is_empty());
    }

    #[test]
    fn subpart_and_version_numbers_are_left_alone() {
        // `3a` (subfigure) and `v3` carry a letter, so they are not bare numbers.
        assert!(findings("Figure 3a shows\n").is_empty());
    }

    #[test]
    fn trailing_punctuation_stays_out_of_span() {
        let src = "See Figure 3, which\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        // The comma is left out of the span.
        assert_eq!(&src[out[0].start..out[0].end], "Figure 3");
    }

    #[test]
    fn decimal_number_is_flagged() {
        let src = "See Section 3.2 for\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(&src[out[0].start..out[0].end], "Section 3.2");
    }

    #[test]
    fn paragraph_break_is_left_alone() {
        // The number starts a new paragraph, not a reference.
        assert!(findings("Figure\n\n3\n").is_empty());
    }

    #[test]
    fn single_line_break_is_flagged() {
        // Wrapped source: `Figure` and `3` on adjacent lines is still a reference.
        assert_eq!(findings("see Figure\n3 here\n").len(), 1);
    }

    #[test]
    fn math_is_left_alone() {
        assert!(findings("$Section 2$\n").is_empty());
    }

    #[test]
    fn flags_each_occurrence() {
        assert_eq!(findings("Figure 3 and Table 4\n").len(), 2);
    }
}
