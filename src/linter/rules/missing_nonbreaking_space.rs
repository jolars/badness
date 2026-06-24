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
//! Scope is deliberately tight: only a same-line `WORD WHITESPACE \cmd` shape is
//! flagged. A *source line break* before the command (`Figure\n\ref{x}`) is also
//! a breakable space in LaTeX, but replacing a newline with `~` reflows the
//! source and overlaps the formatter's job, so it is left for a later pass.
//!
//! The command table lives here, not in `data/signatures.json`: which commands
//! want a tie is a lint judgment, not the structural arity/verbatim fact the
//! signature DB carries (AGENTS.md core decision #2). `\nocite` and friends are
//! excluded — they produce no visible output, so a tie before them is
//! meaningless.

use std::path::PathBuf;

use crate::ast::command_name;
use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Rule, RuleContext};

/// Cross-reference and citation commands whose visible output should be tied to
/// the preceding word. Excludes `\nocite` (no output → no tie).
const TIE_COMMANDS: &[&str] = &[
    // Citation family (natbib + biblatex).
    "cite",
    "citep",
    "citet",
    "citealp",
    "citealt",
    "citeauthor",
    "citeyear",
    "citeyearpar",
    "parencite",
    "Parencite",
    "textcite",
    "Textcite",
    "autocite",
    "Autocite",
    "footcite",
    "smartcite",
    "Smartcite",
    "supercite",
    "fullcite",
    // Cross-reference family (latex + amsmath + cleveref + varioref).
    "ref",
    "eqref",
    "cref",
    "Cref",
    "autoref",
    "nameref",
    "pageref",
    "vref",
    "Vref",
    "cpageref",
    "labelcref",
    "crefrange",
    "Crefrange",
];

pub struct MissingNonbreakingSpace;

impl Rule for MissingNonbreakingSpace {
    fn id(&self) -> &'static str {
        "missing-nonbreaking-space"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
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
        // The token immediately before the control word. Trivia floats as a
        // sibling (it is not absorbed into the COMMAND node), and `prev_token`
        // walks globally across node boundaries, so this reaches the real
        // preceding token regardless of nesting.
        let Some(ws) = command.first_token().and_then(|t| t.prev_token()) else {
            return;
        };
        // Only a same-line plain space is a tie candidate. A `~` (already tied),
        // a newline, or a `{`/`}` all fall through here.
        if ws.kind() != SyntaxKind::WHITESPACE {
            return;
        }
        // Require a real word before the space so we never tie at sentence or
        // paragraph start, after `{`, or after another command's `}`.
        if ws.prev_token().map(|t| t.kind()) != Some(SyntaxKind::WORD) {
            return;
        }

        let range = ws.text_range();
        let start = usize::from(range.start());
        let end = usize::from(range.end());
        // Replace the whole whitespace run (lexed as one token) with a single
        // tie. Correct by construction: a `TILDE` is valid here, the splice is
        // contiguous, so the result parses and stays lossless. Unsafe because it
        // alters line-breaking.
        let fix = Fix::unsafe_(
            start,
            end,
            "~",
            format!("Replace the space before `\\{name}` with a non-breaking space `~`"),
        );
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start,
            end,
            message: format!(
                "missing non-breaking space before `\\{name}`; use a tie `~` so the reference stays on the same line"
            ),
            fix: Some(fix),
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
        let ctx = RuleContext {
            path: std::path::Path::new("x.tex"),
            root: &root,
            model: &model,
            resolution: None,
            citations: None,
        };
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
    fn newline_is_out_of_scope() {
        // A source line break before the command is not flagged in v1.
        assert!(findings("Figure\n\\ref{x}\n").is_empty());
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
