//! The BibTeX/BibLaTeX formatter: parse → lower the bib CST to the shared
//! Wadler/Prettier [`Ir`](crate::formatter::ir::Ir) → print.
//!
//! Reuses the language-agnostic engine (`crate::formatter::{ir, printer, style}`);
//! only the lowering in [`core`] is bib-specific, the same split the LaTeX
//! formatter has. A directory module so value/brace logic can grow into a sibling
//! file without churn.

use crate::bib::syntax::{SyntaxKind, SyntaxNode};
use crate::directives::{Verb, parse_directive};

pub mod core;
mod sort;

pub use core::{FormatError, format, format_node, format_with_style};

/// The verb of a structured `@comment{…}` directive that affects lint. BibTeX
/// formatting has no suppression mechanism, so format-only directives retain ordinary
/// block behavior. The linter's `skip`/`off`/`on` placement is the fact that makes the
/// remaining directives attachment-sensitive to formatting and sorting.
fn lint_directive_verb(node: &SyntaxNode) -> Option<Verb> {
    if node.kind() != SyntaxKind::COMMENT_ENTRY {
        return None;
    }
    let text = node.to_string();
    let open = text.find(['{', '('])?;
    let close = text.rfind(['}', ')'])?;
    if close <= open {
        return None;
    }
    let directive = parse_directive(text[open + 1..close].trim())?;
    directive.axis.covers_lint().then_some(directive.verb)
}
