//! Bounded recognition of Beamer overlay bodies for formatter layout.
//!
//! The parser deliberately leaves angle specifications generic. Match only the
//! documented wrapper shapes, without changing attachment or interpreting the
//! overlay's meaning. See Beamer's `doc/beamerug-overlays.tex`.

use rowan::Direction;

use crate::ast::command_name;
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, is_collapsible_trivia};

/// Whether this group fills a body slot of a complete overlay wrapper.
/// Greedy parser attachment can put the group inside the command or beside it
/// after an angle specification. Only the documented number of bodies belongs
/// to the wrapper; subsequent groups retain ordinary formatting.
pub(super) fn is_body_group(group: &SyntaxNode) -> bool {
    let Some(parent) = group.parent() else {
        return false;
    };
    let command = if parent.kind() == SyntaxKind::COMMAND {
        parent
    } else {
        let mut candidate = None;
        let mut groups = 0;
        for element in group.siblings_with_tokens(Direction::Prev).skip(1) {
            match element {
                SyntaxElement::Node(node) if node.kind() == SyntaxKind::COMMAND => {
                    candidate = Some(node);
                    break;
                }
                SyntaxElement::Node(node) if node.kind() == SyntaxKind::GROUP && groups < 2 => {
                    groups += 1;
                }
                SyntaxElement::Token(token)
                    if is_collapsible_trivia(token.kind())
                        || matches!(token.kind(), SyntaxKind::WORD | SyntaxKind::COMMENT) => {}
                _ => return false,
            }
        }
        let Some(command) = candidate else {
            return false;
        };
        command
    };
    body_groups(&command).is_some_and(|bodies| bodies.contains(group))
}

/// The final stream element of a complete wrapper. Keeping its header and
/// bodies in one layout unit prevents a multiline body from stranding part of
/// an angle specification on a separate line.
pub(super) fn wrapper_end(elements: &[SyntaxElement], start: usize) -> Option<usize> {
    let command = elements.get(start)?.as_node()?;
    if command.kind() != SyntaxKind::COMMAND {
        return None;
    }
    let bodies = body_groups(command)?;
    let last = bodies.last()?;
    if last.parent().as_ref() == Some(command) {
        Some(start)
    } else {
        elements[start + 1..]
            .iter()
            .position(|element| element.as_node() == Some(last))
            .map(|offset| start + 1 + offset)
    }
}

fn body_groups(command: &SyntaxNode) -> Option<Vec<SyntaxNode>> {
    let name = command_name(command)?;
    let bodies = match name.as_str() {
        "only" | "uncover" | "visible" | "invisible" | "onslide" | "action" => 1,
        "alt" => 2,
        "temporal" => 3,
        _ => return None,
    };
    let mut elements = command
        .children_with_tokens()
        .skip_while(|element| element.kind() != SyntaxKind::CONTROL_WORD)
        .skip(1)
        .chain(command.siblings_with_tokens(Direction::Next).skip(1))
        .peekable();
    let mut prefix = String::new();
    while let Some(SyntaxElement::Token(token)) = elements.peek() {
        match token.kind() {
            SyntaxKind::WORD => prefix.push_str(token.text()),
            SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT => {}
            _ => return None,
        }
        elements.next();
    }
    let prefix = if name == "onslide" {
        prefix.strip_prefix(['*', '+']).unwrap_or(&prefix)
    } else {
        prefix.as_str()
    };
    let has_overlay = prefix
        .strip_prefix('<')
        .and_then(|text| text.strip_suffix('>'))
        .is_some_and(|text| !text.contains(['<', '>']));
    if !prefix.is_empty() && !has_overlay || name == "temporal" && !has_overlay {
        return None;
    }
    let mut found = Vec::with_capacity(bodies);
    for _ in 0..bodies {
        while elements.peek().is_some_and(|element| {
            is_collapsible_trivia(element.kind()) || element.kind() == SyntaxKind::COMMENT
        }) {
            elements.next();
        }
        let Some(SyntaxElement::Node(body)) = elements.next() else {
            return None;
        };
        if body.kind() != SyntaxKind::GROUP {
            return None;
        }
        found.push(body);
    }
    Some(found)
}
