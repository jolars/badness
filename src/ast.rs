//! Small generic accessors over the CST — reading a `COMMAND` node's name and
//! its literal `{…}` argument text. Purely syntactic: these know nothing about
//! what any command *means*, so both the syntactic `project/` layer and the
//! semantic layer build on them without meaning leaking downward (AGENTS.md
//! decision #2). This is the seed of the `ast/` layer ravel has; it was
//! extracted when its second consumer (the semantic label/reference model)
//! appeared.

use crate::syntax::{SyntaxKind, SyntaxNode};

/// The control-word name of a `COMMAND` node (the leading `\` stripped), or
/// `None` for a control symbol. The grammar bumps the control word as the
/// command's first token (`grammar.rs`, `fn command`).
pub fn command_name(command: &SyntaxNode) -> Option<String> {
    command
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| token.kind() == SyntaxKind::CONTROL_WORD)
        .map(|token| token.text().trim_start_matches('\\').to_string())
}

/// The literal text inside the `n`-th `GROUP` argument of `command`, with the
/// enclosing braces dropped. Concatenates the group's inner token text so
/// content split across `WORD`/`.`/`/`/`UNDERSCORE` tokens (e.g.
/// `chapters/my_file`, `sec:intro`) reassembles. Returns `None` when there is
/// no `n`-th group, or when it holds non-token content (a nested command — not
/// a flat literal).
pub fn nth_group_text(command: &SyntaxNode, n: usize) -> Option<String> {
    let group = command
        .children()
        .filter(|child| child.kind() == SyntaxKind::GROUP)
        .nth(n)?;

    let mut text = String::new();
    for element in group.children_with_tokens() {
        match element {
            rowan::NodeOrToken::Token(token) => match token.kind() {
                SyntaxKind::L_BRACE | SyntaxKind::R_BRACE => {}
                _ => text.push_str(token.text()),
            },
            // A nested node (e.g. a COMMAND) means the argument isn't a flat
            // literal; treat the whole thing as unresolvable.
            rowan::NodeOrToken::Node(_) => return None,
        }
    }
    Some(text)
}

/// The environment name of a `BEGIN` or `END` node — the literal text of its
/// `NAME_GROUP` child, braces dropped. Returns `None` when the node has no
/// `NAME_GROUP` (a malformed `\begin`) or it holds non-token content. The grammar
/// emits the name as a `NAME_GROUP` (`grammar.rs`, `fn name_group`).
pub fn environment_name(begin_or_end: &SyntaxNode) -> Option<String> {
    let group = begin_or_end
        .children()
        .find(|child| child.kind() == SyntaxKind::NAME_GROUP)?;

    let mut text = String::new();
    for element in group.children_with_tokens() {
        match element {
            rowan::NodeOrToken::Token(token) => match token.kind() {
                SyntaxKind::L_BRACE | SyntaxKind::R_BRACE => {}
                _ => text.push_str(token.text()),
            },
            rowan::NodeOrToken::Node(_) => return None,
        }
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn command(src: &str) -> SyntaxNode {
        SyntaxNode::new_root(parse(src).green)
            .descendants()
            .find(|node| node.kind() == SyntaxKind::COMMAND)
            .expect("a COMMAND node")
    }

    #[test]
    fn command_name_strips_backslash() {
        assert_eq!(
            command_name(&command("\\section{Hi}\n")).as_deref(),
            Some("section")
        );
    }

    #[test]
    fn nth_group_text_reassembles_inner_tokens() {
        assert_eq!(
            nth_group_text(&command("\\label{sec:intro}\n"), 0).as_deref(),
            Some("sec:intro")
        );
    }

    #[test]
    fn nth_group_text_none_for_nested_command() {
        assert_eq!(nth_group_text(&command("\\input{\\jobname}\n"), 0), None);
    }

    #[test]
    fn nth_group_text_none_when_group_absent() {
        assert_eq!(nth_group_text(&command("\\input\n"), 0), None);
    }
}
