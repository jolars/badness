//! `blank-line-in-keyval`: a blank line at the top level of a `key=value`
//! argument, which stops the document compiling.
//!
//! A blank line is a `\par` token, and a keyval-family processor walks its
//! entries with macros that are not `\long`, so the `\par` aborts the call. The
//! message TeX gives names the *processor*, never the command the author wrote —
//! measured across the curated setters, `\hypersetup` reports `Paragraph ended
//! before \kv@processor@default was complete`, `\tikzset` and `\pgfkeys` blame
//! `\pgfkeys@addpath`, `\setlist` `\enit@setlist@i`, `\captionsetup`
//! `\caption@setup@options@`, and `\geometry` fails differently again (`Missing
//! \endcsname inserted`). Pointing at the blank line is the whole value of the
//! rule: the compiler's own diagnostic points into package internals.
//!
//! Three scope limits, each measured rather than assumed:
//!
//! - **Top level of the argument only.** A blank line *nested* inside a value's
//!   brace group (`\tikzset{aa/.style={draw,\n\nthick}}`) compiles clean — the
//!   value is stored as a token list rather than walked — so only a `\par`
//!   between entries is a fault. This is the same depth rule the formatter's
//!   `segment_delimited_body` uses for comma splitting.
//! - **Closed groups only.** An unclosed `{` already draws a parse error, and
//!   the runaway group's contents are a recovery artifact rather than the
//!   author's key list; flagging blank lines inside it would bury the real
//!   finding under noise.
//! - **Mandatory `{…}` only.** A `ContentKind::Keyval` optional cannot reach
//!   this shape: the parser's bracket gate refuses across a paragraph break, so
//!   `\begin{axis}[a=1,\n\nb=2]` never builds an `OPTIONAL` node at all.
//!
//! Only the curated built-in tier is consulted, which costs nothing here: the
//! CWL converter deliberately drops a `%keyvals` mark on a mandatory group, so
//! a mandatory `Keyval` claim is hand-curated by construction.
//!
//! The `Safe` autofix drops the blank line, keeping the last newline and the
//! indentation that follows it. It is correct by construction: the edit touches
//! only whitespace, and `ContentKind::Keyval` is precisely the assertion that
//! the processor strips spaces around entries (see `formatter.md`), so the
//! surviving newline cannot change what the argument means. Nor can it change
//! typeset output, since the input it repairs did not typeset at all.

use std::path::PathBuf;

use crate::ast::command_name;
use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
use crate::semantic::signature;
use crate::semantic::signature::{ArgKind, ArgSpec, ContentKind};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "A blank line separating two keys, which aborts the call:",
    source: "\\hypersetup{colorlinks=true,\n\nlinkcolor=blue}\n",
}];

pub struct BlankLineInKeyval;

