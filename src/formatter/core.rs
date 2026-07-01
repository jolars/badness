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
//! - **expl3 code** (inside `\ExplSyntaxOn`…`\ExplSyntaxOff`, or `\ProvidesExpl*`
//!   to EOF): source spaces/tabs are catcode-9 (ignored) and `~` is catcode-10 (a
//!   literal space), so the formatter owns the layout *regardless of [`WrapMode`]*.
//!   In-region code lays out one statement per source line ([`lower_expl_code`]),
//!   brace groups holding code indent as blocks ([`lower_expl_group`]), inter-token
//!   whitespace collapses to a single space, and `~` is a breakable space. Region
//!   membership is recomputed read-only by [`expl3_regions`] (the CST is untouched).
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
//! The lowering (`lower_node`) is the LaTeX-specific part; the surrounding
//! `format`/`format_with_style` framework is generic.

use std::iter::Peekable;

use rowan::{TextRange, TextSize};

use crate::ast::{command_name, environment_name};
use crate::parser::lexer::{ExplToggle, expl_toggle};
use crate::parser::{LatexFlavor, parse_with_flavor};
use crate::semantic::{ArgKind, ArgSpec, ContentKind, SignatureDb, Signatures, scan_definitions};
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
/// `syntax()`. Uses the
/// [`Document`](LatexFlavor::Document) flavor; [`format_with_style_flavored`] is
/// the entry for `.sty`/`.cls`.
pub fn format_with_style(input: &str, style: FormatStyle) -> Result<String, FormatError> {
    format_with_style_flavored(input, style, LatexFlavor::Document)
}

/// Like [`format_with_style`] but parses `input` under an explicit
/// [`LexConfig`], so a [`Package`](LatexFlavor::Package) flavor (`.sty`/`.cls`)
/// lexes with `@` as a letter (the implicit `\makeatletter`) and a `.dtx` runs
/// the docstrip mode. A bare [`LatexFlavor`] coerces in. The wrap mode is a
/// `style` concern, decided by the caller (`.sty`/`.cls` default to
/// [`crate::formatter::WrapMode::Preserve`] via
/// [`crate::file_discovery::FileKind::default_wrap`]).
pub fn format_with_style_flavored(
    input: &str,
    style: FormatStyle,
    config: impl Into<crate::parser::LexConfig>,
) -> Result<String, FormatError> {
    format_with_style_flavored_with_signatures(input, style, config, &SignatureDb::default())
}

/// Like [`format_with_style_flavored`] but additionally folds an `external`
/// signature scope — the merged definitions of the document's loaded local
/// packages ([`crate::semantic::collect_package_signatures`] /
/// [`crate::incremental::scope_signatures`]) — into the lowering, so calls to
/// package-defined macros are shaped by their real arity/verbatim-ness. The
/// document's own definitions always win over `external`. The CLI uses this for a
/// real file path; passing an empty DB recovers [`format_with_style_flavored`].
pub fn format_with_style_flavored_with_signatures(
    input: &str,
    style: FormatStyle,
    config: impl Into<crate::parser::LexConfig>,
    external: &SignatureDb,
) -> Result<String, FormatError> {
    let parsed = parse_with_flavor(input, config);
    if !parsed.errors.is_empty() {
        return Err(FormatError::ParseErrors {
            count: parsed.errors.len(),
        });
    }

    format_node_with_signatures(&parsed.syntax(), style, external)
}

/// Format an on-disk file's `content` (located at `path`, parsed under `config`),
/// pulling the signatures of its local loaded packages in from disk so calls to
/// package-defined macros are shaped correctly. The shared CLI entry for both
/// `format` and `format --check` — using one entry keeps the two consistent, so a
/// formatted file checks clean. `path`'s directory anchors local `.sty`/`.cls`
/// resolution; stdin (no path) uses [`format_with_style_flavored`] instead.
pub fn format_file_with_packages(
    content: &str,
    path: &std::path::Path,
    style: FormatStyle,
    config: impl Into<crate::parser::LexConfig>,
) -> Result<String, FormatError> {
    let parsed = parse_with_flavor(content, config);
    if !parsed.errors.is_empty() {
        return Err(FormatError::ParseErrors {
            count: parsed.errors.len(),
        });
    }
    let root = parsed.syntax();
    let external = crate::semantic::disk_scope_signatures(&root, path);
    format_node_with_signatures(&root, style, &external)
}

/// Format an already-parsed CST `root` under `style`. This is the
/// reparse-free entry: the language server hands it the salsa-cached tree
/// (`db.parsed_tree`) instead of re-running the parser. The caller owns the
/// `ParseErrors` guard — this entry assumes the parse was clean and only
/// enforces the `ERROR`-token invariant ([`validate_supported_tokens`]).
/// [`format_with_style`] is the parse-then-format convenience wrapper.
pub fn format_node(root: &SyntaxNode, style: FormatStyle) -> Result<String, FormatError> {
    format_node_with_signatures(root, style, &SignatureDb::default())
}

/// Like [`format_node`] but folds an `external` signature scope (loaded local
/// packages' merged definitions) into the lowering. The language server passes the
/// salsa-cached [`crate::incremental::scope_signatures`] here; the document's own
/// definitions always win over `external`. An empty DB recovers [`format_node`].
pub fn format_node_with_signatures(
    root: &SyntaxNode,
    style: FormatStyle,
    external: &SignatureDb,
) -> Result<String, FormatError> {
    validate_supported_tokens(root)?;

    let ctx = FormatContext::new(style);
    let mut formatted = format_root(root, ctx, external, None);
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

/// Range formatting: lay out only the top-level blocks overlapping `range`,
/// returning the formatted text for the `[first block start, last block end]`
/// span. The caller ([`crate::lsp`]) expands the editor selection to whole
/// top-level-block boundaries before calling, so `range` is already block-aligned.
///
/// The whole document is still scanned for `\newcommand` signatures and expl3
/// regions ([`format_root`]), so a selected block depending on an earlier
/// definition or sitting inside an ancestor `\ExplSyntaxOn` is laid out exactly as
/// in a full format; only *emission* is filtered (see [`LowerCtx::range`]). Unlike
/// [`format_node_with_signatures`], the document-level trailing-edge normalization
/// is **not** applied — this is a mid-document fragment, so no final newline is
/// forced. Trailing whitespace is trimmed (the slice it replaces ends at a block
/// boundary), keeping the diff against the original slice clean.
pub fn format_node_range_with_signatures(
    root: &SyntaxNode,
    style: FormatStyle,
    external: &SignatureDb,
    range: TextRange,
) -> Result<String, FormatError> {
    validate_supported_tokens(root)?;

    let ctx = FormatContext::new(style);
    let mut formatted = format_root(root, ctx, external, Some(range));
    let trimmed_len = formatted.trim_end_matches([' ', '\t', '\n', '\r']).len();
    formatted.truncate(trimmed_len);
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

fn format_root(
    root: &SyntaxNode,
    ctx: FormatContext,
    external: &SignatureDb,
    range: Option<TextRange>,
) -> String {
    // Scan the document's own `\newcommand`/`\newenvironment`/xparse definitions
    // once, so the lowering resolves a locally-defined construct's arity (not just
    // the built-in DB's). They are overlaid on top of `external` — the merged
    // signatures of any loaded local packages — so a document redefinition wins
    // over a package. `external` is empty for the contextless entry points, in
    // which case this is exactly the old document-only scan. Held by value for the
    // whole lowering.
    let mut user = external.clone();
    user.merge_from(&scan_definitions(root));
    // The expl3 source regions, recomputed read-only from the same toggle set the
    // lexer uses ([`expl_toggle`]). Inside them source whitespace is catcode-9
    // (ignored) and `~` is catcode-10 (a literal space), so the formatter fully owns
    // layout. Held by value for the whole lowering, like `user`.
    let regions = expl3_regions(root);
    let cx = LowerCtx {
        wrap: ctx.style().wrap,
        signatures: Signatures::new(&user),
        expl3_regions: &regions,
        range,
    };
    let ir = lower_node(root, cx);
    Printer::new(ctx.style()).print(&ir)
}

/// Whether two byte ranges overlap (share at least one byte). Half-open, so ranges
/// that merely touch at a boundary (`a.end == b.start`) do not overlap — used by
/// the range-formatting emission filter to keep a top-level block's leading/trailing
/// trivia (which abuts but does not overlap the block-aligned range) out of the
/// fragment.
fn ranges_overlap(a: TextRange, b: TextRange) -> bool {
    a.start() < b.end() && b.start() < a.end()
}

/// The byte ranges of the document's expl3 regions, in document order. A region
/// runs from an opener (`\ExplSyntaxOn`, or a `\ProvidesExpl*` declaration, which
/// opens expl3 for the rest of the file) through the matching `\ExplSyntaxOff`
/// (inclusive of both toggle commands), or to end of input when unclosed. The
/// toggle set is read from [`expl_toggle`] — the *same* fixed set the lexer flips
/// its `expl_syntax` flag on — so the two never drift.
///
/// Matches only [`SyntaxKind::CONTROL_WORD`] tokens, so a `\ExplSyntaxOn` written
/// inside `\verb`/a comment (a `VERB`/`COMMENT` token, never a `CONTROL_WORD`) is
/// not a toggle, exactly as in the lexer. The CST is untouched; this is a pure
/// read-only side channel (the sanctioned byte-range pattern, `AGENTS.md` #4).
fn expl3_regions(root: &SyntaxNode) -> Vec<TextRange> {
    let mut regions = Vec::new();
    let mut open: Option<TextSize> = None;
    for token in root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SyntaxKind::CONTROL_WORD)
    {
        match expl_toggle(token.text()) {
            // A redundant inner `\ExplSyntaxOn` does not restart the region (the
            // lexer's flag is an idempotent set-true).
            Some(ExplToggle::On) if open.is_none() => open = Some(token.text_range().start()),
            Some(ExplToggle::Off) => {
                if let Some(start) = open.take() {
                    regions.push(TextRange::new(start, token.text_range().end()));
                }
                // A stray `\ExplSyntaxOff` with no open region is ignored (toggling
                // an already-false flag is a no-op), matching the lexer.
            }
            _ => {}
        }
    }
    if let Some(start) = open.take() {
        // An unclosed region runs to end of input (the lexer's flag simply stays
        // true to EOF).
        regions.push(TextRange::new(start, root.text_range().end()));
    }
    regions
}

/// The state threaded through every lowering call: the active [`WrapMode`] plus the
/// per-document [`Signatures`] overlay (scanned definitions over the built-in DB)
/// that [`lower_begin`] consults for environment arity. `Copy`, so it passes by
/// value like the bare `wrap` mode it replaced.
#[derive(Clone, Copy)]
struct LowerCtx<'a> {
    wrap: WrapMode,
    signatures: Signatures<'a>,
    /// Sorted, non-overlapping byte ranges of the document's expl3 regions (see
    /// [`expl3_regions`]). Inside these, source whitespace is catcode-9 (ignored)
    /// and `~` is catcode-10 (a literal space), so the formatter lays out the code
    /// itself — regardless of [`WrapMode`]. Borrowed from a `Vec` owned by
    /// [`format_root`], exactly like `signatures`.
    expl3_regions: &'a [TextRange],
    /// Range-formatting emission filter. When `Some`, only the [`SyntaxKind::ROOT`]
    /// children overlapping this byte range are lowered (the in-range top-level
    /// blocks); the rest are skipped and never produce IR (see [`lower_node`]).
    /// `None` (the default) lowers the whole document. The filter applies *only* at
    /// `ROOT` — every selected block still lowers in full, at its real indent-0
    /// context, so the formatter stays the sole authority on layout.
    range: Option<TextRange>,
}

