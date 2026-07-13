//! `dash-length`: a dash of the wrong length for its context. Mirrors ChkTeX
//! rule 8 ("Wrong length of dash may have been used").
//!
//! LaTeX distinguishes three dashes by the number of ASCII hyphens: `-` (hyphen,
//! for compounds), `--` (en dash, for number ranges), and `---` (em dash, for a
//! parenthetical break). Two contexts have an unambiguous "right" length:
//!
//! - **Between numbers** a range takes an en dash, so `5-10` and `5---10` are
//!   wrong. The fix rewrites the run to `--`. It is `Unsafe`: an en dash changes
//!   the typeset glyph, and a hyphen between numbers is occasionally intentional
//!   (a part number, a negative in a hyphenated coordinate), so `--fix` leaves it
//!   alone; `--unsafe-fixes` and the editor code action apply it. Correct by
//!   construction (tenet 1): `--` parses and the edit stays lossless.
//! - **Between words** an en dash (`--`) is almost always a mistake -- a hyphen
//!   joins a compound (`well-known`) and an em dash (`---`) sets a break -- but
//!   *which* correct form was meant is genuinely ambiguous, so the finding is
//!   reported **without** a fix (tenet 1: withhold the ambiguous rewrite, still
//!   report). One exception is carved out: an en dash joining coordinate proper
//!   names of equal standing (`Barzilai--Borwein`, `Newton--Raphson`,
//!   `Cauchy--Schwarz`) is correct typography, so the finding is suppressed when
//!   the first letter of *either* flanking segment is uppercase. That leans toward
//!   false negatives, catching the common lowercase-compound slip (`well--known`)
//!   while never nagging a legitimate name pairing.
//!
//! To keep false positives out, the rule only inspects a dash run that sits
//! *inside* a single `WORD` with content on both sides **and** is the only dash
//! run in that word. That excludes dates (`2020-01-15`), ISBNs, phone numbers,
//! spaced dashes (a standalone `--` token has no in-word neighbor), and
//! leading/trailing option flags (`--verbose`), all of which lex with the run at a
//! word edge or alongside other runs. The rule reads only `WORD` tokens, so
//! comments, `\verb`, and verbatim (which never lex as `WORD`) are untouched, and
//! math is skipped (a `-` there is a minus, not a dash).

use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[
    Example {
        caption: "A hyphen where a number range wants an en dash:",
        source: "See pages 5-10 for the proof.\n",
    },
    Example {
        caption: "An en dash between words (ambiguous, so reported without a fix):",
        source: "A well--known result.\n",
    },
];

pub struct DashLength;

impl Rule for DashLength {
    fn id(&self) -> &'static str {
        "dash-length"
    }

    fn emits_fix(&self) -> bool {
        true
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a dash of the wrong length for its context (ChkTeX 8). LaTeX sets a \
         hyphen from `-`, an en dash from `--`, and an em dash from `---`. Between \
         two numbers a range takes an en dash, so `5-10` or `5---10` is flagged \
         with an **unsafe** fix to `--` (unsafe because it changes the typeset \
         glyph and a hyphen between numbers is occasionally intentional). Between \
         two words an en dash (`--`) is almost always a mistake, but whether a \
         hyphen or an em dash was meant is ambiguous, so it is reported **without** \
         a fix -- except when it joins coordinate proper names (`Barzilai--Borwein`, \
         `Newton--Raphson`), detected by an uppercase first letter on either flank, \
         where the en dash is correct and the finding is suppressed. To stay \
         conservative the rule only inspects a dash run that sits \
         inside a single word with content on both sides and is the only dash run \
         in that word, so dates (`2020-01-15`), ISBNs, spaced dashes, and option \
         flags (`--verbose`) are left alone. Comments, verbatim, and math are never \
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
        // Cheap reject: most words hold no dash at all.
        if !text.contains('-') {
            return;
        }
        // A `-` in math is a minus, not a dash; leave it alone.
        if ctx.in_math(usize::from(tok.text_range().start())) {
            return;
        }
        let Some((run_start, run_end)) = lone_internal_dash_run(text) else {
            return;
        };
        let before = text[..run_start].chars().next_back();
        let after = text[run_end..].chars().next();
        let len = run_end - run_start;
        let base = usize::from(tok.text_range().start());
        let start = base + run_start;
        let end = base + run_end;

        if is_digit(before) && is_digit(after) {
            // Number range: an en dash `--` is expected. A hyphen or an em dash is
            // wrong; the correct form is unambiguous, so offer an unsafe fix.
            if len == 2 {
                return;
            }
            let kind = if len == 1 { "hyphen" } else { "em dash" };
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: PathBuf::new(),
                start,
                end,
                message: format!("{kind} between numbers; use an en dash `--` for a number range"),
                fix: Some(Fix::unsafe_(
                    start,
                    end,
                    "--",
                    "Replace with an en dash `--`",
                )),
                related: Vec::new(),
            });
        } else if is_letter(before) && is_letter(after) && len == 2 {
            // En dash between capitalized names is a real convention -- an en dash
            // joins coordinate proper names of equal standing (`Barzilai--Borwein`,
            // `Newton--Raphson`, `Cauchy--Schwarz`). Suppress when the first letter of
            // *either* flanking segment is uppercase; that keeps the finding for genuine
            // lowercase-compound mistakes (`well--known`) while staying conservative
            // (we prefer false negatives here).
            let before_first = text[..run_start].chars().next();
            if is_upper(before_first) || is_upper(after) {
                return;
            }
            // En dash between words: usually a mistake, but a hyphen (compound) and
            // an em dash (break) are both plausible, so report without a fix.
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: PathBuf::new(),
                start,
                end,
                message:
                    "en dash `--` between words; use a hyphen `-` for a compound or an em dash `---` for a break"
                        .to_owned(),
                fix: None,
                related: Vec::new(),
            });
        }
    }
}

