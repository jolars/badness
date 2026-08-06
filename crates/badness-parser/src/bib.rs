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
