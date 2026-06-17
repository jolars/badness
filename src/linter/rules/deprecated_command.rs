//! `deprecated-command`: the obsolete two-letter font *switches* (`\bf`, `\it`,
//! …) superseded by the LaTeX 2e `\…shape`/`\…family`/`\…series` declarations.
//!
//! These are the classic `\bf`-style commands the LaTeX team has discouraged
//! since 1994. The replacement is a plain declaration swap (`\bf` → `\bfseries`),
//! so the message names the modern form — the seed of a later autofix (this
//! slice reports only; see the plan / Tenet 5). `\em` is intentionally absent:
//! it is still the supported emphasis switch.
//!
//! The table lives here, not in `data/signatures.json`: deprecation is a lint
//! judgment, not the structural arity/verbatim fact the signature DB carries
//! (AGENTS.md core decision #2).

use std::path::PathBuf;

use rowan::NodeOrToken;

use crate::ast::command_name;
use crate::linter::diagnostic::{Diagnostic, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use super::{Rule, RuleContext};

/// Deprecated control word → its modern replacement.
const DEPRECATED: &[(&str, &str)] = &[
    ("bf", "bfseries"),
    ("it", "itshape"),
    ("rm", "rmfamily"),
    ("sf", "sffamily"),
    ("tt", "ttfamily"),
    ("sc", "scshape"),
    ("sl", "slshape"),
];

pub struct DeprecatedCommand;

impl Rule for DeprecatedCommand {
    fn id(&self) -> &'static str {
        "deprecated-command"
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
        let Some((_, replacement)) = DEPRECATED.iter().find(|(dep, _)| *dep == name) else {
            return;
        };
        // Underline just the control word, not any greedily-attached group, so
        // the caret sits tightly on `\bf`.
        let range = control_word_range(command).unwrap_or_else(|| command.text_range());
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start: usize::from(range.start()),
            end: usize::from(range.end()),
            message: format!("`\\{name}` is deprecated; use `\\{replacement}`"),
        });
    }
}

/// The range of a `COMMAND` node's leading `CONTROL_WORD` token.
fn control_word_range(command: &SyntaxNode) -> Option<rowan::TextRange> {
    command
        .children_with_tokens()
        .find_map(|element| match element {
            NodeOrToken::Token(token) if token.kind() == SyntaxKind::CONTROL_WORD => {
                Some(token.text_range())
            }
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::semantic::SemanticModel;

    fn findings(src: &str) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse(src).green);
        let model = SemanticModel::build(&root);
        let ctx = RuleContext {
            path: std::path::Path::new("x.tex"),
            root: &root,
            model: &model,
            resolution: None,
        };
        let mut out = Vec::new();
        for el in root.descendants_with_tokens() {
            if DeprecatedCommand.interests().contains(&el.kind()) {
                DeprecatedCommand.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_bare_font_switch() {
        let out = findings("{\\bf hi}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "deprecated-command");
        assert!(
            out[0].message.contains("\\bfseries"),
            "got: {}",
            out[0].message
        );
        // Caret covers just `\bf` (bytes 1..4), not the trailing text.
        assert_eq!((out[0].start, out[0].end), (1, 4));
    }

    #[test]
    fn modern_commands_are_fine() {
        assert!(findings("\\textbf{x}\\emph{y}\n").is_empty());
    }

    #[test]
    fn em_is_not_deprecated() {
        assert!(findings("{\\em hi}\n").is_empty());
    }

    #[test]
    fn flags_each_occurrence() {
        assert_eq!(findings("{\\bf a}{\\it b}\n").len(), 2);
    }
}
