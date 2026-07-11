//! `missing-required-argument`: a command invoked with fewer `{…}` groups than
//! the required arity its curated built-in signature declares (ChkTeX warning 14,
//! decided precisely on the tree + signature DB rather than line heuristics).
//!
//! TeX also accepts *unbraced* single-token arguments — `\frac12` and
//! `\frac\alpha\beta` are valid — so a shortfall of attached groups is not by
//! itself an error. The rule therefore fires only when the invocation is cut off
//! by a **hard boundary** where nothing is left to supply the missing argument:
//! the closing `}` of the enclosing group, the closing `$`/`\]` of a math shell,
//! the `\end` of the enclosing environment, an alignment `&`, a `\\` line break,
//! a blank line (the `\par` boundary), or the end of the file. Anything else
//! following the command (a word, another command, a `~`, …) could be the
//! argument, and the rule stays silent.
//!
//! Conservative by construction (a false positive is worse than a miss), several
//! contexts where a bare command is *deliberate* are skipped:
//!
//! - **Definition bodies and unknown-command arguments.** The partial-application
//!   idiom `\newcommand{\bold}{\textbf}` carries `\textbf` as unexecuted code, as
//!   may any argument of a command we don't know. The rule only inspects commands
//!   whose every enclosing group is an argument of a *known, non-definition*
//!   command (or a math script group); a standalone `{…}` scope group is also
//!   skipped, since a starred `\newcommand*{\bold}{\textbf}` parses its groups as
//!   standalone (the `*` breaks greedy attachment).
//! - **Alias forms.** In `\let\bold\textbf` the trailing command is `\let`'s
//!   argument, not an invocation; a command directly preceded by a *bare* command
//!   (no attached groups) is skipped.
//! - **Redefined names.** A file that redefines a built-in
//!   (`\renewcommand{\emph}…`) has changed its arity; the built-in signature no
//!   longer applies, so redefined names are skipped (via the definition scanner).
//! - **Verbatim-argument commands** (`\mintinline`, …) capture their final
//!   argument as a single opaque token with lexer support; their mixed shape is
//!   skipped wholesale.
//!
//! Only the curated built-in tier is consulted — CWL arities are bulk-converted
//! and the linter doesn't know which packages are loaded, so trusting them here
//! would trade false positives for coverage (AGENTS.md conservatism).
//!
//! Whole-file rather than node-shape: the redefined-name gate needs the
//! definition scanner's per-file pass, run once, not once per visited node.
//!
//! **Report-only** (no autofix). The missing argument's *content* is the author's
//! to write; no textual edit is correct by construction.

use std::path::PathBuf;

use crate::ast::{Group, children, command_name, control_word_range};
use crate::linter::diagnostic::{Diagnostic, Severity};
use crate::semantic::define::{is_definition_command, scan_definitions};
use crate::semantic::signature::{self, SignatureDb};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use super::{Example, Rule, RuleContext, StreamVisitor};

const EXAMPLES: &[Example] = &[
    Example {
        caption: "A fraction missing its denominator:",
        source: "$\\frac{1}$\n",
    },
    Example {
        caption: "A command left bare at the end of a group, with nothing to take:",
        source: "\\emph{see \\textbf}\n",
    },
];

pub struct MissingRequiredArgument;

impl Rule for MissingRequiredArgument {
    fn id(&self) -> &'static str {
        "missing-required-argument"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a command invoked with fewer `{…}` groups than the required arity \
         in its curated built-in signature (ChkTeX warning 14, decided on the \
         parse tree and signature database rather than line heuristics). TeX also \
         accepts unbraced single-token arguments (`\\frac12`), so the rule stays \
         silent whenever a following token could still supply the missing \
         argument and fires only at a hard boundary: the end of the enclosing \
         group, math shell, or environment, an alignment `&`, a `\\\\` line \
         break, a blank line, or the end of the file. Contexts where a bare \
         command is deliberate are skipped -- macro-definition bodies \
         (`\\newcommand{\\bold}{\\textbf}`), arguments of unknown commands, \
         standalone `{…}` scope groups, `\\let`-style alias forms, and names the \
         file itself redefines. Report-only: the missing argument's content is \
         the author's to write, so no fix is correct by construction."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    // Streaming rather than node-shape: the redefined-name gate needs the
    // definition scanner's per-file result, computed once (lazily, on the first
    // COMMAND) and shared across the rest of the shared walk — instead of the
    // former separate `descendants()` pass on top of the scanner's own.
    fn stream(&self) -> Option<Box<dyn StreamVisitor>> {
        Some(Box::new(MissingRequiredArgumentVisitor { user_defs: None }))
    }
}

