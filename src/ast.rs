//! Small generic accessors over the CST — reading a `COMMAND` node's name and
//! its literal `{…}` argument text. Purely syntactic: these know nothing about
//! what any command *means*, so both the syntactic `project/` layer and the
//! semantic layer build on them without meaning leaking downward (AGENTS.md
//! decision #2). This is the seed of the `ast/` layer arity has; it was
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

/// The `n`-th `GROUP` argument node of `command`, if present. The thin node-level
/// counterpart to [`nth_group_text`], for callers that must inspect a group's
/// structure (a nested `\name` command, or raw source) rather than flatten it to a
/// literal.
pub fn nth_group(command: &SyntaxNode, n: usize) -> Option<SyntaxNode> {
    command
        .children()
        .filter(|child| child.kind() == SyntaxKind::GROUP)
        .nth(n)
}

/// The control-word name (leading `\` stripped) of a single `COMMAND` wrapped in
/// `group`, as in a `\newcommand{\foo}` / `\NewDocumentCommand{\foo}` name group.
/// Returns `None` unless the group's only non-trivia child is exactly one control
/// word — anything else (plain text, multiple tokens, a control symbol) is not a
/// definable command name we extract.
pub fn group_command_name(group: &SyntaxNode) -> Option<String> {
    let command = group
        .children()
        .find(|child| child.kind() == SyntaxKind::COMMAND)?;
    command_name(&command)
}

/// The raw inner source of `group` with its outer `L_BRACE`/`R_BRACE` dropped, but
/// *all* interior text preserved — nested `{…}` braces included. Unlike
/// [`nth_group_text`], which bails on nested nodes, this reconstructs the verbatim
/// content needed for an xparse argument spec like `{m O{0} m}` (whose `{0}` default
/// parses as a nested `GROUP`). Trivia (whitespace/newlines) is kept verbatim; the
/// caller tokenizes the result.
pub fn group_inner_source(group: &SyntaxNode) -> String {
    let mut text = String::new();
    for element in group.descendants_with_tokens() {
        if let rowan::NodeOrToken::Token(token) = element {
            text.push_str(token.text());
        }
    }
    // Drop the outer braces the group carries as its first/last tokens (tolerating
    // a malformed group missing one).
    let inner = text.strip_prefix('{').unwrap_or(&text);
    inner.strip_suffix('}').unwrap_or(inner).to_string()
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

    #[test]
    fn group_command_name_reads_braced_control_word() {
        let cmd = command("\\newcommand{\\foo}{x}\n");
        let name = nth_group(&cmd, 0).and_then(|g| group_command_name(&g));
        assert_eq!(name.as_deref(), Some("foo"));
    }

    #[test]
    fn group_command_name_none_for_plain_text() {
        let cmd = command("\\newenvironment{thm}{a}{b}\n");
        let name = nth_group(&cmd, 0).and_then(|g| group_command_name(&g));
        assert_eq!(name, None);
    }

    #[test]
    fn group_inner_source_keeps_nested_braces() {
        // The xparse spec group `{m O{d} m}` parses the `{d}` default as a nested
        // GROUP; `nth_group_text` would reject it, but the raw source survives.
        let cmd = command("\\NewDocumentCommand{\\foo}{m O{d} m}{x}\n");
        let spec = nth_group(&cmd, 1).map(|g| group_inner_source(&g));
        assert_eq!(spec.as_deref(), Some("m O{d} m"));
        assert_eq!(nth_group_text(&cmd, 1), None);
    }
}
