//! `verbatim-trailing-text`: non-whitespace text after a verbatim-like
//! environment's `\end{…}` on the *same line*, which LaTeX silently discards
//! (ChkTeX warning 31).
//!
//! A verbatim environment closes by scanning the input line by line until it sees
//! `\end{verbatim}`, then gobbling the rest of that line. So `\end{verbatim} foo`
//! drops `foo` without a trace — the author almost never means that. The rule
//! points at the swallowed run so the intent (a real edit, or a stray character)
//! can be recovered.
//!
//! Scoped to *verbatim-like* environments only, since ordinary environments do
//! **not** gobble their `\end` line — `\end{itemize} text` renders `text`
//! normally. "Verbatim-like" is read straight off the parse: an environment whose
//! opaque body the parser captured as a single `VERBATIM_BODY` token, or (to catch
//! an *empty* verbatim body, which has no such token) whose name is a curated
//! built-in verbatim environment. Only the curated tier is consulted for the name
//! fallback — the CWL tier carries no `verbatim_body` flag, and a wrong route here
//! would invent findings on non-verbatim environments (AGENTS.md conservatism).
//!
//! A trailing `%` comment is *not* flagged: a comment is understood to be dropped
//! regardless, so it is treated as trivia (as is whitespace). Only genuine text
//! preceding the line break counts.
//!
//! **Report-only** (no autofix). Whether the swallowed text should move to the
//! next line or be deleted is the author's call; no single textual edit is correct
//! by construction.

use std::path::PathBuf;

use crate::ast::environment_name;
use crate::linter::diagnostic::{Diagnostic, Severity};
use crate::semantic::signature;
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "Text after `\\end{verbatim}` is silently discarded by LaTeX:",
    source: "\\begin{verbatim}\nsample\n\\end{verbatim} and more\n",
}];

pub struct VerbatimTrailingText;

impl Rule for VerbatimTrailingText {
    fn id(&self) -> &'static str {
        "verbatim-trailing-text"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag non-whitespace text after a verbatim-like environment's `\\end{…}` \
         on the same line (ChkTeX warning 31). LaTeX closes a verbatim environment \
         by scanning line by line to `\\end{verbatim}` and then gobbling the rest \
         of that line, so `\\end{verbatim} foo` silently drops `foo`. Scoped to \
         verbatim-like environments — read off the parse tree (an opaque \
         `VERBATIM_BODY`, or a curated built-in verbatim name for the empty-body \
         case) — because ordinary environments do not gobble their `\\end` line. A \
         trailing `%` comment is treated as trivia, not flagged. Report-only: \
         whether to move or delete the swallowed text is the author's call, so no \
         fix is correct by construction."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::ENVIRONMENT]
    }

    fn check(&self, el: &SyntaxElement, _ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(env) = el.as_node() else {
            return;
        };
        let Some(name) = verbatim_env_name(env) else {
            return;
        };
        // Only a properly closed environment has an `\end` line to gobble; an
        // unclosed verbatim ran to EOF and has no trailing siblings anyway.
        if !env.children().any(|c| c.kind() == SyntaxKind::END) {
            return;
        }
        let Some((start, end)) = trailing_run(env) else {
            return;
        };
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start,
            end,
            message: format!("text after `\\end{{{name}}}` on the same line is silently discarded"),
            fix: None,
        });
    }
}

/// The environment name if `env` is verbatim-like, else `None`. Verbatim-ness is
/// read off the parse: a captured `VERBATIM_BODY` token (the parser routed the
/// body to raw-verbatim scanning) covers every non-empty case, including
/// user-defined verbatim environments; the curated built-in `verbatim_body` flag
/// is the fallback for an *empty* body, which emits no token.
fn verbatim_env_name(env: &SyntaxNode) -> Option<String> {
    let name = env
        .children()
        .find(|c| c.kind() == SyntaxKind::BEGIN)
        .and_then(|begin| environment_name(&begin))?;
    let has_body = env
        .children()
        .any(|c| c.kind() == SyntaxKind::VERBATIM_BODY);
    let is_builtin_verbatim = signature::builtin()
        .environment(&name)
        .is_some_and(|sig| sig.verbatim_body);
    (has_body || is_builtin_verbatim).then_some(name)
}

