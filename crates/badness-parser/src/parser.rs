//! The lexer, the event-stream parser, and the green-tree builder.
//!
//! The pipeline follows rust-analyzer: `lex` produces a flat token
//! stream, the parser emits a flat list of [`events::Event`]s, and
//! [`tree_builder::build_tree`] replays tokens + events into a rowan green
//! tree.
//!
//! `build_tree` is a straight replay and makes no attachment decisions of its
//! own — trivia is already placed by the time it runs. Comment binding is the
//! grammar's (`grammar::trivia::binding_run`), which decides where a `%` run
//! attaches while the walk still has the context to say.

pub mod conditional;
pub mod core;
pub(crate) mod events;
pub(crate) mod grammar;
pub mod lexer;
pub(crate) mod tree_builder;

pub use core::{Parse, SyntaxError, parse, parse_with_flavor, reconstruct};
pub use grammar::is_def_prefix_command;
pub use lexer::{LatexFlavor, LexConfig, Token, lex};
