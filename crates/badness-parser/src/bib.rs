//! The BibTeX/BibLaTeX parser: lexer, event-stream parser, and green-tree
//! builder.
//!
//! A sibling of [`crate::parser`], built on the same lossless rowan CST + flat
//! event-stream architecture but for `.bib` files, which are a distinct grammar
//! with their own [`syntax::SyntaxKind`] and [`syntax::BibLang`] marker. The
//! pipeline follows rust-analyzer: `lex` produces a flat token stream,
//! the parser emits a flat list of [`events::Event`]s, and
//! [`tree_builder::build_tree`] turns tokens + events into a rowan green tree.
//!
//! This is the parser layer only; the `.bib` formatter lives in the
//! `badness-formatter` crate, and the linter and LSP integration in the
//! `badness` crate.

pub mod ast;
pub mod core;
pub(crate) mod events;
pub(crate) mod grammar;
pub mod lexer;
pub mod semantic;
pub mod syntax;
pub(crate) mod tree_builder;

pub use core::{Parse, SyntaxError, parse, reconstruct};
pub use lexer::{Token, lex};

/// Read a suppression directive carried by a structured BibTeX comment.
pub fn comment_directive(node: &syntax::SyntaxNode) -> Option<crate::directives::Directive> {
    if node.kind() != syntax::SyntaxKind::COMMENT_ENTRY {
        return None;
    }
    use syntax::SyntaxKind::{L_BRACE, L_PAREN, R_BRACE, R_PAREN};

    let open = node
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| matches!(token.kind(), L_BRACE | L_PAREN))?;
    let close = node.last_child_or_token()?.into_token()?;
    if !matches!(
        (open.kind(), close.kind()),
        (L_BRACE, R_BRACE) | (L_PAREN, R_PAREN)
    ) {
        return None;
    }
    let body = rowan::TextRange::new(open.text_range().end(), close.text_range().start());
    let body = node
        .text()
        .slice(body - node.text_range().start())
        .to_string();
    crate::directives::parse_directive(body.trim())
}
