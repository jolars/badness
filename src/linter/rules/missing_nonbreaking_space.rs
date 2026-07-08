//! `missing-nonbreaking-space`: a plain space where a TeX tie (`~`) belongs,
//! between a word and a cross-reference or citation command.
//!
//! LaTeX typographic convention ties a reference to its preceding word with a
//! non-breaking space so the two never split across a line break:
//! `Figure~\ref{x}`, `Eq.~\eqref{z}`, `see~\cite{a}`. Authors routinely write a
//! plain space instead, letting LaTeX break the line between the word and the
//! number. This rule flags that space and offers a fix replacing it with `~`.
//!
//! The fix is `Unsafe` (the codebase's first): inserting a tie changes
//! line-breaking, which is exactly the spacing change `Applicability::Unsafe`
//! exists for. So `lint --fix` leaves it alone; `--unsafe-fixes` and the LSP
//! code action apply it. It is still correct by construction (tenet 1) — a
//! `TILDE` is valid wherever the whitespace stood, so the result parses and is
//! lossless.
//!
//! **Two gap shapes, one finding.** A same-line gap (`WORD WHITESPACE \cmd`) and
//! a single *source line break* (`Figure\n\ref{x}`) are both breakable spaces
//! LaTeX may split, so both are flagged. They differ only in the fix:
//!
//! - **Same-line space:** carries the `Unsafe` tie fix above (splice the space
//!   to `~`).
//! - **Single newline:** *report-only, no fix.* There is no `~`-insertion that
//!   keeps the line break—`Figure~\n\ref` and `Figure\n~\ref` both render as a
//!   tie plus a space (double glue). The only correct rewrite replaces the
//!   newline with `~`, which *joins the two source lines*: a reflow, and picking
//!   line breaks is the formatter's job (tenet 1, "a fix owes only correctness,
//!   never line-width"). So we report the finding and withhold the fix for this
//!   shape—the sanctioned move when a fix can't meet the correctness-only bar.
//!
//! A *blank line* (`\par`, two or more newlines) is never flagged: the reference
//! opens a new paragraph, so there is nothing to keep on the same line.
//!
//! The command table lives here, not in `data/signatures.json`: which commands
//! want a tie is a lint judgment, not the structural arity/verbatim fact the
//! signature DB carries (AGENTS.md core decision #2).
//!
//! **What earns a tie: an orphanable unit.** The tie convention exists so a line
//! break cannot strand a self-contained blob—a bare number/label, or a
//! bracketed/parenthetical/superscript citation—alone at a line edge. We flag a
//! command only when its *rendered output* is such a unit:
//!
//! - **Bare-number references** (`\ref`, `\eqref`, `\pageref`, `\labelcref`): the
//!   author writes the noun (`Figure \ref{x}` → "Figure 2"), so a break orphans a
//!   lone number. The canonical case.
//! - **Bracketed/parenthetical/superscript citations** (`\cite`, `\citep`,
//!   `\parencite`, `\autocite`, `\footcite`, `\supercite`, …): the whole citation
//!   is a self-contained blob (`(Smith 2020)`, `[3]`, a footnote mark) that can
//!   orphan at line start.
//!
//! Deliberately **not** flagged, because a break there orphans nothing:
//!
//! - **Self-describing references** (`\autoref`, `\cref`, `\Cref`, `\nameref`,
//!   `\vref`, `\Vref`, `\cpageref`, `\crefrange`, `\Crefrange`): these emit the
//!   noun themselves (`\autoref{x}` → "Figure 2"), so the space before them is
//!   ordinary prose ("in Figure 2"). cleveref also ties the noun to its number
//!   internally, so the only meaningful tie is one we cannot see.
//! - **Textual citations** (`\textcite`, `\citet`, `\citeauthor`, `\citeyear`,
//!   `\citealt`, `\citealp`, `\fullcite`): these weave an author name into the
//!   running prose (`\textcite{x}` → "Smith (2020)"), so there is no bracketed
//!   unit to strand.
//! - **`\nocite` and friends**: no visible output, so a tie is meaningless.

use std::path::PathBuf;

use crate::ast::command_name;
use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxToken};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "A plain space where a tie belongs before a cross-reference:",
    source: "see Figure \\ref{fig:plot}\n",
}];

/// Commands whose rendered output is an orphanable unit—a bare number/label or a
/// bracketed/parenthetical/superscript citation blob—that should be tied to the
/// preceding word. See the module docs for what is excluded and why: this omits
/// self-describing references (`\autoref`, `\cref`, …), textual citations
/// (`\textcite`, `\citet`, …), and `\nocite`.
const TIE_COMMANDS: &[&str] = &[
    // Bare-number / parenthetical-number references. The author supplies the
    // noun (`Figure \ref{x}` → "Figure 2"), so a break strands a lone number.
    "ref",
    "eqref",
    "pageref",
    "labelcref",
    // Bracketed / parenthetical / superscript citations (natbib + biblatex).
    // The whole citation is a self-contained blob a break can orphan.
    "cite",
    "citep",
    "citeyearpar",
    "parencite",
    "Parencite",
    "autocite",
    "Autocite",
    "footcite",
    "smartcite",
    "Smartcite",
    "supercite",
];