/// The byte span of the run of non-trivia text immediately following `env` on the
/// same line (up to the first `NEWLINE` or EOF), or `None` when only whitespace
/// and comments follow. Whitespace and comments are trivia: a trailing `%` comment
/// is dropped whether or not it sits after `\end{verbatim}`, so it does not count.
fn trailing_run(env: &SyntaxNode) -> Option<(usize, usize)> {
    let mut start: Option<usize> = None;
    let mut end: Option<usize> = None;
    let mut cursor = env.next_sibling_or_token();
    while let Some(el) = cursor {
        match el.kind() {
            SyntaxKind::NEWLINE => break,
            SyntaxKind::WHITESPACE | SyntaxKind::COMMENT => {}
            _ => {
                let range = el.text_range();
                start.get_or_insert(usize::from(range.start()));
                end = Some(usize::from(range.end()));
            }
        }
        cursor = next_of(&el);
    }
    Some((start?, end?))
}

fn next_of(el: &SyntaxElement) -> Option<SyntaxElement> {
    match el {
        rowan::NodeOrToken::Node(node) => node.next_sibling_or_token(),
        rowan::NodeOrToken::Token(token) => token.next_sibling_or_token(),
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
            if VerbatimTrailingText.interests().contains(&el.kind()) {
                VerbatimTrailingText.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_trailing_text() {
        let src = "\\begin{verbatim}\ncode\n\\end{verbatim} trailing text\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "verbatim-trailing-text");
        assert!(
            out[0].message.contains("\\end{verbatim}"),
            "{}",
            out[0].message
        );
        // Report-only.
        assert!(out[0].fix.is_none());
        // Span covers exactly the swallowed run, not the leading space or newline.
        assert_eq!(&src[out[0].start..out[0].end], "trailing text");
    }

    #[test]
    fn flags_trailing_text_on_empty_verbatim() {
        // An empty body emits no VERBATIM_BODY token; the built-in name is the
        // fallback signal.
        let src = "\\begin{verbatim}\\end{verbatim} oops\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(&src[out[0].start..out[0].end], "oops");
    }

    #[test]
    fn flags_trailing_text_on_lstlisting() {
        let src = "\\begin{lstlisting}\nx\n\\end{lstlisting} tail\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(&src[out[0].start..out[0].end], "tail");
    }

    #[test]
    fn flags_at_end_of_file_without_newline() {
        let src = "\\begin{verbatim}\nx\n\\end{verbatim} tail";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(&src[out[0].start..out[0].end], "tail");
    }

    #[test]
    fn span_stops_before_the_line_break() {
        let src = "\\begin{verbatim}\nx\n\\end{verbatim} a b\nnext line\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(&src[out[0].start..out[0].end], "a b");
    }

    #[test]
    fn silent_when_end_is_alone_on_its_line() {
        assert!(findings("\\begin{verbatim}\nx\n\\end{verbatim}\nnext\n").is_empty());
    }

    #[test]
    fn silent_on_trailing_whitespace_only() {
        assert!(findings("\\begin{verbatim}\nx\n\\end{verbatim}   \n").is_empty());
    }

    #[test]
    fn silent_on_trailing_comment_only() {
        assert!(findings("\\begin{verbatim}\nx\n\\end{verbatim} % note\n").is_empty());
    }

    #[test]
    fn text_before_a_comment_is_still_flagged() {
        let src = "\\begin{verbatim}\nx\n\\end{verbatim} foo % note\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        // The comment is trivia, so the span ends at the text, not the comment.
        assert_eq!(&src[out[0].start..out[0].end], "foo");
    }

    #[test]
    fn silent_on_non_verbatim_environment() {
        // Ordinary environments do not gobble their `\end` line.
        assert!(findings("\\begin{itemize}\\item a\\end{itemize} text\n").is_empty());
    }

    #[test]
    fn flags_each_verbatim_occurrence() {
        let src = "\\begin{verbatim}\na\n\\end{verbatim} one\n\
                   \\begin{verbatim}\nb\n\\end{verbatim} two\n";
        assert_eq!(findings(src).len(), 2);
    }
}
