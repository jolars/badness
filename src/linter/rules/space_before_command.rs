//! `space-before-command`: a plain space directly before a command whose
//! preceding space is semantically wrong rather than layout -- `\footnote`,
//! `\footnotemark`, `\index`, `\label` (ChkTeX 24/42).
//!
//! These commands should hug the preceding word. A space in front of them is
//! typeset (or affects pagination) in a way the author almost never wants:
//!
//! - `word \footnote{x}` sets a spurious space *before* the footnote mark
//!   ("word ¹" instead of "word¹").
//! - `word \index{x}` / `word \label{x}` produce no glyph themselves, so the
//!   leading space becomes a stray inter-word space next to a zero-width command,
//!   which can widen a gap and shift the page a `\label`/`\index` records
//!   (ChkTeX's "delete this space to maintain correct pagereferences").
//!
//! The fix deletes the space. It is `Unsafe` (like the sibling spacing rules
//! `missing-nonbreaking-space`, `swallowed-space`, and `abbreviation-spacing`):
//! removing the space changes the typeset spacing, which is exactly what
//! `Applicability::Unsafe` is for (`diagnostic.rs`), so `--fix` leaves it alone
//! while `--unsafe-fixes` and the editor code action apply it. It is still correct
//! by construction (tenet 1): deleting an inter-word space leaves the word
//! directly followed by the command, which re-parses and stays lossless.
//!
//! Scope is deliberately tight, mirroring `missing-nonbreaking-space`: only a
//! same-line `WORD SPACE \cmd` shape is flagged, so a space at line start, after a
//! `{`, or after another command's `}` is left alone (a false negative, the
//! conservative direction). Math is skipped -- an inter-token space is
//! insignificant there, so a space before an in-math `\label` types nothing extra
//! -- covering both `$…$`/`\[…\]` (a `MATH` ancestor) and math environments like
//! `equation`/`align` (read off the built-in signature DB's `math` flag).
//!
//! The command table lives here, not in `data/signatures.json`: "a space before
//! this command is wrong" is a lint judgment, not the structural arity/verbatim
//! fact the signature DB carries (AGENTS.md core decision #2).

use std::path::PathBuf;

use crate::ast::{AstNode, AstToken, ControlWord, Environment, child_token, command_name};
use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
use crate::semantic::signature::builtin;
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "A space before a footnote sets a spurious space before the mark:",
    source: "This is important \\footnote{See the appendix.}\n",
}];

/// Commands that should hug the preceding word: a space in front of them is
/// typeset or affects pagination, essentially never intentionally. Curated (not
/// from the signature DB) because "no space before this" is a lint judgment.
const NO_SPACE_COMMANDS: &[&str] = &["footnote", "footnotemark", "index", "label"];

pub struct SpaceBeforeCommand;

impl Rule for SpaceBeforeCommand {
    fn id(&self) -> &'static str {
        "space-before-command"
    }

    fn emits_fix(&self) -> bool {
        true
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a plain space directly before a command that should hug the \
         preceding word -- `\\footnote`, `\\footnotemark`, `\\index`, `\\label` \
         (ChkTeX 24/42). A space before `\\footnote` sets a spurious space before \
         the footnote mark (`word \\footnote{x}` -> \"word ¹\"); a space before a \
         zero-width `\\index`/`\\label` leaves a stray inter-word gap that can \
         shift the recorded page. The fix deletes the space. It is **unsafe** -- \
         removing the space changes the typeset spacing -- so `--fix` leaves it \
         alone while `--unsafe-fixes` and the editor code action apply it. To stay \
         conservative only the same-line `WORD SPACE \\cmd` shape is flagged (a \
         space at line start or after a brace is left alone), and math is skipped \
         (an inter-token space is insignificant there), covering both `$…$` and \
         math environments like `equation`/`align`."
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
        if !NO_SPACE_COMMANDS.contains(&name.as_str()) {
            return;
        }
        // In math the inter-token space is insignificant, so nothing extra is
        // typeset: stay quiet. Covers `$…$`/`\[…\]` and math environments.
        if in_math(command) {
            return;
        }
        // The `CONTROL_WORD` is the command's leading token; the token directly
        // before it (trivia floats as a sibling, and `prev_token` walks globally)
        // must be a same-line space.
        let Some(control_word) = child_token::<ControlWord>(command) else {
            return;
        };
        let Some(space) = control_word.syntax().prev_token() else {
            return;
        };
        if space.kind() != SyntaxKind::WHITESPACE {
            return;
        }
        // Require a real word before the space so we never delete a space at
        // sentence/paragraph start, after `{`, or after another command's `}`.
        if space.prev_token().map(|t| t.kind()) != Some(SyntaxKind::WORD) {
            return;
        }

        let range = space.text_range();
        let start = usize::from(range.start());
        let end = usize::from(range.end());
        // Delete the whole space run (lexed as one token). Correct by construction
        // (tenet 1): the word then directly precedes the command, which re-parses
        // and stays lossless. Unsafe because it changes the typeset spacing.
        let fix = Fix::unsafe_(
            start,
            end,
            "",
            format!("Delete the space before `\\{name}`"),
        );
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start,
            end,
            message: format!(
                "spurious space before `\\{name}`; delete it so no stray space is typeset before the command"
            ),
            fix: Some(fix),
            related: Vec::new(),
        });
    }
}

