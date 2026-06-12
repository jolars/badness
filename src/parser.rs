//! The lexer, the event-stream parser, and the green-tree builder.
//!
//! The pipeline mirrors arity / rust-analyzer: `lex` produces a flat token
//! stream, the parser emits a flat list of [`events::Event`]s, and
//! [`tree_builder::build_tree`] turns tokens + events into a rowan green tree,
//! re-attaching trivia along the way.

pub mod core;
pub(crate) mod events;
pub(crate) mod grammar;
pub mod lexer;
pub(crate) mod tree_builder;

pub use core::{Parse, SyntaxError, parse, reconstruct};
pub use lexer::{Token, lex};
