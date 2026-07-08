//! `makeat-macro`: an `@`-in-name macro (`\foo@bar`, `\p@`, `\@ifnextchar`) used
//! *outside* a `\makeatletter`/`\makeatother` region, where `@` is an ordinary
//! character rather than a letter.
//!
//! Internal LaTeX macros conventionally carry `@` in their name so they are
//! unreachable from a document body, where `@` has its default catcode-12 ("other")
//! and so cannot be part of a control word. Writing `\foo@bar` outside
//! `\makeatletter`…`\makeatother` therefore does *not* call the macro `\foo@bar`:
//! TeX reads `\foo` and then the literal text `@bar`. This is almost always a
//! mistake -- either the surrounding `\makeatletter`/`\makeatother` was forgotten,
//! or the internal macro is being used where it should not be.
//!
//! **Decided exactly from letter-mode tracking, not heuristically.** The lexer
//! already tracks `\makeatletter` state (`parser::lexer`, the `at_letter` flag) and
//! glues `@` into a control word only while it is on. So the *only* way a
//! control-word/control-symbol token can end up immediately abutted (no trivia) by
//! a `WORD` carrying the `@` is if `@` was not a letter there -- i.e. we were
//! outside a region. Inside a region the whole name lexes as a single
//! `CONTROL_WORD` (`\foo@bar`), which this rule never sees split and so never flags.
//! Region membership is deliberately absent from the CST (AGENTS.md); the token
//! split *is* the durable signal, so no region lookup is needed. Two split shapes:
//!
//! - **Embedded/trailing `@`** (`\foo@bar`, `\p@`): `CONTROL_WORD "\foo"` followed
//!   by `WORD "@bar"`. Detected off the `COMMAND` node's control word.
//! - **Leading `@`** (`\@ifnextchar`): `\@` is not a control word (its one char is
//!   not a letter), so it lexes as a bare `CONTROL_SYMBOL "\@"` followed by
//!   `WORD "ifnextchar"`. The legitimate end-of-sentence `\@` (as in `NASA\@.`) is
//!   spared: there the next token is punctuation/whitespace, not a name run.
//!
//! **Report-only.** A correct-by-construction fix would have to wrap the use in
//! `\makeatletter`…`\makeatother` (or move it into an existing region), which is not
//! a tight, local edit, so per tenet 1 no autofix is offered -- the finding stands
//! on its own.

use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxToken};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[
    Example {
        caption: "An internal `@` macro used without `\\makeatletter`:",
        source: "\\my@command\n",
    },
    Example {
        caption: "A leading-`@` macro (a `\\@`-prefixed internal) outside a region:",
        source: "\\@ifstar{\\StarredForm}{\\PlainForm}\n",
    },
];

pub struct MakeatMacro;

impl Rule for MakeatMacro {
    fn id(&self) -> &'static str {
        "makeat-macro"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a macro whose name contains `@` (`\\foo@bar`, `\\p@`, \
         `\\@ifnextchar`) used outside a `\\makeatletter`/`\\makeatother` region. \
         There `@` has its ordinary catcode, so it cannot be part of a control \
         word: `\\foo@bar` is read as `\\foo` followed by the text `@bar`, not as a \
         call to the internal macro `\\foo@bar`. Usually the enclosing \
         `\\makeatletter`/`\\makeatother` was forgotten. Because the formatter's \
         lexer already tracks `\\makeatletter` state, this is decided exactly -- an \
         in-region name lexes as one token and is never flagged; only the split \
         out-of-region form (control word abutting an `@`-word, or `\\@` abutting a \
         letter-word) is. Report-only: a correct fix would mean wrapping the use in \
         `\\makeatletter`/`\\makeatother`, not a tight local edit, so no autofix is \
         offered. The end-of-sentence `\\@` (as in `NASA\\@.`) is not flagged."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::COMMAND, SyntaxKind::CONTROL_SYMBOL]
    }

    fn check(&self, el: &SyntaxElement, _ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        // Embedded/trailing `@`: a `COMMAND`'s control word directly abutting an
        // `@`-word (`\foo@bar`, `\p@`).
        if let Some(command) = el.as_node() {
            let Some(control_word) = command
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| t.kind() == SyntaxKind::CONTROL_WORD)
            else {
                return;
            };
            let Some(next) = control_word.next_token() else {
                return;
            };
            if next.kind() == SyntaxKind::WORD
                && next.text().starts_with('@')
                && let Some(run) = at_letter_run(&next)
            {
                report(self, control_word.text(), &control_word, &next, run, sink);
            }
            return;
        }

        // Leading `@`: a bare `\@` control symbol directly abutting a name run
        // (`\@ifnextchar`, `\@@module`). A plain `\@` before punctuation/space
        // (sentence spacing) has no such run and is left alone.
        let Some(symbol) = el.as_token() else {
            return;
        };
        if symbol.text() != "\\@" {
            return;
        }
        let Some(next) = symbol.next_token() else {
            return;
        };
        if next.kind() == SyntaxKind::WORD
            && let Some(run) = at_letter_run(&next)
        {
            report(self, symbol.text(), symbol, &next, run, sink);
        }
    }
}

