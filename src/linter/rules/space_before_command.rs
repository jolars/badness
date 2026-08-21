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
//! -- covering both `$…$`/`\[…\]` and parser-recognized math-environment bodies
//! through [`RuleContext::in_math`]. The environment's `\begin` header stays
//! outside that shared `MATH`-range index.
//!
//! For the zero-width `\index`/`\label` there is a mirror gate on the *other*
//! side: because they type no glyph, the leading space is a real interword space
//! bridging the preceding word to whatever follows the group. Deleting it is only
//! correct when the group is trailed by a break (whitespace, newline, or
//! paragraph/document end); if visible content abuts the group
//! (`We write \index{$x$}$x$`), removing the space would merge the word into it,
//! so the finding is withheld (mirroring the pre-space `WORD` gate).
//!
//! The command table lives here, not in `data/signatures.json`: "a space before
//! this command is wrong" is a lint judgment, not the structural arity/verbatim
//! fact the signature DB carries (AGENTS.md core decision #2).

use std::path::PathBuf;

use crate::ast::{AstToken, ControlWord, child_token, command_name};
use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
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

/// Zero-width commands (no glyph of their own): the pre-space bridges the
/// preceding word to whatever follows the group, so deleting it is only correct
/// when the group is itself trailed by a break. A subset of `NO_SPACE_COMMANDS`.
const ZERO_WIDTH_COMMANDS: &[&str] = &["index", "label"];

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
         math environments like `equation`/`align`. For the zero-width \
         `\\index`/`\\label` the fix is withheld unless the group is trailed by \
         whitespace, a newline, or paragraph end, since otherwise the leading \
         space is a real interword space to the following content."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::COMMAND]
    }

    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
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
        if !ctx.in_text(usize::from(command.text_range().start())) {
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
        // A zero-width `\index`/`\label` types no glyph, so the leading space is a
        // real interword space bridging the preceding word to whatever follows the
        // group. Deleting it is only correct when the group is trailed by a break
        // (whitespace, newline, or paragraph/document end); if visible content abuts
        // the group, removing the space merges the word into it. Mirror the
        // pre-space WORD gate and stay quiet (conservative false negative).
        if ZERO_WIDTH_COMMANDS.contains(&name.as_str()) && !followed_by_break(command) {
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

/// Whether the token immediately after the whole command node (argument group
/// included) is a break: same-line whitespace, a newline, or end of input
/// (paragraph/document end). Keeps the zero-width `\index`/`\label` fix from
/// deleting a load-bearing interword space when visible content abuts the group.
/// The command node greedily includes its argument group (decision #8), so its
/// last token is the closing `}` and `next_token()` walks to the token past it
/// (trailing trivia floats as a sibling, so it is what `next_token()` returns).
fn followed_by_break(command: &SyntaxNode) -> bool {
    match command.last_token().and_then(|t| t.next_token()) {
        None => true, // end of input == paragraph/document end
        Some(t) => matches!(t.kind(), SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE),
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
        assert_eq!(fix.edits[0].content, "");
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
        assert_eq!((fix.edits[0].start, fix.edits[0].end), (4, 6));
        assert_eq!(fix.edits[0].content, "");
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
    fn math_environment_header_is_unknown() {
        // `array`'s column specification belongs to its `\begin` header, but no
        // positional text-domain claim exists for it.
        let src = "\\begin{array}{word \\footnote{x}}\n  a = b\n\\end{array}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn flags_each_occurrence() {
        assert_eq!(findings("a \\footnote{x} and b \\index{y}\n").len(), 2);
    }

    #[test]
    fn zero_width_abutting_visible_content_is_suppressed() {
        // The space before `\index` here is a real interword space bridging
        // "write" to the following `$x \in E$`; deleting it would render
        // "write$x$". No finding (mirrors the pre-space WORD gate).
        assert!(findings("We write \\index{$x$}$x \\in E$ done\n").is_empty());
        // A bare word abutting the group is the same shape.
        assert!(findings("word \\index{term}next\n").is_empty());
        // `\label` is zero-width too.
        assert!(findings("word \\label{sec}$x$\n").is_empty());
    }

    #[test]
    fn zero_width_trailed_by_break_is_still_flagged() {
        // Whitespace, a newline, or end of input after the group all leave a
        // separating break, so deleting the leading space is still correct.
        assert_eq!(findings("word \\index{term} more\n").len(), 1);
        assert_eq!(findings("word \\index{term}\n").len(), 1);
        assert_eq!(findings("word \\index{term}").len(), 1);
    }

    #[test]
    fn footnote_is_not_gated_by_trailing_content() {
        // `\footnote` emits a visible mark, so its pre-space is spurious
        // regardless of what follows: the zero-width gate does not apply.
        assert_eq!(findings("word \\footnote{x}next\n").len(), 1);
    }
}
