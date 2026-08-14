//! The formatter: parse → lower CST to a Wadler/Prettier [`Ir`](ir::Ir) → print.
//!
//! The MVP is an identity lowering (`format(x) == x`); see [`core`]. The IR
//! engine (`ir`, `printer`, `style`, `context`) is a language-agnostic
//! Wadler/Prettier layout engine; the LaTeX-specific part is the lowering in
//! [`core`].

pub(crate) mod colspec;
pub(crate) mod context;
pub mod core;
pub(crate) mod ir;
pub mod perturb;
pub(crate) mod printer;
pub mod sentence;
pub mod style;

pub use core::{
    FormatError, declared_scope, format, format_node, format_node_range_with_signatures,
    format_node_range_with_signatures_sentence, format_node_with_signatures,
    format_node_with_signatures_sentence, format_with_declarations_sentence, format_with_style,
    format_with_style_flavored, format_with_style_flavored_sentence,
    format_with_style_flavored_with_signatures,
};
pub use sentence::SentenceOptions;
pub use style::{FormatStyle, LineEnding, MathWrap, WrapMode};
