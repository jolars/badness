//! Builds a rowan green tree from a token stream and a list of parser events.
//!
//! **EXTRACTION CANDIDATE.** A copy of [`crate::parser::tree_builder`] over the
//! bib [`SyntaxKind`]; identical except for the kind type. See
//! [`super::events`] for the rationale.

use rowan::{GreenNode, GreenNodeBuilder};

use crate::bib::events::Event;
use crate::bib::lexer::Token;
use crate::bib::syntax::SyntaxKind;

/// Replay `events` against `tokens` to construct the green tree. The whole file
/// is wrapped in a single `ROOT` node.
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
