//! The formatter entry points and the CST → [`Ir`] lowering.
//!
//! The first opinionated rule is **whitespace normalization**: trailing
//! whitespace is trimmed, runs of 2+ blank lines collapse to a single blank
//! line, and the document ends with exactly one newline. Everything else is
//! still emitted verbatim — paragraph structure, intra-line spacing, and
//! protected regions (`\verb`, verbatim bodies, comments) are preserved.
//!
//! The mechanism flows entirely through the Wadler [`Ir`]: each maximal run of
//! `WHITESPACE`/`NEWLINE` trivia is replaced by a single break primitive
//! ([`Ir::hard_line`] for one newline, [`Ir::empty_line`] for a blank line),
//! whose printer (`super::printer`) defers indentation and so drops trailing
//! whitespace for free.
//!
//! The lowering (`lower_node`) is the LaTeX-specific part that replaces ravel's
//! R `ir_expr_node` dispatch; the surrounding `format`/`format_with_style`
//! framework mirrors ravel's `src/formatter/core.rs`.

use std::iter::Peekable;

use crate::parser::parse;
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};

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
    // Normalize the document's trailing edge: drop any trailing blank lines and
    // per-line trailing whitespace at EOF, then guarantee exactly one final
    // newline. Empty output stays empty. Only ASCII whitespace/newlines are
    // trimmed, so trailing Unicode content (e.g. a non-breaking space) survives.
    let trimmed_len = formatted.trim_end_matches([' ', '\t', '\n', '\r']).len();
    formatted.truncate(trimmed_len);
    if !formatted.is_empty() {
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

/// Lower a CST node to IR. Child nodes recurse; non-trivia tokens (and the
/// protected `\verb`/verbatim/comment tokens) are emitted verbatim; maximal runs
/// of `WHITESPACE`/`NEWLINE` trivia are collapsed into a single break primitive
/// by [`classify_trivia`]. Comments deliberately *break* a trivia run (they are
/// content, never collapsed away), so the run on either side is classified
/// independently.
fn lower_node(node: &SyntaxNode) -> Ir {
    let mut out = Vec::new();
    let mut iter = node.children_with_tokens().peekable();
    while let Some(element) = iter.next() {
        match element {
            SyntaxElement::Node(child) => out.push(lower_node(&child)),
            SyntaxElement::Token(token) if is_collapsible_trivia(token.kind()) => {
                let (newlines, trailing_ws) = consume_trivia_run(&token, &mut iter);
                out.push(classify_trivia(newlines, trailing_ws));
            }
            SyntaxElement::Token(token) => out.push(Ir::verbatim(token.text())),
        }
    }
    Ir::concat(out)
}

/// Whitespace and newlines are the only trivia the formatter rewrites. Comments
/// are preserved verbatim and so are *not* collapsible.
fn is_collapsible_trivia(kind: SyntaxKind) -> bool {
    matches!(kind, SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE)
}

/// Consume the maximal run of collapsible trivia beginning at `first`, returning
/// the number of newlines it spans and the whitespace following the *last*
/// newline (the run's preserved leading indentation; whitespace before a newline
/// is trailing whitespace and is dropped). For a run with no newline the whole
/// run is whitespace and is returned as `trailing_ws`.
fn consume_trivia_run(
    first: &SyntaxToken,
    iter: &mut Peekable<impl Iterator<Item = SyntaxElement>>,
) -> (usize, String) {
    let mut newlines = 0;
    let mut trailing_ws = String::new();
    absorb(first, &mut newlines, &mut trailing_ws);
    loop {
        match iter.peek() {
            Some(SyntaxElement::Token(tok)) if is_collapsible_trivia(tok.kind()) => {}
            _ => break,
        }
        let token = match iter.next() {
            Some(SyntaxElement::Token(tok)) => tok,
            _ => unreachable!("peeked a collapsible trivia token"),
        };
        absorb(&token, &mut newlines, &mut trailing_ws);
    }
    (newlines, trailing_ws)
}

fn absorb(tok: &SyntaxToken, newlines: &mut usize, trailing_ws: &mut String) {
    if tok.kind() == SyntaxKind::NEWLINE {
        *newlines += 1;
        trailing_ws.clear();
    } else {
        trailing_ws.push_str(tok.text());
    }
}

/// Map a trivia run to a single IR primitive: no newline → the inline whitespace
/// kept verbatim; one newline → a [`Ir::hard_line`]; two or more → a single
/// [`Ir::empty_line`] (one blank line). Preserved indentation, if any, follows
/// the break verbatim.
fn classify_trivia(newlines: usize, trailing_ws: String) -> Ir {
    match newlines {
        0 => Ir::verbatim(trailing_ws),
        1 => break_with_indent(Ir::hard_line(), trailing_ws),
        _ => break_with_indent(Ir::empty_line(), trailing_ws),
    }
}

fn break_with_indent(brk: Ir, trailing_ws: String) -> Ir {
    if trailing_ws.is_empty() {
        brk
    } else {
        Ir::concat([brk, Ir::verbatim(trailing_ws)])
    }
}
