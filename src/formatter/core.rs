//! The formatter entry points and the CST → [`Ir`] lowering.
//!
//! MVP milestone: an **identity lowering**. Every CST token is emitted verbatim,
//! so `format(x) == x` byte-for-byte. This proves the parse → lower → print
//! pipeline end-to-end before any opinionated rules land; each real rule then
//! becomes a small, verifiable diff against this known-good baseline.
//!
//! The lowering (`lower_node`) is the LaTeX-specific part that replaces ravel's
//! R `ir_expr_node` dispatch; the surrounding `format`/`format_with_style`
//! framework mirrors ravel's `src/formatter/core.rs`.

use crate::parser::parse;
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use super::context::FormatContext;
use super::ir::Ir;
use super::printer::Printer;
use super::style::FormatStyle;

/// Why a document could not be formatted. The formatter only operates on a clean
/// parse: anything the parser flagged, or any `ERROR` token, is refused rather
/// than silently reshaped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// The input parsed with `count` syntax error(s); the formatter only
    /// supports input the parser accepts without diagnostics.
    ParseErrors { count: usize },
    /// The CST contains an `ERROR` token the lowering does not handle.
    UnsupportedConstruct { kind: SyntaxKind, snippet: String },
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ParseErrors { count } => write!(
                f,
                "input contains {count} parser diagnostic(s); formatter only supports parseable input"
            ),
            Self::UnsupportedConstruct { kind, snippet } => {
                write!(
                    f,
                    "unsupported construct for formatter: {kind:?} near {snippet:?}"
                )
            }
        }
    }
}

impl std::error::Error for FormatError {}

/// Format `input` with the default [`FormatStyle`].
pub fn format(input: &str) -> Result<String, FormatError> {
    format_with_style(input, FormatStyle::default())
}

/// Format `input` under `style`. Returns [`FormatError`] if the input does not
/// parse cleanly. Note: badness's [`crate::parser::Parse`] carries `errors` +
/// `syntax()` (ravel uses `diagnostics` + `cst`).
pub fn format_with_style(input: &str, style: FormatStyle) -> Result<String, FormatError> {
    let parsed = parse(input);
    if !parsed.errors.is_empty() {
        return Err(FormatError::ParseErrors {
            count: parsed.errors.len(),
        });
    }

    let root = parsed.syntax();
    validate_supported_tokens(&root)?;

    let ctx = FormatContext::new(style);
    let mut formatted = format_root(&root, ctx);
    // Preserve a final newline the lowering may have dropped (a no-op under the
    // identity lowering, kept for parity with ravel and future rules).
    if input.ends_with('\n') && !formatted.ends_with('\n') {
        formatted.push('\n');
    }
    Ok(formatted)
}

/// Refuse any `ERROR` token. A clean parse should contain none, but the parser
/// can emit them on recovery; the formatter never reshapes around them.
fn validate_supported_tokens(root: &SyntaxNode) -> Result<(), FormatError> {
    for element in root.descendants_with_tokens() {
        let Some(token) = element.into_token() else {
            continue;
        };
        if token.kind() == SyntaxKind::ERROR {
            return Err(FormatError::UnsupportedConstruct {
                kind: token.kind(),
                snippet: token.text().to_string(),
            });
        }
    }
    Ok(())
}

fn format_root(root: &SyntaxNode, ctx: FormatContext) -> String {
    let ir = lower_node(root);
    Printer::new(ctx.style()).print(&ir)
}

/// Lower a CST node to IR by concatenating its children. Tokens are emitted
/// verbatim (via [`Ir::verbatim`], which reproduces embedded newlines exactly),
/// nodes recurse. With no break-points introduced, the printer reproduces the
/// input byte-for-byte — the identity baseline that real rules refine.
fn lower_node(node: &SyntaxNode) -> Ir {
    Ir::concat(node.children_with_tokens().map(lower_element))
}

fn lower_element(element: SyntaxElement) -> Ir {
    match element {
        SyntaxElement::Node(node) => lower_node(&node),
        SyntaxElement::Token(token) => Ir::verbatim(token.text()),
    }
}