impl Rule for BlankLineInKeyval {
    fn id(&self) -> &'static str {
        "blank-line-in-keyval"
    }

    fn emits_fix(&self) -> bool {
        true
    }

    fn default_severity(&self) -> Severity {
        Severity::Error
    }

    fn description(&self) -> &'static str {
        "Flag a blank line at the top level of a `key=value` argument. A blank \
         line is a `\\par` token and a keyval processor walks its entries with \
         macros that are not `\\long`, so the call aborts -- and the error TeX \
         reports names the processor rather than the command the author wrote \
         (`\\hypersetup` yields \"Paragraph ended before \
         `\\kv@processor@default` was complete\"), which is what makes the \
         finding worth more than the compiler's own message. Scoped by \
         measurement: a blank line *nested* inside a value's brace group \
         (`\\tikzset{aa/.style={draw,\\n\\nthick}}`) compiles clean and is not \
         flagged, an unclosed `{` is left to the parse error it already draws, \
         and only the hand-curated signature tier is consulted. The autofix \
         drops the blank line and keeps the following indentation; it is safe \
         by construction, since it edits only whitespace and \
         `ContentKind::Keyval` is exactly the claim that the processor strips \
         spaces around entries."
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
        let Some(sig) = signature::builtin().command(&name) else {
            return;
        };
        if !sig.args.iter().any(is_keyval_brace) {
            return;
        }
        // A name the file redefines is the user's macro, whose argument protocol
        // we know nothing about — the same conservatism `deprecated-command` and
        // `missing-required-argument` apply.
        if ctx.user_definitions().command(&name).is_some() {
            return;
        }
        let mut slot = 0usize;
        for child in command.children() {
            let is_bracket = match child.kind() {
                SyntaxKind::GROUP => false,
                SyntaxKind::OPTIONAL => true,
                _ => continue,
            };
            let Some(spec) = match_arg_slot(&sig.args, &mut slot, is_bracket) else {
                continue;
            };
            if !is_keyval_brace(&spec) || !is_closed(&child) {
                continue;
            }
            for run in blank_runs(&child) {
                sink.push(Diagnostic {
                    rule: self.id(),
                    severity: self.default_severity(),
                    path: PathBuf::new(),
                    start: run.start,
                    end: run.end,
                    message: format!(
                        "blank line in `\\{name}`'s key-value argument; the `\\par` \
                         aborts the call"
                    ),
                    fix: Some(Fix::safe(
                        run.start,
                        run.end,
                        run.joined.clone(),
                        "Remove the blank line",
                    )),
                    related: Vec::new(),
                });
            }
        }
    }
}

/// Whether `spec` is a mandatory `{…}` slot carrying the keyval claim.
fn is_keyval_brace(spec: &ArgSpec) -> bool {
    spec.required && spec.kind == ArgKind::Brace && spec.content == ContentKind::Keyval
}

/// Match the next attached group to a signature slot, advancing `slot` past it.
/// Skips leading optional slots the document omitted, so a mandatory keyval slot
/// still binds when an optional before it is absent (`\setlist{…}` without its
/// `[…]`). Mirrors the formatter's `match_arg_slot`; returns `None` for a group
/// past the declared arity, leaving `slot` untouched so later groups still match.
fn match_arg_slot(args: &[ArgSpec], slot: &mut usize, is_bracket: bool) -> Option<ArgSpec> {
    while *slot < args.len() {
        let spec = args[*slot];
        if (spec.kind == ArgKind::Bracket) == is_bracket {
            *slot += 1;
            return Some(spec);
        }
        if is_bracket {
            // A `[…]` never consumes a mandatory slot.
            return None;
        }
        *slot += 1; // an omitted optional
    }
    None
}

/// Whether the group carries its closing delimiter. An unclosed `{` already
/// draws a parse error and its extent is recovery, not the author's argument.
fn is_closed(group: &SyntaxNode) -> bool {
    group
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .any(|t| matches!(t.kind(), SyntaxKind::R_BRACE | SyntaxKind::R_BRACKET))
}

/// One `\par`-forming whitespace run at the group's top level: its byte span and
/// the text that replaces it once the blank line is gone.
struct BlankRun {
    start: usize,
    end: usize,
    joined: String,
}

