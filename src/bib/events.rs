//! Flat parser events for the BibTeX grammar.
//!
//! **EXTRACTION CANDIDATE.** This is a copy of [`crate::parser::events`] over the
//! bib [`SyntaxKind`]; the two are identical except for the kind type. Kept
//! separate per `AGENTS.md` ("copy now, extract later") — once the bib parser is
//! proven, both can move to a shared, kind-generic crate.
//!
//! The parser emits a flat `Vec<Event>` rather than building the tree directly
//! (the rust-analyzer / arity shape). Tokens are referenced by index into the
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
