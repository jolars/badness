//! badness-parser — the lossless CST parser, semantic model, and
//! command-signature database behind [badness](https://badness.dev/), for
//! LaTeX (`.tex`, `.sty`/`.cls`, `.dtx`, `.ins`) and BibTeX (`.bib`).
//!
//! The parser treats input as generic TeX surface syntax and always produces a
//! lossless rowan tree: `reconstruct(text) == text`, byte for byte. Semantics
//! (arity, verbatim-ness, sectioning) are layered on top in [`semantic`],
//! never inside the grammar.

pub mod ast;
pub mod bib;
pub mod directives;
pub mod parser;
pub mod semantic;
pub mod syntax;

// Re-export rowan so embedders can name the exact tree types this crate is
// built against without pinning a matching rowan version themselves.
pub use rowan;
