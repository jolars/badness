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
//!   existing line breaks are respected.
//! - **Prose-argument reflow** (under [`WrapMode::Reflow`]): an argument the
//!   signature DB marks `prose` (a `\footnote`/`\caption` body, a sectioning
//!   title) is reflowed to the line width like a paragraph — joined when it fits,
//!   wrapped when it does not (see [`lower_command`] / [`lower_prose_group`]).
//!   Non-prose groups (`\newcommand` body, `\label`) are left as authored.
//! - **Math** (`$…$`, `\(…\)`, `$$…$$`, `\[…\]`): the structured `MATH` body is
//!   formatted by [`lower_math`] — internal whitespace runs collapse to a single
//!   space, runs at the delimiters are trimmed, `^`/`_` scripts are kept tight,
//!   and redundant braces around a single-token script argument are stripped
//!   where the following token would not glue onto it. A comment inside math
//!   forces a line break. Commands keep their authored form (their arguments may
//!   be text).
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
//! The lowering (`lower_node`) is the LaTeX-specific part that replaces arity's
//! R `ir_expr_node` dispatch; the surrounding `format`/`format_with_style`
//! framework mirrors arity's `src/formatter/core.rs`.

use std::iter::Peekable;

use crate::ast::{command_name, environment_name};
use crate::parser::parse;
use crate::semantic::{ArgKind, ArgSpec, Signatures, scan_definitions};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};

use super::context::FormatContext;
use super::ir::Ir;
use super::printer::Printer;
use super::style::{FormatStyle, WrapMode};

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
/// `syntax()` (arity uses `diagnostics` + `cst`).
pub fn format_with_style(input: &str, style: FormatStyle) -> Result<String, FormatError> {
    let parsed = parse(input);
    if !parsed.errors.is_empty() {
        return Err(FormatError::ParseErrors {
            count: parsed.errors.len(),
        });
    }

    format_node(&parsed.syntax(), style)
}

/// Format an already-parsed CST `root` under `style`. This is the
/// reparse-free entry: the language server hands it the salsa-cached tree
/// (`db.parsed_tree`) instead of re-running the parser. The caller owns the
/// `ParseErrors` guard — this entry assumes the parse was clean and only
/// enforces the `ERROR`-token invariant ([`validate_supported_tokens`]).
/// [`format_with_style`] is the parse-then-format convenience wrapper.
pub fn format_node(root: &SyntaxNode, style: FormatStyle) -> Result<String, FormatError> {
    validate_supported_tokens(root)?;

    let ctx = FormatContext::new(style);
    let mut formatted = format_root(root, ctx);
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
    // Scan the document's own `\newcommand`/`\newenvironment`/xparse definitions
    // once, so the lowering resolves a locally-defined environment's arity (not
    // just the built-in DB's). Held by value for the whole lowering.
    let user = scan_definitions(root);
    let cx = LowerCtx {
        wrap: ctx.style().wrap,
        signatures: Signatures::new(&user),
    };
    let ir = lower_node(root, cx);
    Printer::new(ctx.style()).print(&ir)
}

/// The state threaded through every lowering call: the active [`WrapMode`] plus the
/// per-document [`Signatures`] overlay (scanned definitions over the built-in DB)
/// that [`lower_begin`] consults for environment arity. `Copy`, so it passes by
/// value like the bare `wrap` mode it replaced.
#[derive(Clone, Copy)]
struct LowerCtx<'a> {
    wrap: WrapMode,
    signatures: Signatures<'a>,
}

/// Lower a CST node to IR. Most nodes lower generically (see
/// [`lower_element_stream`]); an [`SyntaxKind::ENVIRONMENT`] is special-cased to
/// indent its body (see [`lower_environment`]), and under [`WrapMode::Reflow`] a
/// [`SyntaxKind::PARAGRAPH`] is wrapped to the line width (see
/// [`lower_paragraph_reflow`]). The [`LowerCtx`] (wrap mode + signature overlay) is
/// threaded through so it reaches every nested paragraph (including environment and
/// group bodies).
fn lower_node(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    match node.kind() {
        SyntaxKind::PARAGRAPH if cx.wrap == WrapMode::Reflow => {
            return lower_paragraph_reflow(node, cx);
        }
        SyntaxKind::ENVIRONMENT if !has_verbatim_body(node) && is_alignment_env(node, cx) => {
            return lower_aligned_environment(node, cx);
        }
        SyntaxKind::ENVIRONMENT
            if cx.wrap == WrapMode::Reflow && !has_verbatim_body(node) && is_list_env(node, cx) =>
        {
            return lower_list_environment(node, cx);
        }
        SyntaxKind::ENVIRONMENT if !has_verbatim_body(node) => {
            return lower_environment(node, cx);
        }
        SyntaxKind::COMMAND if cx.wrap == WrapMode::Reflow && command_has_prose_arg(node, cx) => {
            return lower_command(node, cx);
        }
        SyntaxKind::INLINE_MATH => {
            return lower_math(node, cx);
        }
        SyntaxKind::DISPLAY_MATH => {
            return lower_display_math(node, cx);
        }
        SyntaxKind::MATH => {
            return lower_math_body(node, cx);
        }
        SyntaxKind::GROUP if spans_multiple_lines(node) => {
            return lower_bracketed(node, SyntaxKind::L_BRACE, SyntaxKind::R_BRACE, cx);
        }
        SyntaxKind::OPTIONAL if spans_multiple_lines(node) => {
            return lower_bracketed(node, SyntaxKind::L_BRACKET, SyntaxKind::R_BRACKET, cx);
        }
        _ => {}
    }
    Ir::concat(lower_element_stream(node.children_with_tokens(), cx))
}

/// Lower a [`SyntaxKind::PARAGRAPH`] under [`WrapMode::Reflow`]: greedily wrap its
/// prose to the line width. Maximal runs of *adjacent* non-whitespace elements
/// glue into one unbreakable *atom* (so `Hello,` and `\emph{x}` never split);
/// inter-word whitespace — or a lone newline, since a paragraph holds no blank
/// lines — is a break opportunity. The run lowers to an [`Ir::fill`], which the
/// printer wraps word-by-word.
///
/// Three things end a line rather than flow into the fill: an explicit `\\` line
/// break (a [`SyntaxKind::LINE_BREAK`] node — the parser groups `\\` with its
/// `*` / `[len]` so the whole unit stays on one line), a `%` comment (which must
/// terminate its line), and a nested *block* (an environment or multi-line group
/// whose IR carries a forced break). Each emits the run-so-far as a fill, then
/// the line breaks; a fresh run continues after. The paragraph's lines are joined
/// by [`Ir::hard_line`].
fn lower_paragraph_reflow(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    reflow_elements(node.children_with_tokens(), cx)
}

