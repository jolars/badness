//! Flat parser events for the BibTeX grammar.
//!
//! This is a copy of [`crate::parser::events`] over the bib [`SyntaxKind`]; the
//! two are identical except for the kind type, kept separate so the bib grammar
//! can evolve independently. Once the bib parser is proven, the two could be
//! unified into one kind-generic implementation.
//!
//! The parser emits a flat `Vec<Event>` rather than building the tree directly
//! (the rust-analyzer shape). Tokens are referenced by index into the
//! token stream, and there is deliberately **no** `Error` event — syntax errors
//! ride a side channel keyed by byte range (see [`super::core::SyntaxError`]).

use crate::bib::syntax::SyntaxKind;

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// Open a node of the given kind.
    Start(SyntaxKind),
    /// Attach the token at this index in the token stream.
    Tok(usize),
    /// Close the most recently opened node.
    Finish,
}