/// Find the single maximal run of `-` in `text`, returning its byte range only
/// when it is the *only* dash run and has content on both sides (never at a word
/// edge). Returning `None` for any word with zero, multiple, or edge-anchored runs
/// keeps dates, ISBNs, and option flags out of scope.
fn lone_internal_dash_run(text: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut found: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'-' {
            i += 1;
            continue;
        }
        let run_start = i;
        while i < bytes.len() && bytes[i] == b'-' {
            i += 1;
        }
        if found.is_some() {
            // A second run: not a lone dash, so out of scope.
            return None;
        }
        found = Some((run_start, i));
    }
    let (s, e) = found?;
    // Reject a run at either edge of the word (no in-word neighbor to classify).
    if s == 0 || e == bytes.len() {
        return None;
    }
    Some((s, e))
}

fn is_digit(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_ascii_digit())
}

fn is_letter(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphabetic())
}

fn is_upper(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_ascii_uppercase())
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
            if DashLength.interests().contains(&el.kind()) {
                DashLength.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_hyphen_between_numbers_with_unsafe_endash_fix() {
        let src = "See pages 5-10 now.\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "dash-length");
        // Caret on just the hyphen (byte 11).
        assert_eq!((out[0].start, out[0].end), (11, 12));
        let fix = out[0].fix.as_ref().expect("a fix");
        assert_eq!(fix.applicability, Applicability::Unsafe);
        assert_eq!(fix.content, "--");
        // Unsafe: skipped without the opt-in, applied with it.
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).applied,
            0
        );
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), true).output,
            "See pages 5--10 now.\n"
        );
    }

    #[test]
    fn flags_em_dash_between_numbers() {
        let out = findings("pages 5---10\n");
        assert_eq!(out.len(), 1);
        let fix = out[0].fix.as_ref().unwrap();
        assert_eq!(fix.content, "--");
        assert!(out[0].message.contains("em dash between numbers"));
    }

    #[test]
    fn en_dash_between_numbers_is_correct() {
        assert!(findings("pages 5--10 here\n").is_empty());
    }

    #[test]
    fn flags_en_dash_between_words_without_a_fix() {
        let out = findings("A well--known result.\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].fix.is_none());
        assert!(out[0].message.contains("between words"));
    }

    #[test]
    fn en_dash_between_proper_names_is_left_alone() {
        // An en dash joining coordinate proper names is correct typography.
        assert!(findings("the Barzilai--Borwein step size\n").is_empty());
        assert!(findings("a Newton--Raphson iteration\n").is_empty());
    }

    #[test]
    fn en_dash_suppressed_when_either_side_capitalized() {
        // Suppression is an OR: either flank being capitalized is enough.
        assert!(findings("a Foo--bar thing\n").is_empty());
        assert!(findings("a foo--Bar thing\n").is_empty());
    }

    #[test]
    fn hyphenated_compound_is_clean() {
        assert!(findings("a well-known result\n").is_empty());
    }

    #[test]
    fn em_dash_between_words_is_clean() {
        assert!(findings("a word---word break\n").is_empty());
    }

    #[test]
    fn iso_date_is_left_alone() {
        // Two dash runs -> not a lone dash, so out of scope.
        assert!(findings("dated 2020-01-15 today\n").is_empty());
    }

    #[test]
    fn leading_option_flag_is_left_alone() {
        // The run is at the word's leading edge; no in-word neighbor to classify.
        assert!(findings("pass --verbose to it\n").is_empty());
    }

    #[test]
    fn spaced_dash_is_left_alone() {
        // A standalone `--` token has content on neither side within the word.
        assert!(findings("a word -- another\n").is_empty());
    }

    #[test]
    fn math_minus_is_skipped() {
        assert!(findings("$5-10$\n").is_empty());
    }
}