/// Whether `node` sits in math mode: inside a `MATH` node (`$…$`, `\[…\]`,
/// `\left…\right`) or an environment the built-in signature DB marks `math`
/// (`equation`, `align`, …). A space before an in-math command types nothing
/// extra, so such a finding would be noise.
fn in_math(node: &SyntaxNode) -> bool {
    node.ancestors().any(|anc| match anc.kind() {
        SyntaxKind::MATH => true,
        SyntaxKind::ENVIRONMENT => Environment::cast(anc.clone())
            .and_then(|e| e.begin())
            .and_then(|begin| begin.name())
            .and_then(|name| builtin().environment(&name).map(|env| env.math))
            .unwrap_or(false),
        _ => false,
    })
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
            if SpaceBeforeCommand.interests().contains(&el.kind()) {
                SpaceBeforeCommand.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_space_before_footnote_with_unsafe_delete_fix() {
        let src = "word \\footnote{x}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "space-before-command");
        // Caret on the single space (byte 4..5), not the command.
        assert_eq!((out[0].start, out[0].end), (4, 5));
        let fix = out[0].fix.as_ref().expect("a fix");
        assert_eq!(fix.applicability, Applicability::Unsafe);
        assert_eq!(fix.content, "");
        // Unsafe: skipped without the opt-in, applied with it.
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).applied,
            0
        );
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), true).output,
            "word\\footnote{x}\n"
        );
    }

    #[test]
    fn flags_index_and_label_and_footnotemark() {
        assert_eq!(findings("term \\index{term}\n").len(), 1);
        assert_eq!(findings("here \\label{sec:x}\n").len(), 1);
        assert_eq!(findings("mark \\footnotemark\n").len(), 1);
    }

    #[test]
    fn tight_command_is_clean() {
        assert!(findings("word\\footnote{x}\n").is_empty());
    }

    #[test]
    fn multiple_spaces_are_all_deleted() {
        let out = findings("word  \\footnote{x}\n");
        assert_eq!(out.len(), 1);
        let fix = out[0].fix.as_ref().unwrap();
        // The fix span covers both spaces (4..6) and deletes them.
        assert_eq!((fix.start, fix.end), (4, 6));
        assert_eq!(fix.content, "");
    }

    #[test]
    fn command_at_input_start_is_clean() {
        assert!(findings("\\footnote{x}\n").is_empty());
    }

    #[test]
    fn after_brace_is_clean() {
        // Inside a group (prev is `{`) and after a command's `}` (prev-prev is
        // `}`, not a WORD) both stay quiet, like `missing-nonbreaking-space`.
        assert!(findings("{\\footnote{x}}\n").is_empty());
        assert!(findings("\\textbf{a} \\footnote{x}\n").is_empty());
    }

    #[test]
    fn newline_before_command_is_out_of_scope() {
        // A source line break is not a `WHITESPACE` token, so it falls through.
        assert!(findings("word\n\\footnote{x}\n").is_empty());
    }

    #[test]
    fn non_targeted_command_is_left_alone() {
        // A space before an ordinary command is fine.
        assert!(findings("word \\emph{x}\n").is_empty());
    }

    #[test]
    fn inline_math_label_is_left_alone() {
        // A space before `\label` in `$…$` types nothing extra.
        assert!(findings("$x = y \\label{eq}$\n").is_empty());
    }

    #[test]
    fn math_environment_label_is_left_alone() {
        // `equation` is a math environment (signature DB `math` flag), so the
        // space before `\label` is insignificant.
        let src = "\\begin{equation}\n  a = b \\label{eq:1}\n\\end{equation}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn flags_each_occurrence() {
        assert_eq!(findings("a \\footnote{x} and b \\index{y}\n").len(), 2);
    }
}