/// The leading run of the `WORD` `token` that would have been part of the macro
/// name inside a `\makeatletter` region: characters that are letters there, i.e.
/// ASCII letters or `@`. Returns the run length in bytes (== char count, all
/// ASCII), or `None` when the run is empty (nothing to attach -- e.g. `\@.` where
/// the next word is punctuation).
fn at_letter_run(token: &SyntaxToken) -> Option<usize> {
    let len = token
        .text()
        .bytes()
        .take_while(|&b| b == b'@' || b.is_ascii_alphabetic())
        .count();
    (len > 0).then_some(len)
}

/// Push the finding, underlining and naming the reconstructed at-letter macro:
/// `head` (the control word/symbol text) plus the `@`-letter `run` prefix of
/// `word`.
fn report(
    rule: &dyn Rule,
    head: &str,
    head_token: &SyntaxToken,
    word: &SyntaxToken,
    run: usize,
    sink: &mut Vec<Diagnostic>,
) {
    let start = usize::from(head_token.text_range().start());
    let end = usize::from(word.text_range().start()) + run;
    let tail = &word.text()[..run];
    sink.push(Diagnostic {
        rule: rule.id(),
        severity: rule.default_severity(),
        path: PathBuf::new(),
        start,
        end,
        message: format!(
            "`{head}{tail}` uses `@` in a macro name outside a `\\makeatletter` region; \
             `@` is not a letter here, so this reads as `{head}` followed by the text `{tail}`"
        ),
        fix: None,
    });
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
            if MakeatMacro.interests().contains(&el.kind()) {
                MakeatMacro.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_embedded_at_macro() {
        let out = findings("\\my@command\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "makeat-macro");
        assert!(out[0].fix.is_none(), "report-only");
        // Span covers the reconstructed name `\my@command` (bytes 0..11).
        assert_eq!((out[0].start, out[0].end), (0, 11));
        assert!(
            out[0].message.contains("`\\my@command`"),
            "got: {}",
            out[0].message
        );
    }

    #[test]
    fn flags_trailing_at_macro() {
        // `\p@` -> `\p` + `@`; the run is just `@`.
        let out = findings("\\p@\n");
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].start, out[0].end), (0, 3));
    }

    #[test]
    fn flags_leading_at_control_symbol() {
        let out = findings("\\@ifnextchar aX{Y}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "makeat-macro");
        // Span covers `\@ifnextchar` (bytes 0..12).
        assert_eq!((out[0].start, out[0].end), (0, 12));
        assert!(out[0].message.contains("`\\@ifnextchar`"));
    }

    #[test]
    fn flags_double_at_module_prefix() {
        // `\@@foo` -> `\@` + `@foo`; the whole `@foo` run reattaches.
        let out = findings("\\@@foo\n");
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].start, out[0].end), (0, 6));
    }

    #[test]
    fn in_region_name_is_not_split_so_not_flagged() {
        // Inside `\makeatletter` the whole name is one control word -- never split,
        // never flagged.
        assert!(findings("\\makeatletter\\my@command\\makeatother\n").is_empty());
    }

    #[test]
    fn space_before_at_is_ordinary_text() {
        // `\my @command` has a space: `@command` is plainly separate text.
        assert!(findings("\\my @command\n").is_empty());
    }

    #[test]
    fn at_after_group_is_ordinary_text() {
        // `\foo{x}@bar`: the `@bar` follows the argument's `}`, not the control
        // word, so it is genuine text -- not flagged.
        assert!(findings("\\foo{x}@bar\n").is_empty());
    }

    #[test]
    fn sentence_spacing_at_is_left_alone() {
        // The legitimate end-of-sentence `\@` before a period: no name run follows.
        assert!(findings("NASA\\@. Next\n").is_empty());
        assert!(findings("etc\\@ and more\n").is_empty());
    }

    #[test]
    fn email_in_text_is_not_flagged() {
        // A bare `@` in text is one `WORD` starting with a letter, with no command
        // abutting it.
        assert!(findings("Write to user@example.com today.\n").is_empty());
    }

    #[test]
    fn digit_after_control_word_is_not_flagged() {
        // `\foo2` -> `\foo` + `2`: the abutting word does not start with `@`.
        assert!(findings("\\foo2\n").is_empty());
    }

    #[test]
    fn flags_each_occurrence() {
        assert_eq!(findings("\\a@b and \\c@d\n").len(), 2);
    }
}
