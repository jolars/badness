//! The formatter entry points and the CST → [`Ir`] lowering.
//!
//! Implemented rules:
//! - **Whitespace normalization**: trailing whitespace is trimmed, runs of 2+
//!   blank lines collapse to a single blank line, and the document ends with
//!   exactly one newline.
//! - **Environment indentation**: the body of `\begin{…} … \end{…}` is indented
//!   one step, nesting recursively, with `\begin`/`\end` flush. All indentation
//!   is computed by the printer, never preserved from input — so reformatting
//!   re-indents idempotently.
//! - **Group/argument indentation**: the body of a *multi-line* brace group
//!   `{…}` or optional-argument group `[…]` is indented one step, the same way
//!   (delimiters flush, body indented). Single-line groups are left inline;
//!   existing line breaks are respected (no reflow yet).
//!
//! Everything else is emitted verbatim: paragraph structure, intra-line spacing,
//! and protected regions (`\verb`, verbatim bodies, comments) are preserved.
//!
//! The mechanism flows entirely through the Wadler [`Ir`]: each maximal run of
//! `WHITESPACE`/`NEWLINE` trivia is replaced by a single break primitive
//! ([`Ir::hard_line`] for one newline, [`Ir::empty_line`] for a blank line),
//! whose printer (`super::printer`) defers indentation and so drops trailing
//! whitespace for free, and [`Ir::indent`] raises the indent inside environment
//! bodies.
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

/// Lower a CST node to IR. Most nodes lower generically (see
/// [`lower_element_stream`]); an [`SyntaxKind::ENVIRONMENT`] is special-cased to
/// indent its body (see [`lower_environment`]).
fn lower_node(node: &SyntaxNode) -> Ir {
    match node.kind() {
        SyntaxKind::ENVIRONMENT if !has_verbatim_body(node) => return lower_environment(node),
        SyntaxKind::GROUP if spans_multiple_lines(node) => {
            return lower_bracketed(node, SyntaxKind::L_BRACE, SyntaxKind::R_BRACE);
        }
        SyntaxKind::OPTIONAL if spans_multiple_lines(node) => {
            return lower_bracketed(node, SyntaxKind::L_BRACKET, SyntaxKind::R_BRACKET);
        }
        _ => {}
    }
    Ir::concat(lower_element_stream(node.children_with_tokens()))
}

/// Lower a stream of elements: child nodes recurse, non-trivia tokens (and the
/// protected `\verb`/verbatim/comment tokens) are emitted verbatim, and maximal
/// runs of `WHITESPACE`/`NEWLINE` trivia are collapsed into a single break
/// primitive by [`classify_trivia`]. Comments deliberately *break* a trivia run
/// (they are content, never collapsed away), so the run on either side is
/// classified independently.
fn lower_element_stream(elements: impl Iterator<Item = SyntaxElement>) -> Vec<Ir> {
    let mut out = Vec::new();
    let mut iter = elements.peekable();
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
    out
}

/// Lower an `\begin{…} … \end{…}` environment, indenting its body one step. A
/// clean-parse environment is `[BEGIN, body…, END]`: the framing nodes are
/// lowered directly, and the body between them is wrapped in [`Ir::indent`] with
/// a leading [`Ir::hard_line`] (so it starts on its own indented line) and a
/// trailing `hard_line` at the *outer* indent (so `\end` sits flush with
/// `\begin`). All indentation is owned by the printer, so the body's own leading
/// and trailing breaks are trimmed before wrapping — this is what makes
/// re-indentation idempotent.
///
/// Verbatim-like environments never reach here (their opaque `VERBATIM_BODY`
/// token would be corrupted by reflow); [`lower_node`] routes them to the
/// generic path, which emits the body verbatim.
fn lower_environment(node: &SyntaxNode) -> Ir {
    let mut begin = Ir::Nil;
    let mut end = Ir::Nil;
    let mut body_elements: Vec<SyntaxElement> = Vec::new();
    for element in node.children_with_tokens() {
        match &element {
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::BEGIN => {
                begin = lower_node(child);
            }
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::END => {
                end = lower_node(child);
            }
            _ => body_elements.push(element),
        }
    }

    let body = Ir::concat(lower_element_stream(body_elements.into_iter()));
    let body = trim_trailing_break(trim_leading_break(body));

    if matches!(body, Ir::Nil) {
        // Empty body: keep `\begin` and `\end` on their own lines.
        Ir::concat([begin, Ir::hard_line(), end])
    } else {
        Ir::concat([
            begin,
            Ir::indent(Ir::concat([Ir::hard_line(), body])),
            Ir::hard_line(),
            end,
        ])
    }
}

