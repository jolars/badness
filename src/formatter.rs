//! The formatter: parse → lower CST to a Wadler/Prettier [`Ir`](ir::Ir) → print.
//!
//! The MVP is an identity lowering (`format(x) == x`); see [`core`]. The IR
//! engine (`ir`, `printer`, `style`, `context`) is copied ~wholesale from ravel
//! and marked EXTRACTION CANDIDATE — keep it close to ravel's so the eventual
//! shared-crate extraction stays mechanical. The LaTeX-specific part is the
//! lowering in [`core`].

pub mod check;
pub(crate) mod context;
pub mod core;
pub(crate) mod ir;
pub(crate) mod printer;
pub mod style;

pub use check::{CheckError, CheckResult, check_paths, check_paths_with_style};
pub use core::{FormatError, format, format_with_style};
pub use style::FormatStyle;