/// The maximal runs of pure whitespace trivia among the group's *direct*
/// children that hold two or more newlines — TeX's `\par`. A `%` comment breaks
/// a run, since `a\n%c\nb` is not a paragraph break; the run on either side of
/// it is still tested on its own.
fn blank_runs(group: &SyntaxNode) -> Vec<BlankRun> {
    let mut out = Vec::new();
    let mut run: Vec<crate::syntax::SyntaxToken> = Vec::new();
    let mut flush = |run: &mut Vec<crate::syntax::SyntaxToken>| {
        if run
            .iter()
            .filter(|t| t.kind() == SyntaxKind::NEWLINE)
            .count()
            >= 2
        {
            let start = usize::from(run[0].text_range().start());
            let end = usize::from(run[run.len() - 1].text_range().end());
            // Keep the last newline and everything after it, so the following
            // entry stays on its own line at its authored indentation.
            let text: String = run.iter().map(|t| t.text()).collect();
            let joined = text
                .rfind('\n')
                .map(|i| text[i..].to_string())
                .unwrap_or_default();
            out.push(BlankRun { start, end, joined });
        }
        run.clear();
    };
    for element in group.children_with_tokens() {
        match element {
            SyntaxElement::Token(t)
                if matches!(t.kind(), SyntaxKind::NEWLINE | SyntaxKind::WHITESPACE) =>
            {
                run.push(t);
            }
            _ => flush(&mut run),
        }
    }
    flush(&mut run);
    out
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
            if BlankLineInKeyval.interests().contains(&el.kind()) {
                BlankLineInKeyval.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    fn fixed(src: &str) -> String {
        let diags = findings(src);
        let fixes: Vec<_> = diags.iter().filter_map(|d| d.fix.clone()).collect();
        crate::linter::fix::apply_fixes(src, &fixes, false).output
    }

    #[test]
    fn flags_a_blank_line_between_entries() {
        let diags = findings("\\hypersetup{colorlinks=true,\n\nlinkcolor=blue}\n");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "blank-line-in-keyval");
        assert_eq!(diags[0].severity, Severity::Error);
        // The span is the whitespace run itself, not the whole command.
        assert_eq!(
            &"\\hypersetup{colorlinks=true,\n\nlinkcolor=blue}\n"[diags[0].start..diags[0].end],
            "\n\n"
        );
    }

    #[test]
    fn fix_removes_the_blank_line_and_keeps_indentation() {
        assert_eq!(
            fixed("\\lstset{numbers=left,\n\n  frame=single}\n"),
            "\\lstset{numbers=left,\n  frame=single}\n"
        );
    }

    #[test]
    fn single_newline_is_not_a_paragraph_break() {
        assert!(findings("\\lstset{numbers=left,\n  frame=single}\n").is_empty());
    }

    #[test]
    fn a_blank_line_nested_in_a_value_compiles_and_is_not_flagged() {
        // Measured: `\tikzset{aa/.style={draw,\n\nthick}}` compiles clean, because
        // the value is stored as a token list rather than walked.
        assert!(findings("\\tikzset{aa/.style={draw,\n\nthick}}\n").is_empty());
    }

    #[test]
    fn an_unclosed_group_is_left_to_its_parse_error() {
        assert!(findings("\\lstset{numbers=left\n\nsome prose\n").is_empty());
    }

    #[test]
    fn a_non_keyval_argument_is_not_flagged() {
        // `\caption`'s argument is prose; a `\par` there is the author's business.
        assert!(findings("\\caption{one\n\ntwo}\n").is_empty());
    }

    #[test]
    fn an_omitted_leading_optional_still_binds_the_keyval_slot() {
        let diags = findings("\\setlist{noitemsep,\n\ntopsep=0pt}\n");
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn a_present_leading_optional_still_binds_the_keyval_slot() {
        let diags = findings("\\setlist[itemize]{noitemsep,\n\ntopsep=0pt}\n");
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn a_redefined_name_is_the_users_macro() {
        assert!(findings("\\renewcommand{\\lstset}[1]{#1}\n\\lstset{a=1,\n\nb=2}\n").is_empty());
    }

    #[test]
    fn a_leading_blank_line_counts() {
        // Measured: `\lstset{\n\nnumbers=left}` fails to compile too.
        assert_eq!(findings("\\lstset{\n\nnumbers=left}\n").len(), 1);
    }

    #[test]
    fn a_comment_between_newlines_is_not_a_blank_line() {
        assert!(findings("\\lstset{a=1,\n% note\nb=2}\n").is_empty());
    }

    #[test]
    fn two_blank_lines_report_separately() {
        assert_eq!(findings("\\lstset{a=1,\n\nb=2,\n\nc=3}\n").len(), 2);
    }
}
