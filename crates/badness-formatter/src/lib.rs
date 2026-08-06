//! badness-formatter — the deterministic, rule-based formatting engine behind
//! [badness](https://badness.dev/), for LaTeX (`.tex`, `.sty`/`.cls`, `.dtx`,
//! `.ins`) and BibTeX (`.bib`).
//!
//! Layout is decided solely by the formatter's rules and the Wadler/Prettier
//! layout engine, on the lossless CST from [`badness_parser`]. The formatter
//! changes only trivia — it never inserts, deletes, or rewrites a non-trivia
//! token — and `fmt(fmt(x)) == fmt(x)`.
//!
//! This crate is embeddable (it builds for `wasm32-unknown-unknown`); the
//! batch path-walking check API and the CLI live in the `badness` crate.

pub mod ast;
pub mod bib;
pub mod formatter;
pub mod parser;
pub mod semantic;
pub mod syntax;

pub use formatter::{
    FormatError, FormatStyle, LineEnding, MathWrap, SentenceOptions, WrapMode, format,
};

// Re-export rowan so embedders can name the exact tree types this crate is
// built against without pinning a matching rowan version themselves.
pub use rowan;