impl<'a> LowerCtx<'a> {
    /// Whether the document has any expl3 region at all — the cheap short-circuit
    /// for the no-expl3 majority (the slice is empty, so every query is free).
    fn any_expl3(self) -> bool {
        !self.expl3_regions.is_empty()
    }

    /// Whether byte offset `at` falls inside some expl3 region. O(log n) over the
    /// sorted, disjoint range list.
    fn in_expl3_region(self, at: TextSize) -> bool {
        self.expl3_regions
            .binary_search_by(|r| {
                use std::cmp::Ordering;
                if at < r.start() {
                    Ordering::Greater
                } else if at >= r.end() {
                    Ordering::Less
                } else {
                    Ordering::Equal
                }
            })
            .is_ok()
    }

    /// Whether `range` intersects some expl3 region (used to route a paragraph that
    /// is wholly or partly in-region).
    fn overlaps_expl3(self, range: TextRange) -> bool {
        self.expl3_regions
            .iter()
            .any(|r| r.start() < range.end() && range.start() < r.end())
    }
}

/// Lower a CST node to IR. Most nodes lower generically (see
/// [`lower_element_stream`]); an [`SyntaxKind::ENVIRONMENT`] is special-cased to
/// indent its body (see [`lower_environment`]), and under [`WrapMode::Reflow`] a
/// [`SyntaxKind::PARAGRAPH`] is wrapped to the line width (see
/// [`lower_paragraph_reflow`]). The [`LowerCtx`] (wrap mode + signature overlay) is
/// threaded through so it reaches every nested paragraph (including environment and
/// group bodies).
fn lower_node(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    // Range-formatting emission filter: at the document root, lower only the
    // children (top-level blocks plus the trivia between them) overlapping the
    // requested range; skip the rest entirely. The filter lives at `ROOT` so each
    // emitted block still lowers in full below — see [`LowerCtx::range`]. A `None`
    // range (the whole-document default) never reaches here.
    if let Some(range) = cx.range
        && node.kind() == SyntaxKind::ROOT
    {
        let filtered = node
            .children_with_tokens()
            .filter(move |el| ranges_overlap(range, el.text_range()));
        return Ir::concat(lower_element_stream(filtered, cx));
    }
    // expl3 code layout (catcode-9 whitespace / catcode-10 `~`) applies regardless
    // of `WrapMode`, so it is checked before the wrap-gated arms below. A paragraph
    // overlapping a region is split at the toggles; a brace/optional group inside a
    // region lays out its body as expl3 code.
    if cx.any_expl3() {
        match node.kind() {
            SyntaxKind::PARAGRAPH if cx.overlaps_expl3(node.text_range()) => {
                return lower_expl_paragraph(node, cx);
            }
            SyntaxKind::GROUP if cx.in_expl3_region(node.text_range().start()) => {
                return lower_expl_group(node, SyntaxKind::L_BRACE, SyntaxKind::R_BRACE, cx);
            }
            SyntaxKind::OPTIONAL if cx.in_expl3_region(node.text_range().start()) => {
                return lower_expl_group(node, SyntaxKind::L_BRACKET, SyntaxKind::R_BRACKET, cx);
            }
            // A command and its greedily-attached `{…}`/`[…]` arguments lay out as a
            // fill so the arguments break independently (only an over-long one
            // detonates) rather than the generic concat breaking every group.
            SyntaxKind::COMMAND if cx.in_expl3_region(node.text_range().start()) => {
                return lower_expl_code(node.children_with_tokens(), cx);
            }
            _ => {}
        }
    }
    match node.kind() {
        // A `.dtx` documentation-layer prose paragraph (its first content token is
        // a `DOC_MARGIN`): reflow the bare prose and re-emit a `% ` margin on each
        // wrapped line. Checked before the generic paragraph reflow so the margin
        // is stripped and re-synthesized rather than glued into the fill.
        SyntaxKind::PARAGRAPH if cx.wrap == WrapMode::Reflow && is_dtx_doc_paragraph(node) => {
            return lower_dtx_doc_paragraph(node, cx);
        }
        SyntaxKind::PARAGRAPH if cx.wrap == WrapMode::Reflow => {
            return lower_paragraph_reflow(node, cx);
        }
        // A `.dtx` docstrip frame (`%␣␣␣␣\begin{macrocode}`, a documentation-layer
        // `% \begin{itemize}`): the body is never indented and the closing frame is
        // kept whole at column 0. Routed before the alignment/list lowerers so a
        // margin-framed environment never reaches a layout that would reindent its
        // frame margins.
        SyntaxKind::ENVIRONMENT if !has_verbatim_body(node) && is_margin_framed(node) => {
            return lower_margin_framed_environment(node, cx);
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
        SyntaxKind::COMMAND if cx.wrap == WrapMode::Reflow && command_has_managed_arg(node, cx) => {
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
    reflow_elements(node.children_with_tokens(), cx, ReflowKind::Prose)
}

/// Whether `node` is a `.dtx` documentation-layer paragraph. A pure CST-shape
/// fact, like [`is_margin_framed`]: `DOC_MARGIN` exists only under the `.dtx` lexer
/// config, so this is unambiguous and always false elsewhere, and it needs no
/// signature lookup. Two shapes count:
/// - The first content token (skipping leading `WHITESPACE`/`NEWLINE` trivia) is a
///   `DOC_MARGIN` — the margin sits inside the paragraph (the first line of a doc
///   block, or a `% \item` body line opening after the `\begin{…}` break).
/// - The margin *floated out*: when a doc paragraph follows a `%` blank line, its
///   leading `%` is attached as inter-paragraph trivia, so the nearest preceding
///   token (skipping inline whitespace on the same line) is a `DOC_MARGIN`. This is
///   the common multi-paragraph case (see [`margin_floats_into_paragraph`], which
///   drops the floated margin so the reflow re-emits a canonical one).
///
/// A guard-led line (`%<…>`, a `GUARD` token) is *not* doc prose, so guards keep
/// their column-0 pin untouched.
fn is_dtx_doc_paragraph(node: &SyntaxNode) -> bool {
    let margin_inside = node
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !is_collapsible_trivia(t.kind()))
        .is_some_and(|t| t.kind() == SyntaxKind::DOC_MARGIN);
    margin_inside
        || node
            .first_token()
            .is_some_and(|t| margin_precedes_on_line(&t))
}

/// Whether the nearest token before `token`, skipping inline `WHITESPACE` on the
/// same line (stopping at any `NEWLINE` or other token), is a `DOC_MARGIN`: the
/// floated leading margin of a doc paragraph. Mirrors [`is_margin_framed`]'s
/// backward walk.
fn margin_precedes_on_line(token: &SyntaxToken) -> bool {
    let mut prev = token.prev_token();
    while let Some(t) = prev {
        match t.kind() {
            SyntaxKind::WHITESPACE => prev = t.prev_token(),
            SyntaxKind::DOC_MARGIN => return true,
            _ => return false,
        }
    }
    false
}

/// Whether `margin` is the floated leading `%` of a reflowable `.dtx` doc
/// paragraph: scanning forward over inline `WHITESPACE` (not a `NEWLINE`), the next
/// sibling is a `PARAGRAPH` that reflows. Such a margin is dropped during reflow
/// because the paragraph's own [`Ir::margin_prefix`] re-emits a canonical `% ` on
/// every line. A `%`-only blank line fails this (its margin is followed by a
/// newline), so it stays a column-0 separator.
fn margin_floats_into_paragraph(margin: &SyntaxToken) -> bool {
    let mut next = margin.next_sibling_or_token();
    while let Some(SyntaxElement::Token(t)) = &next {
        if t.kind() == SyntaxKind::WHITESPACE {
            next = t.next_sibling_or_token();
        } else {
            break;
        }
    }
    matches!(
        next,
        Some(SyntaxElement::Node(n))
            if n.kind() == SyntaxKind::PARAGRAPH
                && is_dtx_doc_paragraph(&n)
                && dtx_paragraph_reflows(&n)
    )
}

/// Lower a `.dtx` documentation paragraph under [`WrapMode::Reflow`]. When the
/// paragraph is pure running prose ([`dtx_paragraph_reflows`]) the bare prose is
/// reflowed to the line width via [`reflow_elements`] in [`ReflowKind::DtxProse`]
/// mode, which drops each line's `%` margin and re-emits a canonical `% ` margin
/// on every reflowed line (see [`Ir::margin_prefix`]). When it instead contains or
/// sits inside an environment (a `% \begin{itemize}` list, a `macrocode` block, a
/// `macro`/`environment` doc block) it is lowered *preserve-style* so frame margins
/// and item lines round-trip byte-for-byte; reflowing structured doc content is out
/// of scope for the first cut.
fn lower_dtx_doc_paragraph(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    if dtx_paragraph_reflows(node) {
        reflow_elements(node.children_with_tokens(), cx, ReflowKind::DtxProse)
    } else {
        Ir::concat(lower_element_stream(node.children_with_tokens(), cx))
    }
}

/// Whether a `.dtx` documentation paragraph is pure running prose safe to reflow:
/// it neither contains an `ENVIRONMENT` (a margin-framed list/`macrocode`/doc block
/// whose frame margins must stay column-0) nor sits inside one (its body lines,
/// e.g. `\item`s, must keep their authored breaks). Anything structured is left on
/// the byte-faithful preserve path.
fn dtx_paragraph_reflows(node: &SyntaxNode) -> bool {
    !node
        .descendants()
        .any(|d| d.kind() == SyntaxKind::ENVIRONMENT)
        && !node
            .ancestors()
            .any(|a| a.kind() == SyntaxKind::ENVIRONMENT)
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
///
/// [`ReflowKind`] selects how a *lone* source newline is treated (see that type).
/// `Prose` rejoins it into the surrounding fill (paragraphs, prose arguments);
/// `Statement` preserves it, so a code-like brace-group body keeps one logical line
/// per source line and only an *over-long* line wraps — never collapsing the author's
/// statement-per-line structure (`\draw …;` / `\draw …;`) into a single run.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReflowKind {
    /// Running prose: a lone newline is a break opportunity the fill rejoins.
    Prose,
    /// Code-like statements (a `\newcommand` definition body): a lone newline ends
    /// the line, so each source line stays its own logical line; only width forces a
    /// wrap. Flush continuation keeps the wrap idempotent (a wrapped tail re-parses
    /// as a line already at the body indent).
    Statement,
    /// A `.dtx` documentation-layer prose paragraph: behaves like [`Self::Prose`]
    /// (a lone newline rejoins), but the per-line `%` documentation margin
    /// (`DOC_MARGIN`) is *dropped* from each line and each fill segment is wrapped
    /// in an [`Ir::margin_prefix`] so a canonical `% ` margin is re-emitted at
    /// column 0 on every reflowed line.
    DtxProse,
}

/// The canonical `.dtx` documentation margin re-emitted on each reflowed prose
/// line under [`ReflowKind::DtxProse`]: a `%` plus one space.
const DTX_DOC_MARGIN: &str = "% ";

fn reflow_elements(
    elements: impl Iterator<Item = SyntaxElement>,
    cx: LowerCtx<'_>,
    kind: ReflowKind,
) -> Ir {
    // Collected up front so the single-newline arm can look ahead at the next
    // physical line ([`line_is_command_only`]). Inline prose commands (`\footnote`,
    // `\emph`, …) are flattened into the stream so their bodies reflow as running
    // text rather than block-breaking their braces (see [`flatten_inline_prose`]).
    let elements: Vec<SyntaxElement> = flatten_inline_prose(elements.collect(), cx);

    // Under `.dtx` prose reflow each fill segment is wrapped in a `% ` margin
    // prefix and the per-line `DOC_MARGIN` tokens are dropped; `None` otherwise.
    let margin: Option<&str> = (kind == ReflowKind::DtxProse).then_some(DTX_DOC_MARGIN);

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
    /// run as a fill segment. Under `.dtx` prose reflow (`margin` set) the fill is
    /// wrapped in an [`Ir::margin_prefix`] so a `% ` margin is re-emitted on every
    /// reflowed line.
    fn end_line(
        atom: &mut Vec<Ir>,
        run: &mut Vec<Ir>,
        lines: &mut Vec<Ir>,
        seps: &mut Vec<Ir>,
        pending_sep: &mut Ir,
        margin: Option<&str>,
    ) {
        flush_atom(atom, run);
        if !run.is_empty() {
            let fill = Ir::fill(run.drain(..));
            let segment = match margin {
                Some(m) => Ir::margin_prefix(m, fill),
                None => fill,
            };
            push_segment(segment, lines, seps, pending_sep);
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
                    end_line(
                        &mut atom,
                        &mut run,
                        &mut lines,
                        &mut seps,
                        &mut pending_sep,
                        margin,
                    );
                    pending_sep = Ir::empty_line();
                    line_all_commands = true;
                    line_has_content = false;
                } else if newlines == 1 {
                    // A single source newline. Under `Statement` reflow every source
                    // line is its own logical line, so the break always ends the line.
                    // Under `Prose` it is normally just an atom boundary the fill
                    // rejoins, except a line that is *only* command(s) — on either side
                    // of the break — is kept on its own line: end the line so the break
                    // survives instead of collapsing to a fill space.
                    let prev_is_command = line_has_content && line_all_commands;
                    let next_is_command = line_is_command_only(&elements, idx, cx);
                    if kind == ReflowKind::Statement || prev_is_command || next_is_command {
                        end_line(
                            &mut atom,
                            &mut run,
                            &mut lines,
                            &mut seps,
                            &mut pending_sep,
                            margin,
                        );
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
                    end_line(
                        &mut atom,
                        &mut run,
                        &mut lines,
                        &mut seps,
                        &mut pending_sep,
                        margin,
                    );
                }
                atom.push(Ir::verbatim(token.text()));
                end_line(
                    &mut atom,
                    &mut run,
                    &mut lines,
                    &mut seps,
                    &mut pending_sep,
                    margin,
                );
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
                end_line(
                    &mut atom,
                    &mut run,
                    &mut lines,
                    &mut seps,
                    &mut pending_sep,
                    margin,
                );
                line_all_commands = true;
                line_has_content = false;
            }
            // Under `.dtx` prose reflow, a per-line `%` documentation margin
            // (`DOC_MARGIN`) is dropped: the canonical `% ` margin is re-emitted on
            // every reflowed line by the enclosing [`Ir::margin_prefix`] (see
            // `end_line`), so gluing the source `%` into the fill would double it.
            // The single space following it is inter-word whitespace the fill
            // re-derives. A `GUARD` is *not* dropped (guards keep their column-0 pin).
            SyntaxElement::Token(token)
                if margin.is_some() && token.kind() == SyntaxKind::DOC_MARGIN => {}
            // Any other token (WORD, `~`, `&`, `#`, `^`, `_`, brackets, `\verb`,
            // a bare control symbol) glues onto the current atom — prose content,
            // so this physical line is no longer command-only. A `.dtx` margin/guard
            // (only under the dtx config) pins to column 0 instead of reflowing.
            SyntaxElement::Token(token) => {
                atom.push(lower_loose_token(token));
                line_has_content = true;
                line_all_commands = false;
            }
            // An explicit `\\` line break (with its `*` / `[len]`, grouped by the
            // parser into one node) rides the end of the current line, then breaks.
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::LINE_BREAK => {
                atom.push(lower_node(child, cx));
                end_line(
                    &mut atom,
                    &mut run,
                    &mut lines,
                    &mut seps,
                    &mut pending_sep,
                    margin,
                );
                line_all_commands = true;
                line_has_content = false;
            }
            SyntaxElement::Node(child) => {
                let ir = lower_node(child, cx);
                if ir.contains_forced_break() {
                    // A block amid prose: end the current line, then place the
                    // block on its own line(s); a fresh run continues after.
                    end_line(
                        &mut atom,
                        &mut run,
                        &mut lines,
                        &mut seps,
                        &mut pending_sep,
                        margin,
                    );
                    push_segment(ir, &mut lines, &mut seps, &mut pending_sep);
                    line_all_commands = true;
                    line_has_content = false;
                } else {
                    // A block-level `COMMAND` keeps the line command-only; an inline
                    // command (`\citep`, `\ref`, …) is running-text content, as is any
                    // other inline node (math, an inline group), and disqualifies it.
                    atom.push(ir);
                    line_has_content = true;
                    line_all_commands &=
                        child.kind() == SyntaxKind::COMMAND && !command_is_inline(child, cx);
                }
            }
        }
        idx += 1;
    }
    end_line(
        &mut atom,
        &mut run,
        &mut lines,
        &mut seps,
        &mut pending_sep,
        margin,
    );

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

/// Lower a `PARAGRAPH` that overlaps an expl3 region. The paragraph is split at the
/// `\ExplSyntaxOn`/`Off` toggles into maximal in-region and out-of-region runs;
/// each in-region run lays out as expl3 code ([`lower_expl_code`]), each out-of-region
/// run keeps the ordinary prose/stream treatment. The common case — a whole
/// paragraph inside a region (a `.sty`/`.dtx` body, or a blank-line-separated
/// `\ExplSyntaxOn…Off` block) — is a single in-region run. Runs are joined by a hard
/// line break (a region boundary always begins a fresh line).
fn lower_expl_paragraph(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    let elements: Vec<SyntaxElement> = node.children_with_tokens().collect();
    let mut segments: Vec<Ir> = Vec::new();
    let mut i = 0;
    while i < elements.len() {
        let in_region = cx.in_expl3_region(elements[i].text_range().start());
        let start = i;
        while i < elements.len()
            && cx.in_expl3_region(elements[i].text_range().start()) == in_region
        {
            i += 1;
        }
        let run = elements[start..i].iter().cloned();
        let ir = if in_region {
            lower_expl_code(run, cx)
        } else if cx.wrap == WrapMode::Reflow {
            reflow_elements(run, cx, ReflowKind::Prose)
        } else {
            Ir::concat(lower_element_stream(run, cx))
        };
        if !matches!(ir, Ir::Nil) {
            segments.push(ir);
        }
    }
    Ir::join(Ir::hard_line(), segments)
}

/// Lay out a stream of elements known to be inside an expl3 region as expl3 code.
///
/// In an expl3 region TeX catcodes change: source spaces/tabs are **ignored**
/// (catcode 9) and `~` is a **literal space** (catcode 10). Source whitespace is
/// therefore insignificant and the formatter owns layout:
/// - **Statements** are split on source newlines — each source line is its own
///   logical line (the expl3 convention is one call per line). This sidesteps the
///   unsolvable problem of grouping a multi-token call (`\cs_new:Npn \foo:n #1 {…}`
///   is several sibling CST nodes, not one) and stays idempotent: a flushed
///   continuation re-parses to a line already at the same indent.
/// - **Inter-token spacing** collapses to a single space (any catcode-9 run is
///   inert, and one space keeps the token boundary so re-lexing never merges two
///   tokens).
/// - **`~`** renders verbatim and introduces a *soft* break (flat: nothing; broken:
///   newline) — a line carrying a tie is wrapped in a group so its ties break
///   together only when the line overflows. A following newline is catcode-9
///   ignored, so the break preserves meaning.
/// - **Brace/optional groups** recurse through [`lower_node`] → [`lower_expl_group`]
///   (an inner block indents; a fitting one stays inline). A multi-line block lands
///   on its own line(s) (Allman), like the block-amid-prose rule in
///   [`reflow_elements`].
fn lower_expl_code(elements: impl Iterator<Item = SyntaxElement>, cx: LowerCtx<'_>) -> Ir {
    let elements: Vec<SyntaxElement> = elements.collect();
    let mut lines: Vec<Ir> = Vec::new();
    let mut seps: Vec<Ir> = Vec::new();
    let mut pending_sep = Ir::hard_line();

    // The current logical line is built as a Wadler *fill*: `atom` accumulates the
    // glued pieces of the atom in progress; `parts` is the alternating
    // `[atom, sep, atom, …]` the printer fills greedily. `sep_before_next` is the
    // separator to emit before the next atom — `Line` for an inter-token space
    // (flat: one space, keeping the token boundary), `SoftLine` after a `~` (flat:
    // nothing, since the `~` is itself the space).
    let mut atom: Vec<Ir> = Vec::new();
    let mut parts: Vec<Ir> = Vec::new();
    let mut sep_before_next: Option<Ir> = None;

    /// Commit the glued atom in progress as one fill atom, prefixing the pending
    /// separator when it is not the first atom of the line.
    fn flush_atom(atom: &mut Vec<Ir>, parts: &mut Vec<Ir>, sep_before_next: &mut Option<Ir>) {
        if atom.is_empty() {
            return;
        }
        if !parts.is_empty() {
            parts.push(sep_before_next.take().unwrap_or(Ir::Line));
        }
        parts.push(Ir::concat(atom.drain(..)));
        *sep_before_next = None;
    }

    /// Commit the in-progress line (if any) as the next logical line, recording the
    /// pending line separator before it and resetting line state.
    fn commit_line(
        atom: &mut Vec<Ir>,
        parts: &mut Vec<Ir>,
        sep_before_next: &mut Option<Ir>,
        lines: &mut Vec<Ir>,
        seps: &mut Vec<Ir>,
        pending_sep: &mut Ir,
    ) {
        flush_atom(atom, parts, sep_before_next);
        if !parts.is_empty() {
            // A multi-atom line is a fill (greedy independent breaks); a single
            // atom needs no fill.
            let line = if parts.len() == 1 {
                parts.drain(..).next().unwrap()
            } else {
                Ir::Fill(std::mem::take(parts).into())
            };
            seps.push(std::mem::replace(pending_sep, Ir::hard_line()));
            lines.push(line);
        }
        parts.clear();
        *sep_before_next = None;
    }

    let mut idx = 0;
    while idx < elements.len() {
        match &elements[idx] {
            // Insignificant whitespace: a single newline ends the statement line, a
            // blank line promotes the next line separator, an inline run is a single
            // (breakable) space before the next atom.
            SyntaxElement::Token(token) if is_collapsible_trivia(token.kind()) => {
                let newlines = consume_trivia_run_slice(&elements, &mut idx);
                if newlines >= 1 {
                    commit_line(
                        &mut atom,
                        &mut parts,
                        &mut sep_before_next,
                        &mut lines,
                        &mut seps,
                        &mut pending_sep,
                    );
                    if newlines >= 2 {
                        pending_sep = Ir::empty_line();
                    }
                } else {
                    flush_atom(&mut atom, &mut parts, &mut sep_before_next);
                    // Keep a tie's soft break if one is already pending.
                    if sep_before_next.is_none() {
                        sep_before_next = Some(Ir::Line);
                    }
                }
                continue;
            }
            // `~`: a literal space. Glue it to the end of the current atom, then
            // close the atom with a soft break (flat: nothing; broken: newline).
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::TILDE => {
                atom.push(Ir::verbatim(token.text()));
                flush_atom(&mut atom, &mut parts, &mut sep_before_next);
                sep_before_next = Some(Ir::SoftLine);
            }
            // A comment ends its line (it must terminate the source line).
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::COMMENT => {
                atom.push(Ir::verbatim(token.text()));
                commit_line(
                    &mut atom,
                    &mut parts,
                    &mut sep_before_next,
                    &mut lines,
                    &mut seps,
                    &mut pending_sep,
                );
            }
            SyntaxElement::Token(token) => atom.push(lower_loose_token(token)),
            SyntaxElement::Node(child) => {
                let ir = lower_node(child, cx);
                if ir.contains_forced_break() {
                    // A multi-line block (group, environment, display math): end the
                    // current line and place the block on its own line(s) (Allman).
                    commit_line(
                        &mut atom,
                        &mut parts,
                        &mut sep_before_next,
                        &mut lines,
                        &mut seps,
                        &mut pending_sep,
                    );
                    seps.push(std::mem::replace(&mut pending_sep, Ir::hard_line()));
                    lines.push(ir);
                } else {
                    atom.push(ir);
                }
            }
        }
        idx += 1;
    }
    commit_line(
        &mut atom,
        &mut parts,
        &mut sep_before_next,
        &mut lines,
        &mut seps,
        &mut pending_sep,
    );

    let mut result: Vec<Ir> = Vec::with_capacity(lines.len().saturating_mul(2));
    for (i, line) in lines.into_iter().enumerate() {
        if i > 0 {
            result.push(seps[i].clone());
        }
        result.push(line);
    }
    Ir::concat(result)
}

/// Lower a brace `{…}` or optional `[…]` group inside an expl3 region as a code
/// block: the body lays out as expl3 code ([`lower_expl_code`]) indented one step,
/// the whole wrapped in a soft [`Ir::group`] so it stays inline (`{ body }`, with
/// canonical inner spaces) when it fits and detonates to an indented block when the
/// body spans lines or overflows. The inline-vs-block decision is width/structure
/// driven (never source newlines), keeping reformatting idempotent. Mirrors
/// [`lower_prose_group`] but recurses into expl3 code and uses [`Ir::line`] so the
/// inline form carries spaces.
fn lower_expl_group(
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
    let body = trim_trailing_break(trim_leading_break(lower_expl_code(
        body_elements.into_iter(),
        cx,
    )));
    if matches!(body, Ir::Nil) {
        Ir::concat([open_ir, close_ir])
    } else {
        Ir::group(Ir::concat([
            open_ir,
            Ir::indent(Ir::concat([Ir::line(), body])),
            Ir::line(),
            close_ir,
        ]))
    }
}

/// Lower a single loose token (one not collapsed into a trivia run) to inline IR.
/// A `.dtx` documentation margin (`DOC_MARGIN`) or docstrip guard (`GUARD`) pins
/// to column 0 via [`Ir::column_zero`] so docstrip's left-margin anchor survives
/// any surrounding LaTeX nesting; every other token splices verbatim. These tokens
/// only exist under the `.dtx` lexer config, so non-`.dtx` lowering is unaffected.
fn lower_loose_token(token: &SyntaxToken) -> Ir {
    if matches!(token.kind(), SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD) {
        Ir::column_zero(token.text())
    } else {
        Ir::verbatim(token.text())
    }
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
            // The floated leading `%` of a reflowable `.dtx` doc paragraph (one that
            // follows a `%` blank line): drop it and the inline whitespace after it,
            // since the paragraph's own [`Ir::margin_prefix`] re-emits a canonical
            // `% ` on every reflowed line. Without this the margin would double up.
            SyntaxElement::Token(token)
                if cx.wrap == WrapMode::Reflow
                    && token.kind() == SyntaxKind::DOC_MARGIN
                    && margin_floats_into_paragraph(&token) =>
            {
                while let Some(SyntaxElement::Token(t)) = iter.peek() {
                    if t.kind() == SyntaxKind::WHITESPACE {
                        iter.next();
                    } else {
                        break;
                    }
                }
            }
            SyntaxElement::Token(token) => out.push(lower_loose_token(&token)),
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
/// The leading comment-bind run (an own-line `%` run the parser attached as
/// leading children *before* the `BEGIN` node). It is not body: it lowers to its
/// own line(s) above `\begin`, at the environment's own indentation. Returns
/// [`Ir::Nil`] when there is no such run. Shared by every environment lowerer so
/// the bound comment is rendered the same way regardless of body shape.
fn lower_environment_leading(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    let mut leading: Vec<SyntaxElement> = Vec::new();
    for element in node.children_with_tokens() {
        if matches!(&element, SyntaxElement::Node(c) if c.kind() == SyntaxKind::BEGIN) {
            break;
        }
        leading.push(element);
    }
    if leading.is_empty() {
        Ir::Nil
    } else {
        Ir::concat(lower_element_stream(leading.into_iter(), cx))
    }
}

fn lower_environment(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    let leading = lower_environment_leading(node, cx);
    let mut begin = Ir::Nil;
    let mut end = Ir::Nil;
    let mut body_elements: Vec<SyntaxElement> = Vec::new();
    let mut seen_begin = false;
    for element in node.children_with_tokens() {
        match &element {
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::BEGIN => {
                seen_begin = true;
                begin = lower_begin(child, cx);
            }
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::END => {
                end = lower_node(child, cx);
            }
            _ if !seen_begin => {}
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

    let env = if matches!(body, Ir::Nil) {
        // Empty body: keep `\begin` and `\end` on their own lines (no edge blank).
        Ir::concat([begin, Ir::hard_line(), end])
    } else if environment_no_indent(node, cx) {
        // `document` and friends: lay the body on its own lines, but flush against
        // the surrounding indentation rather than nesting it.
        Ir::concat([begin, lead, body, trail, end])
    } else {
        Ir::concat([begin, Ir::indent(Ir::concat([lead, body])), trail, end])
    };
    Ir::concat([leading, env])
}

/// Whether the environment is *margin-framed*: a `.dtx` documentation margin
/// (`DOC_MARGIN`) or docstrip guard (`GUARD`) sits immediately before its `\begin`
/// on the same physical line — `%␣␣␣␣\begin{macrocode}`, a documentation-layer
/// `% \begin{itemize}`. The `\begin`/`\end` are docstrip *frame lines* anchored at
/// column 0, so the body must not be indented (indenting would push the frame
/// margins off column 0 and split the closing `%␣␣␣␣\end{…}` frame — the corruption
/// this fixes). A pure CST-shape fact: it walks back over inline whitespace from
/// `\begin` and asks only "is the previous token a margin/guard on this line", with
/// no signature lookup, so it stays out of the semantic layer (decision #2) and
/// covers `macrocode` and any prose-layer environment uniformly. `DOC_MARGIN`/
/// `GUARD` exist only under the `.dtx` config, so this is always false elsewhere.
fn is_margin_framed(node: &SyntaxNode) -> bool {
    let Some(begin) = node.children().find(|c| c.kind() == SyntaxKind::BEGIN) else {
        return false;
    };
    let mut tok = begin.first_token().and_then(|t| t.prev_token());
    while let Some(t) = tok {
        match t.kind() {
            SyntaxKind::WHITESPACE => tok = t.prev_token(),
            SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD => return true,
            _ => return false,
        }
    }
    false
}

/// Split a trailing closing-frame margin run off `body`, returning it (the docstrip
/// `\end` frame's `%␣␣␣␣` prefix) so the caller can ride it onto the `\end` line at
/// column 0 instead of leaving it as body tail with a break before `\end` (which
/// would split the frame). The frame is the maximal trailing run of inline
/// `WHITESPACE` / `DOC_MARGIN` / `GUARD` tokens, and only counts as a frame when it
/// actually contains a margin/guard; the `NEWLINE` before it stays in `body` as the
/// trailing break that becomes the frame line's leading break. Returns `None` when
/// `\end` has no preceding margin on its own line (e.g. a prose-layer `\end{…}`
/// authored flush against content), so the caller falls back to the plain
/// no-indent shape.
fn split_closing_frame(body: &mut Vec<SyntaxElement>) -> Option<Vec<SyntaxElement>> {
    let mut boundary = body.len();
    let mut has_margin = false;
    while boundary > 0 {
        match &body[boundary - 1] {
            SyntaxElement::Token(t)
                if matches!(t.kind(), SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD) =>
            {
                has_margin = true;
                boundary -= 1;
            }
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::WHITESPACE => boundary -= 1,
            _ => break,
        }
    }
    has_margin.then(|| body.split_off(boundary))
}

/// Lower a *margin-framed* environment (see [`is_margin_framed`]): a `.dtx`
/// docstrip frame whose `\begin`/`\end` sit on column-0 margin lines. Unlike
/// [`lower_environment`] this never indents the body (the frames are not a real
/// indentation scope) and it pulls the closing `%␣␣␣␣` frame back onto the `\end`
/// line so the terminator stays a single byte-faithful frame line. The body is
/// still lowered as ordinary content — for `macrocode` that is real code whose
/// interior groups/environments indent relative to their column-0 base; for a
/// prose-layer environment it is margin lines, each pinned to column 0 by
/// [`Ir::column_zero`].
fn lower_margin_framed_environment(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    let leading = lower_environment_leading(node, cx);
    let mut begin = Ir::Nil;
    let mut end = Ir::Nil;
    let mut body_elements: Vec<SyntaxElement> = Vec::new();
    let mut seen_begin = false;
    for element in node.children_with_tokens() {
        match &element {
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::BEGIN => {
                seen_begin = true;
                begin = lower_begin(child, cx);
            }
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::END => {
                end = lower_node(child, cx);
            }
            _ if !seen_begin => {}
            _ => body_elements.push(element),
        }
    }

    // Pull the `%␣␣␣␣` that frames `\end` onto the `\end` line; what remains is the
    // real body.
    let frame = split_closing_frame(&mut body_elements);
    let frame_ir = frame
        .map(|f| Ir::concat(lower_element_stream(f.into_iter(), cx)))
        .filter(|ir| !matches!(ir, Ir::Nil));

    // A `%` trailing the `\begin{…}` header on the same line is the space-suppression
    // idiom: lift it onto the `\begin` line, as [`lower_environment`] does, so it is
    // not relocated to its own line.
    let (begin, body) = match leading_inline_comment(&body_elements) {
        Some(comment) => (
            Ir::concat([begin, Ir::verbatim(comment.text())]),
            lower_body_dropping_leading_comment(body_elements, cx),
        ),
        None => (
            begin,
            Ir::concat(lower_element_stream(body_elements.into_iter(), cx)),
        ),
    };
    let (lead_blank, body) = peel_leading_break(body);
    let (trail_blank, body) = peel_trailing_break(body);
    let lead = if lead_blank {
        Ir::empty_line()
    } else {
        Ir::hard_line()
    };
    // The break that separates the body (or `\begin`, for an empty body) from the
    // `\end` frame line.
    let close_break = if trail_blank {
        Ir::empty_line()
    } else {
        Ir::hard_line()
    };

    let env = match (matches!(body, Ir::Nil), frame_ir) {
        // Empty body, framed close: `\begin` then the `%␣␣␣␣\end` frame line.
        (true, Some(frame_ir)) => Ir::concat([begin, close_break, frame_ir, end]),
        // Empty body, no frame: `\begin` and `\end` on their own lines.
        (true, None) => Ir::concat([begin, Ir::hard_line(), end]),
        // Body then the `%␣␣␣␣\end` frame line at column 0.
        (false, Some(frame_ir)) => Ir::concat([begin, lead, body, close_break, frame_ir, end]),
        // Body but no closing margin: behave like a no-indent environment.
        (false, None) => Ir::concat([begin, lead, body, close_break, end]),
    };
    Ir::concat([leading, env])
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
                out.push(lower_loose_token(&token));
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
        reflow_elements(children.into_iter(), cx, ReflowKind::Prose)
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
            SyntaxElement::Token(token) => head.push(lower_loose_token(token)),
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
    let leading = lower_environment_leading(node, cx);
    let mut begin = Ir::Nil;
    let mut end = Ir::Nil;
    let mut body_elements: Vec<SyntaxElement> = Vec::new();
    let mut seen_begin = false;
    for element in node.children_with_tokens() {
        match &element {
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::BEGIN => {
                seen_begin = true;
                begin = lower_begin(child, cx);
            }
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::END => {
                end = lower_node(child, cx);
            }
            _ if !seen_begin => {}
            _ => body_elements.push(element),
        }
    }

    let Some(body) = lower_list_body(&body_elements, cx) else {
        return lower_environment(node, cx);
    };
    Ir::concat([
        leading,
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
        .map(|chunk| reflow_elements(chunk.iter().cloned(), cx, ReflowKind::Prose))
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
    let leading = lower_environment_leading(node, cx);
    let mut begin = Ir::Nil;
    let mut end = Ir::Nil;
    let mut body_elements: Vec<SyntaxElement> = Vec::new();
    let mut seen_begin = false;
    for element in node.children_with_tokens() {
        match &element {
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::BEGIN => {
                seen_begin = true;
                begin = lower_begin(child, cx);
            }
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::END => {
                end = lower_node(child, cx);
            }
            _ if !seen_begin => {}
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
        leading,
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
/// break), when a cell carries a forced break that cannot collapse to one line (a
/// nested block, or a blank line inside the cell — a lone continuation newline is
/// joined, not a fallback), or on a mid-row comment.
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
    /// fallback. A lone newline *inside* a cell is a continuation line; it lowers to
    /// a top-level [`Ir::HardLine`], which we collapse to a space so the cell stays
    /// on one aligned row. A blank line (`\par`, an [`Ir::EmptyLine`]) in a cell, or
    /// a forced break nested inside a child block (`\begin{cases}…`), is *not*
    /// collapsed and still (correctly) falls back — it cannot sit on one aligned row.
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
        // A lone interior newline classifies to a top-level `Ir::HardLine`
        // (`classify_trivia`); collapse it to a space so a continuation line joins
        // onto its aligned row. A blank line inside a cell is an `Ir::EmptyLine`
        // (untouched here), and a nested block's breaks live inside a child `Ir`, so
        // both keep tripping `contains_forced_break` below and fall back.
        let joined = lower_element_stream(cell.drain(..), cx)
            .into_iter()
            .map(|ir| {
                if matches!(ir, Ir::HardLine) {
                    Ir::line()
                } else {
                    ir
                }
            })
            .collect::<Vec<_>>();
        let ir = Ir::concat(joined);
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

    // A brace-group body under reflow is laid out as code-like statements: each
    // source line stays its own logical line, but an over-long one wraps to the
    // width instead of forcing the printer to break the innermost nested prose
    // group (the only soft break a rigid `lower_element_stream` body would expose).
    // Optional `[…]` bodies and the non-reflow modes keep the generic stream.
    let body = if cx.wrap == WrapMode::Reflow && open == SyntaxKind::L_BRACE {
        reflow_elements(body_elements.into_iter(), cx, ReflowKind::Statement)
    } else {
        Ir::concat(lower_element_stream(body_elements.into_iter(), cx))
    };
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

/// Whether `command`'s signature marks any argument the [`lower_command`] path
/// must handle specially — a non-[`Opaque`](ContentKind::Opaque) content kind
/// ([`Prose`](ContentKind::Prose) or a [`TokenList`](ContentKind::TokenList)).
/// The cheap guard that gates the
/// [`lower_command`] path in [`lower_node`]: a command with no such argument (the
/// overwhelming common case) lowers generically, so nothing regresses.
fn command_has_managed_arg(command: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    command_name(command)
        .and_then(|name| cx.signatures.command(&name))
        .is_some_and(|sig| {
            sig.args
                .iter()
                .any(|spec| spec.content != ContentKind::Opaque)
        })
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
        .is_some_and(|sig| {
            sig.inline
                && sig
                    .args
                    .iter()
                    .any(|spec| spec.content == ContentKind::Prose)
        })
}

/// Whether `command` is an *inline* command that sits in running text (`\citep`,
/// `\ref`, `\emph`, …), per the signature DB's [`CommandSig::inline`] flag. Paragraph
/// reflow uses this so such a command flows into the fill as an atom even when the
/// author isolated it on its own source line, rather than being preserved as a
/// command-only line (see [`line_is_command_only`]). Broader than
/// [`command_is_inline_prose`], which additionally requires a prose argument.
fn command_is_inline(command: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    command_name(command)
        .and_then(|name| cx.signatures.command(&name))
        .is_some_and(|sig| sig.inline)
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
                let prose = match_arg_slot(&sig.args, &mut slot, is_bracket)
                    .is_some_and(|spec| spec.content == ContentKind::Prose);
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

/// Lower a `COMMAND` whose signature marks an argument's content kind (see
/// [`command_has_managed_arg`], which gates this path). Each attached `{…}`/`[…]`
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
                let (open, close) = if is_bracket {
                    (SyntaxKind::L_BRACKET, SyntaxKind::R_BRACKET)
                } else {
                    (SyntaxKind::L_BRACE, SyntaxKind::R_BRACE)
                };
                let spec = match_arg_slot(&sig.args, &mut slot, is_bracket);
                match spec.map(|s| s.content) {
                    Some(ContentKind::Prose) => {
                        out.push(lower_prose_group(&child, open, close, cx));
                    }
                    Some(ContentKind::TokenList) => {
                        // A collapsible token list (e.g. a `\citep` key list): fold a
                        // multi-line authored form to one line, falling back to the
                        // generic block form when the body is not safely collapsible.
                        out.push(
                            collapse_arg_group(&child, open, close, cx)
                                .unwrap_or_else(|| lower_node(&child, cx)),
                        );
                    }
                    _ => out.push(lower_node(&child, cx)),
                }
            }
            SyntaxElement::Node(child) => out.push(lower_node(&child, cx)),
            SyntaxElement::Token(token) if is_collapsible_trivia(token.kind()) => {
                let (newlines, trailing_ws) = consume_trivia_run(&token, &mut iter);
                out.push(classify_trivia(newlines, trailing_ws));
            }
            SyntaxElement::Token(token) => out.push(lower_loose_token(&token)),
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

    let body = reflow_elements(body_elements.into_iter(), cx, ReflowKind::Prose);
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

/// Lower a signature-marked *collapsible* argument group (see [`ContentKind::TokenList`])
/// as a single inline atom: interior newlines collapse to spaces, so a citation list
/// written across lines (`\citep{\n  a,\n  b\n}`) formats identically to its one-line
/// form (`\citep{a, b}`) — an incidental source line break inside such an argument
/// must not change the output (determinism). Unlike [`lower_prose_group`], the keys
/// are *not* reflowed to the width; they stay together on one line (a token list, not
/// prose).
///
/// Returns `None` — the caller falls back to the generic form ([`lower_node`]) — when
/// the group is *not* safely collapsible: it holds a blank-line paragraph break, a `%`
/// comment (which must end its line), or force-break content (a nested environment,
/// display math, `\\`). Those keep the indented multi-line block form. Mirrors
/// [`lower_bracketed`]'s delimiter handling and edge-break trimming.
fn collapse_arg_group(
    node: &SyntaxNode,
    open: SyntaxKind,
    close: SyntaxKind,
    cx: LowerCtx<'_>,
) -> Option<Ir> {
    let mut open_ir = Ir::Nil;
    let mut close_ir = Ir::Nil;
    let mut body: Vec<Ir> = Vec::new();
    let mut iter = node.children_with_tokens().peekable();
    while let Some(element) = iter.next() {
        match element {
            SyntaxElement::Token(t) if t.kind() == open && matches!(open_ir, Ir::Nil) => {
                open_ir = Ir::verbatim(t.text());
            }
            SyntaxElement::Token(t) if t.kind() == close => {
                close_ir = Ir::verbatim(t.text());
            }
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {
                let (newlines, trailing_ws) = consume_trivia_run(&t, &mut iter);
                if newlines >= 2 {
                    return None; // a blank-line `\par`: keep the block form
                }
                // A lone newline collapses to a single space; pure inline whitespace
                // stays verbatim, matching the one-line generic lowering.
                body.push(if newlines == 1 {
                    Ir::verbatim(" ")
                } else {
                    Ir::verbatim(trailing_ws)
                });
            }
            // A `%` comment must terminate its line, so the group cannot collapse.
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::COMMENT => return None,
            SyntaxElement::Token(t) => body.push(Ir::verbatim(t.text())),
            SyntaxElement::Node(child) => {
                let ir = lower_node(&child, cx);
                if ir.contains_forced_break() {
                    return None; // nested block content: keep the block form
                }
                body.push(ir);
            }
        }
    }
    let body = trim_trailing_break(trim_leading_break(Ir::concat(body)));
    Some(Ir::concat([open_ir, body, close_ir]))
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
                body = trim_trailing_break(lower_display_math_body(&n, cx));
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

/// The line-breaking role of a top-level math atom (see [`lower_display_math_body`]).
#[derive(Clone, Copy, PartialEq)]
enum MathRole {
    /// A term: a variable, number, group, command-with-arguments, script, etc.
    Operand,
    /// A binary operator (`+`, `-`, `\cdot`, `\times`, …) sitting between two
    /// operands. A break may be inserted *before* it.
    Binary,
    /// A relation (`=`, `\leq`, `\to`, …). The first one anchors the alignment;
    /// a later one is also a break point.
    Relation,
}

/// One top-level atom of a display-math body, paired with its [`MathRole`].
struct MathPiece {
    ir: Ir,
    role: MathRole,
}

/// Relation control words (the `\` stripped) that anchor alignment / break a long
/// display equation. Curated, growing like the signature DB; anything absent is a
/// plain operand.
const MATH_RELATION_COMMANDS: &[&str] = &[
    "le",
    "leq",
    "ge",
    "geq",
    "ne",
    "neq",
    "equiv",
    "approx",
    "approxeq",
    "sim",
    "simeq",
    "cong",
    "propto",
    "asymp",
    "doteq",
    "models",
    "vdash",
    "dashv",
    "perp",
    "parallel",
    "mid",
    "in",
    "ni",
    "notin",
    "subset",
    "subseteq",
    "subsetneq",
    "supset",
    "supseteq",
    "supsetneq",
    "sqsubseteq",
    "sqsupseteq",
    "prec",
    "preceq",
    "succ",
    "succeq",
    "ll",
    "gg",
    "lll",
    "ggg",
    "to",
    "rightarrow",
    "longrightarrow",
    "Rightarrow",
    "Longrightarrow",
    "implies",
    "impliedby",
    "iff",
    "mapsto",
    "longmapsto",
    "leftarrow",
    "Leftarrow",
    "gets",
    "leftrightarrow",
    "Leftrightarrow",
    "Longleftrightarrow",
    "hookrightarrow",
    "hookleftarrow",
    "triangleq",
    "coloneqq",
    "eqqcolon",
    "lesssim",
    "gtrsim",
];

/// Binary-operator control words (the `\` stripped) a long display equation may
/// break before. Curated; see [`MATH_RELATION_COMMANDS`].
const MATH_BINARY_COMMANDS: &[&str] = &[
    "pm",
    "mp",
    "times",
    "div",
    "cdot",
    "ast",
    "star",
    "circ",
    "bullet",
    "cup",
    "cap",
    "uplus",
    "sqcup",
    "sqcap",
    "vee",
    "wedge",
    "lor",
    "land",
    "oplus",
    "ominus",
    "otimes",
    "oslash",
    "odot",
    "setminus",
    "amalg",
    "diamond",
    "wr",
    "dagger",
    "ddagger",
    "bigtriangleup",
    "bigtriangledown",
    "triangleleft",
    "triangleright",
];

/// Classify a bare operator token by its literal text.
fn classify_math_op_text(text: &str) -> MathRole {
    match text {
        "=" | "<" | ">" => MathRole::Relation,
        "+" | "-" => MathRole::Binary,
        _ => MathRole::Operand,
    }
}

/// The [`MathRole`] of a top-level math atom. `prev` is the effective role of the
/// preceding atom: a `+`/`-` (or any binary operator) with no operand to its left
/// is unary — it glues to its operand and is *not* a break point — so it degrades
/// to an [`MathRole::Operand`].
fn math_atom_role(el: &SyntaxElement, prev: MathRole) -> MathRole {
    let raw = match el {
        SyntaxElement::Token(t) => classify_math_op_text(t.text()),
        SyntaxElement::Node(n) if n.kind() == SyntaxKind::COMMAND => crate::ast::command_name(n)
            .map_or(MathRole::Operand, |name| {
                if MATH_RELATION_COMMANDS.contains(&name.as_str()) {
                    MathRole::Relation
                } else if MATH_BINARY_COMMANDS.contains(&name.as_str()) {
                    MathRole::Binary
                } else {
                    MathRole::Operand
                }
            }),
        _ => MathRole::Operand,
    };
    if raw == MathRole::Binary && prev != MathRole::Operand {
        MathRole::Operand
    } else {
        raw
    }
}

/// Collect the top-level atoms of a display-math `MATH` body as [`MathPiece`]s,
/// collapsing trivia runs exactly as [`lower_math_seq`] does. Returns `None` —
/// signalling the caller to take the plain non-breaking path — when the body
/// holds a comment (a comment forces its own break, which does not compose with
/// the operator-break layout) or has fewer than two atoms (nothing to break).
fn collect_math_pieces(node: &SyntaxNode, cx: LowerCtx<'_>) -> Option<Vec<MathPiece>> {
    let mut pieces: Vec<MathPiece> = Vec::new();
    let mut prev_role = MathRole::Operand;
    let mut iter = node.children_with_tokens().peekable();
    while let Some(el) = iter.next() {
        match el {
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {
                consume_trivia_run(&t, &mut iter);
            }
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::COMMENT => return None,
            other => {
                let role = math_atom_role(&other, prev_role);
                prev_role = role;
                pieces.push(MathPiece {
                    ir: lower_math_element(other, cx),
                    role,
                });
            }
        }
    }
    (pieces.len() >= 2).then_some(pieces)
}

/// Lower a display-math `MATH` body, additionally letting a too-long body *break*
/// before its top-level binary/relation operators (amsmath style). The layout is
/// two-level: every top-level *relation* aligns in a single column (a chain of
/// `=` reads as a stack, the second `=` under the first), and a *binary* operator
/// hangs one relation-width deeper, under the first term of its right-hand side (a
/// `+`-chain tucks under the first summand). The left-hand side and the first
/// relation stay flat on the opening line. The whole body is one [`Ir::group`], so
/// it stays on a single line whenever it fits — degrading to [`lower_math_body`]
/// otherwise.
fn lower_display_math_body(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    let Some(pieces) = collect_math_pieces(node, cx) else {
        return lower_math_body(node, cx);
    };

    let flat_width = |ir: &Ir| {
        Printer::new(FormatStyle::default())
            .print_flat(ir)
            .chars()
            .count()
    };

    // With no top-level relation, continuation lines hang at the base indent: the
    // body simply breaks before each top-level binary operator.
    let Some(anchor) = pieces.iter().position(|p| p.role == MathRole::Relation) else {
        let mut parts: Vec<Ir> = Vec::with_capacity(pieces.len() * 2);
        for (i, piece) in pieces.iter().enumerate() {
            if i > 0 {
                let break_here =
                    piece.role == MathRole::Binary && pieces[i - 1].role == MathRole::Operand;
                parts.push(if break_here {
                    Ir::line()
                } else {
                    Ir::text(" ")
                });
            }
            parts.push(piece.ir.clone());
        }
        return Ir::group(Ir::concat(parts));
    };

    // The relation column: the left-hand side (atoms before the first relation)
    // sits flat on the opening line, and the first relation follows one space
    // later. Every top-level relation aligns here.
    let rel_col = if anchor == 0 {
        0
    } else {
        let head = Ir::join(Ir::text(" "), pieces[..anchor].iter().map(|p| p.ir.clone()));
        flat_width(&head) + 1
    };

    let mut parts: Vec<Ir> = Vec::new();
    // Left-hand side, flat on the opening line.
    for (i, piece) in pieces[..anchor].iter().enumerate() {
        if i > 0 {
            parts.push(Ir::text(" "));
        }
        parts.push(piece.ir.clone());
    }

    // Each relation opens a segment running to the next relation. The first
    // segment's relation stays on the opening line (one space after the LHS);
    // every later relation starts a fresh continuation line at `rel_col`. Inside a
    // segment, a break before a binary operator hangs one relation-width deeper,
    // under the first right-hand-side term.
    let mut i = anchor;
    let mut first_segment = true;
    while i < pieces.len() {
        if first_segment {
            if anchor > 0 {
                parts.push(Ir::text(" "));
            }
        } else {
            parts.push(Ir::line());
        }
        parts.push(pieces[i].ir.clone());
        let relw = flat_width(&pieces[i].ir);

        let start = i + 1;
        let mut j = start;
        while j < pieces.len() && pieces[j].role != MathRole::Relation {
            j += 1;
        }
        let mut rhs: Vec<Ir> = Vec::with_capacity((j - start) * 2);
        for k in start..j {
            let break_here =
                pieces[k].role == MathRole::Binary && pieces[k - 1].role == MathRole::Operand;
            rhs.push(if break_here {
                Ir::line()
            } else {
                Ir::text(" ")
            });
            rhs.push(pieces[k].ir.clone());
        }
        parts.push(Ir::align(relw + 1, Ir::concat(rhs)));

        first_segment = false;
        i = j;
    }

    Ir::group(Ir::align(rel_col, Ir::concat(parts)))
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
/// solely of *block-level* command(s) and inline whitespace — the unit
/// [`reflow_elements`] keeps on its own line rather than reflowing into its
/// neighbours. The line runs until the next newline, comment, or end of the stream;
/// any non-trivia element that is not a block command (a word, a control symbol, a
/// group, math, a `\\`, a block, or an *inline* command like `\citep`/`\ref` — see
/// [`command_is_inline`]) disqualifies it. A line with no command (e.g. an empty or
/// comment-only line) is not a command line.
fn line_is_command_only(elements: &[SyntaxElement], start: usize, cx: LowerCtx<'_>) -> bool {
    let mut saw_command = false;
    for element in &elements[start..] {
        match element {
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::NEWLINE => break,
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::COMMENT => break,
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::WHITESPACE => continue,
            SyntaxElement::Node(n)
                if n.kind() == SyntaxKind::COMMAND && !command_is_inline(n, cx) =>
            {
                saw_command = true
            }
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

#[cfg(test)]
mod expl3_region_tests {
    use super::*;
    use crate::parser::parse;

    /// The expl3 regions of `input`, as `(start, end)` byte pairs.
    fn regions(input: &str) -> Vec<(usize, usize)> {
        let parsed = parse(input);
        assert!(parsed.errors.is_empty(), "test input should parse cleanly");
        expl3_regions(&parsed.syntax())
            .into_iter()
            .map(|r| (r.start().into(), r.end().into()))
            .collect()
    }

    #[test]
    fn on_off_pair_spans_both_toggles() {
        let input = r"a \ExplSyntaxOn b \ExplSyntaxOff c";
        let start = input.find(r"\ExplSyntaxOn").unwrap();
        let end = input.find(r"\ExplSyntaxOff").unwrap() + r"\ExplSyntaxOff".len();
        assert_eq!(regions(input), vec![(start, end)]);
    }

    #[test]
    fn unclosed_region_runs_to_eof() {
        let input = r"x \ExplSyntaxOn y z";
        let start = input.find(r"\ExplSyntaxOn").unwrap();
        assert_eq!(regions(input), vec![(start, input.len())]);
    }

    #[test]
    fn provides_expl_opens_to_eof() {
        let input = "\\ProvidesExplPackage\n\\cs_new:N \\foo:";
        assert_eq!(regions(input), vec![(0, input.len())]);
    }

    #[test]
    fn stray_off_is_ignored() {
        assert!(regions(r"a \ExplSyntaxOff b").is_empty());
    }

    #[test]
    fn redundant_inner_on_does_not_restart() {
        let input = r"\ExplSyntaxOn a \ExplSyntaxOn b \ExplSyntaxOff";
        let end = input.find(r"\ExplSyntaxOff").unwrap() + r"\ExplSyntaxOff".len();
        assert_eq!(regions(input), vec![(0, end)]);
    }

    #[test]
    fn toggle_inside_verb_is_not_a_region() {
        // `\ExplSyntaxOn` inside a `\verb` argument lexes as a `VERB` token, never a
        // `CONTROL_WORD`, so it must not open a region (mirrors the lexer).
        assert!(regions(r"\verb|\ExplSyntaxOn| text").is_empty());
    }

    #[test]
    fn toggle_inside_comment_is_not_a_region() {
        assert!(regions("% \\ExplSyntaxOn\ntext").is_empty());
    }
}