/// Lower a delimited group — a brace group `{…}` (`open`/`close` =
/// `L_BRACE`/`R_BRACE`) or an optional-argument group `[…]`
/// (`L_BRACKET`/`R_BRACKET`) — indenting its body one step, exactly like
/// [`lower_environment`] but with token delimiters instead of `BEGIN`/`END`
/// nodes. Only called for multi-line groups (see [`spans_multiple_lines`]);
/// single-line groups stay inline on the generic path.
///
/// Inside a group the parser emits body tokens directly (no `PARAGRAPH`
/// wrapping), so the only `open` token is the first child and the only `close`
/// token is the last — but an `OPTIONAL` body may contain a stray `[` (TeX does
/// not nest `[`), so the opener is captured only once (`open_ir` still `Nil`).
fn lower_bracketed(node: &SyntaxNode, open: SyntaxKind, close: SyntaxKind) -> Ir {
    let mut open_ir = Ir::Nil;
    let mut close_ir = Ir::Nil;
    let mut body_elements: Vec<SyntaxElement> = Vec::new();
    for element in node.children_with_tokens() {
        match &element {
            SyntaxElement::Token(t) if t.kind() == open && matches!(open_ir, Ir::Nil) => {
                open_ir = Ir::verbatim(t.text());
            }
            SyntaxElement::Token(t) if t.kind() == close => {
                close_ir = Ir::verbatim(t.text());
            }
            _ => body_elements.push(element),
        }
    }

    let body = Ir::concat(lower_element_stream(body_elements.into_iter()));
    let body = trim_trailing_break(trim_leading_break(body));

    if matches!(body, Ir::Nil) {
        // Empty multi-line body collapses to the bare delimiters, e.g. `{\n}` → `{}`.
        Ir::concat([open_ir, close_ir])
    } else {
        Ir::concat([
            open_ir,
            Ir::indent(Ir::concat([Ir::hard_line(), body])),
            Ir::hard_line(),
            close_ir,
        ])
    }
}

/// True if `node` directly contains a `NEWLINE` token — i.e. the group itself
/// spans multiple physical lines. Newlines inside a *nested* group/environment
/// belong to that child node, not to `node`, so this attributes line-spanning to
/// the group that physically owns the break — which keeps re-indentation stable.
fn spans_multiple_lines(node: &SyntaxNode) -> bool {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .any(|t| t.kind() == SyntaxKind::NEWLINE)
}

/// True if `node` directly contains a `VERBATIM_BODY` token — i.e. it is a
/// verbatim-like environment whose body must be emitted byte-for-byte.
fn has_verbatim_body(node: &SyntaxNode) -> bool {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .any(|t| t.kind() == SyntaxKind::VERBATIM_BODY)
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
/// (a genuine inter-word space) kept verbatim; one newline → a [`Ir::hard_line`];
/// two or more → a single [`Ir::empty_line`] (one blank line). Whitespace that
/// followed the last newline is *indentation*, which the printer owns and
/// recreates, so it is dropped here — keeping it would double-indent on reformat.
fn classify_trivia(newlines: usize, trailing_ws: String) -> Ir {
    match newlines {
        0 => Ir::verbatim(trailing_ws),
        1 => Ir::hard_line(),
        _ => Ir::empty_line(),
    }
}

/// A break the indenter supplies itself and so trims from a body edge: a forced
/// line break, an inline whitespace chunk (indentation), or [`Ir::Nil`]. A
/// `VERBATIM_BODY` (force-break verbatim, or non-blank text) is never trimmable,
/// so protected content survives.
fn is_trimmable_break(ir: &Ir) -> bool {
    match ir {
        Ir::HardLine | Ir::EmptyLine | Ir::Nil => true,
        Ir::Verbatim { text, force_break } => {
            !force_break && text.chars().all(|c| c == ' ' || c == '\t')
        }
        _ => false,
    }
}

/// Drop leading break/indentation IR from `ir`, recursing into a leading
/// `Concat` (the body's first break is often buried inside the first paragraph).
fn trim_leading_break(ir: Ir) -> Ir {
    if is_trimmable_break(&ir) {
        return Ir::Nil;
    }
    match ir {
        Ir::Concat(items) => {
            let mut v: Vec<Ir> = items.iter().cloned().collect();
            while !v.is_empty() {
                let head = trim_leading_break(v.remove(0));
                if matches!(head, Ir::Nil) {
                    continue;
                }
                v.insert(0, head);
                break;
            }
            Ir::concat(v)
        }
        other => other,
    }
}

/// Drop trailing break/indentation IR from `ir`, recursing into a trailing
/// `Concat` (mirror of [`trim_leading_break`]).
fn trim_trailing_break(ir: Ir) -> Ir {
    if is_trimmable_break(&ir) {
        return Ir::Nil;
    }
    match ir {
        Ir::Concat(items) => {
            let mut v: Vec<Ir> = items.iter().cloned().collect();
            while let Some(last) = v.pop() {
                let tail = trim_trailing_break(last);
                if matches!(tail, Ir::Nil) {
                    continue;
                }
                v.push(tail);
                break;
            }
            Ir::concat(v)
        }
        other => other,
    }
}