/// Holds the per-file user-definition signatures (redefined built-ins are
/// skipped). Scanned once, lazily, the first time a COMMAND is seen.
struct MissingRequiredArgumentVisitor {
    user_defs: Option<SignatureDb>,
}

impl StreamVisitor for MissingRequiredArgumentVisitor {
    fn visit(&mut self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(node) = el.as_node() else {
            return;
        };
        if node.kind() != SyntaxKind::COMMAND {
            return;
        }
        let Some(name) = command_name(node) else {
            return;
        };
        let Some(sig) = signature::builtin().command(&name) else {
            return;
        };
        // A verbatim-argument command's final argument is an opaque VERB token, not
        // a group; its mixed shape is skipped wholesale.
        if sig.verbatim {
            return;
        }
        let required = sig.args.iter().filter(|a| a.required).count();
        if required == 0 {
            return;
        }
        // The file redefined this name; the built-in arity no longer applies.
        let user_defs = self
            .user_defs
            .get_or_insert_with(|| scan_definitions(ctx.root));
        if user_defs.command(&name).is_some() {
            return;
        }
        let braced = children::<Group>(node).count();
        if braced >= required
            || in_unsafe_group(node)
            || follows_bare_command(node)
            || !at_hard_boundary(node)
        {
            return;
        }
        let range = control_word_range(node).unwrap_or_else(|| node.text_range());
        let message = match (braced, required) {
            (0, 1) => format!("`\\{name}` is missing its required argument"),
            (0, _) => format!("`\\{name}` is missing its {required} required arguments"),
            _ => format!(
                "`\\{name}` is missing {} of its {required} required arguments",
                required - braced
            ),
        };
        sink.push(Diagnostic {
            rule: "missing-required-argument",
            severity: Severity::Warning,
            path: PathBuf::new(),
            start: usize::from(range.start()),
            end: usize::from(range.end()),
            message,
            fix: None,
        });
    }
}

/// Trivia skipped when scanning for the neighboring meaningful element.
/// `NEWLINE` is deliberately *not* here: both scans count newlines to detect the
/// blank-line (`\par`) boundary.
fn is_trivia(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::WHITESPACE | SyntaxKind::COMMENT | SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD
    )
}

/// Whether `command` sits inside a group whose content may be *carried code*
/// rather than executed here. Every enclosing `GROUP`/`OPTIONAL` must be an
/// argument of a known, non-definition command (or a math script group);
/// anything else — a definition's argument (`\newcommand{\bold}{\textbf}`), an
/// unknown command's argument, a `\begin`'s argument, or a standalone scope
/// group (which is also how a starred `\newcommand*`'s groups parse, the `*`
/// breaking greedy attachment) — is skipped.
fn in_unsafe_group(command: &SyntaxNode) -> bool {
    for ancestor in command.ancestors().skip(1) {
        if !matches!(ancestor.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL) {
            continue;
        }
        let Some(owner) = ancestor.parent() else {
            return true;
        };
        match owner.kind() {
            SyntaxKind::SUBSCRIPT | SyntaxKind::SUPERSCRIPT => {}
            SyntaxKind::COMMAND => {
                let known_non_definition = command_name(&owner).is_some_and(|owner_name| {
                    !is_definition_command(&owner_name)
                        && signature::builtin().command(&owner_name).is_some()
                });
                if !known_non_definition {
                    return true;
                }
            }
            _ => return true,
        }
    }
    false
}

