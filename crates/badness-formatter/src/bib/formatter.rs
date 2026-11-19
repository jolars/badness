//! The BibTeX/BibLaTeX formatter: parse → lower the bib CST to the shared
//! Wadler/Prettier [`Ir`](crate::formatter::ir::Ir) → print.
//!
//! Reuses the language-agnostic engine (`crate::formatter::{ir, printer, style}`);
//! only the lowering in [`core`] is bib-specific, the same split the LaTeX
//! formatter has. A directory module so value/brace logic can grow into a sibling
//! file without churn.

use crate::bib::{comment_directive, syntax::SyntaxNode};
use crate::directives::Verb;

pub mod core;
mod sort;

pub use core::{FormatError, format, format_node, format_with_style};

/// Lint directives keep their attachment during formatting and sorting.
fn lint_directive_verb(node: &SyntaxNode) -> Option<Verb> {
    comment_directive(node)
        .and_then(|directive| directive.axis.covers_lint().then_some(directive.verb))
}
