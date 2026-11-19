//! Expl3 command positions following the lexer's letter modes.

use crate::ast::{AstNode, Environment};
use crate::parser::lexer::{
    ExplToggle, docstrip_discards_suffix, dtx_has_expl_signal, expl_toggle,
};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};
use rowan::{TextSize, WalkEvent};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ModeIndex {
    commands: Vec<TextSize>,
}

impl ModeIndex {
    pub fn build(root: &SyntaxNode, dtx: bool) -> Self {
        Self::build_view(root, dtx, false)
    }

    /// Include ordinary and inline-guarded DocStrip code in the lint view.
    pub fn build_for_lint(root: &SyntaxNode, dtx: bool) -> Self {
        Self::build_view(root, dtx, true)
    }

    fn build_view(root: &SyntaxNode, dtx: bool, lint: bool) -> Self {
        let implicit = dtx && dtx_has_expl_signal(&root.text().to_string());
        let mut index = Self::default();
        let mut active = false;
        let mut doc_line = false;
        let mut discarded_suffix = false;
        let mut saved = Vec::new();
        for event in root.preorder_with_tokens() {
            match event {
                WalkEvent::Enter(SyntaxElement::Node(node)) if dtx && is_macrocode(&node) => {
                    saved.push(active);
                    active |= implicit;
                }
                WalkEvent::Leave(SyntaxElement::Node(node)) if dtx && is_macrocode(&node) => {
                    active = saved.pop().unwrap_or(false);
                }
                WalkEvent::Enter(SyntaxElement::Token(token)) => match token.kind() {
                    SyntaxKind::DOC_MARGIN => doc_line = true,
                    SyntaxKind::GUARD if dtx => {
                        discarded_suffix = docstrip_discards_suffix(token.text());
                    }
                    SyntaxKind::NEWLINE => {
                        doc_line = false;
                        discarded_suffix = false;
                    }
                    SyntaxKind::CONTROL_WORD | SyntaxKind::CONTROL_SYMBOL => {
                        let executable = !doc_line && !discarded_suffix;
                        if active && executable && (!dtx || lint || !saved.is_empty()) {
                            index.commands.push(token.text_range().start());
                        }
                        if executable
                            && token.kind() == SyntaxKind::CONTROL_WORD
                            && let Some(toggle) = expl_toggle(token.text())
                        {
                            active = toggle == ExplToggle::On;
                            if let Some(outer) = saved.last_mut() {
                                *outer = active;
                            }
                        }
                    }
                    _ if discarded_suffix && token.text().contains(['\n', '\r']) => {
                        discarded_suffix = false;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        index
    }

    pub fn at_command(&self, start: TextSize) -> bool {
        self.commands.binary_search(&start).is_ok()
    }
}

fn is_macrocode(node: &SyntaxNode) -> bool {
    Environment::cast(node.clone())
        .and_then(|env| env.name())
        .is_some_and(|name| matches!(name.as_str(), "macrocode" | "macrocode*"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{LexConfig, parse_with_flavor};

    fn at(text: &str, needle: &str, dtx: bool) -> bool {
        let parsed = parse_with_flavor(
            text,
            LexConfig {
                dtx,
                ..Default::default()
            },
        );
        assert_eq!(parsed.syntax().text().to_string(), text);
        ModeIndex::build(&parsed.syntax(), dtx)
            .at_command(TextSize::from(text.find(needle).unwrap() as u32))
    }

    #[test]
    fn explicit_modes_and_protected_text() {
        assert!(at("\\ExplSyntaxOn\n\\tl", "\\tl", false));
        assert!(at(
            "\\ProvidesExplPackage{foo}{2026/01/01}{1}{Demo}\n\\tl",
            "\\tl",
            false
        ));
        assert!(!at("\\ExplSyntaxOn\n\\ExplSyntaxOff\n\\tl", "\\tl", false));
        assert!(!at("% \\ExplSyntaxOn\n\\tl", "\\tl", false));
        assert!(!at("\\verb|\\ExplSyntaxOn|\n\\tl", "\\tl", false));
    }

    #[test]
    fn dtx_implicit_mode_and_chunk_restoration() {
        let text = "%<@@=demo>\n%    \\begin{macrocode}\n\\tl\n\\ExplSyntaxOff\n\\seq\n%    \\end{macrocode}\n% Documentation \\int\n%    \\begin{macrocode}\n\\cs\n%    \\end{macrocode}\n";
        assert!(at(text, "\\tl", true));
        assert!(!at(text, "\\seq", true));
        assert!(!at(text, "\\int", true));
        assert!(at(text, "\\cs", true));
        let text = "%    \\begin{macrocode}\n\\tl\n%    \\end{macrocode}\n% \\ProvidesExplPackage{demo}{}{}{}\n";
        assert!(at(text, "\\tl", true));
    }
}
