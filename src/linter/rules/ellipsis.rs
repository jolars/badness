//! `ellipsis`: a literal run of three (or more) periods where a real ellipsis
//! command belongs (`\dots` in text, `\ldots`/`\cdots` in math). Mirrors ChkTeX
//! rule 11 and `lacheck`.
//!
//! Typing `...` sets three tight full stops with ordinary inter-letter spacing;
//! LaTeX's ellipsis commands set correctly spaced dots. The right command
//! depends on mode:
//!
//! - **Text.** `\dots` is the kernel's text ellipsis and is unambiguous, so the
//!   fix is `Safe`: it swaps just the dot run for `\dots`. A trailing space is
//!   appended only when the run is immediately followed by an ASCII letter
//!   (`foo...bar` -> `foo\dots bar`), so the control word cannot glue onto the
//!   following word; TeX absorbs that space, so meaning is preserved. Correct by
//!   construction (tenet 1): the result parses and stays lossless.
//! - **Math.** `\ldots` (baseline, for lists like `a_1, \ldots, a_n`) and
//!   `\cdots` (centered, for `a_1 + \cdots + a_n`) are *not* interchangeable; the
//!   right one depends on the neighboring atoms. We guess from the adjacent
//!   characters — an operator/relation neighbor (`+ - * / = < >`) picks `\cdots`,
//!   otherwise `\ldots` — and mark the fix `Unsafe`, since the guess can be wrong
//!   and the choice changes the typeset output. `--fix` leaves it alone;
//!   `--unsafe-fixes` and the editor code action apply it.
//!
//! The rule reads only `WORD` tokens, so comments, `\verb`, and verbatim
//! environments (which never lex as `WORD`) are untouched — protected regions
//! stay protected. Whether a run sits in math is read straight off the CST (a
//! `MATH` ancestor), never re-lexed.

use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxToken};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[
    Example {
        caption: "Literal dots in text:",
        source: "See Chapter 2, 3, ... for details.\n",
    },
    Example {
        caption: "Literal dots in a math sum (an operator neighbor picks `\\cdots`):",
        source: "$a_1 + ... + a_n$\n",
    },
];

pub struct Ellipsis;

impl Rule for Ellipsis {
    fn id(&self) -> &'static str {
        "ellipsis"
    }

    fn emits_fix(&self) -> bool {
        true
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a literal run of three or more periods (`...`) where a real \
         ellipsis command belongs. `...` sets three tight full stops; LaTeX's \
         ellipsis commands set correctly spaced dots. In text the fix is a \
         **safe** swap to `\\dots` (a space is added before a following letter so \
         the control word cannot glue onto the next word). In math `\\ldots` \
         (baseline, for comma lists) and `\\cdots` (centered, for operator \
         chains) are not interchangeable, so the fix is **unsafe**: it guesses \
         from the neighboring atoms -- an operator or relation picks `\\cdots`, \
         otherwise `\\ldots` -- and applies only under `--unsafe-fixes` or as an \
         editor code action. Comments and verbatim are never touched."
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
        // Cheap reject: most words hold no dot run at all.
        if !text.contains("...") {
            return;
        }
        let base = usize::from(tok.text_range().start());
        let in_math = ctx.in_math(base);
        let bytes = text.as_bytes();

        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'.' {
                i += 1;
                continue;
            }
            let run_start = i;
            while i < bytes.len() && bytes[i] == b'.' {
                i += 1;
            }
            if i - run_start < 3 {
                continue;
            }
            let run_end = i;
            // A single space keeps `\dots`/`\ldots` from gluing onto a directly
            // following ASCII letter; TeX absorbs it, so meaning is preserved.
            let needs_space = text[run_end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic());
            let (command, applicability) = if in_math {
                (math_command(tok, text, run_start, run_end), false)
            } else {
                ("\\dots", true)
            };
            let replacement = if needs_space {
                format!("{command} ")
            } else {
                command.to_owned()
            };
            let start = base + run_start;
            let end = base + run_end;
            let description = format!("Replace `...` with `{command}`");
            let fix = if applicability {
                Fix::safe(start, end, replacement, description)
            } else {
                Fix::unsafe_(start, end, replacement, description)
            };
            let message = if in_math {
                format!(
                    "literal `...` ellipsis; use `{command}` in math (`\\ldots` for lists, `\\cdots` for operator chains)"
                )
            } else {
                "literal `...` ellipsis; use `\\dots`".to_owned()
            };
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: PathBuf::new(),
                start,
                end,
                message,
                fix: Some(fix),
                related: Vec::new(),
            });
        }
    }
}