/// Whether the previous meaningful sibling (within the paragraph) is a *bare*
/// command — no attached groups. Then `command` may be that command's argument
/// rather than an invocation (`\let\bold\textbf`, `\expandafter\frac`), so the
/// rule stays silent.
fn follows_bare_command(command: &SyntaxNode) -> bool {
    let mut newlines = 0;
    let mut prev = command.prev_sibling_or_token();
    while let Some(el) = prev {
        let kind = el.kind();
        if kind == SyntaxKind::NEWLINE {
            newlines += 1;
            if newlines >= 2 {
                return false; // a blank line: nothing before it can consume us
            }
        } else if !is_trivia(kind) {
            return el.as_node().is_some_and(is_bare_command);
        }
        prev = prev_of(&el);
    }
    false
}

fn is_bare_command(node: &SyntaxNode) -> bool {
    node.kind() == SyntaxKind::COMMAND
        && !node.children_with_tokens().any(|child| {
            matches!(
                child.kind(),
                SyntaxKind::GROUP | SyntaxKind::OPTIONAL | SyntaxKind::VERB
            )
        })
}

/// Whether the next meaningful element after `command` (in document order,
/// climbing out of exhausted parents) is a boundary that cannot supply an
/// argument: the closing `}` of the enclosing group, a closing `$`/`\]`, an
/// alignment `&`, a `\\` line break, the `\end` of the enclosing environment, a
/// blank line, or the end of the file. Anything else could be an unbraced
/// argument, so the rule must stay silent.
fn at_hard_boundary(command: &SyntaxNode) -> bool {
    let mut newlines = 0;
    let mut cursor: SyntaxElement = command.clone().into();
    loop {
        let mut next = next_of(&cursor);
        while let Some(el) = next {
            let kind = el.kind();
            if kind == SyntaxKind::NEWLINE {
                newlines += 1;
                if newlines >= 2 {
                    return true; // blank line: the `\par` boundary
                }
            } else if !is_trivia(kind) {
                return match kind {
                    SyntaxKind::R_BRACE
                    | SyntaxKind::DOLLAR
                    | SyntaxKind::AMPERSAND
                    | SyntaxKind::LINE_BREAK
                    | SyntaxKind::END => true,
                    // `\]` closes display math; every other control symbol
                    // (`\%`, `\,`, …) is a valid single-token argument.
                    SyntaxKind::CONTROL_SYMBOL => {
                        el.as_token().is_some_and(|token| token.text() == "\\]")
                    }
                    _ => false,
                };
            }
            next = next_of(&el);
        }
        // The parent is exhausted; continue with what follows it.
        match parent_of(&cursor) {
            Some(parent) => cursor = parent.into(),
            None => return true, // end of file
        }
    }
}

fn next_of(el: &SyntaxElement) -> Option<SyntaxElement> {
    match el {
        rowan::NodeOrToken::Node(node) => node.next_sibling_or_token(),
        rowan::NodeOrToken::Token(token) => token.next_sibling_or_token(),
    }
}

fn prev_of(el: &SyntaxElement) -> Option<SyntaxElement> {
    match el {
        rowan::NodeOrToken::Node(node) => node.prev_sibling_or_token(),
        rowan::NodeOrToken::Token(token) => token.prev_sibling_or_token(),
    }
}

