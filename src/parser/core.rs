//! The parser entry point and its output type.
//!
//! Phase 0 produces a *flat* tree — every token is a direct child of `ROOT` —
//! which is enough to exercise the lex → events → green-tree pipeline and to
//! enforce the losslessness invariant end to end. The real grammar arrives in
//! Phase 1; it will emit `Start`/`Finish` events around the same token stream
//! without changing this entry point's signature.

use rowan::GreenNode;

use crate::parser::events::Event;
use crate::parser::lexer::lex;
use crate::parser::tree_builder::build_tree;
use crate::syntax::SyntaxNode;

/// A parsed document: the green tree plus any syntax errors gathered alongside
/// it. Errors never abort the parse (see `AGENTS.md`, Core decision #5).
#[derive(Debug, Clone)]
pub struct Parse {
    pub green: GreenNode,
    pub errors: Vec<SyntaxError>,
}

impl Parse {
    /// Materialize a fresh red-tree cursor over the parsed document. Cheap (an
    /// atomic clone of the green node).
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }
}

/// A syntax error, carried on a side channel keyed by byte range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub message: String,
    pub start: usize,
    pub end: usize,
}

/// Parse LaTeX source into a lossless CST.
pub fn parse(input: &str) -> Parse {
    let tokens = lex(input);
    // Phase 0: flat tree. Every token becomes a direct child of ROOT.
    let events: Vec<Event> = (0..tokens.len()).map(Event::Tok).collect();
    let green = build_tree(&tokens, &events);
    Parse {
        green,
        errors: Vec::new(),
    }
}

/// Parse `input` and render the CST back to source. By the losslessness
/// invariant this always equals `input`.
pub fn reconstruct(input: &str) -> String {
    parse(input).syntax().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconstruct_is_identity() {
        let input = "\\section{Hi}\n\nbody $x^2$ % c\n";
        assert_eq!(reconstruct(input), input);
    }

    #[test]
    fn flat_tree_children_are_all_tokens() {
        let parse = parse(r"\a{b}");
        let root = parse.syntax();
        // No nested nodes yet: every child of ROOT is a token in Phase 0.
        assert!(root.children().next().is_none());
        assert!(root.children_with_tokens().all(|e| e.as_token().is_some()));
    }
}