/// Pick `\cdots` vs `\ldots` for a math dot run from its neighboring atoms: an
/// operator or relation on either side (`a + ... + b`, `x = ... = y`) reads as a
/// centered `\cdots`; anything else (comma lists, juxtaposition) defaults to the
/// baseline `\ldots`. A heuristic — hence the `Unsafe` fix.
fn math_command(tok: &SyntaxToken, text: &str, run_start: usize, run_end: usize) -> &'static str {
    let before = if run_start > 0 {
        text[..run_start].chars().next_back()
    } else {
        neighbor_char(tok.prev_token(), SyntaxToken::prev_token, |t| {
            t.chars().next_back()
        })
    };
    let after = if run_end < text.len() {
        text[run_end..].chars().next()
    } else {
        neighbor_char(tok.next_token(), SyntaxToken::next_token, |t| {
            t.chars().next()
        })
    };
    if is_operator(before) || is_operator(after) {
        "\\cdots"
    } else {
        "\\ldots"
    }
}

/// Walk `step` from `first`, skipping whitespace/newline/comment trivia, and read
/// a boundary character off the first content token with `pick`.
fn neighbor_char(
    first: Option<SyntaxToken>,
    step: fn(&SyntaxToken) -> Option<SyntaxToken>,
    pick: fn(&str) -> Option<char>,
) -> Option<char> {
    let mut cur = first;
    while let Some(tok) = cur {
        match tok.kind() {
            SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT => {
                cur = step(&tok);
            }
            _ => return pick(tok.text()),
        }
    }
    None
}

/// Whether `c` is a binary operator or relation character that reads as a
/// centered-dots (`\cdots`) context.
fn is_operator(c: Option<char>) -> bool {
    matches!(c, Some('+' | '-' | '*' | '/' | '=' | '<' | '>'))
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
            if Ellipsis.interests().contains(&el.kind()) {
                Ellipsis.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_text_ellipsis_with_safe_dots_fix() {
        let src = "one, two, ...\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "ellipsis");
        // Caret on just the three dots (bytes 10..13).
        assert_eq!((out[0].start, out[0].end), (10, 13));
        let fix = out[0].fix.as_ref().expect("a fix");
        assert_eq!(fix.applicability, Applicability::Safe);
        assert_eq!(fix.content, "\\dots");
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).output,
            "one, two, \\dots\n"
        );
    }

    #[test]
    fn adds_space_before_following_letter() {
        let src = "foo...bar\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        // Dots at bytes 3..6 inside the `foo...bar` word.
        assert_eq!((out[0].start, out[0].end), (3, 6));
        let fix = out[0].fix.as_ref().expect("a fix");
        assert_eq!(fix.content, "\\dots ");
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).output,
            "foo\\dots bar\n"
        );
    }

    #[test]
    fn math_operator_neighbor_picks_cdots_unsafe() {
        let src = "$a + ... + b$\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        let fix = out[0].fix.as_ref().expect("a fix");
        assert_eq!(fix.applicability, Applicability::Unsafe);
        assert_eq!(fix.content, "\\cdots");
        // Unsafe fix is skipped without the opt-in, applied with it.
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).applied,
            0
        );
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), true).output,
            "$a + \\cdots + b$\n"
        );
    }

    #[test]
    fn math_comma_list_picks_ldots() {
        // `a,...,b` lexes as one WORD; the in-token comma neighbors pick `\ldots`.
        let out = findings("$a,...,b$\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].fix.as_ref().unwrap().content, "\\ldots");
    }

    #[test]
    fn math_letter_neighbors_default_to_ldots_with_space() {
        // `1...n`: neither neighbor is an operator -> `\ldots`, and the trailing
        // `n` forces a space so the control word does not glue.
        let out = findings("$1...n$\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].fix.as_ref().unwrap().content, "\\ldots ");
    }

    #[test]
    fn two_dots_are_not_flagged() {
        assert!(findings("wait.. what\n").is_empty());
    }

    #[test]
    fn existing_commands_are_clean() {
        assert!(findings("a\\dots b and $x_1,\\ldots,x_n$\n").is_empty());
    }

    #[test]
    fn flags_each_run() {
        assert_eq!(findings("a... b...\n").len(), 2);
    }
}
