//! Flat parser events.
//!
//! The parser emits a flat `Vec<Event>` rather than building the tree directly
//! (the rust-analyzer shape). Tokens are referenced by index into the
//! token stream, and there is deliberately **no** `Error` event — syntax errors
//! ride a side channel keyed by byte range (see [`super::core::SyntaxError`]).

use crate::syntax::SyntaxKind;

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// Open a node of the given kind.
    Start(SyntaxKind),
    /// Attach the token at this index in the token stream.
    Tok(usize),
    /// Attach a `WORD` sub-token: the `start..end` byte slice of the token at
    /// `idx`. Used to split a math `WORD` glued around operators (`a+2*1`) into
    /// separate operand/operator atoms (`a`, `+`, `2`, `*`, `1`) inside math
    /// mode, without touching the catcode-faithful lexer. Losslessness is
    /// preserved because the `SubTok` slices emitted for one token cover its full
    /// byte range contiguously (see [`super::grammar`]).
    SubTok {
        idx: usize,
        start: usize,
        end: usize,
    },
    /// Close the most recently opened node.
    Finish,
}