pub struct MissingNonbreakingSpace;

impl Rule for MissingNonbreakingSpace {
    fn id(&self) -> &'static str {
        "missing-nonbreaking-space"
    }

    fn emits_fix(&self) -> bool {
        true
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a plain space where a TeX tie (`~`) belongs, before a command whose \
         output a line break would orphan: a bare-number reference (`Figure \
         \\ref{x}`, `\\eqref`, `\\pageref`) or a bracketed citation (`see \
         \\cite{a}`, `\\parencite`, `\\autocite`). A tie keeps the reference on \
         the same line. Self-describing references (`\\autoref`, `\\cref`) and \
         textual citations (`\\textcite`, `\\citet`) are not flagged -- they emit \
         their own noun, so a break orphans nothing. Both a same-line space and a \
         single source line break before the command are flagged (a blank line is \
         not -- that starts a new paragraph). For a same-line space the fix is \
         **unsafe** -- inserting a tie changes line breaking -- so `--fix` leaves \
         it alone; `--unsafe-fixes` and the editor code action apply it. A line \
         break is report-only: rewriting the newline to `~` would join the two \
         lines, a reflow the formatter owns."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::COMMAND]
    }

    fn check(&self, el: &SyntaxElement, _ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(command) = el.as_node() else {
            return;
        };
        let Some(name) = command_name(command) else {
            return;
        };
        if !TIE_COMMANDS.contains(&name.as_str()) {
            return;
        }
        let Some(first) = command.first_token() else {
            return;
        };
        // Walk back over the inter-word gap immediately before the control word:
        // a contiguous run of WHITESPACE / NEWLINE trivia. It floats as siblings
        // (not absorbed into the COMMAND node) and `prev_token` crosses node
        // boundaries, so this reaches the real gap regardless of nesting. The
        // token that ends the walk is the anchor the reference would tie to.
        let mut run: Vec<SyntaxToken> = Vec::new();
        let mut newlines = 0usize;
        let mut anchor: Option<SyntaxToken> = None;
        let mut cursor = first.prev_token();
        while let Some(tok) = cursor {
            match tok.kind() {
                SyntaxKind::WHITESPACE => {}
                SyntaxKind::NEWLINE => newlines += 1,
                // A `~` (already tied), a `{`/`}`, a comment, or the anchor word.
                _ => {
                    anchor = Some(tok);
                    break;
                }
            }
            cursor = tok.prev_token();
            run.push(tok);
        }
        // No gap at all (the command abuts a `~`/brace): nothing to tie.
        let (Some(nearest), Some(earliest)) = (run.first(), run.last()) else {
            return;
        };
        // Require a real word before the gap so we never tie at sentence or
        // paragraph start, after `{`, or after another command's `}`.
        if anchor.map(|t| t.kind()) != Some(SyntaxKind::WORD) {
            return;
        }
        // A blank line (`\par`) starts a new paragraph: nothing to keep together.
        if newlines >= 2 {
            return;
        }

        let start = usize::from(earliest.text_range().start());
        let end = usize::from(nearest.text_range().end());
        // Same-line gap: replace the whole whitespace run (lexed as one token)
        // with a single tie. Correct by construction: a `TILDE` is valid here,
        // the splice is contiguous, so the result parses and stays lossless.
        // Unsafe because it alters line-breaking.
        //
        // A single source line break gets *no* fix: the only correct rewrite
        // (newline -> `~`) joins the two source lines, and that reflow is the
        // formatter's call (tenet 1), so we report the finding and withhold the
        // fix for this shape.
        let fix = (newlines == 0).then(|| {
            Fix::unsafe_(
                start,
                end,
                "~",
                format!("Replace the space before `\\{name}` with a non-breaking space `~`"),
            )
        });
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start,
            end,
            message: format!(
                "missing non-breaking space before `\\{name}`; use a tie `~` so the reference stays on the same line"
            ),
            fix,
        });
    }
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
        let ctx = RuleContext::new(std::path::Path::new("x.tex"), &root, &model, None, None);
        let mut out = Vec::new();
        for el in root.descendants_with_tokens() {
            if MissingNonbreakingSpace.interests().contains(&el.kind()) {
                MissingNonbreakingSpace.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_space_before_ref() {
        let out = findings("Figure \\ref{x}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "missing-nonbreaking-space");
        // Caret on the single space (byte 6..7), not the command.
        assert_eq!((out[0].start, out[0].end), (6, 7));
    }

    #[test]
    fn flags_space_before_cite() {
        assert_eq!(findings("see \\cite{a}\n").len(), 1);
    }

    #[test]
    fn accepts_word_ending_in_period() {
        // `.` is a word character, so `Eq.` is one WORD token and ties normally.
        assert_eq!(findings("Eq. \\eqref{z}\n").len(), 1);
    }

    #[test]
    fn tie_already_present_is_clean() {
        assert!(findings("Figure~\\ref{x}\n").is_empty());
    }

    #[test]
    fn command_at_input_start_is_clean() {
        assert!(findings("\\ref{x}\n").is_empty());
    }

    #[test]
    fn after_brace_is_clean() {
        // Inside a group (prev is `{`) and after a command's `}` (prev-prev is
        // `}`, not a WORD) both stay quiet.
        assert!(findings("{\\ref{x}}\n").is_empty());
        assert!(findings("\\textbf{a} \\ref{x}\n").is_empty());
    }

    #[test]
    fn nocite_is_not_flagged() {
        assert!(findings("foo \\nocite{x}\n").is_empty());
    }

    #[test]
    fn self_describing_refs_are_not_flagged() {
        // These emit the reference noun themselves ("Figure 2"), so the space
        // before them is ordinary prose, not an orphanable number.
        for cmd in ["autoref", "cref", "Cref", "nameref", "vref", "cpageref"] {
            assert!(
                findings(&format!("in \\{cmd}{{x}}\n")).is_empty(),
                "\\{cmd} should not be flagged"
            );
        }
    }

    #[test]
    fn textual_citations_are_not_flagged() {
        // These weave an author name into the prose ("Smith (2020)"), so there
        // is no bracketed unit to strand.
        for cmd in ["textcite", "Textcite", "citet", "citeauthor", "citeyear"] {
            assert!(
                findings(&format!("shown by \\{cmd}{{x}}\n")).is_empty(),
                "\\{cmd} should not be flagged"
            );
        }
    }

    #[test]
    fn parenthetical_citations_are_flagged() {
        // Self-contained citation blobs that a break can orphan.
        for cmd in ["parencite", "autocite", "footcite", "supercite", "citep"] {
            assert_eq!(
                findings(&format!("result \\{cmd}{{x}}\n")).len(),
                1,
                "\\{cmd} should be flagged"
            );
        }
    }

    #[test]
    fn single_newline_is_flagged_report_only() {
        // A single source line break is a breakable space too, so it is flagged
        // -- but with no fix (newline -> `~` would join the two lines, a reflow
        // the formatter owns).
        let out = findings("Figure\n\\ref{x}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "missing-nonbreaking-space");
        // Caret on the newline (byte 6..7).
        assert_eq!((out[0].start, out[0].end), (6, 7));
        assert!(out[0].fix.is_none(), "the newline shape carries no fix");
    }

    #[test]
    fn newline_with_indentation_is_flagged_report_only() {
        // Trailing indentation after the break is part of the same gap.
        let out = findings("Figure\n  \\cite{x}\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].fix.is_none());
    }

    #[test]
    fn blank_line_is_not_flagged() {
        // Two-plus newlines is a `\par`: the reference opens a new paragraph.
        assert!(findings("Figure\n\n\\ref{x}\n").is_empty());
    }

    #[test]
    fn multiple_spaces_collapse_to_one_tie() {
        use crate::linter::diagnostic::Applicability;

        let out = findings("Figure  \\ref{x}\n");
        assert_eq!(out.len(), 1);
        let fix = out[0].fix.as_ref().expect("should carry a fix");
        // The fix span covers both spaces (6..8) and replaces them with one tie.
        assert_eq!((fix.start, fix.end), (6, 8));
        assert_eq!(fix.content, "~");
        assert_eq!(fix.applicability, Applicability::Unsafe);
    }

    #[test]
    fn carries_unsafe_tie_fix() {
        use crate::linter::diagnostic::Applicability;
        use crate::linter::fix::apply_fixes;

        let src = "Figure \\ref{x}\n";
        let out = findings(src);
        let fix = out[0].fix.as_ref().expect("should carry a fix");
        assert_eq!(fix.applicability, Applicability::Unsafe);
        assert_eq!((fix.start, fix.end), (out[0].start, out[0].end));
        assert_eq!(fix.content, "~");
        // Applied with the unsafe opt-in, the space becomes a tie.
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), true).output,
            "Figure~\\ref{x}\n"
        );
        // Without the opt-in, the unsafe fix is skipped.
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).applied,
            0
        );
    }

    #[test]
    fn flags_each_occurrence() {
        assert_eq!(findings("Figure \\ref{a} and Table \\ref{b}\n").len(), 2);
    }
}
