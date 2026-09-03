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
//! math is skipped (a `-` there is a minus, not a dash). Several contexts are
//! skipped via the shared gates in `super`: a rule-command span (`\cline{1-3}`,
//! `\cmidrule(lr){2-3}` — the `n-m` is a column span, issue #34), a key argument
//! (`\label{fig:1-3}`, `\cite{smith2020-1}` — an opaque identifier), a
//! typewriter-font argument (`\texttt{03-02}` — monospace sets the hyphen
//! literally, with no en-dash ligature), and pgf/TikZ coordinate space — a
//! picture environment (`tikzpicture`, pgfplots `axis`, …) or a pgfmath-expression
//! argument (`\addplot3 {(y^2-1)^2}`, `\pgfmathparse{…}`) — where a `-` between
//! numbers is a subtraction the pgfmath parser evaluates, so the en-dash rewrite
//! would corrupt a meaning-bearing minus. Angle-delimited command and environment
//! specifications are skipped too: in Beamer's `\item<1-2>` and
//! `\begin{onlyenv}<2-3>`, the single hyphen is range syntax. None of these is
//! typeset range text.

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

    fn default_enabled(&self) -> bool {
        false
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
         flags (`--verbose`) are left alone. Column spans in rule commands \
         (`\\cline{1-3}`, `\\cmidrule(lr){2-3}`) and key arguments \
         (`\\label{fig:1-3}`, `\\cite{smith2020-1}`) are specs and opaque \
         identifiers rather than typeset ranges, so they are skipped too. The \
         same applies to angle-delimited command and environment specifications \
         such as Beamer's `\\item<1-2>` and `\\begin{onlyenv}<2-3>`. \
         Comments, verbatim, and math are never touched."
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
        if !ctx.in_text(usize::from(tok.text_range().start())) {
            return;
        }
        let Some((run_start, run_end)) = lone_internal_dash_run(text) else {
            return;
        };
        // Treat an angle-delimited specification directly after a command or
        // environment opener as macro syntax rather than prose. This catches
        // Beamer overlay ranges (`\item<1-2>`, `\only<2-3>{...}`), where replacing
        // the single hyphen with `--` silently changes which slides are produced.
        if in_angle_delimited_spec(tok, run_start, run_end) {
            return;
        }
        // A column span in a rule command (`\cline{1-3}`, `\cmidrule(lr){2-3}`)
        // and a key argument (`\label{fig:1-3}`, `\cite{smith2020-1}`) hold a
        // spec or an opaque identifier, never a typeset range. Monospace text
        // (`\texttt{03-02}`) sets the hyphen literally, with no en-dash ligature.
        if super::in_rule_span_argument(tok)
            || super::in_key_argument(tok, ctx)
            || super::in_typewriter_argument(tok)
        {
            return;
        }
        // pgf/TikZ coordinate and pgfmath-expression space: a `-` between numbers
        // is subtraction (`(y^2-1)^2`, `(2-1,3)`), not a typeset range, so the
        // en-dash rewrite would corrupt a meaning-bearing minus. Skip inside a
        // picture environment (`tikzpicture`, `axis`, …) and inside a pgfmath
        // argument (`\addplot3 {(y^2-1)^2}`, `\pgfmathparse{…}`).
        if super::in_pgf_picture(tok) || super::in_pgfmath_argument(tok) {
            return;
        }
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

/// Whether the dash run sits between `<` and `>` in a token directly following
/// a command or environment opener. With no authored gap, an argument protocol
/// is plausible enough to skip under the rule's false-negative bias; a spaced
/// expression such as `\foo <1-2>` remains in scope.
fn in_angle_delimited_spec(
    tok: &crate::syntax::SyntaxToken,
    run_start: usize,
    run_end: usize,
) -> bool {
    let text = tok.text();
    let before = &text[..run_start];
    let after = &text[run_end..];
    let Some(open) = before.rfind('<') else {
        return false;
    };
    if before[open + 1..].contains('>') {
        return false;
    }
    let Some(close) = after.find('>') else {
        return false;
    };
    if after[..close].contains('<') {
        return false;
    }

    let follows_command = matches!(
        tok.prev_sibling_or_token(),
        Some(SyntaxElement::Node(node)) if node.kind() == SyntaxKind::COMMAND
    );
    let follows_environment_begin = tok.parent_ancestors().any(|node| {
        node.kind() == SyntaxKind::ENVIRONMENT
            && node
                .children()
                .find(|child| child.kind() == SyntaxKind::BEGIN)
                .is_some_and(|begin| begin.text_range().end() == tok.text_range().start())
    });
    follows_command || follows_environment_begin
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
        assert_eq!(fix.edits[0].content, "--");
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
        assert_eq!(fix.edits[0].content, "--");
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

    #[test]
    fn prose_argument_ranges_are_checked() {
        let src = "\\textbf{pages 5-10}\\section{pages 5-10}\\footnote{pages 5-10}\n";
        assert_eq!(findings(src).len(), 3);
    }

    #[test]
    fn beamer_overlay_specs_are_left_alone() {
        let src = concat!(
            "\\item<1-2> First\n",
            "\\only<1-2|handout:1>{Second}\n",
            "\\only{Third}<2-3>\n",
            "\\onslide+<3-4> Third\n",
            "\\begin{actionenv}<4-5>Fourth\\end{actionenv}\n",
        );
        assert!(findings(src).is_empty());
    }

    #[test]
    fn angle_bracketed_prose_range_is_still_checked() {
        assert_eq!(
            findings("The interval <1-2> is closed; see \\LaTeX <3-4>.\n").len(),
            2
        );
    }

    #[test]
    fn a_redefined_text_command_does_not_inherit_its_builtin_domain() {
        let src = "\\renewcommand{\\text}[1]{\\ensuremath{#1}} $\\text{5-10}$\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn cline_span_is_left_alone() {
        // Issue #34: `1-3` in `\cline` is a column span, not a number range.
        assert!(findings("\\cline{1-3}\n").is_empty());
    }

    #[test]
    fn cmidrule_spans_are_left_alone() {
        // Attached (`{2-3}` greedily bound to the command) and detached (the
        // `(lr)` trim breaks greedy attachment, leaving the span a sibling).
        assert!(findings("\\cmidrule{2-3}\n").is_empty());
        assert!(findings("\\cmidrule[0.5pt]{4-5}\n").is_empty());
        assert!(findings("\\cmidrule(lr){2-3}\n").is_empty());
        assert!(findings("\\cmidrule(lr){2-3} \\cmidrule(r){4-5}\n").is_empty());
    }

    #[test]
    fn key_arguments_are_left_alone() {
        // A label or cite key is an opaque identifier, not a typeset range.
        assert!(findings("\\label{fig:1-3}\n").is_empty());
        assert!(findings("\\cite{smith2020-1}\n").is_empty());
        assert!(findings("See \\ref{sec:2-4} now.\n").is_empty());
    }

    #[test]
    fn texttt_hyphen_is_left_alone() {
        // Monospace sets the hyphen literally (an MSC code, a version string); the
        // `--`/`---` ligatures are off, so there is no en dash to reach for.
        assert!(findings("\\texttt{03-02}\n").is_empty());
        assert!(findings("the class \\texttt{03-02} covers it\n").is_empty());
    }

    #[test]
    fn text_outside_texttt_is_still_flagged() {
        // The gate is scoped to the argument; prose around it still flags.
        assert_eq!(findings("pages 5-10, see \\texttt{03-02}\n").len(), 1);
    }

    #[test]
    fn pgfmath_expression_argument_is_left_alone() {
        // `(y^2-1)^2` is a pgfmath subtraction, not a number range; the en-dash
        // rewrite would corrupt the minus. The `3` detaches the group from
        // `\addplot`, so this exercises the detached-argument walk.
        assert!(findings("\\addplot3 {(y^2-1)^2};\n").is_empty());
        // Attached form: `\pgfmathparse{y-1}`.
        assert!(findings("\\pgfmathparse{y-1}\n").is_empty());
    }

    #[test]
    fn pgf_picture_coordinate_is_left_alone() {
        // Coordinate arithmetic inside a picture environment is subtraction, not a
        // range: `(2-1,3)` must not become `(2--1,3)`.
        assert!(
            findings("\\begin{tikzpicture}\n\\node at (2-1,3) {x};\n\\end{tikzpicture}\n")
                .is_empty()
        );
        // pgfplots `axis` is a picture environment too.
        assert!(findings("\\begin{axis}\n\\addplot {(y^2-1)^2};\n\\end{axis}\n").is_empty());
    }

    #[test]
    fn prose_range_outside_pgf_is_still_flagged() {
        // The gates are scoped to pgf context; ordinary prose still flags.
        assert_eq!(findings("See pages 5-10 now.\n").len(), 1);
    }

    #[test]
    fn uncurated_multicolumn_content_is_unknown() {
        // No positional text-domain claim has been curated for `\multicolumn`,
        // so text-only diagnostics stay out of all of its arguments.
        assert!(findings("\\multicolumn{2}{c}{pages 5-10}\n").is_empty());
    }
}