fn parent_of(el: &SyntaxElement) -> Option<SyntaxNode> {
    match el {
        rowan::NodeOrToken::Node(node) => node.parent(),
        rowan::NodeOrToken::Token(token) => token.parent(),
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
        let ctx = RuleContext::new(
            std::path::Path::new("x.tex"),
            &root,
            &model,
            None,
            None,
            None,
        );
        let mut out = Vec::new();
        let mut visitor = MissingRequiredArgument.stream().expect("streaming rule");
        for el in root.descendants_with_tokens() {
            visitor.visit(&el, &ctx, &mut out);
        }
        visitor.finish(&ctx, &mut out);
        out
    }

    #[test]
    fn flags_frac_missing_denominator_in_inline_math() {
        let src = "$\\frac{1}$\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "missing-required-argument");
        assert_eq!(
            out[0].message,
            "`\\frac` is missing 1 of its 2 required arguments"
        );
        // Report-only.
        assert!(out[0].fix.is_none());
        // Caret covers just the `\frac` control word.
        let at = src.find("\\frac").unwrap();
        assert_eq!((out[0].start, out[0].end), (at, at + "\\frac".len()));
    }

    #[test]
    fn flags_bare_command_at_end_of_known_command_argument() {
        let out = findings("\\emph{see \\textbf}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].message,
            "`\\textbf` is missing its required argument"
        );
    }

    #[test]
    fn flags_before_blank_line() {
        let out = findings("\\frac{1}\n\nmore text\n");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].message,
            "`\\frac` is missing 1 of its 2 required arguments"
        );
    }

    #[test]
    fn flags_at_end_of_file() {
        let out = findings("\\textbf\n");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].message,
            "`\\textbf` is missing its required argument"
        );
    }

    #[test]
    fn flags_completely_bare_frac() {
        let out = findings("$\\frac$\n");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].message,
            "`\\frac` is missing its 2 required arguments"
        );
    }

    #[test]
    fn flags_before_alignment_tab() {
        let out = findings("\\begin{tabular}{ll}\n\\textbf & b\n\\end{tabular}\n");
        assert_eq!(out.len(), 1);
        assert!(
            out[0].message.contains("\\textbf"),
            "got: {}",
            out[0].message
        );
    }

    #[test]
    fn flags_before_line_break_and_end_in_math_environment() {
        let out = findings("\\begin{align}\n\\frac \\\\\nx&\\emph\n\\end{align}\n");
        let names: Vec<&str> = out
            .iter()
            .map(|d| d.message.split('`').nth(1).unwrap())
            .collect();
        assert_eq!(names, vec!["\\frac", "\\emph"]);
    }

    #[test]
    fn optional_group_does_not_satisfy_required_arity() {
        let out = findings("$\\sqrt[3]$\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("\\sqrt"), "got: {}", out[0].message);
    }

    #[test]
    fn silent_when_unbraced_token_could_be_the_argument() {
        assert!(findings("$\\frac12$\n").is_empty());
        assert!(findings("$\\frac{1}2$\n").is_empty());
        assert!(findings("$\\frac\\alpha\\beta$\n").is_empty());
        assert!(findings("\\textbf~x\n").is_empty());
    }

    #[test]
    fn silent_when_arity_is_satisfied() {
        assert!(findings("\\textbf{x} and $\\frac{1}{2}$\n").is_empty());
    }

    #[test]
    fn silent_on_starred_form() {
        // The `*` parses as a WORD sibling that breaks greedy attachment; it also
        // reads as "could be the argument", so starred variants are never checked.
        assert!(findings("\\section*{A}\n").is_empty());
    }

    #[test]
    fn silent_when_groups_attach_across_comment_and_newline() {
        assert!(findings("$\\frac{1} % half\n{2}$\n").is_empty());
    }

    #[test]
    fn silent_in_definition_body() {
        // The partial-application idiom: `\textbf` is carried, not invoked.
        assert!(findings("\\newcommand{\\bold}{\\textbf}\n").is_empty());
    }

    #[test]
    fn silent_in_starred_definition_body() {
        // `\newcommand*`'s groups parse standalone (the `*` breaks attachment);
        // the standalone-group gate covers them.
        assert!(findings("\\newcommand*{\\bold}{\\textbf}\n").is_empty());
    }

    #[test]
    fn silent_in_standalone_scope_group() {
        assert!(findings("{\\textbf}\n").is_empty());
    }

    #[test]
    fn silent_in_unknown_command_argument() {
        assert!(findings("\\mymacro{\\textbf}\n").is_empty());
    }

    #[test]
    fn silent_after_bare_command_alias_form() {
        // `\textbf` is `\let`'s argument, not an invocation.
        assert!(findings("\\let\\bold\\textbf\n\nx\n").is_empty());
    }

    #[test]
    fn silent_when_name_is_redefined_in_the_file() {
        // The redefinition changes `\emph`'s arity; the built-in no longer applies.
        assert!(findings("\\renewcommand{\\emph}{nothing}\nsee \\emph\n").is_empty());
    }

    #[test]
    fn silent_on_verbatim_argument_command() {
        // `\mintinline`'s shape mixes a group with an opaque VERB capture; skipped.
        assert!(findings("\\mintinline\n").is_empty());
    }
}
