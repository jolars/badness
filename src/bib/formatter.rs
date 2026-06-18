//! The BibTeX/BibLaTeX formatter: parse → lower the bib CST to the shared
//! Wadler/Prettier [`Ir`](crate::formatter::ir::Ir) → print.
//!
//! Reuses the language-agnostic engine (`crate::formatter::{ir, printer, style}`,
//! all EXTRACTION CANDIDATEs); only the lowering in [`core`] is bib-specific — the
//! same split the LaTeX formatter has. A directory module so value/brace logic can
//! grow into a sibling file without churn.

pub mod core;

pub use core::{FormatError, format, format_node, format_with_style};
