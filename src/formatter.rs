//! The formatter: parse → lower CST to a Wadler/Prettier [`Ir`](ir::Ir) → print.
//!
//! The MVP is an identity lowering (`format(x) == x`); see [`core`]. The IR
//! engine (`ir`, `printer`, `style`, `context`) is a language-agnostic
//! Wadler/Prettier layout engine; the LaTeX-specific part is the lowering in
//! [`core`].

pub mod check;
pub(crate) mod context;
pub mod core;
pub(crate) mod ir;
pub(crate) mod printer;
pub mod style;

pub use check::{CheckError, CheckResult, check_paths, check_paths_with_style};
pub use core::{
    FormatError, format, format_file_with_packages, format_node, format_node_with_signatures,
    format_with_style, format_with_style_flavored, format_with_style_flavored_with_signatures,
};
pub use style::{FormatStyle, WrapMode};
