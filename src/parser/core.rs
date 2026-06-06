//! The parser entry point and its output type.
//!
//! `parse` runs the pipeline: [`lex`] → [`grammar::parse`] (the recursive
//! descent, which emits events + errors) → [`build_tree`] (the green tree).
//! Syntax errors ride a side channel and never abort the parse.

use rowan::GreenNode;

use crate::parser::grammar;
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
    let (events, errors) = grammar::parse(&tokens);
    let green = build_tree(&tokens, &events);
    Parse { green, errors }
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
    fn command_wraps_its_argument_group() {
        use crate::syntax::SyntaxKind;
        let parse = parse(r"\a{b}");
        let command = parse
            .syntax()
            .descendants()
            .find(|n| n.kind() == SyntaxKind::COMMAND)
            .expect("a COMMAND node");
        assert!(
            command.children().any(|n| n.kind() == SyntaxKind::GROUP),
            "the argument should be a nested GROUP node"
        );
        assert!(parse.errors.is_empty());
    }
}
