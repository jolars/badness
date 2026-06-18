//! The BibTeX parser entry point and its output type.
//!
//! `parse` runs the pipeline: [`lex`] → [`grammar::parse`] (the recursive
//! descent, which emits events + errors) → [`build_tree`] (the green tree).
//! Syntax errors ride a side channel and never abort the parse.

use rowan::GreenNode;

use crate::bib::grammar;
use crate::bib::lexer::lex;
use crate::bib::syntax::SyntaxNode;
use crate::bib::tree_builder::build_tree;

/// A parsed `.bib` file: the green tree plus any syntax errors gathered
/// alongside it. Errors never abort the parse.
#[derive(Debug, Clone)]
pub struct Parse {
    pub green: GreenNode,
    pub errors: Vec<SyntaxError>,
}

impl Parse {
    /// Materialize a fresh red-tree cursor over the parsed file. Cheap (an atomic
    /// clone of the green node).
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

/// Parse BibTeX/BibLaTeX source into a lossless CST.
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
    use crate::bib::syntax::SyntaxKind;

    #[test]
    fn reconstruct_is_identity() {
        let input = "@article{key,\n  title = {Hi},\n  year = 2020,\n}\n";
        assert_eq!(reconstruct(input), input);
    }

    #[test]
    fn entry_has_key_and_fields() {
        let parse = parse("@article{key, title = {Hi}}");
        let entry = parse
            .syntax()
            .descendants()
            .find(|n| n.kind() == SyntaxKind::ENTRY)
            .expect("an ENTRY node");
        assert!(
            entry.children().any(|n| n.kind() == SyntaxKind::KEY),
            "the entry should have a KEY node"
        );
        assert!(
            entry.children().any(|n| n.kind() == SyntaxKind::FIELD),
            "the entry should have a FIELD node"
        );
        assert!(parse.errors.is_empty());
    }
}
