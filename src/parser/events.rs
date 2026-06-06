//! Flat parser events.
//!
//! The parser emits a flat `Vec<Event>` rather than building the tree directly
//! (the rust-analyzer / ravel shape). Tokens are referenced by index into the
//! token stream, and there is deliberately **no** `Error` event — syntax errors
//! ride a side channel keyed by byte range (see [`super::core::SyntaxError`]).

use crate::syntax::SyntaxKind;

// `Start`/`Finish` are unused by the Phase 0 flat parser (which emits only
// `Tok`); the Phase 1 grammar wraps tokens in nodes and exercises them.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// Open a node of the given kind.
    Start(SyntaxKind),
    /// Attach the token at this index in the token stream.
    Tok(usize),
    /// Close the most recently opened node.
    Finish,
}