/// Greedily reflow a stream of inline elements to the line width, the shared core
/// of paragraph reflow ([`lower_paragraph_reflow`]) and prose-argument reflow
/// ([`lower_prose_group`]). Maximal runs of *adjacent* non-whitespace elements glue
/// into one unbreakable *atom* (so `Hello,` and `\emph{x}` never split); inter-word
/// whitespace or a lone newline is a break opportunity. A run of atoms lowers to an
/// [`Ir::fill`], which the printer wraps word-by-word.
///
/// Three things end a fill line rather than flow into it: an explicit `\\` line
/// break (a [`SyntaxKind::LINE_BREAK`] node), a `%` comment (which must terminate
/// its line), and a nested *block* (an environment or multi-line group whose IR
/// carries a forced break). Each commits the run-so-far as a fill, then a fresh run
/// continues after, the lines joined by [`Ir::hard_line`].
///
/// A lone newline is normally a break opportunity the fill rejoins, *except* when a
/// physical line is made up solely of command(s) (a `\usepackage{…}` line, a
/// `\section{…}` header — see [`line_is_command_only`]): the break on either side of
/// such a line is preserved, keeping it on its own line. Prose lines around it still
/// reflow.
///
/// Unlike a `PARAGRAPH` (which holds no blank lines by construction), an argument
/// *group* body may contain blank-line paragraph breaks; a blank-line trivia run
/// ends the current line and separates the next with an [`Ir::empty_line`].
fn reflow_elements(elements: impl Iterator<Item = SyntaxElement>, cx: LowerCtx<'_>) -> Ir {
    // Collected up front so the single-newline arm can look ahead at the next
    // physical line ([`line_is_command_only`]). Inline prose commands (`\footnote`,
    // `\emph`, …) are flattened into the stream so their bodies reflow as running
    // text rather than block-breaking their braces (see [`flatten_inline_prose`]).
    let elements: Vec<SyntaxElement> = flatten_inline_prose(elements.collect(), cx);

    // Glued pieces of the atom in progress.
    let mut atom: Vec<Ir> = Vec::new();
    // Atoms of the current fill run (the current logical line).
    let mut run: Vec<Ir> = Vec::new();
    // Completed lines (fills and blocks), interleaved with `seps` at the end.
    let mut lines: Vec<Ir> = Vec::new();
    // The separator *preceding* each committed line (`seps[0]` is unused). A blank
    // line in the source promotes the next separator to an [`Ir::empty_line`].
    let mut seps: Vec<Ir> = Vec::new();
    // The separator to record before the next committed line. Default: one break.
    let mut pending_sep: Ir = Ir::hard_line();
    // Whether the current *physical* source line so far consists solely of
    // command(s) (and inline whitespace). Such a line is kept on its own line
    // rather than reflowed into its neighbours (see the single-newline arm). Both
    // reset at every physical-line boundary.
    let mut line_all_commands = true;
    let mut line_has_content = false;

    /// Commit the atom in progress (if any) as one atom of the current run.
    fn flush_atom(atom: &mut Vec<Ir>, run: &mut Vec<Ir>) {
        if !atom.is_empty() {
            run.push(Ir::concat(atom.drain(..)));
        }
    }
    /// Commit `content` as the next logical line, recording the separator before
    /// it and resetting `pending_sep` to a single break.
    fn push_segment(content: Ir, lines: &mut Vec<Ir>, seps: &mut Vec<Ir>, pending_sep: &mut Ir) {
        seps.push(std::mem::replace(pending_sep, Ir::hard_line()));
        lines.push(content);
    }
    /// End the current logical line: flush the atom and, when non-empty, commit the
    /// run as a fill segment.
    fn end_line(
        atom: &mut Vec<Ir>,
        run: &mut Vec<Ir>,
        lines: &mut Vec<Ir>,
        seps: &mut Vec<Ir>,
        pending_sep: &mut Ir,
    ) {
        flush_atom(atom, run);
        if !run.is_empty() {
            push_segment(Ir::fill(run.drain(..)), lines, seps, pending_sep);
        }
    }

    let mut idx = 0;
    while idx < elements.len() {
        match &elements[idx] {
            // Whitespace / newline run: a physical-line and atom boundary.
            SyntaxElement::Token(token) if is_collapsible_trivia(token.kind()) => {
                let newlines = consume_trivia_run_slice(&elements, &mut idx);
                if newlines >= 2 {
                    // A blank line ends the line and promotes the next separator.
                    end_line(&mut atom, &mut run, &mut lines, &mut seps, &mut pending_sep);
                    pending_sep = Ir::empty_line();
                    line_all_commands = true;
                    line_has_content = false;
                } else if newlines == 1 {
                    // A single source newline. Normally just an atom boundary the
                    // fill rejoins, but a line that is *only* command(s) — on either
                    // side of the break — is kept on its own line: end the line so
                    // the break survives instead of collapsing to a fill space.
                    let prev_is_command = line_has_content && line_all_commands;
                    let next_is_command = line_is_command_only(&elements, idx);
                    if prev_is_command || next_is_command {
                        end_line(&mut atom, &mut run, &mut lines, &mut seps, &mut pending_sep);
                    } else {
                        flush_atom(&mut atom, &mut run);
                    }
                    line_all_commands = true;
                    line_has_content = false;
                } else {
                    // Pure inline whitespace: an atom boundary within the line.
                    flush_atom(&mut atom, &mut run);
                }
                continue;
            }
            // A comment trailing content rides the end of that line, then forces a
            // break. But a comment that *begins* its own physical line stays on its
            // own line: end the current line first so the preceding prose run commits
            // separately, instead of reflowing the bare `%` up into that run.
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::COMMENT => {
                if !line_has_content {
                    end_line(&mut atom, &mut run, &mut lines, &mut seps, &mut pending_sep);
                }
                atom.push(Ir::verbatim(token.text()));
                end_line(&mut atom, &mut run, &mut lines, &mut seps, &mut pending_sep);
                line_all_commands = true;
                line_has_content = false;
            }
            // A `\`-at-end-of-line control symbol (`\` + newline) carries its own
            // newline but nothing after it — kept verbatim for losslessness, it
            // ends the line: emit the part before the break as a flat atom and let
            // the line break supply the newline, so the result reparses to the same
            // token (idempotent) instead of leaving an unbreakable multi-line atom
            // inside the fill. Restricted to control symbols: a multi-line `VERB`
            // token (a brace-verbatim argument spanning lines) has real content
            // after its newline and must be emitted whole by the arm below.
            SyntaxElement::Token(token)
                if token.kind() == SyntaxKind::CONTROL_SYMBOL && token.text().contains('\n') =>
            {
                let before = token.text().split_once('\n').map(|(b, _)| b).unwrap_or("");
                if !before.is_empty() {
                    atom.push(Ir::verbatim(before));
                }
                end_line(&mut atom, &mut run, &mut lines, &mut seps, &mut pending_sep);
                line_all_commands = true;
                line_has_content = false;
            }
            // Any other token (WORD, `~`, `&`, `#`, `^`, `_`, brackets, `\verb`,
            // a bare control symbol) glues onto the current atom — prose content,
            // so this physical line is no longer command-only.
            SyntaxElement::Token(token) => {
                atom.push(Ir::verbatim(token.text()));
                line_has_content = true;
                line_all_commands = false;
            }
            // An explicit `\\` line break (with its `*` / `[len]`, grouped by the
            // parser into one node) rides the end of the current line, then breaks.
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::LINE_BREAK => {
                atom.push(lower_node(child, cx));
                end_line(&mut atom, &mut run, &mut lines, &mut seps, &mut pending_sep);
                line_all_commands = true;
                line_has_content = false;
            }
            SyntaxElement::Node(child) => {
                let ir = lower_node(child, cx);
                if ir.contains_forced_break() {
                    // A block amid prose: end the current line, then place the
                    // block on its own line(s); a fresh run continues after.
                    end_line(&mut atom, &mut run, &mut lines, &mut seps, &mut pending_sep);
                    push_segment(ir, &mut lines, &mut seps, &mut pending_sep);
                    line_all_commands = true;
                    line_has_content = false;
                } else {
                    // A `COMMAND` keeps the line command-only; any other inline node
                    // (math, an inline group) is content that disqualifies it.
                    atom.push(ir);
                    line_has_content = true;
                    line_all_commands &= child.kind() == SyntaxKind::COMMAND;
                }
            }
        }
        idx += 1;
    }
    end_line(&mut atom, &mut run, &mut lines, &mut seps, &mut pending_sep);

    // Interleave the recorded separators between committed lines.
    let mut result: Vec<Ir> = Vec::with_capacity(lines.len().saturating_mul(2));
    for (i, line) in lines.into_iter().enumerate() {
        if i > 0 {
            result.push(seps[i].clone());
        }
        result.push(line);
    }
    Ir::concat(result)
}

