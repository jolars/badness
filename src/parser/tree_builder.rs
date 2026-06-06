//! Builds a rowan green tree from a token stream and a list of parser events.

use rowan::{GreenNode, GreenNodeBuilder};

use crate::parser::events::Event;
use crate::parser::lexer::Token;
use crate::syntax::SyntaxKind;

/// Replay `events` against `tokens` to construct the green tree. The whole
/// document is wrapped in a single `ROOT` node.
pub(crate) fn build_tree(tokens: &[Token], events: &[Event]) -> GreenNode {
    let mut builder = GreenNodeBuilder::new();
    builder.start_node(SyntaxKind::ROOT.into());
    for event in events {
        match *event {
            Event::Start(kind) => builder.start_node(kind.into()),
            Event::Tok(idx) => {
                let tok = &tokens[idx];
                builder.token(tok.kind.into(), &tok.text);
            }
            Event::Finish => builder.finish_node(),
        }
    }
    builder.finish_node();
    builder.finish()
}