/// Lower a stream of elements: child nodes recurse, non-trivia tokens (and the
/// protected `\verb`/verbatim/comment tokens) are emitted verbatim, and maximal
/// runs of `WHITESPACE`/`NEWLINE` trivia are collapsed into a single break
/// primitive by [`classify_trivia`]. Comments deliberately *break* a trivia run
/// (they are content, never collapsed away), so the run on either side is
/// classified independently.
fn lower_element_stream(
    elements: impl Iterator<Item = SyntaxElement>,
    cx: LowerCtx<'_>,
) -> Vec<Ir> {
    let mut out = Vec::new();
    let mut iter = elements.peekable();
    while let Some(element) = iter.next() {
        match element {
            SyntaxElement::Node(child) => out.push(lower_node(&child, cx)),
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
/// re-indentation idempotent. A blank line the author placed against `\begin`/
/// `\end` is preserved as a single blank line (the leading/trailing `hard_line`
/// becomes an [`Ir::empty_line`]); the empty-body case keeps a single break.
///
/// Verbatim-like environments never reach here (their opaque `VERBATIM_BODY`
/// token would be corrupted by reflow); [`lower_node`] routes them to the
/// generic path, which emits the body verbatim.
fn lower_environment(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    let mut begin = Ir::Nil;
    let mut end = Ir::Nil;
    let mut body_elements: Vec<SyntaxElement> = Vec::new();
    for element in node.children_with_tokens() {
        match &element {
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::BEGIN => {
                begin = lower_begin(child, cx);
            }
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::END => {
                end = lower_node(child, cx);
            }
            _ => body_elements.push(element),
        }
    }

    // A `%` that trails the `\begin{…}` header on the same source line belongs to
    // that line (the space-suppression idiom), not the indented body. Lift it onto
    // the `\begin` line and drop it from the body so it is not emitted twice. A
    // comment the author placed on its own line is left in the body untouched.
    let (begin, body) = match leading_inline_comment(&body_elements) {
        Some(comment) => {
            let begin = Ir::concat([begin, Ir::verbatim(comment.text())]);
            (
                begin,
                lower_body_dropping_leading_comment(body_elements, cx),
            )
        }
        None => (
            begin,
            Ir::concat(lower_element_stream(body_elements.into_iter(), cx)),
        ),
    };
    // Trim the body's own edge breaks (the indenter re-supplies them), but if the
    // author left a blank line touching `\begin`/`\end`, preserve it as a single
    // blank line — LaTeX blank lines are deliberate visual spacing, so we keep one
    // rather than collapse to zero (interior runs already collapse to one).
    let (lead_blank, body) = peel_leading_break(body);
    let (trail_blank, body) = peel_trailing_break(body);
    let lead = if lead_blank {
        Ir::empty_line()
    } else {
        Ir::hard_line()
    };
    let trail = if trail_blank {
        Ir::empty_line()
    } else {
        Ir::hard_line()
    };

    if matches!(body, Ir::Nil) {
        // Empty body: keep `\begin` and `\end` on their own lines (no edge blank).
        Ir::concat([begin, Ir::hard_line(), end])
    } else if environment_no_indent(node, cx) {
        // `document` and friends: lay the body on its own lines, but flush against
        // the surrounding indentation rather than nesting it.
        Ir::concat([begin, lead, body, trail, end])
    } else {
        Ir::concat([begin, Ir::indent(Ir::concat([lead, body])), trail, end])
    }
}

/// The `%` comment that trails the `\begin{…}` header on the *same* source line —
/// only inline whitespace, never a newline, separates the header from it. Such a
/// comment is the space-suppression idiom and belongs on the header line; a
/// comment the author placed on its own line (a newline intervenes) returns
/// `None` and stays in the body. Scans the body in source order, descending into
/// the first node (the body's leading paragraph holds the comment as its first
/// token): inline whitespace is skipped, a comment matches, and anything else —
/// a newline or real content — ends the scan.
fn leading_inline_comment(body_elements: &[SyntaxElement]) -> Option<SyntaxToken> {
    for element in body_elements {
        match element {
            SyntaxElement::Token(token) => match token.kind() {
                SyntaxKind::WHITESPACE => continue,
                SyntaxKind::COMMENT => return Some(token.clone()),
                _ => return None,
            },
            SyntaxElement::Node(node) => {
                for token in node
                    .descendants_with_tokens()
                    .filter_map(|e| e.into_token())
                {
                    match token.kind() {
                        SyntaxKind::WHITESPACE => continue,
                        SyntaxKind::COMMENT => return Some(token),
                        _ => return None,
                    }
                }
            }
        }
    }
    None
}

/// Lower an environment body whose leading inline comment has already been lifted
/// onto the `\begin` header by [`lower_environment`]. The comment is dropped from
/// the body to avoid emitting it twice: a bare comment token is skipped outright,
/// and the leading paragraph is re-lowered with its leading whitespace-and-comment
/// run stripped (see [`lower_node_dropping_leading_comment`]). Everything after
/// the comment lowers through the normal stream path.
fn lower_body_dropping_leading_comment(body_elements: Vec<SyntaxElement>, cx: LowerCtx<'_>) -> Ir {
    let mut out: Vec<Ir> = Vec::new();
    let mut iter = body_elements.into_iter();
    for element in iter.by_ref() {
        match element {
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::WHITESPACE => continue,
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::COMMENT => break,
            SyntaxElement::Node(node) => {
                out.push(lower_node_dropping_leading_comment(&node, cx));
                break;
            }
            // Unreachable given `leading_inline_comment` matched, but stay lossless.
            SyntaxElement::Token(token) => {
                out.push(Ir::verbatim(token.text()));
                break;
            }
        }
    }
    out.extend(lower_element_stream(iter, cx));
    Ir::concat(out)
}

/// Re-lower `node` with its leading whitespace-and-comment run dropped, using the
/// same dispatch [`lower_node`] would (reflow for a `PARAGRAPH` under
/// [`WrapMode::Reflow`], the generic stream otherwise). Used by
/// [`lower_body_dropping_leading_comment`] to strip a comment lifted onto the
/// `\begin` header.
fn lower_node_dropping_leading_comment(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    let mut children: Vec<SyntaxElement> = node.children_with_tokens().collect();
    let mut i = 0;
    while matches!(
        children.get(i).and_then(|c| c.as_token()).map(|t| t.kind()),
        Some(SyntaxKind::WHITESPACE)
    ) {
        i += 1;
    }
    if matches!(
        children.get(i).and_then(|c| c.as_token()).map(|t| t.kind()),
        Some(SyntaxKind::COMMENT)
    ) {
        children.drain(..=i);
    }
    if node.kind() == SyntaxKind::PARAGRAPH && cx.wrap == WrapMode::Reflow {
        reflow_elements(children.into_iter(), cx)
    } else {
        Ir::concat(lower_element_stream(children.into_iter(), cx))
    }
}

/// Whether the environment's body should be left at the surrounding indentation
/// level rather than nested one step in (the `noIndent` signature flag — see
/// [`crate::semantic::signature::EnvironmentSig::no_indent`]). The canonical case
/// is `document`, whose body conventionally sits flush against the margin.
fn environment_no_indent(node: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    node.children()
        .find(|child| child.kind() == SyntaxKind::BEGIN)
        .and_then(|begin| environment_name(&begin))
        .and_then(|name| cx.signatures.environment(&name))
        .is_some_and(|sig| sig.no_indent)
}

/// Lower a `\begin{name}` node, keeping the environment's *declared* argument
/// groups on the `\begin` header line instead of letting a source line break push
/// them onto their own (indented) line. For example `\begin{tabular}\n{cc}` renders
/// as a single `\begin{tabular}{cc}` header.
///
/// The arity comes from the [`Signatures`] overlay (`cx.signatures`): a document's
/// own `\newenvironment{thm}[1]…` is honored just like a built-in `tabular`, with
/// the scanned definition shadowing a built-in of the same name. The first `arity`
/// argument groups are glued to `\begin{name}` (intervening breaks and inline
/// whitespace dropped), and anything past the declared arity — which the greedy
/// parser may have over-attached — lowers generically, preserving today's behavior.
/// Environments neither the document nor the DB knows, or that take no arguments,
/// also take the generic path, so nothing regresses. A `\begin` header carrying a
/// comment is left to the generic path too: gluing across a `%` comment would let
/// it swallow the next line.
fn lower_begin(begin: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    let arity = environment_name(begin)
        .and_then(|name| cx.signatures.environment(&name))
        .map(|sig| sig.args.len())
        .unwrap_or(0);
    let has_comment = begin
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .any(|token| token.kind() == SyntaxKind::COMMENT);
    if arity == 0 || has_comment {
        return lower_node(begin, cx);
    }

    let mut head: Vec<Ir> = Vec::new();
    let mut tail: Vec<SyntaxElement> = Vec::new();
    let mut args_seen = 0;
    let mut in_tail = false;
    for element in begin.children_with_tokens() {
        if in_tail {
            tail.push(element);
            continue;
        }
        match &element {
            SyntaxElement::Node(child)
                if matches!(child.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL) =>
            {
                head.push(lower_node(child, cx));
                args_seen += 1;
                if args_seen == arity {
                    in_tail = true;
                }
            }
            // The `\begin` control word and the `{name}` group stay on the line.
            SyntaxElement::Node(child) => head.push(lower_node(child, cx)),
            // Drop header breaks/whitespace: the arguments glue to `\begin{name}`.
            SyntaxElement::Token(token) if is_collapsible_trivia(token.kind()) => {}
            SyntaxElement::Token(token) => head.push(Ir::verbatim(token.text())),
        }
    }
    if !tail.is_empty() {
        head.extend(lower_element_stream(tail.into_iter(), cx));
    }
    Ir::concat(head)
}

/// True if `node` (an `ENVIRONMENT`) names a list environment the signature DB
/// marks `list` — `itemize`/`enumerate`/`description`, whose `\item`s the
/// formatter lays out one per line with a hanging indent (see
/// [`lower_list_environment`]).
fn is_list_env(node: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    node.children()
        .find(|child| child.kind() == SyntaxKind::BEGIN)
        .and_then(|begin| environment_name(&begin))
        .and_then(|name| cx.signatures.environment(&name))
        .is_some_and(|sig| sig.list)
}

/// One `\item` of a list environment: the rendered marker (`\item`, or
/// `\item[label]`), the width to hang continuation lines at (the rendered width of
/// the control word plus a space — `\item `, *not* the label, so a wide
/// `description` label does not push the body's left edge around), and the item's
/// body split into paragraph *chunks* (a blank line in the source starts a new
/// chunk). `blank_before` records whether a blank line separated this item from the
/// previous one, so it is reproduced.
struct ListItem {
    marker: String,
    hang: usize,
    chunks: Vec<Vec<SyntaxElement>>,
    blank_before: bool,
}

/// A flattened list-body element: either a real CST element or an explicit
/// paragraph boundary (a blank line), which [`flatten_list_body`] reifies because
/// item collection spans paragraph breaks but the trivia carrying them lives
/// *between* the body's `PARAGRAPH` nodes.
enum FlatItem {
    El(SyntaxElement),
    Blank,
}

/// Lower a list environment (`itemize`/`enumerate`/`description`): each `\item`
/// starts its own line at the body indent and its body is reflowed with the
/// continuation lines hanging-indented at the control word's width (`\item `), so a
/// `description` item's wide `[label]` trails on the first line but does not deepen
/// the body indent. The framing (`\begin`/`\end`, the indented body with
/// leading/trailing `hard_line`) matches [`lower_environment`].
///
/// Only reached under [`WrapMode::Reflow`] (see [`lower_node`]). Falls back to the
/// plain [`lower_environment`] when the body has no `\item` to anchor on, so an
/// unusual shape degrades to today's indented body rather than misformatting.
fn lower_list_environment(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    let mut begin = Ir::Nil;
    let mut end = Ir::Nil;
    let mut body_elements: Vec<SyntaxElement> = Vec::new();
    for element in node.children_with_tokens() {
        match &element {
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::BEGIN => {
                begin = lower_begin(child, cx);
            }
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::END => {
                end = lower_node(child, cx);
            }
            _ => body_elements.push(element),
        }
    }

    let Some(body) = lower_list_body(&body_elements, cx) else {
        return lower_environment(node, cx);
    };
    Ir::concat([
        begin,
        Ir::indent(Ir::concat([Ir::hard_line(), body])),
        Ir::hard_line(),
        end,
    ])
}

/// Build the body IR of a list environment: split into items at each top-level
/// `\item` and render each as `\item` + a hanging-indented reflow of its content.
/// Returns `None` (caller falls back) when the body carries no `\item`.
fn lower_list_body(body_elements: &[SyntaxElement], cx: LowerCtx<'_>) -> Option<Ir> {
    let flat = flatten_list_body(body_elements);

    // Content before the first `\item` (usually just trivia); kept as its own
    // leading segment so nothing is dropped.
    let mut preamble: Vec<Vec<SyntaxElement>> = vec![Vec::new()];
    let mut items: Vec<ListItem> = Vec::new();
    let mut blank_pending = false;
    for fi in flat {
        match fi {
            FlatItem::Blank => {
                // A paragraph boundary: it separates items (recorded on the next
                // item) and, within an item, starts a fresh content chunk.
                blank_pending = true;
                match items.last_mut() {
                    Some(item) => item.chunks.push(Vec::new()),
                    None => preamble.push(Vec::new()),
                }
            }
            FlatItem::El(el) if is_item_command(&el) => {
                let (marker, hang, leading) = split_item_marker(&el, cx);
                items.push(ListItem {
                    marker,
                    hang,
                    chunks: vec![leading],
                    blank_before: blank_pending,
                });
                blank_pending = false;
            }
            FlatItem::El(el) => {
                match items.last_mut() {
                    Some(item) => item.chunks.last_mut().unwrap().push(el),
                    None => preamble.last_mut().unwrap().push(el),
                }
                blank_pending = false;
            }
        }
    }

    if items.is_empty() {
        return None;
    }

    let mut segments: Vec<Ir> = Vec::new();
    let mut seps: Vec<Ir> = Vec::new();
    let preamble_ir = reflow_chunks(&preamble, cx);
    if !matches!(preamble_ir, Ir::Nil) {
        seps.push(Ir::hard_line()); // unused (segment 0 has no preceding separator)
        segments.push(preamble_ir);
    }
    for item in &items {
        seps.push(if item.blank_before {
            Ir::empty_line()
        } else {
            Ir::hard_line()
        });
        segments.push(render_list_item(item, cx));
    }

    let mut result: Vec<Ir> = Vec::with_capacity(segments.len().saturating_mul(2));
    for (i, segment) in segments.into_iter().enumerate() {
        if i > 0 {
            result.push(seps[i].clone());
        }
        result.push(segment);
    }
    Some(Ir::concat(result))
}

/// Render one [`ListItem`]: the marker, then a space and the item's body reflowed
/// inside an [`Ir::align`] whose width is the item's `hang` (the control word plus
/// the separating space — `\item `), so wrapped lines hang under where the body
/// would start after a bare `\item`, regardless of how wide the `[label]` is. An
/// empty item (marker with no body) renders as the bare marker.
fn render_list_item(item: &ListItem, cx: LowerCtx<'_>) -> Ir {
    let content = reflow_chunks(&item.chunks, cx);
    let marker = Ir::verbatim(item.marker.clone());
    if matches!(content, Ir::Nil) {
        return marker;
    }
    Ir::concat([marker, Ir::verbatim(" "), Ir::align(item.hang, content)])
}

/// Reflow each paragraph chunk of an item body and join the (non-empty) results
/// with an [`Ir::empty_line`], so a blank line inside an item becomes a blank line
/// between its paragraphs (still under the hanging indent).
fn reflow_chunks(chunks: &[Vec<SyntaxElement>], cx: LowerCtx<'_>) -> Ir {
    let parts = chunks
        .iter()
        .map(|chunk| reflow_elements(chunk.iter().cloned(), cx))
        .filter(|ir| !matches!(ir, Ir::Nil));
    Ir::join(Ir::empty_line(), parts)
}

/// Flatten a list-environment body into a stream of inline elements, reifying each
/// paragraph boundary (a blank line) as a [`FlatItem::Blank`]. Body-level trivia
/// between paragraphs is dropped — the boundary it represents is already carried
/// by the `Blank` inserted before each non-first paragraph.
fn flatten_list_body(body_elements: &[SyntaxElement]) -> Vec<FlatItem> {
    let mut out: Vec<FlatItem> = Vec::new();
    let mut started = false;
    for element in body_elements {
        match element {
            SyntaxElement::Node(p) if p.kind() == SyntaxKind::PARAGRAPH => {
                if started {
                    out.push(FlatItem::Blank);
                }
                out.extend(p.children_with_tokens().map(FlatItem::El));
                started = true;
            }
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {}
            other => {
                out.push(FlatItem::El(other.clone()));
                started = true;
            }
        }
    }
    out
}

/// Whether `el` is a `\item` command node — the marker that starts a new list
/// item.
fn is_item_command(el: &SyntaxElement) -> bool {
    el.as_node().is_some_and(|node| {
        node.kind() == SyntaxKind::COMMAND && command_name(node).as_deref() == Some("item")
    })
}

/// Split a `\item` command node into its rendered marker string (the control word
/// plus any leading optional `[label]`, the only argument an item marker takes),
/// the *hang* width for continuation lines (the control word's rendered width plus
/// one for the separating space — deliberately excluding the `[label]` so a wide
/// `description` label does not deepen the body indent), and the trailing elements
/// that are really body content — a `{…}` group the greedy parser over-attached,
/// which belongs to the item body, not the marker.
fn split_item_marker(el: &SyntaxElement, cx: LowerCtx<'_>) -> (String, usize, Vec<SyntaxElement>) {
    let node = el.as_node().expect("item command is a node");
    let mut marker_parts: Vec<Ir> = Vec::new();
    let mut content: Vec<SyntaxElement> = Vec::new();
    let mut hang = 1; // the space separating the marker from the body
    let mut in_content = false;
    for child in node.children_with_tokens() {
        if in_content {
            content.push(child);
            continue;
        }
        match &child {
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::CONTROL_WORD => {
                hang += t.text().chars().count();
                marker_parts.push(Ir::verbatim(t.text()));
            }
            // Trivia between the control word and an optional label is not part of
            // the marker.
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {}
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::OPTIONAL => {
                marker_parts.push(lower_node(n, cx));
            }
            // A brace group (or anything else) is body content, not the marker.
            other => {
                in_content = true;
                content.push(other.clone());
            }
        }
    }
    let marker = Printer::new(FormatStyle::default()).print_flat(&Ir::concat(marker_parts));
    (marker, hang, content)
}

/// True if `node` (an `ENVIRONMENT`) names an environment the signature DB marks
/// `align` — an `align`/matrix-family environment whose `&` columns the formatter
/// lays out into a grid (see [`lower_aligned_environment`]).
fn is_alignment_env(node: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    node.children()
        .find(|child| child.kind() == SyntaxKind::BEGIN)
        .and_then(|begin| environment_name(&begin))
        .and_then(|name| cx.signatures.environment(&name))
        .is_some_and(|sig| sig.align)
}

/// One row of an alignment grid: its rendered, trimmed cell strings, the flat text
/// of the `\\` that terminated the row (`None` for a final row written without a
/// trailing line break), and an optional end-of-line comment that trails the row
/// (rendered *after* the `\\`, so the break is never commented out).
struct AlignRow {
    cells: Vec<String>,
    line_break: Option<String>,
    trailing_comment: Option<String>,
}

/// One item in an alignment grid: either a [`AlignRow`] or a *passthrough* line —
/// a physical line that is not a grid row (a comment-only line, or a line made up
/// solely of horizontal-rule commands like `\hline`/`\midrule`). A passthrough is
/// kept verbatim between rows and never counted toward column widths.
enum GridItem {
    Row(AlignRow),
    Passthrough(String),
}

/// Lower an `align`/matrix-family environment, laying out its `&` columns into a
/// grid so the ampersands line up. The framing (`\begin`/`\end`, the indented
/// body with leading/trailing `hard_line`) is identical to [`lower_environment`];
/// only the body differs — it is the rendered grid rather than a generic element
/// stream.
///
/// Falls back to [`lower_environment`] whenever the body is not a clean
/// single-paragraph grid (see [`build_alignment_grid`]): a blank-line break, or a
/// cell that cannot collapse to one aligned line (a mid-row comment or a nested
/// block). Comment-only and rule-only lines (`\hline`, `\midrule`, …) are *not* a
/// reason to fall back — they are kept as passthrough lines between rows. The
/// fallback is always available, so an unhandled shape degrades to today's plain
/// indented body, never a panic or corruption.
fn lower_aligned_environment(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    let mut begin = Ir::Nil;
    let mut end = Ir::Nil;
    let mut body_elements: Vec<SyntaxElement> = Vec::new();
    for element in node.children_with_tokens() {
        match &element {
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::BEGIN => {
                begin = lower_begin(child, cx);
            }
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::END => {
                end = lower_node(child, cx);
            }
            _ => body_elements.push(element),
        }
    }

    let Some(items) = build_alignment_grid(&body_elements, cx) else {
        return lower_environment(node, cx);
    };
    if !items.iter().any(|item| matches!(item, GridItem::Row(_))) {
        // A body with no actual rows (empty, `\\`-only, or comment-only) has no
        // grid; let the generic path render it.
        return lower_environment(node, cx);
    }

    let body = render_alignment_rows(&items);
    Ir::concat([
        begin,
        Ir::indent(Ir::concat([Ir::hard_line(), body])),
        Ir::hard_line(),
        end,
    ])
}

/// Split an alignment environment body into a sequence of grid items (rows and
/// passthrough lines), or `None` to signal the caller should fall back to the
/// generic environment lowering.
///
/// Rows are delimited by *top-level* `\\` ([`SyntaxKind::LINE_BREAK`]) nodes and
/// cells by top-level `&` ([`SyntaxKind::AMPERSAND`]) tokens; a `&` nested inside a
/// group or sub-environment lives in a child node, never a direct body child, so
/// it is correctly invisible here. Each cell's elements lower through the generic
/// [`lower_element_stream`] and render *flat* (so inline math/groups normalize as
/// they do elsewhere), trimmed of surrounding space.
///
/// **Comments and rule lines.** A physical line between rows that is made up solely
/// of comments and/or horizontal-rule commands (`\hline`, `\midrule`, …) is kept as
/// a [`GridItem::Passthrough`] line, not a cell. A comment at the end of a row's
/// last physical line — directly after the row's `\\`, or trailing the final row —
/// is attached as the row's `trailing_comment`. A comment in the *middle* of a row
/// (with more cells after it) cannot sit on an aligned line — its text runs to end
/// of line, commenting out the rest — so it returns `None` and falls back.
///
/// Returns `None` when [`flatten_alignment_body`] rejects the body (a blank-line
/// break), when a cell carries a forced break (a nested block or a continuation
/// line), or on a mid-row comment.
fn build_alignment_grid(
    body_elements: &[SyntaxElement],
    cx: LowerCtx<'_>,
) -> Option<Vec<GridItem>> {
    let inline = flatten_alignment_body(body_elements)?;
    let printer = Printer::new(FormatStyle::default());

    /// Render the accumulated cell elements flat and trimmed, pushing the result
    /// onto `cells`. Returns `None` on a cell that cannot collapse to one line.
    ///
    /// Collapsible trivia at the cell's edges is dropped first: the structural
    /// newline after `\begin`/each `\\` and the indentation before the next cell
    /// are *boundary* whitespace, not cell content; left in, the leading newline
    /// would lower to a forced break (an [`Ir::hard_line`]) and spuriously trip the
    /// fallback. A newline *inside* a cell still lowers to a forced break and so
    /// (correctly) falls back — a continuation line cannot sit on one aligned row.
    fn finish_cell(
        cell: &mut Vec<SyntaxElement>,
        cells: &mut Vec<String>,
        printer: &Printer,
        cx: LowerCtx<'_>,
    ) -> Option<()> {
        let is_edge_trivia = |e: &SyntaxElement| {
            e.as_token()
                .is_some_and(|t| is_collapsible_trivia(t.kind()))
        };
        while cell.first().is_some_and(&is_edge_trivia) {
            cell.remove(0);
        }
        while cell.last().is_some_and(&is_edge_trivia) {
            cell.pop();
        }
        // A comment in a cell is handled by the caller (passthrough / trailing /
        // fallback) and never reaches here in a handled case; this guard keeps the
        // fallback safe if one ever slips through an unmodeled path.
        if cell.iter().any(|e| {
            e.as_token()
                .is_some_and(|t| t.kind() == SyntaxKind::COMMENT)
        }) {
            return None;
        }
        let ir = Ir::concat(lower_element_stream(cell.drain(..), cx));
        if ir.contains_forced_break() {
            return None;
        }
        cells.push(printer.print_flat(&ir).trim().to_string());
        Some(())
    }

    let mut items: Vec<GridItem> = Vec::new();
    let mut cells: Vec<String> = Vec::new();
    let mut cell: Vec<SyntaxElement> = Vec::new();
    let mut final_pushed = false;

    let mut idx = 0;
    while idx < inline.len() {
        // A row boundary: no committed cells and the current cell holds only
        // boundary trivia. Only here can a non-row (passthrough / trailing-comment)
        // line begin.
        let at_boundary = cells.is_empty() && cell_is_blank(&cell);
        if at_boundary
            && is_comment_or_rule_start(&inline[idx], cx)
            && let Some(line) = non_row_line(&inline, idx, &printer, cx)
        {
            // A comment on its own line (a newline separates it from the previous
            // grid token), or any non-row line with no row yet before it, is a
            // passthrough between rows.
            let own_line = cell_has_newline(&cell);
            let prev_is_row = matches!(items.last(), Some(GridItem::Row(_)));
            if own_line || !prev_is_row {
                items.push(GridItem::Passthrough(line.text));
                cell.clear();
                idx = line.next;
                continue;
            }
            // Not on its own line: it directly follows the previous row's `\\`. A
            // pure comment there trails that row; a rule there (the `\\ \hline`
            // form) is not modeled — fall through so the cell path falls back.
            if !line.has_rule {
                if let Some(GridItem::Row(row)) = items.last_mut() {
                    row.trailing_comment = Some(line.text);
                }
                cell.clear();
                idx = line.next;
                continue;
            }
        }

        match &inline[idx] {
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::AMPERSAND => {
                finish_cell(&mut cell, &mut cells, &printer, cx)?;
            }
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::LINE_BREAK => {
                finish_cell(&mut cell, &mut cells, &printer, cx)?;
                let line_break = printer
                    .print_flat(&lower_node(child, cx))
                    .trim()
                    .to_string();
                items.push(GridItem::Row(AlignRow {
                    cells: std::mem::take(&mut cells),
                    line_break: Some(line_break),
                    trailing_comment: None,
                }));
            }
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::COMMENT => {
                // A comment that is *not* at a boundary trails cell content. It is
                // clean only when nothing more in the body belongs to the row;
                // otherwise it would comment out later cells — fall back.
                if !rest_is_only_trivia(&inline, idx + 1) {
                    return None;
                }
                let text = token.text().trim_end().to_string();
                finish_cell(&mut cell, &mut cells, &printer, cx)?;
                items.push(GridItem::Row(AlignRow {
                    cells: std::mem::take(&mut cells),
                    line_break: None,
                    trailing_comment: Some(text),
                }));
                final_pushed = true;
                break;
            }
            _ => cell.push(inline[idx].clone()),
        }
        idx += 1;
    }

    // The final segment (content after the last `\\`). Drop it when it is a single
    // empty cell — the "body ended in `\\`" case — so the trailing break stays on
    // the prior row without adding a blank line; otherwise it is a real last row.
    if !final_pushed {
        finish_cell(&mut cell, &mut cells, &printer, cx)?;
        let final_is_empty = cells.len() == 1 && cells[0].is_empty();
        if !final_is_empty {
            items.push(GridItem::Row(AlignRow {
                cells,
                line_break: None,
                trailing_comment: None,
            }));
        }
    }

    Some(items)
}

/// A non-row line recognized at a grid boundary: its rendered text and the index
/// at which the body resumes (past the line's terminating newline).
struct NonRowLine {
    text: String,
    next: usize,
    has_rule: bool,
}

/// Try to read a *non-row* line — one made up solely of comments, horizontal-rule
/// commands (`\hline`, `\midrule`, …), and inline whitespace — starting at `start`
/// (which the caller guarantees is a comment or rule command). Returns `None` when
/// the line contains anything else (a cell, a `&`, a `\\`), so the caller treats it
/// as ordinary cell content. The rendered text is the line flattened and trimmed
/// (comments verbatim), exactly as cells and `\\` are rendered.
fn non_row_line(
    inline: &[SyntaxElement],
    start: usize,
    printer: &Printer,
    cx: LowerCtx<'_>,
) -> Option<NonRowLine> {
    let mut i = start;
    let mut content_end = start;
    let mut has_rule = false;
    let mut has_comment = false;
    while i < inline.len() {
        match &inline[i] {
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::NEWLINE => break,
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::WHITESPACE => {}
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::COMMENT => {
                // A comment runs to end of line, so it is the line's last content.
                has_comment = true;
                i += 1;
                content_end = i;
                break;
            }
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::COMMAND && is_rule_command(n, cx) => {
                has_rule = true;
                i += 1;
                content_end = i;
                continue;
            }
            _ => return None,
        }
        i += 1;
    }
    if !(has_rule || has_comment) {
        return None;
    }
    // Resume past the line's terminating newline (and any trailing whitespace).
    let mut next = content_end;
    while next < inline.len() {
        match &inline[next] {
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::WHITESPACE => next += 1,
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::NEWLINE => {
                next += 1;
                break;
            }
            _ => break,
        }
    }
    let ir = Ir::concat(lower_element_stream(
        inline[start..content_end].iter().cloned(),
        cx,
    ));
    let text = printer.print_flat(&ir).trim().to_string();
    Some(NonRowLine {
        text,
        next,
        has_rule,
    })
}

/// Whether `element` begins a candidate non-row line — a comment, or a command the
/// signature DB flags as a horizontal rule (`\hline`, `\midrule`, …).
fn is_comment_or_rule_start(element: &SyntaxElement, cx: LowerCtx<'_>) -> bool {
    match element {
        SyntaxElement::Token(t) => t.kind() == SyntaxKind::COMMENT,
        SyntaxElement::Node(n) => n.kind() == SyntaxKind::COMMAND && is_rule_command(n, cx),
    }
}

/// Whether `node` (a `COMMAND`) is a horizontal-rule command per the signature DB.
fn is_rule_command(node: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    command_name(node)
        .and_then(|name| cx.signatures.command(&name))
        .is_some_and(|sig| sig.rule)
}

/// Whether the accumulated cell holds only collapsible trivia (no real content) —
/// i.e. the parser is at a grid boundary.
fn cell_is_blank(cell: &[SyntaxElement]) -> bool {
    cell.iter().all(|e| {
        e.as_token()
            .is_some_and(|t| is_collapsible_trivia(t.kind()))
    })
}

/// Whether the boundary trivia accumulated since the last grid token includes a
/// newline — i.e. a following comment sits on its *own* physical line rather than
/// trailing the previous row's `\\`.
fn cell_has_newline(cell: &[SyntaxElement]) -> bool {
    cell.iter().any(|e| {
        e.as_token()
            .is_some_and(|t| t.kind() == SyntaxKind::NEWLINE)
    })
}

/// Whether everything from `from` onward is collapsible trivia — nothing of the
/// row remains, so a comment at the current position is a clean trailing comment.
fn rest_is_only_trivia(inline: &[SyntaxElement], from: usize) -> bool {
    inline[from..].iter().all(|e| {
        e.as_token()
            .is_some_and(|t| is_collapsible_trivia(t.kind()))
    })
}

/// Flatten an alignment environment's body into a single stream of inline
/// elements, descending one level into the lone body `PARAGRAPH` (where the `&`
/// and `\\` separators live). Inter-paragraph trivia is dropped (it is just the
/// body's own leading/trailing break, which the indenter re-supplies).
///
/// Returns `None` when the body holds more than one paragraph — a blank-line
/// break, which the single grid does not model — so the caller falls back.
fn flatten_alignment_body(body_elements: &[SyntaxElement]) -> Option<Vec<SyntaxElement>> {
    let mut inline: Vec<SyntaxElement> = Vec::new();
    let mut paragraphs = 0;
    for element in body_elements {
        match element {
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::PARAGRAPH => {
                paragraphs += 1;
                if paragraphs > 1 {
                    return None;
                }
                inline.extend(child.children_with_tokens());
            }
            SyntaxElement::Token(token) if is_collapsible_trivia(token.kind()) => {}
            other => inline.push(other.clone()),
        }
    }
    Some(inline)
}

/// Render the grid to IR: pad every non-last cell in a row to its column width
/// (left-align), join cells with `" & "`, append the row's `\\` and any trailing
/// comment, and join all items with [`Ir::hard_line`]. A row is one [`Ir::text`]
/// (no newline; cells are flat), which the caller indents one step. The row's
/// *last* cell is never padded, so no line carries trailing whitespace.
/// [`GridItem::Passthrough`] lines (comments, `\hline`/`\midrule`, …) are emitted
/// verbatim between rows and never counted toward column widths.
fn render_alignment_rows(items: &[GridItem]) -> Ir {
    // Column width = the max char-count over every cell in that column (including
    // last cells, so a long final cell still widens the column above it). Char
    // count matches the printer's own column metric. Passthrough lines do not
    // participate.
    let mut col_widths: Vec<usize> = Vec::new();
    for item in items {
        let GridItem::Row(row) = item else { continue };
        for (c, cell) in row.cells.iter().enumerate() {
            let width = cell.chars().count();
            if c == col_widths.len() {
                col_widths.push(width);
            } else if width > col_widths[c] {
                col_widths[c] = width;
            }
        }
    }

    let lines = items.iter().map(|item| {
        let row = match item {
            GridItem::Passthrough(text) => return Ir::text(text.clone()),
            GridItem::Row(row) => row,
        };
        let mut line = String::new();
        let last = row.cells.len().saturating_sub(1);
        for (c, cell) in row.cells.iter().enumerate() {
            if c > 0 {
                line.push_str(" & ");
            }
            line.push_str(cell);
            if c < last {
                let pad = col_widths[c].saturating_sub(cell.chars().count());
                line.push_str(&" ".repeat(pad));
            }
        }
        if let Some(line_break) = &row.line_break {
            line.push(' ');
            line.push_str(line_break);
        }
        // The trailing comment always follows the `\\` so the break is never
        // commented out.
        if let Some(comment) = &row.trailing_comment {
            line.push(' ');
            line.push_str(comment);
        }
        Ir::text(line)
    });
    Ir::join(Ir::hard_line(), lines)
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
fn lower_bracketed(node: &SyntaxNode, open: SyntaxKind, close: SyntaxKind, cx: LowerCtx<'_>) -> Ir {
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

    // A comment glued to the open delimiter (`{%`, with no newline between them)
    // must ride on the open-delimiter line. Pushing it to its own indented line
    // would turn the newline the formatter inserts after `{` into real whitespace
    // inside the group, changing `\cmd{%\n}` (an empty group — the `%` eats the
    // source newline) into `\cmd{ }` (a group holding a space). The parser emits
    // leading whitespace/newlines as their own trivia tokens, so the first body
    // element is the comment iff it was glued to the opener.
    let has_leading_comment = body_elements
        .first()
        .and_then(SyntaxElement::as_token)
        .is_some_and(|t| t.kind() == SyntaxKind::COMMENT);
    let open_ir = if has_leading_comment {
        let comment = body_elements.remove(0);
        Ir::concat([open_ir, Ir::verbatim(comment.as_token().unwrap().text())])
    } else {
        open_ir
    };

    let body = Ir::concat(lower_element_stream(body_elements.into_iter(), cx));
    let body = trim_trailing_break(trim_leading_break(body));

    if matches!(body, Ir::Nil) {
        if has_leading_comment {
            // `{%\n}`: the comment already rode the open delimiter, so the close
            // must still drop to its own line — collapsing to `{%}` would comment
            // out the closing brace.
            Ir::concat([open_ir, Ir::hard_line(), close_ir])
        } else {
            // Empty multi-line body collapses to the bare delimiters, e.g. `{\n}` → `{}`.
            Ir::concat([open_ir, close_ir])
        }
    } else {
        Ir::concat([
            open_ir,
            Ir::indent(Ir::concat([Ir::hard_line(), body])),
            Ir::hard_line(),
            close_ir,
        ])
    }
}

/// Whether `command`'s signature marks any argument as reflowable prose. The
/// cheap guard that gates the [`lower_command`] path in [`lower_node`]: a command
/// with no prose argument (the overwhelming common case) lowers generically, so
/// nothing regresses.
fn command_has_prose_arg(command: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    command_name(command)
        .and_then(|name| cx.signatures.command(&name))
        .is_some_and(|sig| sig.args.iter().any(|spec| spec.prose))
}

/// Whether `command` is an *inline* prose command — one whose prose argument sits
/// in running text (`\footnote`, `\emph`, `\textbf`, …) rather than heading its own
/// line. Such a command is flattened into the surrounding reflow stream (see
/// [`flatten_inline_prose`]) so its body wraps as part of the paragraph and its
/// `{`/`}` glue to the adjacent words, instead of block-breaking the braces onto
/// their own lines ([`lower_prose_group`]).
///
/// Driven by the signature DB's explicit [`CommandSig::inline`] flag, not derived:
/// block-level prose commands that head their own line (`\section`, `\caption`)
/// leave it unset and keep the block treatment.
fn command_is_inline_prose(command: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    command_name(command)
        .and_then(|name| cx.signatures.command(&name))
        .is_some_and(|sig| sig.inline && sig.args.iter().any(|spec| spec.prose))
}

/// Pre-pass over a reflow element stream: replace each *inline* prose command
/// ([`command_is_inline_prose`]) with its surface tokens, splicing its prose
/// argument's body directly into the stream. The body's inter-word whitespace then
/// becomes break opportunities in the surrounding paragraph fill, and the prose
/// `{`/`}` glue onto the adjacent words — so an inline footnote wraps as running
/// text instead of exploding into a block. Non-prose arguments and the control
/// word are kept verbatim; nested inline prose commands are expanded recursively.
fn flatten_inline_prose(elements: Vec<SyntaxElement>, cx: LowerCtx<'_>) -> Vec<SyntaxElement> {
    let mut out = Vec::new();
    for element in elements {
        match &element {
            SyntaxElement::Node(node)
                if node.kind() == SyntaxKind::COMMAND && command_is_inline_prose(node, cx) =>
            {
                expand_inline_prose(node, cx, &mut out);
            }
            _ => out.push(element),
        }
    }
    out
}

/// Expand one inline prose command into `out` (see [`flatten_inline_prose`]): the
/// control word and any non-prose argument are emitted verbatim, while each prose
/// argument is spliced delimiter-and-body via [`splice_prose_group`]. Slot matching
/// mirrors [`lower_command`] so an omitted optional does not misalign positions.
fn expand_inline_prose(node: &SyntaxNode, cx: LowerCtx<'_>, out: &mut Vec<SyntaxElement>) {
    let Some(sig) = command_name(node).and_then(|name| cx.signatures.command(&name)) else {
        out.push(SyntaxElement::Node(node.clone()));
        return;
    };
    let mut slot = 0usize;
    for child in node.children_with_tokens() {
        match child {
            SyntaxElement::Node(group)
                if matches!(group.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL) =>
            {
                let is_bracket = group.kind() == SyntaxKind::OPTIONAL;
                let prose =
                    match_arg_slot(&sig.args, &mut slot, is_bracket).is_some_and(|spec| spec.prose);
                if prose {
                    splice_prose_group(&group, cx, out);
                } else {
                    out.push(SyntaxElement::Node(group));
                }
            }
            other => out.push(other),
        }
    }
}

/// Splice a prose group's delimiters and body into `out` (see
/// [`flatten_inline_prose`]). The group's own `{`/`[` and `}`/`]` tokens are
/// emitted around the body; the body's leading and trailing whitespace is dropped
/// so the delimiters glue tight to the first and last words, and nested inline
/// prose commands inside the body are expanded recursively.
fn splice_prose_group(group: &SyntaxNode, cx: LowerCtx<'_>, out: &mut Vec<SyntaxElement>) {
    let mut open: Option<SyntaxElement> = None;
    let mut close: Option<SyntaxElement> = None;
    let mut body: Vec<SyntaxElement> = Vec::new();
    for element in group.children_with_tokens() {
        match &element {
            SyntaxElement::Token(t)
                if matches!(t.kind(), SyntaxKind::L_BRACE | SyntaxKind::L_BRACKET)
                    && open.is_none() =>
            {
                open = Some(element);
            }
            SyntaxElement::Token(t)
                if matches!(t.kind(), SyntaxKind::R_BRACE | SyntaxKind::R_BRACKET) =>
            {
                close = Some(element);
            }
            _ => body.push(element),
        }
    }
    while body.first().is_some_and(is_collapsible_trivia_element) {
        body.remove(0);
    }
    while body.last().is_some_and(is_collapsible_trivia_element) {
        body.pop();
    }
    if let Some(open) = open {
        out.push(open);
    }
    out.extend(flatten_inline_prose(body, cx));
    if let Some(close) = close {
        out.push(close);
    }
}

/// True when `element` is a collapsible-trivia token (whitespace/newline), the
/// boundary whitespace [`splice_prose_group`] trims so a prose delimiter glues to
/// its body.
fn is_collapsible_trivia_element(element: &SyntaxElement) -> bool {
    matches!(element, SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()))
}

/// Lower a `COMMAND` whose signature marks an argument as prose (see
/// [`command_has_prose_arg`], which gates this path). Each attached `{…}`/`[…]`
/// group is matched to its signature slot — kind-aware, so an omitted optional does
/// not misalign positions (`\section{Title}` binds the `{title}` slot, not a
/// leading `[short]`) — and a group filling a prose slot is reflowed via
/// [`lower_prose_group`]. Everything else (non-prose slots, groups past the declared
/// arity that the greedy parser over-attached, trivia) lowers exactly as the generic
/// path would.
fn lower_command(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    let Some(sig) = command_name(node).and_then(|name| cx.signatures.command(&name)) else {
        // Defensive: the guard already proved a prose signature exists.
        return Ir::concat(lower_element_stream(node.children_with_tokens(), cx));
    };

    let mut out: Vec<Ir> = Vec::new();
    let mut slot = 0usize;
    let mut iter = node.children_with_tokens().peekable();
    while let Some(element) = iter.next() {
        match element {
            SyntaxElement::Node(child)
                if matches!(child.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL) =>
            {
                let is_bracket = child.kind() == SyntaxKind::OPTIONAL;
                let prose =
                    match_arg_slot(&sig.args, &mut slot, is_bracket).is_some_and(|spec| spec.prose);
                if prose {
                    let (open, close) = if is_bracket {
                        (SyntaxKind::L_BRACKET, SyntaxKind::R_BRACKET)
                    } else {
                        (SyntaxKind::L_BRACE, SyntaxKind::R_BRACE)
                    };
                    out.push(lower_prose_group(&child, open, close, cx));
                } else {
                    out.push(lower_node(&child, cx));
                }
            }
            SyntaxElement::Node(child) => out.push(lower_node(&child, cx)),
            SyntaxElement::Token(token) if is_collapsible_trivia(token.kind()) => {
                let (newlines, trailing_ws) = consume_trivia_run(&token, &mut iter);
                out.push(classify_trivia(newlines, trailing_ws));
            }
            SyntaxElement::Token(token) => out.push(Ir::verbatim(token.text())),
        }
    }
    Ir::concat(out)
}

/// Match the next attached argument group (a brace group, or a bracket group when
/// `is_bracket`) to a signature slot, advancing `slot` past it. Skips leading
/// optional (`[…]`) slots the document omitted, so a mandatory prose slot still
/// binds when an optional before it is absent. Returns the matched [`ArgSpec`], or
/// `None` when the group has no matching slot (e.g. an unexpected `[…]` the greedy
/// parser over-attached, or a group past the declared arity), in which case `slot`
/// is left untouched so later groups still match.
fn match_arg_slot(args: &[ArgSpec], slot: &mut usize, is_bracket: bool) -> Option<ArgSpec> {
    while *slot < args.len() {
        let spec = args[*slot];
        let spec_bracket = matches!(spec.kind, ArgKind::Bracket);
        if spec_bracket == is_bracket {
            *slot += 1;
            return Some(spec);
        }
        if spec_bracket {
            // A declared optional the document omitted: skip it and keep matching.
            *slot += 1;
            continue;
        }
        // A required `{…}` slot but the group is a `[…]`: not this slot. Leave the
        // slot intact for a later brace group and treat this group as non-prose.
        return None;
    }
    None
}

/// Lower a prose argument group: like [`lower_bracketed`], but the body is reflowed
/// to the line width ([`reflow_elements`]) and the whole thing is wrapped in a soft
/// [`Ir::group`] so it stays on one line when it fits (`\footnote{short}`) and
/// breaks the delimiters onto their own lines, indenting and word-wrapping the body,
/// when it does not. Empty bodies collapse to the bare delimiters.
fn lower_prose_group(
    node: &SyntaxNode,
    open: SyntaxKind,
    close: SyntaxKind,
    cx: LowerCtx<'_>,
) -> Ir {
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

    let body = reflow_elements(body_elements.into_iter(), cx);
    if matches!(body, Ir::Nil) {
        Ir::concat([open_ir, close_ir])
    } else {
        Ir::group(Ir::concat([
            open_ir,
            Ir::indent(Ir::concat([Ir::soft_line(), body])),
            Ir::soft_line(),
            close_ir,
        ]))
    }
}

/// True if `node` directly contains a `NEWLINE` token — i.e. the group itself
/// spans multiple physical lines. Newlines inside a *nested* group/environment
/// belong to that child node, not to `node`, so this attributes line-spanning to
/// the group that physically owns the break — which keeps re-indentation stable.
/// Lower inline `$…$`/`\(…\)` or display `$$…$$`/`\[…\]` math. The delimiter
/// tokens are direct children of the math node and are emitted verbatim; the
/// `MATH` child (the body) is formatted by [`lower_math_body`].
fn lower_math(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    Ir::concat(node.children_with_tokens().map(|el| match el {
        SyntaxElement::Node(n) if n.kind() == SyntaxKind::MATH => lower_math_body(&n, cx),
        SyntaxElement::Node(n) => lower_node(&n, cx),
        SyntaxElement::Token(t) => Ir::verbatim(t.text()),
    }))
}

/// Lower display math (`$$…$$` or `\[…\]`) as a block: the delimiters land on
/// their own lines with the body collapsed by [`lower_math_body`] and indented one
/// level, mirroring [`lower_bracketed`]'s shape. Display math is conceptually its
/// own vertical space, so unlike inline math (`\[ F \]` → `\[F\]`) it never
/// collapses onto a single line. An empty body degenerates to the bare adjacent
/// delimiters (`\[\]`, `$$$$`).
fn lower_display_math(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    // Delimiters are one token for `\[`/`\]` but two `DOLLAR` tokens for `$$`, so
    // accumulate every delimiter token on each side of the `MATH` body.
    let mut open = String::new();
    let mut close = String::new();
    let mut body = Ir::Nil;
    let mut body_empty = true;
    let mut seen_body = false;
    for element in node.children_with_tokens() {
        match element {
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::MATH => {
                body_empty = math_body_is_empty(&n);
                body = trim_trailing_break(lower_math_body(&n, cx));
                seen_body = true;
            }
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {}
            SyntaxElement::Token(t) if seen_body => close.push_str(t.text()),
            SyntaxElement::Token(t) => open.push_str(t.text()),
            // Unexpected non-MATH node child: defer to generic lowering.
            SyntaxElement::Node(n) => {
                body = lower_node(&n, cx);
                body_empty = false;
                seen_body = true;
            }
        }
    }

    if body_empty {
        Ir::concat([Ir::verbatim(open), Ir::verbatim(close)])
    } else {
        Ir::concat([
            Ir::verbatim(open),
            Ir::indent(Ir::concat([Ir::hard_line(), body])),
            Ir::hard_line(),
            Ir::verbatim(close),
        ])
    }
}

/// Format a math body (a `MATH` node, or a `{…}` group body in math): collapse
/// internal `WHITESPACE`/`NEWLINE` runs to a single space, drop the runs at the
/// edges (trimming just inside the delimiters), keep `^`/`_` scripts tight, and
/// let a `%` comment force a line break (so a trailing comment never swallows the
/// closing delimiter).
fn lower_math_body(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    lower_math_seq(node.children_with_tokens(), cx)
}

/// The separator owed before the next math atom.
#[derive(PartialEq)]
enum MathSep {
    /// Nothing yet (start of body) or just emitted an atom with no following gap.
    None,
    /// A collapsed whitespace/newline run: one space.
    Space,
    /// A comment forced a line break: a [`Ir::hard_line`].
    Break,
}

/// The shared math-atom sequencer (see [`lower_math_body`]). A trailing `Break`
/// (owed by a comment at the body's end) is emitted rather than trimmed, so the
/// caller's closing delimiter lands on its own line; a trailing `Space` is
/// dropped.
fn lower_math_seq(elements: impl Iterator<Item = SyntaxElement>, cx: LowerCtx<'_>) -> Ir {
    let mut out: Vec<Ir> = Vec::new();
    let mut sep = MathSep::None;
    let mut started = false;
    let mut iter = elements.peekable();
    while let Some(el) = iter.next() {
        match el {
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {
                consume_trivia_run(&t, &mut iter);
                if started && sep == MathSep::None {
                    sep = MathSep::Space;
                }
            }
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::COMMENT => {
                if sep == MathSep::Space {
                    out.push(Ir::verbatim(" "));
                }
                out.push(Ir::verbatim(t.text()));
                started = true;
                sep = MathSep::Break;
            }
            other => {
                match sep {
                    MathSep::Space => out.push(Ir::verbatim(" ")),
                    MathSep::Break => out.push(Ir::hard_line()),
                    MathSep::None => {}
                }
                out.push(lower_math_element(other, cx));
                started = true;
                sep = MathSep::None;
            }
        }
    }
    if sep == MathSep::Break {
        out.push(Ir::hard_line());
    }
    Ir::concat(out)
}

/// Lower one math atom (a non-trivia element of a math body).
fn lower_math_element(el: SyntaxElement, cx: LowerCtx<'_>) -> Ir {
    match el {
        SyntaxElement::Node(n) => match n.kind() {
            SyntaxKind::SCRIPTED => lower_scripted(&n, cx),
            SyntaxKind::SUBSCRIPT | SyntaxKind::SUPERSCRIPT => lower_script(&n, cx),
            SyntaxKind::GROUP => lower_math_group(&n, cx),
            SyntaxKind::LEFT_RIGHT => lower_left_right(&n, cx),
            // A command keeps its authored form: its arguments may be text
            // (`\text{…}`, `\operatorname{…}`), so Stage A does not reformat
            // inside commands. Verbatim is lossless on a clean parse.
            SyntaxKind::COMMAND => Ir::verbatim(n.text().to_string()),
            // Environments, or anything unexpected: defer to generic lowering.
            _ => lower_node(&n, cx),
        },
        SyntaxElement::Token(t) => Ir::verbatim(t.text()),
    }
}

/// Lower a `{…}` math group: keep the braces, format the body in math mode.
fn lower_math_group(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    let inner = node
        .children_with_tokens()
        .filter(|el| !matches!(el.kind(), SyntaxKind::L_BRACE | SyntaxKind::R_BRACE));
    Ir::concat([
        Ir::verbatim("{"),
        lower_math_seq(inner, cx),
        Ir::verbatim("}"),
    ])
}

/// Lower a `\left( … \right)` pair: the `\left`/`\right` control words and their
/// delimiter tokens are emitted verbatim, the inner `MATH` body is trimmed and
/// collapsed by [`lower_math_body`], and the trivia the parser kept between a
/// delimiter command and its delimiter (for losslessness) is dropped.
///
/// A non-empty body is set off by one space just inside each delimiter, so
/// `\left (  x + y  \right )` becomes `\left( x + y \right)`. That spacing is also
/// what keeps a control-word delimiter from gluing onto the body (`\left\langle x`
/// stays two tokens, never `\left\langlex`). An empty body stays tight
/// (`\left.\right.`).
fn lower_left_right(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    Ir::concat(node.children_with_tokens().filter_map(|el| match el {
        SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => None,
        SyntaxElement::Node(n) if n.kind() == SyntaxKind::MATH => {
            if math_body_is_empty(&n) {
                None
            } else {
                Some(Ir::concat([
                    Ir::verbatim(" "),
                    lower_math_body(&n, cx),
                    Ir::verbatim(" "),
                ]))
            }
        }
        SyntaxElement::Token(t) => Some(Ir::verbatim(t.text())),
        SyntaxElement::Node(n) => Some(lower_node(&n, cx)),
    }))
}

/// Whether a math body has no visible content (only whitespace/newlines), so a
/// `\left( … \right)` around it should not gain inner spaces.
fn math_body_is_empty(node: &SyntaxNode) -> bool {
    node.text().to_string().trim().is_empty()
}

/// Lower a `SCRIPTED` atom: the base then its `^`/`_` scripts, all tight (the
/// trivia the parser kept inside the node for losslessness is dropped here).
fn lower_scripted(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    Ir::concat(node.children_with_tokens().filter_map(|el| match el {
        SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => None,
        SyntaxElement::Node(n)
            if matches!(n.kind(), SyntaxKind::SUBSCRIPT | SyntaxKind::SUPERSCRIPT) =>
        {
            Some(lower_script(&n, cx))
        }
        other => Some(lower_math_element(other, cx)),
    }))
}

/// Lower a `SUBSCRIPT`/`SUPERSCRIPT`: the `_`/`^` glued tightly to its argument,
/// stripping redundant braces around a single-token argument where safe (see
/// [`strippable_script_arg`]).
fn lower_script(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    Ir::concat(node.children_with_tokens().filter_map(|el| match el {
        SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => None,
        SyntaxElement::Token(t)
            if matches!(t.kind(), SyntaxKind::CARET | SyntaxKind::UNDERSCORE) =>
        {
            Some(Ir::verbatim(t.text()))
        }
        SyntaxElement::Node(n) if n.kind() == SyntaxKind::GROUP && strippable_script_arg(&n) => {
            Some(lower_stripped_group(&n, cx))
        }
        other => Some(lower_math_element(other, cx)),
    }))
}

/// Whether a script-argument brace group `{X}` may safely drop its braces: `X`
/// must be a single TeX token (a one-character word or a lone control word) and
/// the token following the group must not glue onto `X` once the braces are gone
/// (so `x^{2}y` stays braced — `2y` would re-lex as one word, and `x^{\alpha}b`
/// stays braced — `\alphab` would be one control word).
fn strippable_script_arg(group: &SyntaxNode) -> bool {
    let mut inner = group.children_with_tokens().filter(|el| {
        !matches!(
            el.kind(),
            SyntaxKind::L_BRACE
                | SyntaxKind::R_BRACE
                | SyntaxKind::WHITESPACE
                | SyntaxKind::NEWLINE
        )
    });
    let Some(only) = inner.next() else {
        return false; // empty `{}` — never strip (`^` needs an argument)
    };
    if inner.next().is_some() {
        return false; // more than one token between the braces
    }
    match only {
        SyntaxElement::Token(t)
            if t.kind() == SyntaxKind::WORD && t.text().chars().count() == 1 =>
        {
            next_token_safe_after(group, false)
        }
        SyntaxElement::Node(n) if is_lone_control_word(&n) => next_token_safe_after(group, true),
        _ => false,
    }
}

/// A `COMMAND` node consisting solely of a control word with no attached
/// arguments (e.g. `\alpha`) — the form whose braces are droppable in script
/// position.
fn is_lone_control_word(node: &SyntaxNode) -> bool {
    if node.kind() != SyntaxKind::COMMAND {
        return false;
    }
    let mut children = node.children_with_tokens();
    let first_is_control_word = matches!(
        children.next(),
        Some(SyntaxElement::Token(t)) if t.kind() == SyntaxKind::CONTROL_WORD
    );
    first_is_control_word && children.next().is_none()
}

/// True if the token following `group` in the tree would not glue onto a stripped
/// single-token argument. `letter_only` (for a control-word argument) forbids only
/// a following ASCII letter; otherwise any word character forbids the strip.
fn next_token_safe_after(group: &SyntaxNode, letter_only: bool) -> bool {
    let next = group.last_token().and_then(|t| t.next_token());
    match next.as_ref().and_then(|t| t.text().chars().next()) {
        None => true,
        Some(c) if letter_only => !c.is_ascii_alphabetic(),
        Some(c) => !crate::parser::lexer::is_word_char(c),
    }
}

/// Lower the single inner token of a strippable script-argument group, without
/// its braces.
fn lower_stripped_group(group: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    match group.children_with_tokens().find(|el| {
        !matches!(
            el.kind(),
            SyntaxKind::L_BRACE
                | SyntaxKind::R_BRACE
                | SyntaxKind::WHITESPACE
                | SyntaxKind::NEWLINE
        )
    }) {
        Some(el) => lower_math_element(el, cx),
        None => Ir::nil(),
    }
}

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

/// Consume the maximal run of collapsible trivia in `elements` beginning at
/// `*i`, advancing `*i` past it and returning the number of newlines it spans.
/// The index-based analogue of [`consume_trivia_run`], used by
/// [`reflow_elements`], which needs to look ahead past the run (the peekable
/// iterator form cannot). The dropped indentation/`trailing_ws` is irrelevant to
/// reflow, which re-derives spacing from the fill.
fn consume_trivia_run_slice(elements: &[SyntaxElement], i: &mut usize) -> usize {
    let mut newlines = 0;
    while let Some(SyntaxElement::Token(tok)) = elements.get(*i) {
        if !is_collapsible_trivia(tok.kind()) {
            break;
        }
        if tok.kind() == SyntaxKind::NEWLINE {
            newlines += 1;
        }
        *i += 1;
    }
    newlines
}

/// Whether the physical source line beginning at `start` in `elements` consists
/// solely of command(s) and inline whitespace — the unit [`reflow_elements`]
/// keeps on its own line rather than reflowing into its neighbours. The line runs
/// until the next newline, comment, or end of the stream; any non-trivia element
/// that is not a `COMMAND` node (a word, a control symbol, a group, math, a `\\`,
/// a block) disqualifies it. A line with no command (e.g. an empty or
/// comment-only line) is not a command line.
fn line_is_command_only(elements: &[SyntaxElement], start: usize) -> bool {
    let mut saw_command = false;
    for element in &elements[start..] {
        match element {
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::NEWLINE => break,
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::COMMENT => break,
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::WHITESPACE => continue,
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::COMMAND => saw_command = true,
            _ => return false,
        }
    }
    saw_command
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

/// Drop leading break/indentation IR from `ir`, reporting whether the trimmed-away
/// break carried a blank line (an [`Ir::empty_line`]). Recurses into a leading
/// `Concat` (the body's first break is often buried inside the first paragraph).
/// [`lower_environment`] uses the blank flag to re-supply one blank line at the
/// body's leading edge; callers that only want the trim use [`trim_leading_break`].
fn peel_leading_break(ir: Ir) -> (bool, Ir) {
    if is_trimmable_break(&ir) {
        return (matches!(ir, Ir::EmptyLine), Ir::Nil);
    }
    match ir {
        Ir::Concat(items) => {
            let mut v: Vec<Ir> = items.iter().cloned().collect();
            let mut blank = false;
            while !v.is_empty() {
                let (b, head) = peel_leading_break(v.remove(0));
                blank |= b;
                if matches!(head, Ir::Nil) {
                    continue;
                }
                v.insert(0, head);
                break;
            }
            (blank, Ir::concat(v))
        }
        other => (false, other),
    }
}

/// Mirror of [`peel_leading_break`] for the trailing edge.
fn peel_trailing_break(ir: Ir) -> (bool, Ir) {
    if is_trimmable_break(&ir) {
        return (matches!(ir, Ir::EmptyLine), Ir::Nil);
    }
    match ir {
        Ir::Concat(items) => {
            let mut v: Vec<Ir> = items.iter().cloned().collect();
            let mut blank = false;
            while let Some(last) = v.pop() {
                let (b, tail) = peel_trailing_break(last);
                blank |= b;
                if matches!(tail, Ir::Nil) {
                    continue;
                }
                v.push(tail);
                break;
            }
            (blank, Ir::concat(v))
        }
        other => (false, other),
    }
}

/// Drop leading break/indentation IR from `ir`, discarding the blank flag (see
/// [`peel_leading_break`]).
fn trim_leading_break(ir: Ir) -> Ir {
    peel_leading_break(ir).1
}

/// Drop trailing break/indentation IR from `ir`, discarding the blank flag (see
/// [`peel_trailing_break`]).
fn trim_trailing_break(ir: Ir) -> Ir {
    peel_trailing_break(ir).1
}
