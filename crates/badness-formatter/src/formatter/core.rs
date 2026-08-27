//! The formatter entry points and the CST → [`Ir`] lowering.
//!
//! Lowering normalizes whitespace and indentation, reflows prose arguments under
//! [`WrapMode::Reflow`], formats structured math, and owns layout inside expl3
//! regions. Protected regions remain verbatim.
//!
//! [`lower_node`] contains the LaTeX-specific lowering. The surrounding format
//! entry points and the Wadler-style [`Ir`] printer are language-independent.

use std::cell::RefCell;
use std::collections::HashMap;
use std::iter::Peekable;

use rowan::{TextRange, TextSize};

use super::colspec::{self, ColAlign};
use crate::ast::{AstNode, Environment, Group, command_name};
use crate::declarations::ResolvedDeclarations;
use crate::directives;
use crate::parser::is_def_prefix_command;
use crate::parser::lexer::{ExplToggle, expl_toggle};
use crate::parser::{LatexFlavor, parse_with_declarations, parse_with_flavor};
use crate::semantic::expl3::{StatementMap, segment_expl_statements};
use crate::semantic::tikz::statement_glue;
use crate::semantic::{
    ArgKind, ArgumentDomain, ContentKind, DelimiterRole, MathClass, SignatureDb, Signatures, expl3,
    match_arg_slot, match_verbatim_arg_slot, math_atoms, scan_definitions,
};
use crate::syntax::{
    SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken, is_collapsible_trivia, is_param_digit,
};

use super::context::FormatContext;
use super::ir::Ir;
use super::printer::Printer;
use super::sentence::{ResolvedProfile, SentenceOptions, is_sentence_boundary_text};
use super::style::{
    FormatStyle, ItemIndent, LineEnding, MathWrap, WrapMode, apply_line_ending, detect_line_ending,
};

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
/// [`LexConfig`](crate::parser::LexConfig), so a [`Package`](LatexFlavor::Package) flavor (`.sty`/`.cls`)
/// lexes with `@` as a letter (the implicit `\makeatletter`) and a `.dtx` runs
/// the docstrip mode. A bare [`LatexFlavor`] coerces in. The wrap mode is a
/// `style` concern, decided by the caller and independent of the flavor; it
/// defaults to [`WrapMode::Reflow`] for every file kind.
pub fn format_with_style_flavored(
    input: &str,
    style: FormatStyle,
    config: impl Into<crate::parser::LexConfig>,
) -> Result<String, FormatError> {
    format_with_style_flavored_with_signatures(input, style, config, &SignatureDb::default())
}

/// Like [`format_with_style_flavored`] but with explicit
/// [`SentenceOptions`](crate::formatter::SentenceOptions) for the
/// `sentence`/`semantic` wrap modes (the document language and user no-break
/// abbreviations). The CLI resolves these from `badness.toml`; other wrap modes
/// ignore them.
pub fn format_with_style_flavored_sentence(
    input: &str,
    style: FormatStyle,
    config: impl Into<crate::parser::LexConfig>,
    sentence: SentenceOptions<'_>,
) -> Result<String, FormatError> {
    let parsed = parse_with_flavor(input, config);
    if !parsed.errors.is_empty() {
        return Err(FormatError::ParseErrors {
            count: parsed.errors.len(),
        });
    }
    format_node_with_signatures_sentence(&parsed.syntax(), style, &SignatureDb::default(), sentence)
}

/// Like [`format_with_style_flavored_sentence`] but under a project's
/// [declarations](crate::declarations), and with no other signature scope.
///
/// This is the entry for content with no path to anchor local `.sty`/`.cls`
/// resolution against: the CLI's **stdin**, the language server's
/// cache-miss/cancellation fallback, and the dprint plugin, which is sandboxed
/// with no filesystem at all. Each of those would otherwise reach for
/// [`format_with_style_flavored_sentence`], which parses declaration-blind — and
/// a formatter honoring `[environments.…]` for `badness format file.tex` but not
/// for `badness format < file.tex` (nor for the one editor request that races an
/// edit) would be a trap.
///
/// The declarations reach **both** the parse and the signature scope, exactly as
/// they do on the path-bearing entries in the `badness` crate
/// (`formatter::format_file_with_packages_sentence`), so this differs from the
/// full path only by the package tiers it cannot reach — never by precedence,
/// and never by which constructs it recognizes.
pub fn format_with_declarations_sentence(
    input: &str,
    style: FormatStyle,
    config: impl Into<crate::parser::LexConfig>,
    sentence: SentenceOptions<'_>,
    declared: &ResolvedDeclarations,
) -> Result<String, FormatError> {
    let parsed = parse_with_declarations(input, config, declared);
    if !parsed.errors.is_empty() {
        return Err(FormatError::ParseErrors {
            count: parsed.errors.len(),
        });
    }
    format_node_with_signatures_sentence(
        &parsed.syntax(),
        style,
        &declared_scope(declared),
        sentence,
    )
}

/// The signature scope a project's declarations alone make up: the top tier of
/// the disk- and salsa-backed scopes (`semantic::collect_package_signatures`,
/// `incremental::scope_signatures`, both in the `badness` crate), with nothing
/// under it.
pub fn declared_scope(declared: &ResolvedDeclarations) -> SignatureDb {
    let mut scope = SignatureDb::default();
    scope.merge_declarations(declared);
    scope
}

/// Like [`format_with_style_flavored`] but additionally folds an `external`
/// signature scope — the merged definitions of the document's loaded local
/// packages (`semantic::load::collect_package_signatures` and the salsa-cached
/// `incremental::scope_signatures`, both in the `badness` crate) — into the lowering, so calls to
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
/// salsa-cached `incremental::scope_signatures` (in the `badness` crate) here; the document's own
/// definitions always win over `external`. An empty DB recovers [`format_node`].
pub fn format_node_with_signatures(
    root: &SyntaxNode,
    style: FormatStyle,
    external: &SignatureDb,
) -> Result<String, FormatError> {
    format_node_with_signatures_sentence(root, style, external, SentenceOptions::default())
}

/// Like [`format_node_with_signatures`] but with explicit
/// [`SentenceOptions`](crate::formatter::SentenceOptions) for the
/// `sentence`/`semantic` wrap modes.
pub fn format_node_with_signatures_sentence(
    root: &SyntaxNode,
    style: FormatStyle,
    external: &SignatureDb,
    sentence: SentenceOptions<'_>,
) -> Result<String, FormatError> {
    validate_supported_tokens(root)?;

    let ctx = FormatContext::with_sentence(style, sentence);
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
    apply_line_ending(&mut formatted, resolve_line_ending(root, style));
    Ok(formatted)
}

/// Range formatting: lay out only the document-level blocks overlapping `range`,
/// returning the formatted text for the `[first block start, last block end]`
/// span. The caller (the `badness` crate's LSP) expands the editor selection to whole
/// document-level-block boundaries before calling, so `range` is already
/// block-aligned. Direct children of the canonical no-indent `document` environment
/// count as document-level blocks alongside children of `ROOT`.
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
    format_node_range_with_signatures_sentence(
        root,
        style,
        external,
        range,
        SentenceOptions::default(),
    )
}

/// Like [`format_node_range_with_signatures`] but with explicit
/// [`SentenceOptions`](crate::formatter::SentenceOptions) for the
/// `sentence`/`semantic` wrap modes.
pub fn format_node_range_with_signatures_sentence(
    root: &SyntaxNode,
    style: FormatStyle,
    external: &SignatureDb,
    range: TextRange,
    sentence: SentenceOptions<'_>,
) -> Result<String, FormatError> {
    validate_supported_tokens(root)?;

    let ctx = FormatContext::with_sentence(style, sentence);
    let mut formatted = format_root(root, ctx, external, Some(range));
    let trimmed_len = formatted.trim_end_matches([' ', '\t', '\n', '\r']).len();
    formatted.truncate(trimmed_len);
    // Detected from the whole document, not the fragment: the replacement has to
    // match the endings of the text it splices into, and a block that happens to
    // hold no line break of its own would otherwise answer `Lf`.
    apply_line_ending(&mut formatted, resolve_line_ending(root, style));
    Ok(formatted)
}

/// The concrete ending `style` calls for on this document ([`LineEnding::Auto`]
/// resolved against what the source used).
fn resolve_line_ending(root: &SyntaxNode, style: FormatStyle) -> LineEnding {
    if style.line_ending == LineEnding::Auto {
        style.line_ending.resolve(detect_line_ending(&root.text()))
    } else {
        style.line_ending.resolve(LineEnding::Lf)
    }
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
    user.merge_from(&scan_definitions(root), None);
    // The expl3 source regions, recomputed read-only from the same toggle set the
    // lexer uses ([`expl_toggle`]). Inside them source whitespace is catcode-9
    // (ignored) and `~` is catcode-10 (a literal space), so the formatter fully owns
    // layout. Held by value for the whole lowering, like `user`.
    let regions = expl3_regions(root);
    // The spans the author turned layout off over, resolved from this file's own
    // comment directives (see [`crate::directives`]). A pure function of the
    // tree, held by value for the whole lowering like `regions`. Empty for the
    // overwhelming majority of documents, in which case every query is free.
    let suppressed = directives::Suppressions::build(root);
    // The sentence-boundary profile for the `sentence`/`semantic` wrap modes,
    // resolved from the run's [`SentenceOptions`]. `Copy`, borrowing the merged
    // no-break slice `ctx` still owns for the whole call, so it rides `LowerCtx`
    // like the bare `wrap` mode. Never consulted under `reflow`/`preserve`.
    let profile = ctx.sentence().resolved();
    // The `.dtx` doc-paragraph reflow-safety memo (see [`DtxReflowCache`]),
    // owned here for the whole lowering like `user` and `regions`.
    let dtx_reflow_cache = DtxReflowCache::default();
    let cx = LowerCtx {
        wrap: ctx.style().wrap,
        item_indent: ctx.style().item_indent,
        indent_width: ctx.style().indent_width,
        // Resolved here (never `Auto` past this point), so library callers get the
        // derivation from `wrap` for free.
        math_wrap: ctx.style().math_wrap.resolve(ctx.style().wrap),
        stable_target: ctx.style().stable_wrap_target(),
        signatures: Signatures::new(&user),
        expl3_regions: &regions,
        suppressed: suppressed.format_ranges(),
        profile,
        range,
        dtx_reflow_cache: &dtx_reflow_cache,
        dtx_margin_probe: false,
        preserve_dtx_nested_layout: false,
        in_dtx_doc_region: false,
        in_alignment_cell: false,
        absorbed_control_newline: None,
        is_dtx: root
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| matches!(t.kind(), SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD)),
    };
    // Saturate `Group::expand` at the lowering->printer seam: after this, the
    // flag is the single representation of "forced open" the printer trusts.
    // Lowering-time `contains_forced_break` queries use the immutable summary
    // stored at each group boundary; this final pass still marks every nested
    // group for the printer in one bottom-up walk.
    let ir = lower_node(root, cx).propagate_breaks();
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

/// Whether `range` lies wholly inside the body of the canonical `document`
/// environment. Its body is the formatter's canonical no-indent case, so exposing
/// direct body blocks does not discard any ancestor indentation context.
fn document_body_contains(node: &SyntaxNode, range: TextRange) -> bool {
    let Some(environment) = Environment::cast(node.clone()) else {
        return false;
    };
    if environment.name().as_deref() != Some("document") {
        return false;
    }
    let (Some(begin), Some(end)) = (environment.begin(), environment.end()) else {
        return false;
    };
    range.start() >= begin.syntax().text_range().end()
        && range.end() <= end.syntax().text_range().start()
}

/// The nearest preceding sibling *element* of `node`, skipping `WHITESPACE`/`NEWLINE`
/// trivia. Used by the definee gate to find the command a toggle would be the
/// definee of.
fn prev_nontrivia_element(node: &SyntaxNode) -> Option<SyntaxElement> {
    let mut prev = node.prev_sibling_or_token();
    while let Some(el) = prev {
        if let SyntaxElement::Token(t) = &el
            && is_collapsible_trivia(t.kind())
        {
            prev = el.prev_sibling_or_token();
            continue;
        }
        return Some(el);
    }
    None
}

/// Whether an expl3 toggle `CONTROL_WORD` sits at *top-level statement* position, so
/// TeX actually executes it and switches catcodes at load. Two shapes are rejected
/// (issue #69), both false positives of the name-only model:
///
/// - **Definee position:** the toggle command's immediately-preceding non-trivia
///   sibling is a `\def`/`\let`-family primitive, so the toggle is the control
///   sequence being defined (`\protected\def\ProvidesExplPackage{…}`), never run.
/// - **Nested in a group / definition body:** an ancestor of the toggle's command is
///   a `GROUP` or `OPTIONAL`, so the toggle is tokenized into a replacement text and
///   executed — if ever — only when that macro runs, not at load.
///
/// The lexer's letter mode keeps the naive name-only model: mis-lexing a name only
/// splits CST tokens (lossless, cosmetic); only mis-*owning* layout rewrites meaning.
fn toggle_is_top_level(token: &SyntaxToken) -> bool {
    let Some(command) = token.parent() else {
        return true;
    };
    if command.kind() != SyntaxKind::COMMAND {
        // Not the head of a command node — leave the naive model in charge.
        return true;
    }
    // Rule 1: nested inside an attached group or definition body.
    for ancestor in command.ancestors().skip(1) {
        match ancestor.kind() {
            SyntaxKind::GROUP | SyntaxKind::OPTIONAL => return false,
            SyntaxKind::ROOT => break,
            _ => {}
        }
    }
    // Rule 2: definee of a `\def`/`\let`-family command. `command_name` strips the
    // leading `\`, so reconstruct it for the shared curated set.
    if let Some(SyntaxElement::Node(prev)) = prev_nontrivia_element(&command)
        && prev.kind() == SyntaxKind::COMMAND
        && let Some(name) = command_name(&prev)
        && (is_def_prefix_command(&format!("\\{name}"))
            || matches!(name.as_str(), "let" | "futurelet"))
    {
        return false;
    }
    true
}

/// The byte ranges of the document's expl3 regions, in document order. A region runs
/// from an opener (`\ExplSyntaxOn`, or a `\ProvidesExpl*` declaration, which opens
/// expl3 for the rest of the file) through the matching `\ExplSyntaxOff` (inclusive
/// of both toggle commands), or to end of input when unclosed. The toggle *name set*
/// is read from [`expl_toggle`] — the same fixed set the lexer flips its `expl_syntax`
/// flag on — but the formatter additionally applies a *positional* gate
/// ([`toggle_is_top_level`]): only a top-level toggle opens a formatter-owned region.
/// The name set stays shared so the two never drift; positional layout ownership
/// remains formatter-specific.
///
/// Matches only [`SyntaxKind::CONTROL_WORD`] tokens, so a `\ExplSyntaxOn` written
/// inside `\verb`/a comment (a `VERB`/`COMMENT` token, never a `CONTROL_WORD`) is
/// not a toggle, exactly as in the lexer. The CST is untouched.
///
/// `pub` so the linter (in the `badness` crate) shares the *same* region
/// computation (the `unclosed-math-delimiter` rule suppresses inside expl3
/// code), keeping the formatter and linter from drifting on what counts as an
/// expl3 region.
pub fn expl3_regions(root: &SyntaxNode) -> Vec<TextRange> {
    let mut regions: Vec<TextRange> = Vec::new();
    let mut open: Option<TextSize> = None;
    for token in root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SyntaxKind::CONTROL_WORD)
    {
        // Positional gate: a gated-out toggle is skipped entirely (`On` and `Off`
        // alike), so a stored or definee toggle neither opens nor closes a region.
        let toggle = match expl_toggle(token.text()) {
            Some(t) if toggle_is_top_level(&token) => t,
            _ => continue,
        };
        match toggle {
            // A redundant inner `\ExplSyntaxOn` does not restart the region (the
            // lexer's flag is an idempotent set-true).
            ExplToggle::On if open.is_none() => open = Some(token.text_range().start()),
            ExplToggle::On => {}
            ExplToggle::Off => {
                if let Some(start) = open.take() {
                    regions.push(TextRange::new(start, token.text_range().end()));
                }
                // A stray `\ExplSyntaxOff` with no open region is ignored (toggling
                // an already-false flag is a no-op), matching the lexer.
            }
        }
    }
    if let Some(start) = open.take() {
        // An unclosed region runs to end of input (the lexer's flag simply stays
        // true to EOF).
        regions.push(TextRange::new(start, root.text_range().end()));
    }
    let regions = intersect_macrocode_bodies(root, regions);
    let regions = subtract_doc_margin_lines(root, regions);
    subtract_guarded_line_runs(root, regions)
}

/// In a `.dtx` (any `DOC_MARGIN` token present), restrict the expl3 regions to
/// `macrocode`/`macrocode*` chunk *bodies* — the only lines docstrip extracts as
/// code. The margin subtraction below removes doc lines that carry their `%` in
/// column 0, but the doc part is not obliged to margin every line (a stray
/// `␣%` comment, issue #58): any non-chunk line is documentation regardless of
/// its first column, so relayout must never own it. A no-op for non-`.dtx`
/// documents, where an unmargined `\begin{macrocode}` is just an ordinary
/// user environment.
fn intersect_macrocode_bodies(root: &SyntaxNode, regions: Vec<TextRange>) -> Vec<TextRange> {
    if regions.is_empty()
        || !root
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == SyntaxKind::DOC_MARGIN)
    {
        return regions;
    }
    // `macrocode` never nests, so the bodies are disjoint and in document order.
    // A body runs from the end of the `\begin` frame to the start of the `\end`
    // frame's `\end` (or the chunk end when the frame is missing at EOF); the
    // frame line's own margin/whitespace inside that span is removed by the
    // margin subtraction pass.
    let bodies: Vec<TextRange> = root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::ENVIRONMENT)
        .filter_map(Environment::cast)
        .filter(|e| matches!(e.name().as_deref(), Some("macrocode" | "macrocode*")))
        .filter_map(|e| {
            let start = e.begin()?.syntax().text_range().end();
            let end = e
                .end()
                .map(|end| end.syntax().text_range().start())
                .unwrap_or_else(|| e.syntax().text_range().end());
            (start < end).then_some(TextRange::new(start, end))
        })
        .collect();
    let mut out = Vec::with_capacity(regions.len());
    let mut b = bodies.iter().peekable();
    for region in regions {
        while let Some(&&body) = b.peek() {
            if body.end() <= region.start() {
                b.next();
                continue;
            }
            if body.start() >= region.end() {
                break;
            }
            let start = region.start().max(body.start());
            let end = region.end().min(body.end());
            if start < end {
                out.push(TextRange::new(start, end));
            }
            if body.end() >= region.end() {
                break;
            }
            b.next();
        }
    }
    out
}

/// Remove every `.dtx` documentation line from the expl3 regions. In a `.dtx`,
/// an `\ExplSyntaxOn` in one `macrocode` chunk is regularly matched by the
/// `\ExplSyntaxOff` several chunks later, so the lexical region spans the
/// documentation in between — margined doc lines and the `%    \end{macrocode}`
/// frame lines themselves. Those lines are *not* expl3 code (at package-load
/// time they are `%` comments; the margin must stay in column 0), so relayout
/// must never own them: subtract each doc-margined line (its `DOC_MARGIN` opens
/// the line by construction — the lexer emits margins at line start only)
/// through its terminating newline. Code lines inside chunk bodies carry no
/// margin and stay in-region. A no-op for non-`.dtx` documents (no `DOC_MARGIN`
/// tokens, and the common all-code case short-circuits on the first hole scan).
fn subtract_doc_margin_lines(root: &SyntaxNode, regions: Vec<TextRange>) -> Vec<TextRange> {
    if regions.is_empty() {
        return regions;
    }
    // One pass over the leaves: a DOC_MARGIN opens a hole, the next NEWLINE
    // (inclusive) closes it.
    let mut holes: Vec<TextRange> = Vec::new();
    let mut hole_start: Option<TextSize> = None;
    for token in root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
    {
        match token.kind() {
            SyntaxKind::DOC_MARGIN => {
                hole_start.get_or_insert(token.text_range().start());
            }
            SyntaxKind::NEWLINE => {
                if let Some(start) = hole_start.take() {
                    holes.push(TextRange::new(start, token.text_range().end()));
                }
            }
            _ => {}
        }
    }
    if let Some(start) = hole_start.take() {
        holes.push(TextRange::new(start, root.text_range().end()));
    }
    subtract_holes(regions, &holes)
}

/// Remove every maximal run of two-or-more consecutive docstrip-guarded lines
/// (`%<…>…`) from the expl3 regions. A fully-guarded chunk (a docstrip release
/// block, `%<latexrelease>…%<latexrelease>\EndIncludeInRelease`, or any run of
/// `%<*name>`/`%<name>` lines) pins **every** line to column 0 by its guard, so
/// the expl3 block layout cannot own it: reflowing indents a delimiter or wraps a
/// long line off its guard, stranding code onto an *unguarded* line — a docstrip
/// meaning change (the code leaves its guard's scope) that also re-parses
/// differently, so the layout never reaches a fixed point (issue #72, latex2e
/// `ltcmdhooks.dtx`). Handing such a run to the generic (non-expl3) lowering
/// preserves it verbatim, which is idempotent and meaning-preserving.
///
/// The two-line threshold keeps an *isolated* guarded line (a lone `%<trace> …`
/// statement amid unguarded code, or a lone `%<*name>` block marker) in-region,
/// where it lays out on its own line as before — such a line's content stays on
/// its line and never strands. A no-op for non-`.dtx` documents (no `GUARD`
/// tokens).
fn subtract_guarded_line_runs(root: &SyntaxNode, regions: Vec<TextRange>) -> Vec<TextRange> {
    if regions.is_empty() {
        return regions;
    }
    // One pass over the leaves grouping guard-led source lines into runs: a line
    // is guard-led when its first token is a `GUARD`. A run of two or more
    // adjacent guard-led lines becomes one hole spanning them (each line reaches
    // through its terminating newline).
    let mut holes: Vec<TextRange> = Vec::new();
    let mut at_line_start = true;
    let mut line_is_guard = false;
    let mut cur_line_start = TextSize::new(0);
    // The run of consecutive guard-led lines in progress: (start, end, line count).
    let mut run: Option<(TextSize, TextSize, usize)> = None;
    fn flush(run: &mut Option<(TextSize, TextSize, usize)>, holes: &mut Vec<TextRange>) {
        if let Some((start, end, count)) = run.take()
            && count >= 2
        {
            holes.push(TextRange::new(start, end));
        }
    }
    for token in root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
    {
        if at_line_start {
            cur_line_start = token.text_range().start();
            line_is_guard = token.kind() == SyntaxKind::GUARD;
            at_line_start = false;
        }
        if token.kind() == SyntaxKind::NEWLINE {
            let line_end = token.text_range().end();
            if line_is_guard {
                match run {
                    // Adjacent to the run in progress (its end abuts this line's
                    // start): extend it.
                    Some((start, end, count)) if end == cur_line_start => {
                        run = Some((start, line_end, count + 1));
                    }
                    // A gap (a non-guarded line intervened) or the first guarded
                    // line: close any prior run and open a fresh one.
                    _ => {
                        flush(&mut run, &mut holes);
                        run = Some((cur_line_start, line_end, 1));
                    }
                }
            } else {
                flush(&mut run, &mut holes);
            }
            at_line_start = true;
        }
    }
    flush(&mut run, &mut holes);
    subtract_holes(regions, &holes)
}

/// Interval subtraction of `holes` from `regions`. Both lists must be sorted and
/// pairwise disjoint; the output is too (the region binary-search lookups rely on
/// that). Shared by the two region-subtraction passes above.
fn subtract_holes(regions: Vec<TextRange>, holes: &[TextRange]) -> Vec<TextRange> {
    if holes.is_empty() {
        return regions;
    }
    let mut out = Vec::with_capacity(regions.len());
    let mut h = holes.iter().peekable();
    for region in regions {
        let mut cursor = region.start();
        while let Some(&&hole) = h.peek() {
            if hole.end() <= cursor {
                h.next();
                continue;
            }
            if hole.start() >= region.end() {
                break;
            }
            if hole.start() > cursor {
                out.push(TextRange::new(cursor, hole.start()));
            }
            cursor = cursor.max(hole.end());
            if cursor >= region.end() {
                break;
            }
            h.next();
        }
        if cursor < region.end() {
            out.push(TextRange::new(cursor, region.end()));
        }
    }
    out
}

/// The state threaded through every lowering call: the active [`WrapMode`] plus the
/// per-document [`Signatures`] overlay (scanned definitions over the built-in DB)
/// that [`lower_begin`] consults for environment arity. `Copy`, so it passes by
/// value like the bare `wrap` mode it replaced.
#[derive(Clone, Copy)]
struct LowerCtx<'a> {
    wrap: WrapMode,
    /// How far list-item continuation lines sit from the `\item` column.
    item_indent: ItemIndent,
    /// One structural indentation step, used by [`ItemIndent::Indent`].
    indent_width: usize,
    /// Display-math break policy, pre-resolved against `wrap` in
    /// [`format_root`] — never [`MathWrap::Auto`] here.
    math_wrap: MathWrap,
    /// Soft equilibrium line target for [`WrapMode::Stable`]. Already derived and
    /// clamped against the hard width by [`FormatStyle::stable_wrap_target`].
    stable_target: usize,
    signatures: Signatures<'a>,
    /// Sorted, non-overlapping byte ranges of the document's expl3 regions (see
    /// [`expl3_regions`]). Inside these, source whitespace is catcode-9 (ignored)
    /// and `~` is catcode-10 (a literal space), so the formatter lays out the code
    /// itself — regardless of [`WrapMode`]. Borrowed from a `Vec` owned by
    /// [`format_root`], exactly like `signatures`.
    expl3_regions: &'a [TextRange],
    /// Sorted, non-overlapping byte ranges the author turned layout off over
    /// (`% badness-format off`/`skip`/`skip-file` and the combined `% badness`
    /// family — see [`crate::directives`]). Content overlapping one of these is
    /// reproduced byte-for-byte instead of laid out. Borrowed from a `Vec` owned
    /// by [`format_root`], exactly like `expl3_regions`.
    suppressed: &'a [TextRange],
    /// The sentence-boundary profile (built-in language plus user no-break
    /// abbreviations) for the [`WrapMode::Sentence`]/[`WrapMode::Semantic`] modes.
    /// `Copy`, borrowing the merged slice owned by [`format_root`]. Never consulted
    /// under [`WrapMode::Reflow`]/[`WrapMode::Stable`]/[`WrapMode::Preserve`], so an
    /// English default (see [`SentenceOptions::default`]) is harmless there.
    profile: ResolvedProfile<'a>,
    /// Range-formatting emission filter. When `Some`, only document-level blocks
    /// overlapping this byte range are lowered; the rest are skipped and never
    /// produce IR (see [`lower_node`]). These are children of [`SyntaxKind::ROOT`]
    /// or direct body children of the canonical no-indent `document` environment.
    /// `None` (the default) lowers the whole document. Every selected block still
    /// lowers in full, at its real indent-0 context, so the formatter stays the sole
    /// authority on layout.
    range: Option<TextRange>,
    /// Memo for [`dtx_doc_paragraph_reflows_safely`]. The answer is needed twice
    /// per `.dtx` doc paragraph — once by the paragraph's own lowering, once by
    /// [`margin_floats_into_paragraph`] deciding whether the floated leading `%`
    /// may be dropped — and computing it means lowering the paragraph, so without
    /// the memo a nested document pays for it repeatedly. Borrowed from
    /// [`format_root`] like `expl3_regions`.
    dtx_reflow_cache: &'a DtxReflowCache,
    /// Set while *probing* whether a `.dtx` doc paragraph reflows safely. The probe
    /// lowers the paragraph, and that lowering must not consult
    /// [`margin_floats_into_paragraph`] again — the two would recurse into each
    /// other, once per nesting level, until the stack runs out. Dropping the
    /// floated margin is an emission detail that cannot change whether the reflow
    /// escapes the margin, so the probe simply keeps it.
    dtx_margin_probe: bool,
    /// A structured `.dtx` documentation paragraph may normalize its margin
    /// frames, but nested inline constructs must not synthesize unmargined lines.
    preserve_dtx_nested_layout: bool,
    /// The current node is being lowered as virtual LaTeX from a fully margined
    /// `.dtx` documentation region. Its physical `DOC_MARGIN` tokens are omitted;
    /// one canonical margin is re-applied by the enclosing IR.
    in_dtx_doc_region: bool,
    /// The current node is part of a non-math alignment cell that must collapse
    /// to one source line. Lone trivia newlines soften to spaces even when parser
    /// attachment nests them inside a command; blank lines and structural block
    /// breaks remain forced and make the grid decline.
    in_alignment_cell: bool,
    /// The exact trailing `\\<newline>` control symbol whose newline is supplied
    /// by an enclosing block's closing frame. Keeping the token's backslash here
    /// while letting the existing structural [`Ir::hard_line`] spell its newline
    /// prevents a second, blank line before the closer (issue #141).
    absorbed_control_newline: Option<TextRange>,
    /// Whether the document carries any `.dtx` documentation margin at all — the
    /// cheap short-circuit for the no-`.dtx` majority, so gates that would
    /// otherwise walk back to the start of a physical line
    /// ([`doc_margin_opens_line`]) cost nothing in an ordinary `.tex` file.
    /// Computed once in [`format_root`].
    is_dtx: bool,
}

/// Memoized [`dtx_doc_paragraph_reflows_safely`] answers, keyed by paragraph node.
type DtxReflowCache = RefCell<HashMap<SyntaxNode, bool>>;

impl<'a> LowerCtx<'a> {
    /// Whether the active wrap mode lays out prose paragraphs at all (as opposed to
    /// [`WrapMode::Preserve`], which leaves authored breaks untouched). Reflow,
    /// sentence, and semantic all route prose through [`reflow_elements`]; the mode
    /// then decides how a completed run is rendered (width fill vs. sentences).
    fn wraps_prose(self) -> bool {
        matches!(
            self.wrap,
            WrapMode::Reflow | WrapMode::Stable | WrapMode::Sentence | WrapMode::Semantic
        )
    }

    /// Mark a body-final `\\<newline>` for absorption into its closing frame.
    /// If this body has no such token, preserve an outer body's marker while
    /// recursively lowering its children.
    fn absorbing_trailing_control_newline(self, body: &[SyntaxElement]) -> Self {
        let Some(token) = trailing_control_newline(body) else {
            return self;
        };
        Self {
            absorbed_control_newline: Some(token.text_range()),
            ..self
        }
    }

    fn absorbs_control_newline(self, token: &SyntaxToken) -> bool {
        self.absorbed_control_newline == Some(token.text_range())
    }

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

    /// Whether `range` lies wholly inside a directive-suppressed span, so
    /// whatever occupies it must be reproduced byte-for-byte instead of laid out.
    ///
    /// **Containment, not overlap**, and the difference is the whole granularity
    /// story. An `off`/`on` region is delimited by comments the author placed,
    /// not by CST boundaries, so it can begin halfway through a construct — and
    /// every construct it begins inside is an *ancestor* of the content it means
    /// to cover. Overlap would therefore suppress the outermost such ancestor:
    /// one directive anywhere in a document body suppresses the whole
    /// `document` environment, and with it the entire file. Containment picks
    /// the outermost node that fits *within* the region instead, so an ancestor
    /// merely straddling the boundary keeps descending and only the blocks the
    /// author actually enclosed are reproduced.
    ///
    /// A node straddling the boundary is laid out normally while its wholly
    /// enclosed children are still reproduced. That is a finer granularity than
    /// ruff's statement level, and it stays sound in the direction that matters:
    /// every byte the author enclosed is preserved.
    fn suppressed(self, range: TextRange) -> bool {
        self.suppressed.iter().any(|r| r.contains_range(range))
    }
}

/// Lower a CST node to IR. Most nodes lower generically (see
/// [`lower_element_stream`]); an [`SyntaxKind::ENVIRONMENT`] is special-cased to
/// indent its body (see [`lower_environment`]), and under each prose-wrapping mode a
/// [`SyntaxKind::PARAGRAPH`] is routed through its line policy (see
/// [`lower_paragraph_reflow`]). The [`LowerCtx`] (wrap mode + signature overlay) is
/// threaded through so it reaches every nested paragraph (including environment and
/// group bodies).
fn lower_node(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    // Directive-suppressed content is reproduced, never laid out. Checked first,
    // above every routing decision, so no arm below can claim a node the author
    // turned the formatter off over.
    //
    // `ROOT` is excluded because a whole-file `skip-file` covers it: suppressing
    // there would emit the document as one opaque blob, which is the right bytes
    // by luck but skips the emission filter a range format depends on. Excluded,
    // the same directive reaches every *child* instead, and both the full and the
    // ranged path fall out of the one mechanism.
    if node.kind() != SyntaxKind::ROOT && cx.suppressed(node.text_range()) {
        return Ir::verbatim(node.text().to_string());
    }
    if cx.is_dtx
        && node.kind() != SyntaxKind::ROOT
        && !inside_macrocode(node)
        && contains_indented_dtx_comment(node)
    {
        let padding = leading_indented_dtx_comment_padding(node)
            .map(Ir::verbatim)
            .unwrap_or(Ir::Nil);
        return Ir::concat([padding, Ir::verbatim(node.text().to_string())]);
    }
    if dtx_doc_region(node, cx) {
        let virtual_doc = LowerCtx {
            in_dtx_doc_region: true,
            preserve_dtx_nested_layout: false,
            ..cx
        };
        return Ir::doc_margin(lower_node(node, virtual_doc));
    }
    if cx.is_dtx
        && node.kind() == SyntaxKind::ENVIRONMENT
        && doc_margin_opens_line(node, cx)
        && (environment_begin_has_newline(node) || !is_margin_framed(node))
    {
        return Ir::verbatim(node.text().to_string());
    }
    if cx.preserve_dtx_nested_layout
        && matches!(
            node.kind(),
            SyntaxKind::COMMAND
                | SyntaxKind::ENVIRONMENT
                | SyntaxKind::GROUP
                | SyntaxKind::OPTIONAL
                | SyntaxKind::INLINE_MATH
                | SyntaxKind::DISPLAY_MATH
                | SyntaxKind::MATH
        )
    {
        return Ir::verbatim(node.text().to_string());
    }
    // A `.dtx` command that opens on a docstrip guard and absorbs later guard
    // tokens is one fully guarded physical-line construct.  Its first guard is
    // a sibling (the command range starts at the control sequence), while the
    // continuation guards are children attached by the parser.  Relaying the
    // node would therefore join those child guards to the preceding tokens and
    // strand the continuations on unguarded lines.  Preserve the command as one
    // opaque slice; an unguarded command with guarded arguments still takes the
    // ordinary expl3 path and keeps its width-driven layout.
    if node.kind() == SyntaxKind::COMMAND
        && doc_margin_opens_line(node, cx)
        && node
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == SyntaxKind::GUARD)
    {
        return Ir::verbatim(node.text().to_string());
    }
    // A guard-led paragraph can carry later guards as children while the first
    // guard remains a sibling. Reflowing that mixed shape treats the first
    // command as ordinary prose, then emits the next column-zero guard without
    // first closing the line, joining two docstrip variants. Preserve the whole
    // paragraph whenever its opening line and a continuation are guarded.
    if node.kind() == SyntaxKind::PARAGRAPH
        && doc_margin_opens_line(node, cx)
        && node
            .first_token()
            .is_some_and(|token| token.kind() == SyntaxKind::CONTROL_WORD)
        && node
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == SyntaxKind::GUARD)
    {
        return Ir::verbatim(node.text().to_string());
    }
    // A fully docstrip-guarded paragraph is the whole hole cut out of an
    // expl3 region. With no byte of the paragraph left in-region it never
    // reaches `lower_expl_paragraph`; the generic Preserve paragraph path
    // would otherwise normalize its whitespace and move guards off column 0.
    if node.kind() == SyntaxKind::PARAGRAPH {
        let text = node.text().to_string();
        if text_is_fully_guarded(&text) {
            // The surrounding environment owns the body-leading break; keeping
            // the paragraph's leading newline inside an opaque IR would give
            // the printer two competing boundary breaks and float the first
            // guard onto the frame line.
            return Ir::verbatim(text.trim_start_matches(['\r', '\n']));
        }
    }
    // Range-formatting emission filter: at the document root, lower only the
    // children (top-level blocks plus the trivia between them) overlapping the
    // requested range; skip the rest entirely. A canonical `document` environment
    // is transparent when the range lies wholly in its body: that body's layout is
    // deliberately flush with the root, so its direct children are equally safe
    // independent blocks. Other environments still lower in full. A `None` range
    // (the whole-document default) never reaches here.
    if let Some(range) = cx.range
        && node.kind() == SyntaxKind::ROOT
    {
        let mut filtered = Vec::new();
        for element in node
            .children_with_tokens()
            .filter(|el| ranges_overlap(range, el.text_range()))
        {
            if let SyntaxElement::Node(child) = &element
                && document_body_contains(child, range)
            {
                filtered.extend(
                    child
                        .children_with_tokens()
                        .filter(|el| ranges_overlap(range, el.text_range())),
                );
            } else {
                filtered.push(element);
            }
        }
        return Ir::concat(lower_element_stream(filtered.into_iter(), cx));
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
                // `Statements::Ignore`: within one command's attached arguments a
                // source newline is just catcode-9 whitespace, not a statement
                // boundary — the width fill alone decides the breaks. Otherwise a
                // fill-broken argument would read as a new statement on the next
                // pass and the layout would never reach a fixed point.
                let fill = lower_expl_code(node.children_with_tokens(), cx, Statements::Ignore);
                // A recognized conditional renders **all-or-nothing**: flat when the
                // whole call fits, else the R4/R5 explosion. Left to the fill above,
                // the branch groups are independent atoms, so an overflow hangs only
                // the last one and splits the branch list across two indents —
                // `\…:nTF {c} {T}` / `{F}`, which reads as a continuation of the
                // enclosing statement rather than as the false branch.
                //
                // Attached to the *node*, not to statement position. The two
                // position-keyed paths in `lower_expl_code` (statement-leading,
                // trailing) additionally join the head, but both are gated off inside
                // a *fallback* statement — deliberately, since whether a conditional
                // name sits trailing there depends on where the line's junk ends,
                // which is not pass-invariant (xtemplate's spliced
                // `cs_ \str_if_eq:nnT … set:Npn` name assembly). The node is the node
                // on every pass, so this carries no such question and covers the
                // fallback case the other two decline.
                if let Some(exploded) = command_name(node)
                    .and_then(|name| expl3::conditional_branches(&name))
                    .and_then(|n| lower_expl_conditional(node, cx, n))
                {
                    return Ir::group(Ir::if_break(fill, exploded));
                }
                return fill;
            }
            _ => {}
        }
    }
    match node.kind() {
        // A `.dtx` documentation-layer prose paragraph (its first content token is
        // a `DOC_MARGIN`): reflow the bare prose and re-emit a `% ` margin on each
        // wrapped line. Checked before the generic paragraph reflow so the margin
        // is stripped and re-synthesized rather than glued into the fill.
        SyntaxKind::PARAGRAPH
            if !cx.in_dtx_doc_region && cx.wraps_prose() && is_dtx_doc_paragraph(node) =>
        {
            return lower_dtx_doc_paragraph(node, cx);
        }
        SyntaxKind::PARAGRAPH if cx.wraps_prose() => {
            return lower_paragraph_reflow(node, cx);
        }
        // Under `Preserve` a (non-`.dtx`) prose paragraph keeps its authored line
        // breaks, but inter-word spacing on each line still normalizes to a single
        // space (see [`lower_prose_stream`]) — `Preserve` governs line breaks only.
        // Inline-prose command bodies (`\emph{…}`) are flattened in so their text
        // collapses too, matching every wrapping mode; opaque argument bodies (a
        // `\newcommand` definition) recurse through [`lower_node`] and stay verbatim.
        // A `.dtx` doc paragraph is excluded (the arm above requires `wraps_prose`),
        // so it falls through to the generic stream and keeps its `%` margins.
        SyntaxKind::PARAGRAPH if cx.wrap == WrapMode::Preserve && !is_dtx_doc_paragraph(node) => {
            let flat = flatten_inline_prose(
                flatten_statements(node.children_with_tokens().collect()),
                cx,
                false,
            );
            return Ir::concat(lower_prose_stream(flat.into_iter(), cx));
        }
        // A `.dtx` docstrip frame (`%␣␣␣␣\begin{macrocode}`, a documentation-layer
        // `% \begin{itemize}`): the body is never indented and the closing frame is
        // kept whole at column 0. Routed before the alignment/list lowerers so a
        // margin-framed environment never reaches a layout that would reindent its
        // frame margins. A *math* environment is excluded: a `bmatrix`/`array` whose
        // `\begin` happens to open a `%␣␣␣␣` line inside a `% \[…\]` doc-math block
        // (l3backend-draw.dtx, l3ldb.dtx) is doc-comment prose, not a docstrip frame —
        // margin-framing it re-breaks `\end{bmatrix}` off its `%` margin and leaves the
        // block's `\]` unparseable on pass 2. It falls through to the generic stream,
        // which keeps the authored margins verbatim (same margin rule as the math /
        // group / optional arms below).
        SyntaxKind::ENVIRONMENT
            if !cx.in_dtx_doc_region
                && !has_verbatim_body(node)
                && is_margin_framed(node)
                && !is_math_env(node, cx) =>
        {
            return lower_margin_framed_environment(node, cx);
        }
        // A named math environment (`equation`, `align`, `gather`, matrix, …) — its
        // body is a `MATH` node (the parser entered math mode). Checked before the
        // generic alignment arm so a math *grid* (`align`/`pmatrix`, both `math` and
        // `align`) takes the math-aware path; a non-math grid (`tabular`,
        // `align` but not `math`) still falls through to `lower_aligned_environment`.
        // The `contains_doc_margin` gate is the same margin rule as the generic
        // arm below: a `bmatrix` nested in a `% \[…\]` doc-math block (l3backend-draw.dtx)
        // must keep its `%` margins verbatim, or the re-broken `\end{bmatrix}` drops
        // off column 0 and the block's closing `\]` is unparseable on pass 2.
        SyntaxKind::ENVIRONMENT
            if !has_verbatim_body(node)
                && !cx.in_dtx_doc_region
                && is_math_env(node, cx)
                && !contains_doc_margin(node, cx) =>
        {
            return lower_math_environment(node, cx);
        }
        // A grid inside a fully owned virtual `.dtx` documentation region lays
        // out after its physical margins have been stripped. The enclosing
        // `Ir::doc_margin` then restores the prefix at column zero, outside the
        // grid's padding. Other margin-carrying grids still decline through
        // `contains_doc_margin`.
        SyntaxKind::ENVIRONMENT
            if !has_verbatim_body(node)
                && is_alignment_env(node, cx)
                && !contains_doc_margin(node, cx) =>
        {
            return lower_aligned_environment(node, cx);
        }
        // A list environment (`itemize`/`enumerate`/`description`): its `\item`s get
        // their continuation lines hanging-indented under the marker. Under a
        // prose-wrapping mode the body is reflowed; under `Preserve` the authored
        // breaks and inner spacing are kept byte-faithful and only the continuation
        // *indentation* is re-hung (see [`lower_item_chunks`]). A doc-margined list
        // under `Preserve` is excluded — like the generic arm below, its `%` margins
        // must stay pinned at column 0 — so it falls through to the generic stream.
        SyntaxKind::ENVIRONMENT
            if !has_verbatim_body(node)
                && is_list_env(node, cx)
                && (cx.wraps_prose() || !contains_doc_margin(node, cx)) =>
        {
            return lower_list_environment(node, cx);
        }
        // A user-defined or otherwise unclassified environment whose body carries a
        // top-level `&` reads as an alignment (`myaligned`, issue #84): `&` at
        // catcode 4 is a column tab, a static CST-shape fact. Known align/math/list
        // environments were routed by the arms above; this generalizes `&`-column
        // layout to the environments the signature DB cannot name, exactly as the
        // environment group-boundary gate generalizes the curated definition-body set
        // Whitespace-only (the grid renderer reflows only trivia) and
        // self-correcting: any shape the grid cannot lay out falls back to
        // [`lower_environment`]. Doc-margined bodies are excluded (same margin rule as
        // the arms above and below), except inside a fully owned virtual region,
        // where `contains_doc_margin` is false and framing is stripped before grid
        // layout.
        SyntaxKind::ENVIRONMENT
            if !has_verbatim_body(node)
                && !contains_doc_margin(node, cx)
                && body_has_top_level_ampersand(node) =>
        {
            return lower_aligned_environment(node, cx);
        }
        // Same margin rule as the math/group/optional arms below: an environment
        // continuing across `.dtx` doc-margined lines is never re-laid. A
        // *margin-framed* environment (its `\begin`/`\end` on `%` frame lines) took
        // the `is_margin_framed` arm above; what reaches here is an environment
        // merely *nested* in doc-margined prose — an `array` inside a `% \[…\]`
        // display-math block (l3color.dtx). Re-breaking its `\begin`/body/`\end`
        // onto fresh lines would push `\end{array}` off its `%` margin, a meaning
        // change (a column-0 line stops being a comment at package-load time) that
        // leaves the orphaned `\]` unparseable on pass 2. The generic stream keeps
        // the authored margins verbatim.
        SyntaxKind::ENVIRONMENT if !has_verbatim_body(node) && !contains_doc_margin(node, cx) => {
            return lower_environment(node, cx);
        }
        // Same margin rule as the environment arm: a conditional spanning `.dtx`
        // doc-margined lines is never re-laid, since moving a divider off its `%`
        // margin is a meaning change. The generic stream keeps margins pinned.
        SyntaxKind::CONDITIONAL if !contains_doc_margin(node, cx) => {
            return lower_conditional(node, cx);
        }
        // Same margin rule as the environment/math/group/optional arms: a command
        // whose argument continues across `.dtx` doc-margined lines
        // (`% \title{^^A\n%   …}`) is never re-laid. Reflowing a managed argument
        // breaks its body onto fresh lines, which drops the `%` margin — and on an
        // unmargined line a `^^A` doc comment re-lexes as content, so the layout
        // stops being whitespace-only and pass 2 no longer parses. The generic
        // stream keeps the authored margins verbatim.
        SyntaxKind::COMMAND
            if (command_has_math_arg(node, cx)
                || cx.wraps_prose() && command_has_managed_arg(node, cx))
                && !contains_doc_margin(node, cx) =>
        {
            return lower_command(node, cx);
        }
        // Like the multi-line group below, math continuing across ordinary `.dtx`
        // doc-margined lines is never re-laid: math relayout would move a `%`
        // margin off column 0. A virtual doc region is different—its shared CST
        // view strips physical framing before specialized math lowering, and the
        // region wrapper regenerates the margins afterward.
        SyntaxKind::INLINE_MATH if !contains_doc_margin(node, cx) => {
            return lower_math(node, cx);
        }
        SyntaxKind::DISPLAY_MATH if !contains_doc_margin(node, cx) => {
            return lower_display_math(node, cx);
        }
        // A bare `MATH` node inside a virtual environment may be a grid cell;
        // its enclosing grid owns separators and row boundaries. Complete
        // inline/display nodes enter their math lowerers through the two arms
        // above and do not need this fallback.
        SyntaxKind::MATH if !cx.in_dtx_doc_region && !contains_doc_margin(node, cx) => {
            return lower_math_body(node, cx);
        }
        // A `.dtx` doc-layer group continuing across margined lines
        // (`\changes{…}{…\n%  …}`) is excluded: re-laying it out would move
        // content off its `%` margin — a meaning change (the line stops being a
        // comment at package-load time). Such a group falls through to the
        // generic stream, which keeps the authored margins verbatim.
        SyntaxKind::GROUP if !contains_doc_margin(node, cx) => {
            // Width-driven Opaque layout under the default mode: block-vs-inline
            // is decided by width, content, and preserved predicates — never by
            // whether the author happened to break the line. A group *opening
            // on* a doc-margined line holds no margin token of its own, so it
            // stays on the residue path below (a width break would land content
            // off its margin — the same gate `lower_optional` carries).
            if matches!(cx.wrap, WrapMode::Reflow) && !doc_margin_opens_line(node, cx) {
                return lower_opaque_group(node, cx);
            }
            // Tier-2 residue (the non-`Reflow` modes and the margined-line
            // corner): the pre-existing behaviour, byte for byte — block form
            // when the author broke the line, the generic inline stream below
            // otherwise. Fixed-point argument on [`spans_multiple_lines`].
            if spans_multiple_lines(node) {
                return lower_bracketed(node, SyntaxKind::L_BRACE, SyntaxKind::R_BRACE, cx, false);
            }
        }
        // Same margin rule as the group above: a `[…]` continuing across
        // doc-margined lines keeps its authored margins.
        // No signature context on the generic path, so no keyval proof: only gaps
        // the author already wrote are break opportunities.
        SyntaxKind::OPTIONAL if !contains_doc_margin(node, cx) => {
            if let Some(ir) = lower_optional(node, cx, false) {
                return ir;
            }
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
///
/// A paragraph in a `statementBody` environment is *not* prose and takes
/// [`ReflowKind::Statement`] instead — see [`in_statement_body_env`].
fn lower_paragraph_reflow(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    reflow_elements(
        node.children_with_tokens(),
        cx,
        paragraph_reflow_kind(node, cx),
    )
}

/// [`ReflowKind::Statement`] for a paragraph in a `statementBody` environment,
/// [`ReflowKind::Prose`] otherwise. Shared by [`lower_paragraph_reflow`] and the
/// `\begin`-tail splice in [`lower_env_body`], so a header the greedy parser
/// over-attached lays out under the same rule as the body it is spliced into.
fn paragraph_reflow_kind(node: &SyntaxNode, cx: LowerCtx<'_>) -> ReflowKind {
    if in_statement_body_env(node, cx) {
        ReflowKind::Statement
    } else {
        ReflowKind::Prose
    }
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
    // The paragraph's first content token, descending into child nodes: a
    // paragraph that *opens* with a command (`%<package>\def\x{1}` after a guard)
    // is not doc prose just because a later line carries a margin. Walking only
    // direct child tokens would skip the opening `COMMAND` and read that later
    // margin, wrapping guarded code in a `% ` margin that comments it out.
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

/// Whether a `.dtx` doc paragraph's *first* content token sits on a margined line —
/// either it is the `DOC_MARGIN` itself, or one precedes it on its line.
///
/// [`is_dtx_doc_paragraph`] is deliberately looser: it accepts a paragraph whose
/// margin appears on any later line, because such a paragraph *is* documentation
/// and its margins must still be respected. But the `DtxProse` reflow re-emits a
/// canonical `% ` on *every* line it produces, so a paragraph whose first line is
/// unmargined would gain a `%` it never had — turning code into a comment
/// (`%<package>\def\x{1}`, where the paragraph opens after a guard). Such a
/// paragraph takes the byte-faithful stream instead.
fn dtx_paragraph_starts_margined(node: &SyntaxNode) -> bool {
    node.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !is_collapsible_trivia(t.kind()))
        .is_some_and(|t| t.kind() == SyntaxKind::DOC_MARGIN || margin_precedes_on_line(&t))
}

/// Whether `node` is a complete, fully margined `.dtx` documentation block that
/// can be formatted as ordinary virtual LaTeX. The physical `%` prefixes are
/// trivia in the CST, but every generated line must regain one; admitting only a
/// line-owning block makes that prefix scope exact.
fn dtx_doc_region(node: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    if !cx.is_dtx
        || cx.in_dtx_doc_region
        || node.kind() != SyntaxKind::ENVIRONMENT
        || has_verbatim_body(node)
        || node
            .descendants()
            .filter_map(Environment::cast)
            .any(|env| matches!(env.name().as_deref(), Some("verbatim" | "verbatim*")))
        || node.text().to_string().contains("\\begin{verbatim")
        || node
            .descendants()
            .filter(|child| child.kind() == SyntaxKind::BEGIN)
            .any(|begin| {
                begin.descendants_with_tokens().any(|element| {
                    element
                        .into_token()
                        .is_some_and(|token| token.kind() == SyntaxKind::NEWLINE)
                })
            })
        || node.descendants_with_tokens().any(|element| {
            element.into_token().is_some_and(|token| {
                matches!(token.kind(), SyntaxKind::GUARD | SyntaxKind::COMMENT)
            })
        })
        || Environment::cast(node.clone()).is_some_and(|environment| {
            matches!(
                environment.name().as_deref(),
                Some("macrocode" | "macrocode*")
            )
        })
    {
        return false;
    }

    let Some(first) = node.first_token() else {
        return false;
    };
    let first_is_margined =
        first.kind() == SyntaxKind::DOC_MARGIN || line_prefix_is_doc_margin(first.prev_token());
    if !first_is_margined {
        return false;
    }

    // The node must own the rest of its closing line. Otherwise a break inside
    // the region could leave following documentation outside the prefix scope.
    let mut next = node.last_token().and_then(|token| token.next_token());
    while let Some(token) = next {
        match token.kind() {
            SyntaxKind::WHITESPACE => next = token.next_token(),
            SyntaxKind::NEWLINE => break,
            _ => return false,
        }
    }

    // Every continuation line must carry its own physical margin in the source.
    // This excludes mixed doc/code constructs and macrocode bodies without
    // needing a semantic guess about where their layers change.
    let tokens: Vec<SyntaxToken> = node
        .descendants_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .collect();
    tokens.iter().enumerate().all(|(index, token)| {
        token.kind() != SyntaxKind::NEWLINE
            || tokens
                .get(index + 1)
                .is_some_and(|next| next.kind() == SyntaxKind::DOC_MARGIN)
    })
}

fn environment_begin_has_newline(node: &SyntaxNode) -> bool {
    Environment::cast(node.clone())
        .and_then(|environment| environment.begin())
        .is_some_and(|begin| {
            begin.syntax().descendants_with_tokens().any(|element| {
                element
                    .into_token()
                    .is_some_and(|token| token.kind() == SyntaxKind::NEWLINE)
            })
        })
}

/// Whether the start of the current physical line, walking backward over only
/// source padding, is a documentation margin.
fn line_prefix_is_doc_margin(mut token: Option<SyntaxToken>) -> bool {
    while let Some(current) = token {
        match current.kind() {
            SyntaxKind::WHITESPACE => token = current.prev_token(),
            SyntaxKind::DOC_MARGIN => return true,
            _ => return false,
        }
    }
    false
}

/// Whether `margin` is the prefix immediately preceding a virtual documentation
/// region. The region IR re-emits the canonical margin, so this source prefix and
/// its padding must be omitted.
fn margin_starts_dtx_doc_region(margin: &SyntaxToken, cx: LowerCtx<'_>) -> bool {
    if margin.kind() != SyntaxKind::DOC_MARGIN || cx.in_dtx_doc_region {
        return false;
    }
    let mut next = margin.next_sibling_or_token();
    while let Some(SyntaxElement::Token(token)) = &next {
        if token.kind() == SyntaxKind::WHITESPACE {
            next = token.next_sibling_or_token();
        } else {
            break;
        }
    }
    let Some(SyntaxElement::Node(node)) = next else {
        return false;
    };
    if dtx_doc_region(&node, cx) {
        return true;
    }
    // A blank doc line ends the preceding paragraph, so the margin can be a
    // root sibling of a paragraph that starts with the virtual environment.
    // The same paragraph may continue with prose after `\end{...}`; that later
    // content does not change ownership of the opener's physical margin.
    if node.kind() != SyntaxKind::PARAGRAPH {
        return false;
    }
    node.children_with_tokens()
        .find(|element| {
            !matches!(
                element,
                SyntaxElement::Token(token) if is_collapsible_trivia(token.kind())
            )
        })
        .and_then(SyntaxElement::into_node)
        .is_some_and(|child| dtx_doc_region(&child, cx))
}

/// Whether `comment` is an ordinary own-line comment kept out of column zero by
/// authored indentation. In `.dtx`, removing that indentation changes even a
/// bare `%` from `COMMENT` to `DOC_MARGIN`, so the enclosing layout unit is
/// opaque.
fn is_indented_dtx_comment(comment: &SyntaxToken) -> bool {
    comment.kind() == SyntaxKind::COMMENT
        && comment
            .prev_token()
            .filter(|previous| previous.kind() == SyntaxKind::WHITESPACE)
            .and_then(|whitespace| whitespace.prev_token())
            .is_some_and(|previous| previous.kind() == SyntaxKind::NEWLINE)
}

fn contains_indented_dtx_comment(node: &SyntaxNode) -> bool {
    node.descendants_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .any(|comment| is_indented_dtx_comment(&comment))
}

/// Recover the indentation token that sits just outside an opaque node beginning
/// with an indented `.dtx` comment. Generic gap lowering owns that token and would
/// otherwise erase it before the node's verbatim bytes are emitted.
fn leading_indented_dtx_comment_padding(node: &SyntaxNode) -> Option<String> {
    let comment = node.first_token()?;
    if !is_indented_dtx_comment(&comment) {
        return None;
    }
    comment
        .prev_token()
        .filter(|previous| previous.kind() == SyntaxKind::WHITESPACE)
        .map(|whitespace| whitespace.text().to_string())
}

fn inside_macrocode(node: &SyntaxNode) -> bool {
    node.ancestors()
        .filter_map(Environment::cast)
        .any(|environment| {
            matches!(
                environment.name().as_deref(),
                Some("macrocode" | "macrocode*")
            )
        })
}

/// Whether the physical line `node` starts on opens with a `.dtx` documentation
/// margin or docstrip guard — i.e. everything on it is documentation (or guarded
/// code) that docstrip anchors at column 0.
///
/// Broader than [`margin_precedes_on_line`], which only accepts a margin
/// *immediately* before its token: here the margin may be arbitrarily far back
/// (`% \begin{function}[EXP, pTF]{…}`). A construct that introduces its own line
/// break must consult this, because a break it emits lands on a line the doc layer
/// never margined — turning documentation into live code. [`contains_doc_margin`]
/// cannot see this: the margin sits *outside* the node, before it on the line.
fn doc_margin_opens_line(node: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    if !cx.is_dtx || cx.in_dtx_doc_region {
        return false;
    }
    let mut prev = node.first_token().and_then(|t| t.prev_token());
    while let Some(t) = prev {
        match t.kind() {
            SyntaxKind::NEWLINE => return false,
            SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD => return true,
            _ => prev = t.prev_token(),
        }
    }
    false
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
fn margin_floats_into_paragraph(margin: &SyntaxToken, cx: LowerCtx<'_>) -> bool {
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
                && dtx_doc_paragraph_reflows_safely(&n, cx)
    )
}

/// Lower a `.dtx` documentation paragraph under [`WrapMode::Reflow`]. When the
/// paragraph is pure running prose ([`dtx_paragraph_reflows`]) the bare prose is
/// reflowed to the line width via [`reflow_elements`] in [`ReflowKind::DtxProse`]
/// mode, which drops each line's `%` margin and re-emits a canonical `% ` margin
/// on every reflowed line (see [`Ir::margin_prefix`]). A complete virtual
/// documentation environment may participate as a self-margin-owning block, so
/// prose on either side keeps reflowing; other paragraphs that contain or sit
/// inside an environment (a `macrocode` block or a `macro`/`environment` doc block)
/// are lowered *preserve-style* so frame margins and item lines round-trip
/// byte-for-byte.
///
/// [`dtx_paragraph_reflows`] is a cheap up-front gate; the exact one is the reflow
/// itself. A forced-break block whose interior lines ride their own margins is
/// committed raw under a canonical first-line margin
/// ([`LineBuilder::push_margined_block`]), and a clean guard line becomes its own
/// column-0 segment ([`collect_guard_line`]), so both reflow with the prose around
/// them. A paragraph whose reflow still commits content *outside* the `% ` margin
/// (a block with an unmargined interior line, a guard line that cannot be
/// isolated — see [`LineBuilder::margin_escaped`]) is re-lowered on the preserve
/// path instead: on an unmargined line a `.dtx` doc comment re-lexes as content,
/// so keeping that layout would break the whitespace-only invariant. The gate
/// reads content only, never [`LowerCtx::wrap`], so `--wrap reflow` on a `.dtx`
/// is exactly as safe as any other mode.
fn lower_dtx_doc_paragraph(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    if dtx_doc_paragraph_reflows_safely(node, cx) {
        reflow_elements(node.children_with_tokens(), cx, ReflowKind::DtxProse)
    } else {
        // Margin frames still normalize, but nested inline constructs stay
        // opaque: a width break inside one would create an unmargined line.
        let preserve = LowerCtx {
            preserve_dtx_nested_layout: true,
            ..cx
        };
        Ir::concat(lower_element_stream(node.children_with_tokens(), preserve))
    }
}

/// Whether a `.dtx` documentation paragraph may be reflowed: it is unstructured
/// ([`dtx_paragraph_reflows`]) *and* the reflow keeps every line under the `% `
/// margin ([`LineBuilder::margin_escaped`]). The second half is exact rather than
/// syntactic, so it is answered by running the reflow and throwing the layout away.
///
/// [`margin_floats_into_paragraph`] needs the same answer — a floated leading `%`
/// may only be dropped when the paragraph really does re-emit a canonical margin —
/// so both go through here, memoized per node, and always under the *probing*
/// context so the two callers cannot disagree.
fn dtx_doc_paragraph_reflows_safely(node: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    if !dtx_paragraph_reflows(node, cx) || !dtx_paragraph_starts_margined(node) {
        return false;
    }
    if let Some(&answer) = cx.dtx_reflow_cache.borrow().get(node) {
        return answer;
    }
    // Answer every *nested* doc paragraph first, innermost-first (reverse
    // pre-order visits a node before its ancestors). The probe below lowers this
    // paragraph in full, so an unanswered nested paragraph would start a probe
    // inside a probe — doubling stack depth and duplicating work at every nesting
    // level. Warmed bottom-up, each nested probe is a cache hit instead.
    let nested: Vec<SyntaxNode> = node
        .descendants()
        .filter(|d| d != node && d.kind() == SyntaxKind::PARAGRAPH && is_dtx_doc_paragraph(d))
        .collect();
    for descendant in nested.into_iter().rev() {
        dtx_doc_paragraph_reflows_safely(&descendant, cx);
    }
    let probe = LowerCtx {
        dtx_margin_probe: true,
        ..cx
    };
    let (_, margin_escaped) =
        reflow_elements_checked(node.children_with_tokens(), probe, ReflowKind::DtxProse);
    let answer = !margin_escaped;
    cx.dtx_reflow_cache
        .borrow_mut()
        .insert(node.clone(), answer);
    answer
}

/// Whether a `.dtx` documentation paragraph has only structures that can reflow
/// under its canonical margin. A direct, fully margin-owned environment composes
/// as a self-owning block; an environment hidden inside another child, an unsafe
/// direct environment, or an enclosing environment keeps the paragraph on the
/// byte-faithful preserve path.
fn dtx_paragraph_reflows(node: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    !node
        .ancestors()
        .any(|ancestor| ancestor.kind() == SyntaxKind::ENVIRONMENT)
        && node.children().all(|child| {
            if child.kind() == SyntaxKind::ENVIRONMENT {
                dtx_doc_region(&child, cx)
            } else {
                !child
                    .descendants()
                    .any(|descendant| descendant.kind() == SyntaxKind::ENVIRONMENT)
            }
        })
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
/// continues after. Ordinary blocks are joined by [`Ir::hard_line`]; a sectioning
/// command that is a direct child of a prose paragraph uses [`Ir::empty_line`] on
/// both sides. Adjacent `\label` commands remain attached below that heading, with
/// the trailing empty line deferred until after the label run.
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
    /// A signature-proven prose *argument* body ([`lower_prose_group`]): like
    /// [`Self::Prose`], but the command-only-line preservation does not apply —
    /// width alone owns the layout. Preserving a command-only line here turns a
    /// width break of pass 1 into a *forced* break on pass 2, and that force
    /// bit leaks upward through every `contains_forced_break` reader (the
    /// opaque-group and optional declines, `collapse_arg_group`,
    /// `finish_cell`), flipping the enclosing construct between its inline and
    /// block forms across passes (pgf's `\emph{… \href{…} …}` table headers).
    /// The residue's own fixed-point argument covers the *interior* refill,
    /// not the propagated bit, so inside a width-owned argument the rule must
    /// not fire at all.
    ProseArg,
    /// Code-like statements (a `\newcommand` definition body, a picture body's
    /// fallback content): a lone newline ends the line, so each source line stays
    /// its own logical line; only width forces a wrap. Flush continuation keeps
    /// the wrap idempotent (a wrapped tail re-parses as a line already at the
    /// body indent). In a curated `statementBody` *environment* body under
    /// [`WrapMode::Reflow`], `STATEMENT` nodes are lowered structurally instead
    /// ([`lower_statement`]: boundaries from the node, continuations hung) and
    /// this authored-line contract governs only the interleaved content no `;`
    /// terminates.
    Statement,
    /// The interior of one structural `STATEMENT` ([`lower_statement`]): like
    /// [`Self::ProseArg`] — width owns the layout, a lone newline is a plain
    /// atom boundary, and the command-only residue is off — but the gaps
    /// additionally consult the TikZ unit model
    /// (`semantic::tikz::statement_glue`): a unit-internal gap (`-- (1,1)`,
    /// `at (2,3)`, `circle (3)`) renders as a single space and never breaks,
    /// so a width wrap lands only at unit boundaries. The verdicts read
    /// non-trivia token text only, so this stays Tier 1: a wrap re-derives
    /// the same units on every pass.
    StatementInterior,
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

/// One committed atom of a logical-line run: the printed [`Ir`] plus the atom's
/// source `text`, retained so the [`WrapMode::Sentence`]/[`WrapMode::Semantic`]
/// renderer can run sentence-boundary detection over the words. Under
/// [`WrapMode::Reflow`]/[`WrapMode::Stable`] only the layout fields are used.
struct RunAtom {
    ir: Ir,
    text: String,
    /// Whether the gap immediately before this atom was a source newline. False
    /// for the first atom and for ordinary inter-word whitespace.
    preferred_break_before: bool,
}

/// How a completed logical-line run is rendered into a single segment.
#[derive(Clone, Copy)]
enum RunRender<'a> {
    /// Greedy width fill (reflow): one [`Ir::fill`] over the run's atoms, the
    /// printer breaking word-by-word at the line width.
    Fill,
    /// Source-break-aware optimal fill used by [`WrapMode::Stable`].
    Stable { target: usize },
    /// One sentence per line (sentence/semantic): cut the run at sentence
    /// boundaries and lay each sentence flat (space-joined), separating sentences
    /// with a hard break. Width is ignored — a long sentence stays on one line.
    Sentence(ResolvedProfile<'a>),
}

/// Split a logical-line run into sentences and lay each one flat. Adjacent atoms
/// within a run are always whitespace-separated (a glued no-whitespace span is a
/// single atom), so the inter-atom separator is a single literal space and the
/// boundary detector always sees `has_whitespace_after = true`; the final atom
/// closes the last sentence regardless. A single inserted space keeps every
/// preserved token boundary from re-lexing into a merged token, so the result
/// reparses to the same tokens (idempotent).
fn render_sentences(run: Vec<RunAtom>, profile: ResolvedProfile<'_>) -> Ir {
    let n = run.len();
    // Break decisions for the n-1 internal gaps, read before the run is consumed.
    let break_after: Vec<bool> = (0..n)
        .map(|i| {
            i + 1 < n
                && is_sentence_boundary_text(
                    &run[i].text,
                    Some(run[i + 1].text.as_str()),
                    true,
                    false,
                    profile,
                )
        })
        .collect();

    let mut sentences: Vec<Ir> = Vec::new();
    let mut current: Vec<Ir> = Vec::new();
    for (i, atom) in run.into_iter().enumerate() {
        if !current.is_empty() {
            current.push(Ir::text(" "));
        }
        current.push(atom.ir);
        if break_after[i] {
            sentences.push(Ir::concat(std::mem::take(&mut current)));
        }
    }
    if !current.is_empty() {
        sentences.push(Ir::concat(current));
    }
    Ir::join(Ir::hard_line(), sentences)
}

/// Accumulator for [`reflow_elements`]: glues atom pieces, collects them into the
/// current logical-line run, and commits completed lines (with their preceding
/// separators). Bundles the state the former nested `flush_atom`/`end_line`/
/// `push_segment` helpers shared so the retained atom text and the render policy
/// ride along without threading extra parameters through every call site.
struct LineBuilder<'a> {
    /// Glued pieces of the atom in progress (its [`Ir`] and its source text).
    atom: Vec<Ir>,
    atom_text: String,
    /// Source-newline preference to attach to the next committed atom.
    preferred_break_before_next: bool,
    /// Atoms of the current run (the current logical line).
    run: Vec<RunAtom>,
    /// Completed lines (fills/sentences and blocks), interleaved with `seps` at the
    /// end.
    lines: Vec<Ir>,
    /// Pieces riding the *last* committed line (see [`Self::append_to_last_line`]),
    /// held flat until the line is sealed. Folding each one into the line as it
    /// arrives would nest a `Concat` per piece, and a whole document collapsed onto
    /// one physical line (the trivia oracle's `all-newlines-to-spaces` variant) then
    /// recurses one frame per piece and overflows the stack.
    line_tail: Vec<Ir>,
    /// The separator *preceding* each committed line (`seps[0]` is unused). A blank
    /// line in the source promotes the next separator to an [`Ir::empty_line`].
    seps: Vec<Ir>,
    /// The separator to record before the next committed line. Default: one break.
    pending_sep: Ir,
    /// Under `.dtx` prose reflow, the `% ` margin re-emitted on each line; `None`
    /// otherwise.
    margin: Option<&'static str>,
    /// Whether some content was committed *outside* the [`Self::margin`] — a
    /// forced-break block whose interior lines do not all ride their own
    /// margins ([`block_rides_own_margins`]), one opening on an unmargined
    /// line, or a column-0 `GUARD` whose line could not be isolated
    /// ([`collect_guard_line`]). Such content lands on a line with no leading
    /// `%`, where a `.dtx` doc comment (`^^A`, a `%` run) re-lexes as content:
    /// the layout would no longer be whitespace-only.
    /// [`lower_dtx_doc_paragraph`] reads this and falls back to the
    /// byte-faithful preserve path. Always `false` when `margin` is `None`.
    margin_escaped: bool,
    /// How a completed run is turned into a segment (fill vs. sentences).
    render: RunRender<'a>,
}

impl<'a> LineBuilder<'a> {
    fn new(margin: Option<&'static str>, render: RunRender<'a>) -> Self {
        Self {
            atom: Vec::new(),
            atom_text: String::new(),
            preferred_break_before_next: false,
            run: Vec::new(),
            lines: Vec::new(),
            line_tail: Vec::new(),
            seps: Vec::new(),
            pending_sep: Ir::hard_line(),
            margin,
            margin_escaped: false,
            render,
        }
    }

    /// Record that content is about to be committed outside the `% ` margin (see
    /// [`Self::margin_escaped`]). A no-op when no margin is in force.
    fn note_margin_escape(&mut self) {
        if self.margin.is_some() {
            self.margin_escaped = true;
        }
    }

    /// Glue one piece (its [`Ir`] and source `text`) onto the atom in progress.
    fn push_atom_piece(&mut self, ir: Ir, text: &str) {
        self.atom.push(ir);
        self.atom_text.push_str(text);
    }

    /// Commit the atom in progress (if any) as one atom of the current run.
    fn flush_atom(&mut self) {
        if !self.atom.is_empty() {
            let text = std::mem::take(&mut self.atom_text);
            // DTX prose owns wrapping only between atoms. Letting a nested group
            // break inside one can split macro-like documentation at an arbitrary
            // brace and synthesize margins that change the next pass's lowering.
            let ir = if self.margin.is_some() {
                self.atom.clear();
                Ir::verbatim(text.clone())
            } else {
                Ir::concat(self.atom.drain(..))
            };
            self.run.push(RunAtom {
                ir,
                text,
                preferred_break_before: std::mem::take(&mut self.preferred_break_before_next),
            });
        }
    }

    /// Mark the next inter-atom gap as an authored line break.
    fn prefer_next_break(&mut self) {
        if !self.run.is_empty() {
            self.preferred_break_before_next = true;
        }
    }

    /// Commit `content` as the next logical line, recording the separator before it
    /// and resetting `pending_sep` to a single break.
    fn push_segment(&mut self, content: Ir) {
        self.seal_last_line();
        self.seps
            .push(std::mem::replace(&mut self.pending_sep, Ir::hard_line()));
        self.lines.push(content);
    }

    /// Separate a paragraph-level sectioning command from adjacent prose. A
    /// `.dtx` documentation paragraph needs a bare `%` on the empty physical
    /// line so the separator remains inside the documentation layer.
    fn separate_section(&mut self) {
        self.pending_sep = if self.margin.is_some() {
            Ir::concat([Ir::hard_line(), Ir::column_zero("%"), Ir::hard_line()])
        } else {
            Ir::empty_line()
        };
    }

    /// Commit a forced-break block as its own segment under the `.dtx` margin:
    /// the canonical `% ` re-attached for its first line (the source margin was
    /// dropped by the `DOC_MARGIN` arm, or floated out of the paragraph), then
    /// the block raw — its interior lines carry their own column-0 margins
    /// byte-faithfully ([`block_rides_own_margins`]), so no [`Ir::margin_prefix`]
    /// wrap is needed (or safe: the printer's re-emitted prefix would collide
    /// with the block's own `Ir::column_zero` margins).
    fn push_margined_block(&mut self, ir: Ir) {
        let margin = self.margin.expect("only called under `.dtx` prose reflow");
        self.push_segment(Ir::concat([Ir::column_zero(margin), ir]));
    }

    /// Glue a trailing comment onto the run so it rides the end of its line: onto
    /// the atom in progress when one is open (a directly-glued `word%…`, whose
    /// missing space is the space-suppression idiom), else onto the last committed
    /// atom with the single separating space restored (`word %…`). The comment must
    /// never become a fill atom of its own — a width break before it would commit
    /// it to the next line, where the own-line `%` re-binds as the next command's
    /// doc comment on reparse and breaks idempotence.
    fn append_trailing_comment(&mut self, text: &str) {
        if !self.atom.is_empty() {
            self.push_atom_piece(Ir::verbatim(text), text);
            return;
        }
        if let Some(last) = self.run.last_mut() {
            let prev = std::mem::replace(&mut last.ir, Ir::Nil);
            last.ir = Ir::concat([prev, Ir::verbatim(" "), Ir::verbatim(text)]);
            last.text.push(' ');
            last.text.push_str(text);
            return;
        }
        // Empty run (the caller guards with `line_has_content`, so this is a
        // safety net): the comment becomes the line's only atom.
        self.push_atom_piece(Ir::verbatim(text), text);
    }

    /// Glue `ir` onto the end of the last committed line (a trailing comment, or
    /// content still on a block's last physical line). No-op when no line has been
    /// committed. Buffered in [`Self::line_tail`] and folded in by
    /// [`Self::seal_last_line`], so N riders cost one `Concat`, not N nested ones.
    fn append_to_last_line(&mut self, ir: Ir) {
        if !self.lines.is_empty() {
            self.line_tail.push(ir);
        }
    }

    /// Fold any buffered riders into the last committed line.
    fn seal_last_line(&mut self) {
        if self.line_tail.is_empty() {
            return;
        }
        let tail = std::mem::take(&mut self.line_tail);
        if let Some(last) = self.lines.last_mut() {
            let prev = std::mem::replace(last, Ir::Nil);
            *last = Ir::concat(std::iter::once(prev).chain(tail));
        }
    }

    /// End the current logical line: flush the atom and, when the run is non-empty,
    /// render it (a fill under reflow, sentences under sentence/semantic) and commit
    /// it. Under `.dtx` prose reflow (`margin` set) the segment is wrapped in an
    /// [`Ir::margin_prefix`] so a `% ` margin is re-emitted on every line.
    fn end_line(&mut self) {
        self.flush_atom();
        if self.run.is_empty() {
            return;
        }
        let run = std::mem::take(&mut self.run);
        let body = self.render_run(run);
        let segment = match self.margin {
            Some(m) => Ir::margin_prefix(m, body),
            None => body,
        };
        self.push_segment(segment);
    }

    /// Commit a reflow run as a hugging fill. A final inline construct whose IR
    /// contains hard breaks can then keep its fitting first line beside the
    /// preceding prose; the construct's remaining lines still break internally.
    fn end_hug_line(&mut self) {
        self.flush_atom();
        if self.run.is_empty() {
            return;
        }
        let atoms: Vec<Ir> = std::mem::take(&mut self.run)
            .into_iter()
            .map(|atom| atom.ir)
            .collect();
        let body = if atoms.len() == 1 {
            atoms.into_iter().next().unwrap()
        } else {
            let mut parts = Vec::with_capacity(atoms.len() * 2 - 1);
            for (index, atom) in atoms.into_iter().enumerate() {
                if index > 0 {
                    parts.push(Ir::Line);
                }
                parts.push(atom);
            }
            Ir::HugFill(parts.into())
        };
        let segment = match self.margin {
            Some(margin) => Ir::margin_prefix(margin, body),
            None => body,
        };
        self.push_segment(segment);
    }

    fn render_run(&self, run: Vec<RunAtom>) -> Ir {
        match self.render {
            RunRender::Fill => Ir::fill(run.into_iter().map(|a| a.ir)),
            RunRender::Stable { target } => {
                let preferred: Vec<bool> = run
                    .iter()
                    .skip(1)
                    .map(|atom| atom.preferred_break_before)
                    .collect();
                Ir::preferred_fill(run.into_iter().map(|a| a.ir), preferred, target)
            }
            RunRender::Sentence(profile) => render_sentences(run, profile),
        }
    }

    /// Emit the accumulated lines, interleaving the recorded separators.
    fn finish(mut self) -> Ir {
        self.end_line();
        self.seal_last_line();
        let mut result: Vec<Ir> = Vec::with_capacity(self.lines.len().saturating_mul(2));
        for (i, line) in self.lines.into_iter().enumerate() {
            if i > 0 {
                result.push(self.seps[i].clone());
            }
            result.push(line);
        }
        Ir::concat(result)
    }
}

fn reflow_elements(
    elements: impl Iterator<Item = SyntaxElement>,
    cx: LowerCtx<'_>,
    kind: ReflowKind,
) -> Ir {
    reflow_elements_checked(elements, cx, kind).0
}

/// [`reflow_elements`], additionally reporting whether the result committed any
/// content outside the `% ` documentation margin (see
/// [`LineBuilder::margin_escaped`]). Only meaningful under
/// [`ReflowKind::DtxProse`]; every other kind always reports `false`.
fn reflow_elements_checked(
    elements: impl Iterator<Item = SyntaxElement>,
    cx: LowerCtx<'_>,
    kind: ReflowKind,
) -> (Ir, bool) {
    // Collected up front so the single-newline arm can look ahead at the next
    // physical line ([`line_is_command_only`]). Inline prose commands (`\footnote`,
    // `\emph`, …) are flattened into the stream so their bodies reflow as running
    // text rather than block-breaking their braces (see [`flatten_inline_prose`]);
    // `STATEMENT` wrappers are spliced out the same way (see
    // [`flatten_statements`]) so their contents reflow as the sibling stream
    // they wrap.
    // Under `Reflow` a statement-body run is lowered *structurally*: `STATEMENT`
    // nodes stay whole and take their own arm below (boundaries from the node,
    // continuations hung — see [`lower_statement`]). Every other path splices
    // the wrappers out and keeps the line-stream behavior.
    let structural = kind == ReflowKind::Statement && cx.wrap == WrapMode::Reflow;
    let elements: Vec<SyntaxElement> = elements.collect();
    let elements = if structural {
        elements
    } else {
        flatten_statements(elements)
    };
    let glue_matched_args = !cx.in_dtx_doc_region
        && !run_carries_doc_margin(&elements, cx)
        && matches!(kind, ReflowKind::Prose | ReflowKind::ProseArg);
    let elements: Vec<SyntaxElement> = flatten_inline_prose(elements, cx, glue_matched_args);

    // Inside one structural statement, gaps consult the TikZ unit model: a
    // unit-internal gap renders as a single space instead of a break
    // opportunity (see [`ReflowKind::StatementInterior`]). Computed over the
    // *flattened* stream so the verdict indices match the loop's.
    let unit_glue: Option<Vec<bool>> =
        (kind == ReflowKind::StatementInterior).then(|| statement_glue(&elements));

    // Under `.dtx` prose reflow each segment is wrapped in a `% ` margin prefix and
    // the per-line `DOC_MARGIN` tokens are dropped; `None` otherwise.
    let margin: Option<&'static str> = (kind == ReflowKind::DtxProse).then_some(DTX_DOC_MARGIN);

    // Sentence/semantic segmentation applies to *prose* runs; a `Statement` run is
    // code (a `\newcommand` body), so it keeps the width fill regardless of mode.
    let render = match cx.wrap {
        WrapMode::Stable if kind != ReflowKind::Statement => RunRender::Stable {
            target: cx.stable_target,
        },
        WrapMode::Sentence | WrapMode::Semantic if kind != ReflowKind::Statement => {
            RunRender::Sentence(cx.profile)
        }
        _ => RunRender::Fill,
    };

    let mut b = LineBuilder::new(margin, render);
    // Whether the current *physical* source line so far consists solely of
    // command(s) (and inline whitespace). Such a line is kept on its own line
    // rather than reflowed into its neighbours (see the single-newline arm). Both
    // reset at every physical-line boundary. This is the *residual* command-line
    // rule: curated block-level commands are intercepted upstream (the
    // block-statement arm below) and never depend on it, so what it decides for
    // is un-signatured and scanned-definition commands — whose block-ness no
    // positive signature property can know — plus block commands glued to
    // adjacent content. That residue reads the lone-newline predicate as
    // sanctioned Tier 2: preservation-only, with the fixed-point argument
    // written on [`line_is_command_only`]. It reaches the count through
    // [`consume_widened_gap_slice`], the widened boundary (see [`WideGap`]).
    let mut line_all_commands = true;
    let mut line_has_content = false;
    // Whether the current physical source line rides a `% ` documentation margin.
    // Only meaningful under `DtxProse`. Initialized `true`: both `DtxProse`
    // callers gate on a margined first line ([`dtx_paragraph_starts_margined`],
    // [`dtx_run_starts_margined`]) — a contract that covers the floated-margin
    // paragraph, whose leading `%` sits *outside* the element stream. Cleared at
    // every newline, re-established by the line's `DOC_MARGIN`.
    let mut line_margined = true;
    // Whether the previous element was a forced-break node committed via
    // `push_segment` (a doc-commented command, an environment, …). A `COMMENT`
    // on the same physical line as such a block — glued directly
    // (`\end{center}%`) or after inline whitespace (`\newcommand{…}{…} % note`)
    // — must ride the block's last line: committing it as its own line changes
    // spacing semantics in the glued case and, because an own-line `%` binds
    // forward as a doc comment on reparse, breaks idempotence (issue #38).
    // `block_gap` records that inline whitespace separated the two, so the
    // riding comment keeps a single space before it.
    let mut prev_was_block = false;
    let mut block_gap = false;
    // Set alongside `prev_was_block` when the committed block *closes* its line: an
    // environment, sectioning command, or curated block command. A trailing `%`
    // still rides (it must never be relocated), but content starts a fresh line.
    // Other forced blocks, such as a doc-commented `\input`, leave their last line
    // open so content from the same source line can ride it.
    let mut prev_block_closes_line = false;

    let mut idx = 0;
    while idx < elements.len() {
        let after_block = std::mem::take(&mut prev_was_block);
        let after_block_gap = std::mem::take(&mut block_gap);
        let after_block_closed = std::mem::take(&mut prev_block_closes_line);
        match &elements[idx] {
            // Whitespace / newline run: a physical-line and atom boundary.
            SyntaxElement::Token(token) if is_collapsible_trivia(token.kind()) => {
                let newlines = consume_widened_gap_slice(&elements, &mut idx);
                // A unit-internal gap (the TikZ unit model, statement interiors
                // only): glue the neighbors into one atom with a single space —
                // no break opportunity, no line boundary. Content-derived, so a
                // wrap re-reads to the same units (Tier 1); the model never
                // glues across a comment or a blank line.
                if newlines < 2
                    && let Some(glue) = &unit_glue
                    && glue.get(idx).copied().unwrap_or(false)
                {
                    if after_block {
                        // The unit is riding a block segment's last line (a
                        // doc-commented statement head): keep riding — the next
                        // element's `ride_after_block` gap form restores the
                        // single space, and riding appends flat, so the
                        // no-break promise holds there too.
                        prev_was_block = true;
                        block_gap = true;
                        prev_block_closes_line = after_block_closed;
                    } else {
                        b.push_atom_piece(Ir::verbatim(" "), " ");
                    }
                    continue;
                }
                if newlines >= 2 {
                    // A blank line ends the line and promotes the next separator.
                    b.end_line();
                    b.pending_sep = Ir::empty_line();
                    line_all_commands = true;
                    line_has_content = false;
                    line_margined = false;
                } else if newlines == 1 {
                    // A single source newline. Under `Statement` reflow every source
                    // line is its own logical line, so the break always ends the line
                    // (structural `STATEMENT` nodes never reach this arm — they
                    // commit through their own arm below — so this authored-line
                    // read governs only the fallback content no `;` terminates).
                    // Under `Semantic` an authored soft break is likewise preserved
                    // (sembr keeps the writer's clause breaks). Under `Prose`/`Sentence`
                    // it is normally just an atom boundary the run rejoins, except a
                    // line that is *only* command(s) — on either side of the break — is
                    // kept on its own line: end the line so the break survives instead
                    // of collapsing to a space. This is the residual rule for commands
                    // no positive signature property covers (see `line_all_commands`
                    // above); curated block commands never reach it.
                    // The command-only residue is skipped under `ProseArg` and
                    // `StatementInterior`: a width-owned body must not mint
                    // forced breaks pass 2 can see and pass 1 could not (see
                    // [`ReflowKind`]).
                    let residue_applies =
                        !matches!(kind, ReflowKind::ProseArg | ReflowKind::StatementInterior);
                    let prev_is_command = residue_applies && line_has_content && line_all_commands;
                    let next_is_command =
                        residue_applies && line_is_command_only(&elements, idx, cx);
                    if kind == ReflowKind::Statement
                        || cx.wrap == WrapMode::Semantic
                        || prev_is_command
                        || next_is_command
                    {
                        b.end_line();
                    } else {
                        b.flush_atom();
                        if cx.wrap == WrapMode::Stable {
                            b.prefer_next_break();
                        }
                    }
                    line_all_commands = true;
                    line_has_content = false;
                    line_margined = false;
                } else {
                    // Pure inline whitespace: an atom boundary within the line.
                    // It stays on the block's physical line, so a comment next
                    // keeps riding the block (with the space restored).
                    b.flush_atom();
                    if after_block {
                        prev_was_block = true;
                        block_gap = true;
                        prev_block_closes_line = after_block_closed;
                    }
                }
                continue;
            }
            // A comment trailing content rides the end of that line, then forces a
            // break. But a comment that *begins* its own physical line stays on its
            // own line: end the current line first so the preceding prose run commits
            // separately, instead of reflowing the bare `%` up into that run.
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::COMMENT => {
                if after_block {
                    // On the block segment's last physical line: ride it instead
                    // of starting a line of its own. A directly-glued comment
                    // (`\end{center}%`) stays glued — the `%` is the
                    // space-suppression idiom — while one separated by inline
                    // whitespace keeps a single space.
                    let comment = if after_block_gap {
                        Ir::concat([Ir::verbatim(" "), Ir::verbatim(token.text())])
                    } else {
                        Ir::verbatim(token.text())
                    };
                    b.append_to_last_line(comment);
                } else if line_has_content {
                    // Trailing content on this physical line: ride its end (never
                    // a fill atom of its own — see `append_trailing_comment`).
                    b.append_trailing_comment(token.text());
                    b.end_line();
                } else {
                    b.end_line();
                    b.push_atom_piece(Ir::verbatim(token.text()), token.text());
                    b.end_line();
                }
                line_all_commands = true;
                line_has_content = false;
            }
            // A `\`-at-end-of-line control symbol (`\` + newline) carries its own
            // newline but nothing after it — kept verbatim for losslessness, it
            // ends the line: emit the part before the break as a flat atom and let
            // the line break supply the newline, so the result reparses to the same
            // token (idempotent) instead of leaving an unbreakable multi-line atom
            // inside the run. Restricted to control symbols: a multi-line `VERB`
            // token (a brace-verbatim argument spanning lines) has real content
            // after its newline and must be emitted whole by the arm below.
            SyntaxElement::Token(token)
                if token.kind() == SyntaxKind::CONTROL_SYMBOL && token.text().contains('\n') =>
            {
                let before = token.text().split_once('\n').map(|(b, _)| b).unwrap_or("");
                if !before.is_empty() {
                    b.push_atom_piece(Ir::verbatim(before), before);
                }
                if !cx.absorbs_control_newline(token) {
                    b.end_line();
                }
                line_all_commands = true;
                line_has_content = false;
            }
            // Under `.dtx` prose reflow, a per-line `%` documentation margin
            // (`DOC_MARGIN`) is dropped: the canonical `% ` margin is re-emitted on
            // every reflowed line by the enclosing [`Ir::margin_prefix`] (see
            // `end_line`), so gluing the source `%` into the run would double it.
            // The single space following it is inter-word whitespace the run
            // re-derives. A `GUARD` is *not* dropped (guards keep their column-0 pin).
            SyntaxElement::Token(token)
                if margin.is_some() && token.kind() == SyntaxKind::DOC_MARGIN =>
            {
                line_margined = true;
            }
            // The enclosing virtual-document region re-emits one canonical
            // margin per generated line. Discard the physical marker and its
            // authored padding before ordinary structural lowering continues.
            SyntaxElement::Token(token)
                if cx.in_dtx_doc_region && token.kind() == SyntaxKind::DOC_MARGIN =>
            {
                while elements.get(idx + 1).is_some_and(|element| {
                    matches!(element, SyntaxElement::Token(next) if next.kind() == SyntaxKind::WHITESPACE)
                }) {
                    idx += 1;
                }
            }
            // A `GUARD` (`%<…>`) pins its whole physical line to column 0. Commit
            // that line as one segment under every reflow kind; otherwise two
            // adjacent guarded commands can join, and the second `%<…>` becomes a
            // trailing comment that swallows its command on the next parse. Under
            // a `.dtx` prose margin the isolated segment is deliberately
            // unmargined—the margin and guard would collide.
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::GUARD => {
                if let Some(line) = collect_guard_line(&elements, &mut idx, cx) {
                    b.end_line();
                    b.push_segment(line);
                    line_all_commands = true;
                    line_has_content = false;
                    continue;
                }
                if margin.is_some() {
                    b.note_margin_escape();
                }
                b.push_atom_piece(lower_loose_token(token, cx), token.text());
                line_has_content = true;
                line_all_commands = false;
            }
            // Any other token (WORD, `~`, `&`, `#`, `^`, `_`, brackets, `\verb`,
            // a bare control symbol) glues onto the current atom — prose content,
            // so this physical line is no longer command-only. A `.dtx` margin/guard
            // (only under the dtx config) pins to column 0 instead of reflowing.
            SyntaxElement::Token(token) => {
                if after_block && !after_block_closed {
                    // Content on the block segment's last physical line
                    // (`\input docstrip.tex`, where the doc comment bound to
                    // `\input` made it a block): ride that line instead of
                    // starting one of its own. Same rule as the `COMMENT` arm
                    // above, and the chain continues so the rest of the line
                    // rides too. A block that *closes* its line (a heading) is
                    // excluded — content after it starts a fresh line.
                    b.append_to_last_line(ride_after_block(
                        lower_loose_token(token, cx),
                        after_block_gap,
                    ));
                    prev_was_block = true;
                } else {
                    b.push_atom_piece(lower_loose_token(token, cx), token.text());
                }
                line_has_content = true;
                line_all_commands = false;
            }
            // A structural statement (`\draw …;` in a curated `statementBody`
            // body): its own logical line, boundaries from the node — never from
            // the trivia around it. Only present when `structural` (every other
            // path spliced the wrapper out up front).
            //
            // A statement opens its own line even when the author *glued* it to
            // what precedes (`…;\draw …`). This is the one sanctioned breach of
            // the glued-divider principle, licensed the way `ContentKind::Keyval`
            // licenses the glued comma split: the curated `statementBody` flag
            // asserts that whitespace between a picture body's statements is
            // insignificant to the package that consumes them, so the inserted
            // break is not a typeset change. The claim is held to the curated
            // standard (`task typeset:check` carries a case), and it is
            // empirically idle: glued statement seams are unattested in the
            // pgf and user corpora (~6000 statements).
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::STATEMENT => {
                let ir = lower_statement(child, cx);
                b.end_line();
                b.push_segment(ir);
                line_all_commands = true;
                line_has_content = false;
                // One statement per line: following content starts a fresh line
                // (`prev_block_closes_line`), while a trailing `%` still rides
                // (the `COMMENT` arm's `after_block` path).
                prev_was_block = true;
                prev_block_closes_line = true;
            }
            // An explicit `\\` line break (with its `*` / `[len]`, grouped by the
            // parser into one node) rides the end of the current line, then breaks.
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::LINE_BREAK => {
                b.push_atom_piece(lower_node(child, cx), &child.text().to_string());
                b.end_line();
                line_all_commands = true;
                line_has_content = false;
            }
            // An inline citation list participates in the surrounding paragraph
            // fill at its top-level commas, just as an inline prose argument
            // participates at its inter-word gaps. Keeping the entries at this
            // altitude lets the first key share the preceding prose line and the
            // closing brace share the final key's line with following prose.
            SyntaxElement::Node(child)
                if matches!(cx.wrap, WrapMode::Reflow | WrapMode::Stable)
                    && margin.is_none()
                    && !after_block
                    && child.kind() == SyntaxKind::COMMAND
                    && command_is_inline(child, cx)
                    && inline_token_list_atoms(child, cx).is_some() =>
            {
                let atoms = inline_token_list_atoms(child, cx)
                    .expect("match guard proved the inline token list");
                for (index, atom) in atoms.into_iter().enumerate() {
                    if index > 0 {
                        b.flush_atom();
                    }
                    b.push_atom_piece(atom, "");
                }
                line_has_content = true;
                line_all_commands = false;
                idx += 1;
                continue;
            }
            SyntaxElement::Node(child) => {
                let ir = lower_node(child, cx);
                // A block-level command — sectioning (`\part` … `\subparagraph`) or
                // curated block (`\usepackage`, `\newcommand`, …) — is a block-level
                // statement: it opens a line and closes one, whatever trivia the
                // author wrote around it. At paragraph level, sectioning additionally
                // receives an empty line on both sides, making the structural
                // boundary visible. The old behavior kept the break only when
                // the source had a newline there (via `line_is_command_only` below),
                // which is exactly the lone-newline predicate trivia-invariant layout
                // forbids — `\subsection{X}\nprose` and `\subsection{X} prose` are the
                // same bytes to the next parse, so both must lay out alike. Reading
                // `CommandSig::sectioning`/`CommandSig::block` keeps the rule in the
                // semantic layer instead of a formatter-owned name list.
                //
                // Two gates scope the statement treatment:
                // - Not under `ReflowKind::Statement`, whose Tier-2 contract is the
                //   authored line: a brace-group body (`\AtBeginDocument{\setcounter
                //   {page}{1}}`, a one-line `\newcommand` body) must not be forced
                //   open. Block-statement synthesis lives at prose altitude.
                // - A *block* (unlike a sectioning) command must be trivia-isolated
                //   on both sides: breaking where the author glued
                //   (`\ProcessOptions\relax`) materializes a space token TeX
                //   typesets. A heading splits even glued — its own `\par` discards
                //   the materialized glue — so sectioning keeps the unconditional
                //   form. Gluedness is a predicate the formatter preserves, so both
                //   reads are Tier-safe.
                //
                // Forced-break lowerings (a comment inside the title) fall through to
                // the block path below, which already opens and closes a line — and
                // does so through the `.dtx` margin-aware routes this arm's plain
                // `end_line` pair would bypass.
                let is_sectioning =
                    child.kind() == SyntaxKind::COMMAND && command_is_sectioning(child, cx);
                // A blank line is a real `\par`, so synthesize one only where the
                // parser already identified a top-level prose paragraph. Nested
                // headings still get the block command's hard-line boundaries;
                // inserting `\par` inside a macro argument can make a non-`long`
                // macro invalid, and inside a conditional it can reshape the
                // wrapper between formatting passes.
                let is_section_boundary = is_sectioning
                    && child
                        .parent()
                        .is_some_and(|parent| parent.kind() == SyntaxKind::PARAGRAPH);
                let is_section_label = child.kind() == SyntaxKind::COMMAND
                    && command_is_label(child)
                    && label_follows_sectioning_run(&elements, idx, cx);
                let section_label_closes_line =
                    is_section_label && next_is_separated(&elements, idx);
                let is_block_stmt = kind != ReflowKind::Statement
                    && child.kind() == SyntaxKind::COMMAND
                    && (is_sectioning
                        || (command_is_block(child, cx)
                            && b.atom.is_empty()
                            && next_is_separated(&elements, idx)));
                if is_section_label && !ir.contains_forced_break() {
                    let glued_to_previous_label = idx > 0
                        && matches!(
                            &elements[idx - 1],
                            SyntaxElement::Node(previous)
                                if previous.kind() == SyntaxKind::COMMAND
                                    && command_is_label(previous)
                        );
                    if !glued_to_previous_label {
                        b.end_line();
                    }
                    b.push_atom_piece(ir, &child.text().to_string());
                    if section_label_closes_line {
                        b.end_line();
                        if next_nontrivia_is_label(&elements, idx) {
                            b.pending_sep = Ir::hard_line();
                        } else {
                            b.separate_section();
                        }
                        line_has_content = false;
                        prev_was_block = true;
                        prev_block_closes_line = true;
                    } else {
                        line_has_content = true;
                    }
                    line_all_commands = true;
                    idx += 1;
                    continue;
                }
                if is_block_stmt && !ir.contains_forced_break() {
                    b.end_line();
                    if is_section_boundary {
                        b.separate_section();
                    }
                    b.push_atom_piece(ir, &child.text().to_string());
                    b.end_line();
                    if is_section_boundary && !next_nontrivia_is_label(&elements, idx) {
                        b.separate_section();
                    }
                    line_all_commands = true;
                    line_has_content = false;
                    // Committed as a block, so a `%` still on the heading's physical
                    // line rides it (the `COMMENT` arm's `after_block` path) instead
                    // of being stranded on a line of its own, where it would rebind as
                    // the next construct's `DOC_COMMENT` at the next parse. Content
                    // does *not* ride: closing the line is the whole rule.
                    prev_was_block = true;
                    prev_block_closes_line = true;
                    idx += 1;
                    continue;
                }
                if ir.contains_forced_break() {
                    if is_section_boundary {
                        // A documentation comment belongs to the heading it
                        // introduces, so separate the whole forced-break command
                        // from preceding prose rather than splitting the comment
                        // from its command.
                        b.end_line();
                        b.separate_section();
                    }
                    // A virtual documentation environment already owns every
                    // physical margin through its `Ir::doc_margin` wrapper. It is
                    // therefore a complete segment in `DtxProse`: surrounding
                    // prose may keep reflowing, but the accumulator must neither
                    // add another `% ` nor record the block as a margin escape.
                    let owns_dtx_margin =
                        margin.is_some() && dtx_doc_region(child, cx);
                    // A margin-framed `macrocode` chunk opening its own margined
                    // source line, reachable only through a reflowing expl3 run
                    // ([`dtx_run_reflows_safely`]): committed raw behind its
                    // byte-exact source frame lead, never the canonical `% ` —
                    // docstrip matches the `%    \begin{macrocode}` line literally.
                    let frame_lead = (margin.is_some()
                        && line_margined
                        && !line_has_content
                        && is_margin_framed_macrocode(child))
                    .then(|| dtx_env_line_lead(child))
                    .flatten();
                    let hugs_preceding_prose = margin.is_none()
                        && child.kind() == SyntaxKind::INLINE_MATH
                        && line_has_content
                        && matches!(kind, ReflowKind::Prose | ReflowKind::ProseArg)
                        && matches!(b.render, RunRender::Fill);
                    let rides_preceding_block = after_block
                        && !after_block_gap
                        && !after_block_closed
                        && !is_sectioning;
                    if rides_preceding_block {
                        // A forced-break node can itself be glued to the forced
                        // block before it (`{a%\n}{b%\n}`). Keep their shared
                        // boundary on one physical line just as the token and
                        // ordinary-node arms do below. Splitting it would create
                        // a TeX space token where the source had no gap.
                        b.append_to_last_line(ir);
                    } else if margin.is_none() && (!b.atom.is_empty() || hugs_preceding_prose) {
                        // A directly glued block (`\newcommand\cls@hook{%`) had no
                        // source break opportunity, so it extends the unbreakable
                        // atom in progress. The one spaced admission is inline math
                        // in running prose, whose opening fragment remains inline
                        // through the explicit hugging-fill rule below. Both paths
                        // are skipped under a `.dtx` margin, where a generated line
                        // also needs physical framing.
                        b.push_atom_piece(ir, &child.text().to_string());
                        if hugs_preceding_prose {
                            // Inline math remains inline at its opening edge even
                            // when protected comments force later lines. A hugging
                            // fill measures only the math node's first line here,
                            // moving it to the next line only when that prefix does
                            // not fit beside the preceding prose.
                            b.end_hug_line();
                        } else {
                            b.end_line();
                        }
                    } else if let Some(lead) = frame_lead {
                        b.end_line();
                        b.push_segment(Ir::concat([lead, ir]));
                    } else if owns_dtx_margin {
                        b.end_line();
                        b.push_segment(ir);
                    } else if margin.is_some() && line_margined && block_rides_own_margins(child) {
                        // A block amid `.dtx` doc prose whose interior lines all
                        // carry their own column-0 margins, opening on a margined
                        // line: commit it raw on a fresh line with the canonical
                        // margin re-attached for its first line. Its interior
                        // bytes are untouched, so the layout stays within the
                        // `% ` margin and the surrounding prose keeps reflowing.
                        b.end_line();
                        b.push_margined_block(ir);
                    } else {
                        // A block amid prose: end the current line, then place the
                        // block on its own line(s); a fresh run continues after.
                        // `push_segment` applies no margin, so under `.dtx` prose
                        // reflow a block that does not ride its own interior
                        // margins (or opens on an unmargined line) escapes the
                        // `% ` margin — recorded so the caller abandons the
                        // reflow for this paragraph.
                        b.end_line();
                        b.note_margin_escape();
                        b.push_segment(ir);
                    }
                    line_all_commands = true;
                    line_has_content = false;
                    prev_was_block = true;
                    // An environment is a complete structural block, so prose after
                    // its closer starts a fresh line whether the source gap was a
                    // space or newline. A block-level statement whose lowering forced
                    // a break (a `%` bound to it as a `DOC_COMMENT`, a comment inside
                    // a title, a multi-line `\title` body) closes its line for the
                    // same reason. Everything else that lands here — an
                    // un-signatured command, a glued block command, `\input`'s bare
                    // filename shape — leaves the line open so following content can
                    // ride it (the `after_block` paths).
                    prev_block_closes_line =
                        is_block_stmt
                            || section_label_closes_line
                            || child.kind() == SyntaxKind::ENVIRONMENT;
                    if is_section_boundary && !next_nontrivia_is_label(&elements, idx) {
                        b.separate_section();
                    } else if section_label_closes_line {
                        if next_nontrivia_is_label(&elements, idx) {
                            b.pending_sep = Ir::hard_line();
                        } else {
                            b.separate_section();
                        }
                    }
                } else {
                    // A block-level `COMMAND` keeps the line command-only; an inline
                    // command (`\citep`, `\ref`, …) is running-text content, as is any
                    // other inline node (math, an inline group), and disqualifies it.
                    if after_block && !after_block_closed {
                        // Same as the token arm: content still on the previous
                        // block's last physical line rides it.
                        b.append_to_last_line(ride_after_block(ir, after_block_gap));
                        prev_was_block = true;
                    } else {
                        b.push_atom_piece(ir, &child.text().to_string());
                    }
                    line_has_content = true;
                    line_all_commands &=
                        child.kind() == SyntaxKind::COMMAND && !command_is_inline(child, cx);
                }
            }
        }
        idx += 1;
    }
    let escaped = b.margin_escaped;
    (b.finish(), escaped)
}

/// Collect the guard-led physical line starting at `elements[*idx]` (a `GUARD`,
/// always at column 0) as one byte-faithful, single-line segment, advancing
/// `*idx` past the line's content; the terminating newline run is left for the
/// caller's trivia arm. Trailing inline whitespace before that newline is
/// skipped (the formatter never emits trailing whitespace — a trivia-only
/// change). Returns `None` with `*idx` untouched when the line cannot be
/// isolated — an element on it spans a newline, or its lowering carries a
/// forced break — in which case the caller records a margin escape.
fn collect_guard_line(elements: &[SyntaxElement], idx: &mut usize, cx: LowerCtx<'_>) -> Option<Ir> {
    let mut end = *idx;
    while let Some(element) = elements.get(end) {
        let spans_lines = match element {
            SyntaxElement::Token(t) => t.text().contains('\n'),
            SyntaxElement::Node(n) => n.text().contains_char('\n'),
        };
        if spans_lines {
            if matches!(element, SyntaxElement::Token(t) if is_collapsible_trivia(t.kind())) {
                break;
            }
            return None;
        }
        end += 1;
    }
    let mut last = end;
    while last > *idx
        && matches!(&elements[last - 1], SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()))
    {
        last -= 1;
    }
    let ir = Ir::concat(lower_element_stream(
        elements[*idx..last].iter().cloned(),
        cx,
    ));
    if ir.contains_forced_break() {
        return None;
    }
    *idx = end;
    Some(ir)
}

/// Lower a `PARAGRAPH` that overlaps an expl3 region. The paragraph is split at the
/// `\ExplSyntaxOn`/`Off` toggles into maximal in-region and out-of-region runs;
/// each in-region run lays out as expl3 code ([`lower_expl_code`]), each out-of-region
/// run keeps the ordinary prose/stream treatment. The common case — a whole
/// paragraph inside a region (a `.sty`/`.dtx` body, or a blank-line-separated
/// `\ExplSyntaxOn…Off` block) — is a single in-region run. Runs are joined by a hard
/// line break (a region boundary always begins a fresh line).
/// Prefix `ir` with the single space that separated it from the block segment it
/// rides, when the source had one. Directly-glued content (`\end{center}%`) stays
/// glued — the missing space is the space-suppression idiom.
fn ride_after_block(ir: Ir, gap: bool) -> Ir {
    if gap {
        Ir::concat([Ir::verbatim(" "), ir])
    } else {
        ir
    }
}

/// Whether an element run carries a `.dtx` documentation margin or docstrip guard
/// anywhere inside it. Both pin to column 0, so a run containing one is never
/// *generic*-prose-reflowed (see [`lower_expl_paragraph`]); a margined run may
/// still reflow under the `% ` margin when [`dtx_run_reflows_safely`] holds.
/// Always false outside the `.dtx` lexer config, where neither token kind exists.
fn run_carries_doc_margin(run: &[SyntaxElement], cx: LowerCtx<'_>) -> bool {
    if !cx.is_dtx {
        return false;
    }
    run.iter().any(|element| match element {
        SyntaxElement::Token(t) => {
            matches!(t.kind(), SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD)
        }
        SyntaxElement::Node(n) => contains_doc_margin(n, cx),
    })
}

/// Slice analogue of [`dtx_paragraph_starts_margined`] for an out-of-region expl3
/// run: the run's first non-trivia token (descending into nodes) must be a
/// `DOC_MARGIN`. Deliberately without the [`margin_precedes_on_line`] backward
/// walk — `prev_token` would cross the run boundary into the previous run's
/// byte-faithful output, and a margin owned there must not count (the reflow
/// would re-emit a second `% ` onto the same physical line).
fn dtx_run_starts_margined(run: &[SyntaxElement]) -> bool {
    for element in run {
        let first = match element {
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => continue,
            SyntaxElement::Token(t) => Some(t.clone()),
            SyntaxElement::Node(n) => n
                .descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| !is_collapsible_trivia(t.kind())),
        };
        if let Some(token) = first {
            return token.kind() == SyntaxKind::DOC_MARGIN;
        }
    }
    false
}

/// Run analogue of [`dtx_doc_paragraph_reflows_safely`] for a doc-margined
/// out-of-region expl3 run: opening on a margined line
/// ([`dtx_run_starts_margined`]), unstructured, and the speculative reflow keeps
/// every line under the `% ` margin. Three deliberate differences from the
/// paragraph gate. "Unstructured" *admits* a margin-framed `macrocode` chunk as
/// a direct run element — such a run is exactly how a doc-layer paragraph
/// overlaps an expl3 region (the chunk bodies are the region; the doc lines
/// around them are the out-of-region rest), and the chunk commits raw behind
/// its byte-exact source frame lead ([`dtx_env_line_lead`]), so docstrip's
/// literal `%    \begin{macrocode}` match survives. Any other environment in
/// the run — or around it, like [`dtx_paragraph_reflows`]'s ancestor half
/// (prose inside a `% \begin{macro}` doc block keeps its authored `%    `
/// margins; reflowing structured doc content stays out of scope) — still
/// declines. And unmemoized: nothing asks twice for a run
/// ([`margin_floats_into_paragraph`] consults only `PARAGRAPH` nodes), so each
/// run is probed at most once.
fn dtx_run_reflows_safely(run: &[SyntaxElement], cx: LowerCtx<'_>) -> bool {
    if !dtx_run_starts_margined(run) {
        return false;
    }
    let structured = run.iter().any(|element| match element {
        SyntaxElement::Node(n) if is_margin_framed_macrocode(n) => false,
        SyntaxElement::Node(n) => n.descendants().any(|d| d.kind() == SyntaxKind::ENVIRONMENT),
        SyntaxElement::Token(_) => false,
    }) || run
        .first()
        .and_then(SyntaxElement::parent)
        .is_some_and(|p| p.ancestors().any(|a| a.kind() == SyntaxKind::ENVIRONMENT));
    if structured {
        return false;
    }
    let probe = LowerCtx {
        dtx_margin_probe: true,
        ..cx
    };
    !reflow_elements_checked(run.iter().cloned(), probe, ReflowKind::DtxProse).1
}

/// Whether `node` is a margin-framed `macrocode`/`macrocode*` chunk — the one
/// environment [`dtx_run_reflows_safely`] admits inside a reflowing run. The
/// curated name set matches [`intersect_macrocode_bodies`]: these are the chunks
/// whose bodies *are* the expl3 regions, so a doc paragraph cannot overlap a
/// region without containing one.
fn is_margin_framed_macrocode(node: &SyntaxNode) -> bool {
    node.kind() == SyntaxKind::ENVIRONMENT
        && Environment::cast(node.clone())
            .is_some_and(|e| matches!(e.name().as_deref(), Some("macrocode" | "macrocode*")))
        && is_margin_framed(node)
}

/// Whether an element is the explicit toggle that closes an expl3 region.
fn is_expl_syntax_off_command(element: &SyntaxElement) -> bool {
    let SyntaxElement::Node(command) = element else {
        return false;
    };
    command.kind() == SyntaxKind::COMMAND
        && command.first_token().is_some_and(|token| {
            token.kind() == SyntaxKind::CONTROL_WORD
                && expl_toggle(token.text()) == Some(ExplToggle::Off)
        })
}

/// The byte-exact line lead of a margin-framed block opening a fresh source
/// line: its preceding siblings walked backward are optional inline
/// `WHITESPACE` then the line's `DOC_MARGIN`. Returns the lead re-lowered as
/// column-0 IR (`%` pinned, whitespace verbatim), or `None` when the block is
/// not led by a directly-preceding margin. Used to commit a `macrocode` chunk
/// raw during `DtxProse` reflow with its source frame margin — docstrip
/// recognizes the literal `%    \begin{macrocode}` line, so the canonical `% `
/// re-emitted for prose must never replace a frame lead.
fn dtx_env_line_lead(node: &SyntaxNode) -> Option<Ir> {
    let mut ws: Option<String> = None;
    let mut prev = node.prev_sibling_or_token();
    if let Some(SyntaxElement::Token(t)) = &prev
        && t.kind() == SyntaxKind::WHITESPACE
        && !t.text().contains('\n')
    {
        ws = Some(t.text().to_string());
        prev = t.prev_sibling_or_token();
    }
    match prev {
        Some(SyntaxElement::Token(t)) if t.kind() == SyntaxKind::DOC_MARGIN => {
            let margin = Ir::column_zero(t.text());
            Some(match ws {
                Some(ws) => Ir::concat([margin, Ir::verbatim(ws)]),
                None => margin,
            })
        }
        _ => None,
    }
}

fn lower_expl_paragraph(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    let elements: Vec<SyntaxElement> = node.children_with_tokens().collect();
    let mut segments: Vec<Ir> = Vec::new();
    let mut seps: Vec<Ir> = Vec::new();
    let mut i = 0;
    while i < elements.len() {
        // Trivia straddling a run boundary feeds the join separator (one break,
        // or a preserved blank line), never the run itself: the separator
        // already begins the fresh line, so a leading newline inside the run
        // would double it — and each pass would grow a blank line.
        let mut boundary_newlines = 0;
        if !segments.is_empty() {
            while i < elements.len() {
                let SyntaxElement::Token(t) = &elements[i] else {
                    break;
                };
                if !is_collapsible_trivia(t.kind()) {
                    break;
                }
                boundary_newlines += t.text().matches('\n').count();
                i += 1;
            }
            if i >= elements.len() {
                break;
            }
        }
        let in_region = cx.in_expl3_region(elements[i].text_range().start());
        let start = i;
        while i < elements.len()
            && cx.in_expl3_region(elements[i].text_range().start()) == in_region
        {
            i += 1;
        }
        // The region ends at the `\ExplSyntaxOff` token, but a comment after
        // inline whitespace still belongs to that physical line. Lend the
        // comment to the expl3 run for layout only; the parser's region remains
        // catcode-exact, while `lower_expl_code` keeps the comment trailing and
        // prevents it from rebinding to the next command on the following pass.
        if in_region
            && elements[start..i]
                .iter()
                .rev()
                .find(|element| !is_collapsible_trivia_element(element))
                .is_some_and(is_expl_syntax_off_command)
        {
            let mut comment = i;
            while let Some(SyntaxElement::Token(token)) = elements.get(comment)
                && token.kind() == SyntaxKind::WHITESPACE
                && !token.text().contains(['\r', '\n'])
            {
                comment += 1;
            }
            if matches!(
                elements.get(comment),
                Some(SyntaxElement::Token(token)) if token.kind() == SyntaxKind::COMMENT
            ) {
                i = comment + 1;
            }
        }
        // The trailing half of the boundary-trivia rule above: trivia at the end
        // of a run also feeds the separator. Left inside the run, a preserved
        // run-final newline would stack with the hard-line separator into a blank
        // line the next pass keeps growing. (Skipped for an all-trivia run, so
        // `i` always advances.)
        if i < elements.len() {
            let mut end = i;
            while end > start && is_collapsible_trivia_element(&elements[end - 1]) {
                end -= 1;
            }
            if end > start {
                i = end;
            }
        }
        let run = &elements[start..i];
        let guarded_text = (!in_region).then(|| {
            let text = run.iter().map(ToString::to_string).collect::<String>();
            text_is_fully_guarded(&text).then_some(text)
        });
        let ir = if let Some(text) = guarded_text.flatten() {
            // Region subtraction deliberately hands fully guarded `.dtx` lines
            // to the non-expl3 side. They remain byte-faithful there: generic
            // lowering would still collapse gaps when a guard is nested inside
            // a command/group rather than exposed as a direct paragraph token.
            // The leading boundary newline belongs to the separator that opened
            // this run, so reproduce from the first guard-bearing element.
            Ir::verbatim(text.trim_start_matches(['\r', '\n']))
        } else if in_region {
            lower_expl_code(run.iter().cloned(), cx, Statements::Structural)
        } else if cx.wraps_prose() && !run_carries_doc_margin(run, cx) {
            reflow_elements(run.iter().cloned(), cx, ReflowKind::Prose)
        } else if cx.wraps_prose() && dtx_run_reflows_safely(run, cx) {
            // `.dtx` documentation prose between expl3 regions, riding `% `
            // margins: reflow it under the margin like a doc paragraph. Gated
            // exactly like [`lower_dtx_doc_paragraph`] — margined first line, no
            // environments, and the speculative escape probe — so a run the
            // reflow cannot keep under the margin still falls through below.
            reflow_elements(run.iter().cloned(), cx, ReflowKind::DtxProse)
        } else {
            // Either a non-wrapping mode, or `.dtx` documentation-layer text
            // between expl3 regions that the gate above declined: it rides
            // margin-framed `macrocode` frames or unmargined lines that generic
            // prose reflow would relocate off column 0 — a meaning change that
            // leaves the next pass unparseable — so it takes the byte-faithful
            // stream in every wrap mode.
            Ir::concat(lower_element_stream(run.iter().cloned(), cx))
        };
        if !matches!(ir, Ir::Nil) {
            if !segments.is_empty() {
                seps.push(if boundary_newlines >= 2 {
                    Ir::empty_line()
                } else {
                    Ir::hard_line()
                });
            }
            segments.push(ir);
        }
    }
    let mut result = Vec::with_capacity(segments.len().saturating_mul(2));
    for (n, seg) in segments.into_iter().enumerate() {
        if n > 0 {
            result.push(seps[n - 1].clone());
        }
        result.push(seg);
    }
    Ir::concat(result)
}

/// Whether every nonempty physical line in `text` begins with a docstrip guard.
/// Guards are recognized only at column zero, so the spelling is the structural
/// fact the `.dtx` lexer exposes as `GUARD` before any formatter pass can move it.
fn text_is_fully_guarded(text: &str) -> bool {
    let mut content_lines = text.lines().filter(|line| !line.is_empty());
    content_lines
        .next()
        .is_some_and(|line| line.starts_with("%<"))
        && content_lines.all(|line| line.starts_with("%<"))
}

/// How [`lower_expl_code`] finds statement boundaries: **structurally**, from
/// the argspec-arity segmentation ([`segment_expl_statements`] — a call unit
/// per logical line, owned by the formatter, with the authored physical line
/// as the per-statement fallback for underivable heads), or not at all
/// ([`Statements::Ignore`]: within one command's attached arguments, where a
/// newline is inert catcode-9 whitespace and the width fill owns the breaks).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Statements {
    Structural,
    Ignore,
}

/// Lay out a stream of elements known to be inside an expl3 region as expl3 code.
///
/// In an expl3 region TeX catcodes change: source spaces/tabs are **ignored**
/// (catcode 9) and `~` is a **literal space** (catcode 10). Source whitespace is
/// therefore insignificant and the formatter owns layout:
/// - **Statements** are structural *call units*: the boundary map from
///   [`segment_expl_statements`] — a head command plus the arguments its
///   argspec arity consumes — decides where logical lines commit, so
///   `\cs_new:Npn \foo:n #1 {…}` is one statement even though it is several
///   sibling CST nodes, regardless of where the author's newlines fell (one
///   call per line, formatter-owned: same-line calls split, mid-call newlines
///   join). A head with no derivable arity falls back to the authored physical
///   line for that statement (Tier 2; see [`crate::semantic::expl3`]). The exception
///   is *within one command's attached arguments* ([`Statements::Ignore`]),
///   where a newline is inert whitespace and only the width fill breaks —
///   see the `COMMAND` arm in [`lower_node`].
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
fn lower_expl_code(
    elements: impl Iterator<Item = SyntaxElement>,
    cx: LowerCtx<'_>,
    statements: Statements,
) -> Ir {
    let elements: Vec<SyntaxElement> = elements.collect();
    // The statement-boundary map (structural mode only): computed over the very
    // element vector lowered below, so indices align by construction.
    let map = (statements == Statements::Structural).then(|| segment_expl_statements(&elements));
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

    /// The fill a line's `[atom, sep, atom, …]` parts commit as: sticky for a
    /// structural statement, hugging for a fallback (or junk-glued) one — see
    /// `commit_line`. Every early line commit must build its head with this,
    /// not a bare [`Ir::Fill`]: a statement that ends one atom earlier on the
    /// next pass hands the same atoms to a different arm, and the two arms have
    /// to measure them the same way (`xo-place.dtx`).
    fn line_fill(mut parts: Vec<Ir>, sticky: bool) -> Ir {
        if parts.len() == 1 {
            return parts.drain(..).next().unwrap();
        }
        if sticky {
            Ir::StickyFill(parts.into())
        } else {
            Ir::HugFill(parts.into())
        }
    }

    /// Commit the in-progress line (if any) as the next logical line, recording the
    /// pending line separator before it and resetting line state (`sticky` resets
    /// to `true`, the structural default).
    fn commit_line(
        atom: &mut Vec<Ir>,
        parts: &mut Vec<Ir>,
        sep_before_next: &mut Option<Ir>,
        lines: &mut Vec<Ir>,
        seps: &mut Vec<Ir>,
        pending_sep: &mut Ir,
        sticky: &mut bool,
    ) {
        flush_atom(atom, parts, sep_before_next);
        if !parts.is_empty() {
            // A multi-atom *structural* line is a *sticky* fill: greedy, but
            // once a hanging brace argument detonates onto its own line every
            // later argument follows, rather than an empty false-branch gluing
            // back onto the block's short closing `}` line — a glue that is
            // not pass-stable, since where the block's own body breaks is not
            // pass-invariant (issue #94). A *fallback* (or junk-glued) line
            // instead commits as a plain greedy fill: greedy packing is
            // self-fulfilling (each printed line re-segments to a fallback
            // statement that re-fills to itself), while a sticky cascade
            // forces atoms that would fit onto broken lines — a shape the next
            // pass's shorter per-line statements do not reproduce. It is a
            // *hugging* greedy fill ([`Ir::HugFill`]): an atom that detonates
            // is measured by its first line, so it stays glued to the head
            // before it (`\vbox to \Gin@req@height{%`) instead of dropping to
            // a line of its own — a placement that would otherwise be keyed on
            // forced-ness, which is exactly what is not pass-invariant here.
            let line = line_fill(std::mem::take(parts), *sticky);
            seps.push(std::mem::replace(pending_sep, Ir::hard_line()));
            lines.push(line);
        }
        parts.clear();
        *sep_before_next = None;
        *sticky = true;
    }

    // True right after a multi-line block was pushed as its own line, surviving
    // an inline (newline-free) whitespace run: a trailing comment there rides
    // the block's closing line (`}%`, the macro-code continuation idiom).
    // Stranding it would mint a fresh *own-line* comment, which the next parse
    // binds leading into the following command — a different
    // shape, so a different layout: idempotence would break.
    let mut after_block = false;
    // Whether the line in progress commits as a sticky fill (structural
    // statements) or a plain greedy fill (fallback/junk-glued statements) —
    // see `commit_line`. Any fallback-marked element makes its line greedy.
    let mut line_sticky = true;
    let mut idx = 0;
    while idx < elements.len() {
        if let Some(m) = map.as_ref()
            && (m.is_fallback(idx) || m.is_glued(idx))
        {
            line_sticky = false;
        }
        match &elements[idx] {
            // Insignificant whitespace: a gap the boundary map marks ends the
            // logical line, a blank line promotes the next line separator, and
            // any other run is a single (breakable) space before the next atom.
            // The run's newline count is read only for the blank-line promotion
            // and the `after_block` clear — both preserved predicates — never
            // for the boundary itself (trivia-invariant layout).
            SyntaxElement::Token(token) if is_collapsible_trivia(token.kind()) => {
                let run_start = idx;
                let newlines = consume_widened_gap_slice(&elements, &mut idx);
                let boundary = map
                    .as_ref()
                    .is_some_and(|m| run_start > 0 && m.boundary_after(run_start - 1));
                if boundary {
                    after_block = false;
                    commit_line(
                        &mut atom,
                        &mut parts,
                        &mut sep_before_next,
                        &mut lines,
                        &mut seps,
                        &mut pending_sep,
                        &mut line_sticky,
                    );
                    if newlines >= 2 {
                        pending_sep = Ir::empty_line();
                    }
                } else {
                    // A trailing comment glues only to a directly-abutting `}`,
                    // never across a line break (own-line-ness is preserved).
                    if newlines >= 1 {
                        after_block = false;
                    }
                    flush_atom(&mut atom, &mut parts, &mut sep_before_next);
                    // Keep a tie's soft break if one is already pending. A gap
                    // before a recognized head mid-way through a fallback
                    // statement, or any top-level gap of a junk-bearing glued
                    // statement, is an unbreakable literal space: a width wrap
                    // there would move material across a printed line boundary
                    // the next pass segments differently (see
                    // [`StatementMap::glue_before`] and
                    // [`StatementMap::is_glued`]) — the line overflows instead.
                    if sep_before_next.is_none() {
                        // Inside a command node's own stream (`Ignore` — no
                        // boundary map), a gap before a *bare* argument (a
                        // consumed `#`-parameter, relation, or bare command;
                        // anything but a `{…}`/`[…]`) is unbreakable:
                        // `glue_before`'s rationale one level down, and the
                        // house style's — a call breaks before a braced
                        // argument, never between its single-token operands.
                        // A width wrap here would emit a newline inside the
                        // node before a non-group child, the shape the
                        // fallback segmentation reads as a statement boundary
                        // (`semantic::expl3::fallback_line`'s bare-line-break
                        // rule); hardening the gap is what makes that rule's
                        // fixed-point argument total: the formatter can only
                        // ever break a command node before a braced argument.
                        let bare_arg_glue = map.is_none() && elements.get(idx).is_some_and(|el| {
                            !matches!(
                                el,
                                SyntaxElement::Node(n)
                                    if matches!(n.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL)
                            ) && !matches!(
                                el,
                                SyntaxElement::Token(t) if t.kind() == SyntaxKind::COMMENT
                            )
                        });
                        sep_before_next = Some(
                            if bare_arg_glue
                                || map
                                    .as_ref()
                                    .is_some_and(|m| m.glue_before(idx) || m.is_glued(idx))
                            {
                                Ir::verbatim(" ")
                            } else {
                                Ir::Line
                            },
                        );
                    }
                }
                continue;
            }
            // `~`: a literal space. Glue it to the end of the current atom, then
            // close the atom with a soft break (flat: nothing; broken: newline).
            // A tie directly before a recognized head mid-fallback-statement
            // must not break either (`xo-or.dtx`'s `=~ \exp_not:c {…}` trace
            // lines), so that gap renders as nothing (`Nil`, the soft break's
            // flat form).
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::TILDE => {
                after_block = false;
                atom.push(Ir::verbatim(token.text()));
                flush_atom(&mut atom, &mut parts, &mut sep_before_next);
                let next = elements[idx + 1..]
                    .iter()
                    .position(|el| {
                        !matches!(el, SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()))
                    })
                    .map(|off| idx + 1 + off);
                sep_before_next = Some(
                    if next.is_some_and(|n| {
                        map.as_ref()
                            .is_some_and(|m| m.glue_before(n) || m.is_glued(n))
                    }) {
                        Ir::Nil
                    } else {
                        Ir::SoftLine
                    },
                );
            }
            // A comment ends its line (it must terminate the source line). One
            // trailing a multi-line block glues onto the block's closing line
            // (see `after_block` above), spaced when the source spaced it. One
            // trailing code rides the committed line *outside* its width fill
            // — zero-width, rustfmt-style: the line may overflow, but prose
            // length never re-breaks code, and relocating the comment would
            // rebind it as the next statement's leading doc comment on the
            // second pass, changing its attachment. An own-line
            // comment stays its own line.
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::COMMENT => {
                if after_block {
                    let block = lines.pop().expect("after_block implies a pushed line");
                    let spaced = sep_before_next.take().is_some();
                    lines.push(Ir::concat(if spaced {
                        vec![block, Ir::verbatim(" "), Ir::verbatim(token.text())]
                    } else {
                        vec![block, Ir::verbatim(token.text())]
                    }));
                    after_block = false;
                } else if atom.is_empty() && parts.is_empty() {
                    atom.push(Ir::verbatim(token.text()));
                    commit_line(
                        &mut atom,
                        &mut parts,
                        &mut sep_before_next,
                        &mut lines,
                        &mut seps,
                        &mut pending_sep,
                        &mut line_sticky,
                    );
                } else {
                    // A non-empty `atom` means the comment directly abuts it
                    // (any trivia would have flushed the atom); an empty one
                    // means source whitespace preceded, kept as one space.
                    let spaced = atom.is_empty();
                    commit_line(
                        &mut atom,
                        &mut parts,
                        &mut sep_before_next,
                        &mut lines,
                        &mut seps,
                        &mut pending_sep,
                        &mut line_sticky,
                    );
                    let line = lines
                        .pop()
                        .expect("trailing comment follows committed code");
                    let comment = if spaced {
                        format!(" {}", token.text())
                    } else {
                        token.text().to_string()
                    };
                    lines.push(Ir::concat(vec![line, Ir::zero_width(comment)]));
                }
            }
            // A docstrip guard (`%<…>`) is recognized only at column 0, so it must
            // lead its output line. Under `Statements::Ignore` (a command's attached
            // arguments) source newlines are catcode-9 whitespace, so without this a
            // guard between two arguments packs onto the previous line as a trailing
            // `%<…>` comment — losing its guard meaning and re-lexing on the next
            // parse as an ordinary comment that swallows the following argument's
            // braces (issue #78, l3backend-basics.dtx's per-backend `.def` list).
            // Commit the line in progress so the guard opens a fresh one, where
            // `lower_loose_token` pins it to column 0; the following code stays on
            // the guard's line via the ordinary inter-token fill.
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::GUARD => {
                after_block = false;
                commit_line(
                    &mut atom,
                    &mut parts,
                    &mut sep_before_next,
                    &mut lines,
                    &mut seps,
                    &mut pending_sep,
                    &mut line_sticky,
                );
                atom.push(lower_loose_token(token, cx));
            }
            SyntaxElement::Token(token) => {
                after_block = false;
                atom.push(lower_loose_token(token, cx));
            }
            // A command with a bound leading `DOC_COMMENT` (an
            // own-line comment binds forward). Rendered as an opaque block it
            // would strand a blank line after the comment (the comment's own
            // newline stacking with the block separator) and split the
            // statement — a shape the next parse reads differently, breaking
            // idempotence. Instead each comment line commits as its own line
            // and the command's remaining children continue the statement.
            SyntaxElement::Node(child)
                if child.kind() == SyntaxKind::COMMAND
                    && child
                        .first_child()
                        .is_some_and(|c| c.kind() == SyntaxKind::DOC_COMMENT) =>
            {
                after_block = false;
                commit_line(
                    &mut atom,
                    &mut parts,
                    &mut sep_before_next,
                    &mut lines,
                    &mut seps,
                    &mut pending_sep,
                    &mut line_sticky,
                );
                let mut rest: Vec<SyntaxElement> = Vec::new();
                for el in child.children_with_tokens() {
                    match &el {
                        SyntaxElement::Node(n) if n.kind() == SyntaxKind::DOC_COMMENT => {
                            for t in n.children_with_tokens() {
                                if let SyntaxElement::Token(t) = t
                                    && t.kind() == SyntaxKind::COMMENT
                                {
                                    seps.push(std::mem::replace(&mut pending_sep, Ir::hard_line()));
                                    lines.push(Ir::verbatim(t.text()));
                                }
                            }
                        }
                        _ => rest.push(el.clone()),
                    }
                }
                let ir = lower_expl_code(rest.into_iter(), cx, Statements::Ignore);
                if !matches!(ir, Ir::Nil) {
                    atom.push(ir);
                }
            }
            SyntaxElement::Node(child) => {
                after_block = false;
                // A junk-bearing *glued* statement bypasses every layout
                // special: its nodes accumulate as plain atoms joined by the
                // hard separators the trivia arm supplies, so the authored
                // line shape survives verbatim (see [`StatementMap::is_glued`]
                // — a conditional explosion or head-hug here would commit a
                // line mid-statement and strand the junk on a fresh line the
                // next pass segments differently).
                let in_glued = map.as_ref().is_some_and(|m| m.is_glued(idx));
                // R2 ("everything divided up using spaces"): an expl3 function's
                // brace argument written flush against its head
                // (`\clist_count:n{#1}`) gets the house style's space. Synthesized
                // *here*, by flushing the atom exactly as a source gap would, so
                // every branch below sees the state the spaced spelling produces —
                // hang, conditional explosion, trailing-hang candidates — with no
                // second spelling to special-case. Inserting the space is a trivia
                // edit and catcode-safe: in-region source spaces are catcode 9, so
                // the token stream is unchanged (a real space is `~`, catcode 10).
                // Junk-glued statements are exempt: their authored line shape is
                // load-bearing (see above).
                if !in_glued
                    && !atom.is_empty()
                    && child.kind() == SyntaxKind::GROUP
                    && expl_arg_takes_leading_space(child)
                {
                    flush_atom(&mut atom, &mut parts, &mut sep_before_next);
                    sep_before_next = Some(Ir::Line);
                }
                // A *statement-leading* expl3 conditional (`\…:nTF {c} {T} {F}`,
                // nothing on the logical line before it) explodes structurally: the
                // head on its own line, then each `T`/`F` branch on its own line at
                // +6 (R4/R5), regardless of whether it would fit inline. Keyed on the
                // command name's argspec suffix ([`expl3::conditional_branches`]); a
                // conditional used mid-line as a value (`,key = \…:nTF …`, atom or
                // parts non-empty) is not statement-leading and stays on the
                // width-driven head-hug path (issue #71).
                //
                // [`expl_conditional_at`] covers the branches wherever greedy
                // attachment put them — on the head, or on a sibling an `N`/`V` slot
                // handed them to — so the explosion does not depend on an accident of
                // the surrounding tokens, and the unit may span several siblings
                // (hence the `last`-driven resume rather than `idx += 1`).
                if !in_glued
                    && parts.is_empty()
                    && atom.is_empty()
                    && child.kind() == SyntaxKind::COMMAND
                    && let Some((cond_ir, last)) = expl_conditional_at(&elements, idx, cx)
                {
                    seps.push(std::mem::replace(&mut pending_sep, Ir::hard_line()));
                    lines.push(cond_ir);
                    after_block = true;
                    idx = last + 1;
                    continue;
                }
                // A *trailing* expl3 conditional — one used mid-line as a value, with
                // head atoms before it on the line and only trivia after it in the
                // statement — is width-conditional. It stays flat on the line when the
                // whole statement fits (issue #71's `,key = \…:nTF {c} {T} {F}` shape),
                // but when head and conditional together overflow, the head drops to
                // its own line, which makes the conditional *statement-leading* on the
                // next parse so it then explodes unconditionally (R4). Committing head
                // and conditional together as one `group(IfBreak { flat, broken })`
                // — measured by the group's *flat* width (head included, so neither
                // fooled by a branch that detonates internally nor evaluated apart
                // from its head) — makes the passes agree: it fits => `head cond` on
                // one line; it overflows => head on its own line then the R4
                // explosion, which re-parses statement-leading and re-explodes to the
                // identical bytes (idempotency failure, `lthooks.dtx`, issue #96). The
                // `!(…)` guard leaves the statement-leading position to the block above.
                // Suppressed in a *fallback* statement too: a conditional name
                // mid-way through an unrecognized line is data being spliced
                // (`xtemplate`'s `cs_ \str_if_eq:nnT {#1} { global } { g }
                // set:Npn` name assembly), not a call — and whether it sits
                // trailing depends on where the line's junk ends, which is not
                // pass-invariant.
                //
                // **Head-attached branches only** — deliberately, unlike the
                // statement-leading arm above. Mid-statement the conditional is not
                // the head of its own unit; the enclosing segmentation already
                // decided it is an *argument* being passed as a token
                // (`\@@_patch_check:NNnn \cs_if_exist:NTF #1 { undef } {…}`, where
                // `{ undef }` is `\@@_patch_check:NNnn`'s third argument, and
                // `\exp_not:N \…:nTF`). Re-scanning it as a head would resolve
                // "branches" belonging to the outer call and explode them — a misread
                // in every one of the eight latex2e/latex3 sites it reached.
                // [`lower_expl_conditional`] reads only the node's own greedily
                // attached children, so it cannot make that mistake.
                let in_fallback = map.as_ref().is_some_and(|m| m.is_fallback(idx));
                let statement_leading = parts.is_empty() && atom.is_empty();
                if !in_glued
                    && !in_fallback
                    && !statement_leading
                    && child.kind() == SyntaxKind::COMMAND
                    && is_trailing_in_statement(&elements, idx, map.as_ref())
                    && let Some(exploded) = command_name(child)
                        .and_then(|name| expl3::conditional_branches(&name))
                        .and_then(|n| lower_expl_conditional(child, cx, n))
                {
                    // The head↔conditional separator: a space when trivia flushed the
                    // atom (`… \…:nTF`), nothing when the conditional directly abuts
                    // the atom in progress (`…\…:nTF`, no space). `flush_atom`'s own
                    // `sep_before_next` handles the *internal* head joins.
                    let sep = if atom.is_empty() {
                        sep_before_next.take().unwrap_or(Ir::Line)
                    } else {
                        Ir::Nil
                    };
                    flush_atom(&mut atom, &mut parts, &mut sep_before_next);
                    let head = if parts.len() == 1 {
                        parts.drain(..).next().unwrap()
                    } else {
                        Ir::StickyFill(std::mem::take(&mut parts).into())
                    };
                    let flat = Ir::concat(vec![head.clone(), sep, lower_node(child, cx)]);
                    let broken = Ir::concat(vec![head, Ir::hard_line(), exploded]);
                    seps.push(std::mem::replace(&mut pending_sep, Ir::hard_line()));
                    lines.push(Ir::group(Ir::if_break(flat, broken)));
                    after_block = true;
                    idx += 1;
                    continue;
                }
                // A *trailing* greedily-hung `{body}` — a brace group with a head
                // before it on the line and only trivia after it in the statement —
                // whose body is a *multi-command* fill (`\int_gset:Nn \g…_int {…}`)
                // flips K&R->Allman across passes. When the body is authored on one
                // source line and overflows, the ordinary hang path hangs the `{`
                // off the head and lets the fill wrap (K&R) on pass 1; but those
                // wrapped lines re-parse as several statements (source newlines are
                // statement boundaries inside braces), so the body then carries a
                // forced break and detonates Allman on pass 2 — the two passes
                // disagree (idempotency failure, `tagpdf.sty` line 1007,
                // `latex-lab-testphase-bookmark.sty` line 298).
                //
                // Decide instead with a three-candidate all-lines-fit group over the
                // *same* body IR — flat (whole statement on one line), Allman-inline
                // (head on its own line, `{ body }` inline), Allman-broken (`{` on
                // its own line, body wrapped) — keyed on the body's *real* one-line
                // fit rather than its authored line count, so a body that fits picks
                // the same form on every pass and one that wraps picks Allman-broken
                // on every pass. The narrow guards below keep this off the shapes the
                // existing hang path already lays out stably: a single-command or
                // bare-value body (no top-level wrap), a forced-break body
                // (comment/guard/margin, or already multi-statement — plain Allman),
                // and the multi-argument/conditional-branch shapes (a preceding
                // trailing group, or a grouped earlier command) whose head this
                // branch cannot measure as one unit from inside a single argument.
                if child.kind() == SyntaxKind::GROUP
                    && atom.is_empty()
                    && !parts.is_empty()
                    && !expl_group_forces_break(child)
                    && expl_group_body_is_multi_atom(child)
                    && is_trailing_in_statement(&elements, idx, map.as_ref())
                    && !statement_has_preceding_group(&elements, idx, map.as_ref())
                    && !head_command_has_grouped_sibling_arg(child)
                    && let ExplGroupPieces::Pieces {
                        open_ir,
                        body,
                        close_ir,
                        spaced,
                        forced: false,
                    } = expl_group_pieces(child, SyntaxKind::L_BRACE, SyntaxKind::R_BRACE, cx)
                    // A body that already carries a *forced* break (a nested group
                    // detonated on width, or the body holds several statements) can
                    // never be a one-liner: the flat/K&R candidates would render a
                    // multi-line body as a `flat` pick and freeze a hybrid the next
                    // parse re-breaks. Such a body wants the plain Allman block, which
                    // the ordinary hang path already emits stably, so fall through.
                    // This read of the non-pass-invariant forced-break predicate is
                    // safe *because* of that fall-through: the ordinary hang path's
                    // plain Allman is byte-identical to `c_allman_broken`, the
                    // candidate the three-way itself picks for a body that wraps, so
                    // a body flipping soft->forced across passes lands on the same
                    // bytes either way. (`expl_group_body_is_multi_atom` needs two
                    // top-level commands, and two *recognized* commands always mint a
                    // structural boundary, so this arm only ever sees fallback bodies
                    // — gating it on `!in_fallback` would disable it entirely.)
                    && !body.contains_forced_break()
                {
                    let sep = sep_before_next.take().unwrap_or(Ir::Line);
                    flush_atom(&mut atom, &mut parts, &mut sep_before_next);
                    let head = if parts.len() == 1 {
                        parts.drain(..).next().unwrap()
                    } else {
                        Ir::StickyFill(std::mem::take(&mut parts).into())
                    };
                    let space = if spaced { Ir::verbatim(" ") } else { Ir::Nil };
                    // Three candidates, chosen by all-lines-fit — every one measured
                    // with its nested brace groups forced *flat*, so a candidate is
                    // accepted only when its content genuinely lays out that way (no
                    // inner group silently detonating to keep each printed line short,
                    // the K&R hybrid). The bodies are therefore raw (no soft group),
                    // and the structural `HardLine`s alone shape each candidate.
                    //
                    // 1. Flat: the whole statement on one line, `{ body }` glued.
                    // 2. Allman-inline: the head on its own line, `{ body }` inline one
                    //    step under it — taken when the body fits one line there.
                    // 3. Allman-broken: `{` on its own line, the body a further step,
                    //    `}` back — the fallback for a body that wraps.
                    //
                    // Both Allman forms re-parse to a head statement followed by a
                    // statement-leading `{body}`, which the continuation hang branch
                    // below re-emits identically (the inline/broken split is then the
                    // soft group's own width choice), so each is a fixed point. Keying
                    // the K&R↔Allman decision on the body's real one-line fit — not on
                    // how many source lines the body happened to occupy — is what
                    // removes the flip (`tagpdf.sty`,
                    // `latex-lab-testphase-bookmark.sty`).
                    let c_flat = Ir::concat(vec![
                        head.clone(),
                        sep,
                        open_ir.clone(),
                        space.clone(),
                        body.clone(),
                        space.clone(),
                        close_ir.clone(),
                    ]);
                    let c_allman_inline = Ir::concat(vec![
                        head.clone(),
                        Ir::indent(Ir::concat(vec![
                            Ir::hard_line(),
                            open_ir.clone(),
                            space.clone(),
                            body.clone(),
                            space,
                            close_ir.clone(),
                        ])),
                    ]);
                    let c_allman_broken = Ir::concat(vec![
                        head,
                        Ir::indent(Ir::concat(vec![
                            Ir::hard_line(),
                            open_ir,
                            Ir::indent(Ir::concat(vec![Ir::hard_line(), body])),
                            Ir::hard_line(),
                            close_ir,
                        ])),
                    ]);
                    seps.push(std::mem::replace(&mut pending_sep, Ir::hard_line()));
                    lines.push(Ir::conditional_group_all_lines(vec![
                        c_flat,
                        c_allman_inline,
                        c_allman_broken,
                    ]));
                    after_block = true;
                    idx += 1;
                    continue;
                }
                // A brace group that *starts a fresh atom* (nothing glued before
                // it — any trivia flushed the atom) is a *continuation*: it indents
                // one step under its head statement, the l3 house style
                // (`\cs_new:Npn \foo:n #1` / `  { body }`, or `\bool_if:nTF {cond}`
                // / `  { true }`). The step is carried by an `Indent` folded around
                // the break *and the group alone* — never the rest of the line,
                // whose atoms the reparse reads as ordinary base-indent statements
                // when a width break separates them (the group's own internal lines
                // must sit at the deeper level either way, so break and body travel
                // together). This holds identically whether statement boundaries
                // are structural (`Structural`) or absent within one command's
                // attached arguments (`Ignore`), so the rule keys only on the
                // group shape, not the statement mode.
                let hang_group = child.kind() == SyntaxKind::GROUP && atom.is_empty();
                let starts_line = parts.is_empty();
                // Each brace argument breaks on its own merits: the l3styleguide's
                // own example keeps a short branch inline (`{ \module_foo_aux:n
                // { X #2 } }`) beside a multi-line sibling, and l3kernel does the
                // same throughout. A sibling's forced break is not this group's
                // business.
                let ir = lower_node(child, cx);
                // No arm of the forced-break dispatch below fires inside a
                // *fallback* statement: there, forced-ness is not
                // pass-invariant. A fallback statement's extent is the authored
                // physical line (Tier 2), so a width wrap inside the group's own
                // *body* prints newlines the reparse re-segments into several
                // fallback statements — the body's IR gains a `HardLine` and a
                // soft group flips forced on pass 2 (l3kernel `expl3.sty`'s
                // backend `.choices:nn` value; latex2e `lipsum.sty`'s
                // `\int_do_until:nNnn` loop).
                //
                // The forced arm's only effect over the soft path is to *commit
                // the line*, i.e. to force every later atom onto its own line —
                // which is exactly what a `StickyFill` does anyway, since a
                // forced atom's `flat_width` is `None` and
                // `printer::step_fill`'s `remainder_broken` then fires
                // unconditionally. Structural statements and
                // [`Statements::Ignore`] streams are both sticky, so the two
                // paths agree there; a *fallback* line commits as a greedy fill
                // with no cascade, so they disagree, and the sibling after
                // the group glued onto the closing `}` on pass 1 and dropped to
                // its own line on pass 2. Committing mid-statement also falsifies
                // the plain fill's own fixed-point argument (each printed line
                // re-segments to a fallback statement that re-fills to itself,
                // [`StatementMap::is_fallback`]) and silently drops the
                // unbreakable `glue_before` space, since `flush_atom` emits a
                // pending separator only when `parts` is non-empty.
                //
                // Nothing is lost by falling through to the soft branches below.
                // The *hanging group* keeps its own break either way: its
                // `flat_width` is `None`, so its leading gap breaks on every pass
                // at every width — only the *sibling* gap after it is left to the
                // fill, which is the point. The other three arms (head-hug, the
                // abutting-atom glue, the no-head-to-hug commit) differ from the
                // fill only in *committing the line*, and a fallback line's fill
                // hugs ([`Ir::HugFill`], `commit_line`): a detonating atom is
                // measured by its first line, so it stays on the head's line
                // exactly as `group_hug` would have put it, and the atoms after it
                // are left to the fill instead of being stranded — which is what
                // re-glues an authored abutment (`}\@ehc`, `}.`, `}{`) the
                // no-head-to-hug commit used to split.
                debug_assert!(
                    !in_fallback || !line_sticky,
                    "a fallback statement must commit as a hugging fill"
                );
                let forced_dispatch = !in_fallback && ir.contains_forced_break();
                // A junk-bearing glued statement: plain atom accumulation, hard
                // separators, no line commits until the boundary (see above).
                if in_glued {
                    atom.push(ir);
                }
                // A trailing command carrying a block argument, with a head
                // before it on the statement line: glue the head to the command
                // with an unbreakable space and let the command's own internal
                // hang absorb any overflow, so the head stays joined and only
                // the block breaks below. Deliberately checked *before* the
                // forced-break dispatch and applied to soft and forced bodies
                // alike: whether such a body is soft or forced is not
                // pass-invariant (a width wrap inside fallback content mints a
                // statement boundary the reparse reads as a hard break —
                // `expl_forced_block_body_mode`'s `\fp_eval:n` closing paren,
                // l3doc's `\string \indexentry {…} {…}` write bodies), so any
                // dispatch split on it — sticky fill when soft, `group_hug`
                // when forced — renders different bytes on the two passes.
                // Gluing renders identically either way: flat when everything
                // fits, joined head with the block hanging otherwise.
                // Structural streams only — under [`Statements::Ignore`] the
                // nested hang branches already treat soft and forced uniformly.
                else if map.is_some()
                    && child.kind() == SyntaxKind::COMMAND
                    && atom.is_empty()
                    && !parts.is_empty()
                    && is_trailing_in_statement(&elements, idx, map.as_ref())
                    && child.children().any(|c| c.kind() == SyntaxKind::GROUP)
                {
                    let head = line_fill(std::mem::take(&mut parts), line_sticky);
                    // The head↔command separator is the pending gap's *flat*
                    // form: a space for an ordinary inter-token gap, nothing
                    // after a tie (`plus ~\__char_show_code:n {…}` must not
                    // grow a space the next parse does not have).
                    let sep = match sep_before_next.take() {
                        Some(Ir::SoftLine) | Some(Ir::Nil) => Ir::Nil,
                        _ => Ir::verbatim(" "),
                    };
                    seps.push(std::mem::replace(&mut pending_sep, Ir::hard_line()));
                    lines.push(Ir::concat(vec![head, sep, ir]));
                    after_block = true;
                } else if forced_dispatch {
                    if !atom.is_empty() {
                        // A block hanging off a *directly-abutting* atom stays
                        // glued (`\cs_if_exist:NF\tag_if_active:T { … }` with a
                        // multi-line body): committing the atom alone would split
                        // the abutting pair — but on the pass before, when the
                        // body still fit softly, they rendered glued, so the two
                        // passes would never agree. Gluing is the fixed point:
                        // the pair abuts identically on every pass.
                        atom.push(ir);
                        commit_line(
                            &mut atom,
                            &mut parts,
                            &mut sep_before_next,
                            &mut lines,
                            &mut seps,
                            &mut pending_sep,
                            &mut line_sticky,
                        );
                    } else if hang_group {
                        // A multi-line brace group separated from its head by a
                        // space (Allman): end the current line — flushing any head
                        // (`\__kernel…` before its `{T}`) as its own line — then
                        // place the group on its own line(s) hung one step, folding
                        // the line separator into the `Indent` (the seps slot gets
                        // `Nil` so the two stay paired). The run's first line has no
                        // separator to fold and stays at the current level.
                        commit_line(
                            &mut atom,
                            &mut parts,
                            &mut sep_before_next,
                            &mut lines,
                            &mut seps,
                            &mut pending_sep,
                            &mut line_sticky,
                        );
                        if !lines.is_empty() {
                            let sep = std::mem::replace(&mut pending_sep, Ir::hard_line());
                            seps.push(Ir::Nil);
                            lines.push(Ir::indent(Ir::concat(vec![sep, ir])));
                        } else {
                            seps.push(std::mem::replace(&mut pending_sep, Ir::hard_line()));
                            lines.push(ir);
                        }
                    } else if !parts.is_empty() {
                        // Head-hug: a detonating *non-group* child (a command
                        // subtree whose first line is a head atom, e.g. the N-arg
                        // `\__kernel…{T}{F}` of `\cs_if_exist:NTF`) follows a head
                        // on this line, separated by a space. Keep them on one line
                        // when the prefix up to the block's first forced break fits,
                        // letting the block body break below — a rest-aware
                        // `group_hug`, so pass-stable (never the `step_fill` local
                        // cascade that would split a short head off a detonating
                        // trailing block).
                        let head = line_fill(std::mem::take(&mut parts), line_sticky);
                        let sep = sep_before_next.take().unwrap_or(Ir::Line);
                        seps.push(std::mem::replace(&mut pending_sep, Ir::hard_line()));
                        lines.push(Ir::group_hug(Ir::concat(vec![head, sep, ir])));
                    } else {
                        // A multi-line block with no head to hug and no group to
                        // hang: place it on its own line(s) at the current level.
                        commit_line(
                            &mut atom,
                            &mut parts,
                            &mut sep_before_next,
                            &mut lines,
                            &mut seps,
                            &mut pending_sep,
                            &mut line_sticky,
                        );
                        seps.push(std::mem::replace(&mut pending_sep, Ir::hard_line()));
                        lines.push(ir);
                    }
                    after_block = true;
                } else if hang_group && starts_line && !lines.is_empty() {
                    // Line-initial: fold the statement separator in; the
                    // `commit_line` seps slot then carries `Nil`.
                    let sep = std::mem::replace(&mut pending_sep, Ir::Nil);
                    atom.push(Ir::indent(Ir::concat(vec![sep, ir])));
                } else if hang_group && !starts_line {
                    // Mid-statement: if the width fill breaks at this gap, the
                    // group starts a continuation line hung one step. Flat, the
                    // leading `Line` is the single inter-token space (the `Nil`
                    // separator adds nothing). A `{`-led continuation line is
                    // safe in a fallback statement too: the reparse reads it as
                    // a statement-leading group and the continuation-hang fold
                    // below re-indents it identically. (A glued statement never
                    // reaches here — the `in_glued` arm above owns its nodes.)
                    // In a fallback statement this is also the *only* path a
                    // hanging group takes, forced or soft (`forced_dispatch`
                    // above): a forced body's `flat_width` is `None`, so
                    // `step_fill` dispatches this atom `Mode::Break` on every
                    // pass at every width and the leading `Line` breaks — the
                    // same bytes the forced arm would have emitted, minus its
                    // line commit.
                    sep_before_next = Some(Ir::Nil);
                    atom.push(Ir::indent(Ir::concat(vec![Ir::Line, ir])));
                } else {
                    atom.push(ir);
                }
            }
        }
        // A structural boundary whose gap holds no trivia (`\foo:\bar:`
        // abutting, or a statement split out of an authored same-line pair):
        // the trivia arm never sees a run there, so commit here. Re-committing
        // an already-committed line is a no-op.
        if map.as_ref().is_some_and(|m| m.boundary_after(idx)) {
            commit_line(
                &mut atom,
                &mut parts,
                &mut sep_before_next,
                &mut lines,
                &mut seps,
                &mut pending_sep,
                &mut line_sticky,
            );
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
        &mut line_sticky,
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

/// Whether an expl3-region brace group *forces* a broken (multi-line) block —
/// independent of width — because its body holds a docstrip guard, a comment, or a
/// `.dtx` margin, the same tokens that drive [`lower_expl_group`]'s block form. Used
/// as the forced-break *self* test in the trailing-hang guards of
/// [`lower_expl_code`] (a body that already forces a break wants the plain Allman
/// block, not the three-candidate hang). Keyed on this width-independent trigger
/// only, so the decision is a pass-stable function of the group's own content —
/// never of width, per the no-sibling-coupling rule (issue #101).
fn expl_group_forces_break(node: &SyntaxNode) -> bool {
    node.descendants_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .any(|t| {
            matches!(
                t.kind(),
                SyntaxKind::COMMENT | SyntaxKind::GUARD | SyntaxKind::DOC_MARGIN
            )
        })
}

/// Whether an expl3 brace group's body holds **two or more** top-level `COMMAND`
/// children — the shape whose K&R hang flips to an Allman block across passes. Two
/// top-level commands (`\int_gset:Nn \g…_int {…}`, `\cmd {x} ~ \cmd {y}`) form a
/// width fill, so when the body overflows it wraps onto several physical lines, which
/// the *next* parse reads as several statements — turning a pass-1 K&R hang (a soft
/// body fill) into a pass-2 Allman block (a forced break), so the two passes disagree
/// (`tagpdf.sty`, `latex-lab-testphase-bookmark.sty`).
///
/// A body with at most one top-level command (`{ \tl_put_right:Ne #3 {…} }` — the
/// command plus its loose `#3`/group arguments, or a bare value) stays one statement:
/// it fits, or it wraps only *inside* that one command's group, never at the top
/// level, so its hang is already pass-stable and is left on the ordinary hang path.
/// Reads only CST shape (top-level `COMMAND` count), no meaning.
fn expl_group_body_is_multi_atom(node: &SyntaxNode) -> bool {
    node.children()
        .filter(|n| n.kind() == SyntaxKind::COMMAND)
        .count()
        >= 2
}

/// Whether an expl3 brace group's body is a *simple run of parameters* — `{#1}`,
/// `{#1#2}`, `{##1}` — the l3styleguide's explicit exception to the
/// divide-with-spaces rule ("With the exception of simple runs of parameter
/// (`{#1}`, `#1#2`, etc.), everything should be divided up using spaces"). Such a
/// run stays tight even inside an expl3-named command's arguments. Outer padding
/// is ignored, so `{ #1 }` normalizes to tight `{#1}`; but any whitespace
/// *between* the parameters, or any non-parameter token, disqualifies the run, so
/// `{ #1 #2 }` and `{ X #2 }` keep the canonical inner spaces (matching the
/// l3styleguide's own worked example). Reads only token kinds and digit text — no
/// meaning, no signature lookup.
fn is_simple_param_run(node: &SyntaxNode) -> bool {
    // Body tokens with the delimiters dropped; a non-token child (a nested group
    // or command) is never a bare parameter run.
    let mut body: Vec<SyntaxToken> = Vec::new();
    for element in node.children_with_tokens() {
        match element {
            SyntaxElement::Token(t) => match t.kind() {
                SyntaxKind::L_BRACE
                | SyntaxKind::R_BRACE
                | SyntaxKind::L_BRACKET
                | SyntaxKind::R_BRACKET => {}
                _ => body.push(t),
            },
            SyntaxElement::Node(_) => return false,
        }
    }
    // Trim the padding whitespace we may be about to remove; any *interior*
    // whitespace survives and disqualifies the run below.
    let is_space =
        |t: &SyntaxToken| matches!(t.kind(), SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE);
    while body.first().is_some_and(is_space) {
        body.remove(0);
    }
    while body.last().is_some_and(is_space) {
        body.pop();
    }
    // A run is `#`s and single-digit indices, adjacent, each digit preceded by a
    // `#` (so `##1` counts, a stray `{1}` or `{ #1 #2 }` does not).
    let mut saw_hash = false;
    let mut prev_hash = false;
    for t in &body {
        match t.kind() {
            SyntaxKind::HASH => {
                saw_hash = true;
                prev_hash = true;
            }
            SyntaxKind::WORD if prev_hash && is_param_digit(t) => {
                prev_hash = false;
            }
            _ => return false,
        }
    }
    saw_hash
}

/// Whether the element at `idx` is the last *meaningful* element of its statement —
/// the boundary map ends the statement at it, or (under [`Statements::Ignore`],
/// where `map` is `None`) only collapsible trivia follows it in the stream. Used
/// to gate the trailing-conditional width-conditional lowering in
/// [`lower_expl_code`]: a conditional with content after it on the same statement
/// is not a clean trailing value, so it stays on the ordinary fill path. A `~`
/// (`TILDE`) or comment is not collapsible trivia, so it counts as following
/// content.
fn is_trailing_in_statement(
    elements: &[SyntaxElement],
    idx: usize,
    map: Option<&StatementMap>,
) -> bool {
    if let Some(m) = map
        && m.boundary_after(idx)
    {
        return true;
    }
    for element in &elements[idx + 1..] {
        match element {
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {}
            _ => return false,
        }
    }
    true
}

/// Whether an *earlier argument* of `child`'s owning command is a brace or
/// bracket group — the mark of a multi-argument call
/// (`\prop_get:NnNTF \g…_prop {#2} \l…_tl {branch}`) rather than a plain
/// `\cmd \target {body}` hang. The trailing-hang three-way sees only the
/// owning command's [`Statements::Ignore`] stream from `child` onward, so it
/// cannot tell the two apart without this look at the earlier children.
///
/// A recognized call owns every argument its argspec consumes, so earlier
/// grouped material appears among `child`'s preceding siblings. A node's
/// children form one statement.
fn head_command_has_grouped_sibling_arg(child: &SyntaxNode) -> bool {
    let Some(owner) = child.parent() else {
        return false;
    };
    // Only an *attached* argument has a head with sibling arguments: a
    // stream-level group's parent is the container itself, which is not a
    // call whose earlier slots could have consumed a group.
    if owner.kind() != SyntaxKind::COMMAND {
        return false;
    }
    let child_start = child.text_range().start();
    owner
        .children_with_tokens()
        .take_while(|el| el.text_range().start() < child_start)
        .any(|el| matches!(el.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL))
}

/// Whether a `GROUP`/`OPTIONAL` sibling precedes the element at `idx` within its
/// statement (back to the previous boundary in the map, or to the stream start
/// under [`Statements::Ignore`], where `map` is `None`). Used to keep the
/// trailing-hang three-way off a group that is *one of several* trailing brace
/// groups — the branch list of a conditional call whose N/V argument broke greedy
/// attachment (`\prop_get:NnNTF \g…_prop {#2} \l…_tl {T} {F}`), or any
/// multi-argument shape — where the existing hang path already lays the branches
/// out stably. A lone trailing group (`\l…_tl {body}`, `\bool_if:NF\…_bool
/// {body}`) has no preceding group and still qualifies.
fn statement_has_preceding_group(
    elements: &[SyntaxElement],
    idx: usize,
    map: Option<&StatementMap>,
) -> bool {
    for j in (0..idx).rev() {
        // A boundary after `j` puts `j` in the previous statement.
        if let Some(m) = map
            && m.boundary_after(j)
        {
            return false;
        }
        match &elements[j] {
            SyntaxElement::Node(n)
                if matches!(n.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Whether an expl3-region group's *flat* form carries the l3 house style's
/// canonical inner spaces (`{ value }`, per the l3styleguide) or stays tight
/// (`{parbox/after}`). Spaced when the group is the attached argument of an
/// expl3-*named* command — the name contains `_` or `:`, a purely lexical fact
/// (no signature or meaning lookup) — or a bare code block (an expl3 function
/// body). Tight when it belongs to an embedded LaTeX2e-named command
/// (`\UseTaggingSocket`, `\@parboxto`), whose authors write tight braces; the
/// house style governs expl3 functions, not 2e code that happens to sit inside
/// a region. Tight, too, for a *simple run of parameters* ([`is_simple_param_run`]),
/// the l3styleguide's own exception (`{#1}`, `#1#2`), regardless of the command.
fn expl_group_is_spaced(node: &SyntaxNode) -> bool {
    if is_simple_param_run(node) {
        return false;
    }
    let Some(parent) = node.parent() else {
        return true;
    };
    if parent.kind() != SyntaxKind::COMMAND {
        return true;
    }
    match command_name(&parent) {
        Some(name) => name.contains('_') || name.contains(':'),
        None => true,
    }
}

/// Whether an expl3-region brace group takes the l3 house style's space *before*
/// its opening brace, so a flush-written `\clist_count:n{#1}` is respaced to
/// `\clist_count:n {#1}`. True for the attached argument of an expl3-*named*
/// command — the name contains `_` or `:`, the same purely lexical fact
/// [`expl_group_is_spaced`] reads (no signature or meaning lookup).
///
/// Deliberately **not** subject to the *simple run of parameter* exception: that
/// exception governs a group's *inner* padding (`{#1}`, never `{ #1 }`), not the
/// gap before it. l3kernel writes the space either way — a sweep of its `.dtx`
/// sources counts 2883 spaced against 9 glued for parameter-run arguments alone,
/// and 8447 against 14 over all expl3-named heads.
///
/// False for an embedded LaTeX2e-named command (`\eqref{#1}`,
/// `\ProvidesExplPackage{demo}{…}`), whose authors write flush braces and whom the
/// house style does not govern; upstream is genuinely mixed there, so the authored
/// gap stands.
fn expl_arg_takes_leading_space(node: &SyntaxNode) -> bool {
    node.parent()
        .filter(|parent| parent.kind() == SyntaxKind::COMMAND)
        .and_then(|parent| command_name(&parent))
        .is_some_and(|name| name.contains('_') || name.contains(':'))
}

/// Lower a brace `{…}` or optional `[…]` group inside an expl3 region as a code
/// block: the body lays out as expl3 code ([`lower_expl_code`]) indented one step,
/// the whole wrapped in a soft [`Ir::group`] so it stays inline when it fits —
/// `{ body }` with canonical inner spaces for an expl3 function's argument or a
/// bare code block, tight `{body}` for an embedded 2e-named command's argument
/// ([`expl_group_is_spaced`]) — and detonates to an indented block when the body
/// spans lines or overflows. The inline-vs-block decision is width/structure
/// driven (never source newlines), keeping reformatting idempotent. Mirrors
/// [`lower_prose_group`] but recurses into expl3 code.
///
/// A group breaks only on its own body: a *sibling* argument's forced break is none
/// of its business. The l3styleguide's own worked example keeps a short true-branch
/// inline (`{ \module_foo_aux:n { X #2 } }`) beside a multi-line false-branch block,
/// and l3kernel follows that throughout.
fn lower_expl_group(
    node: &SyntaxNode,
    open: SyntaxKind,
    close: SyntaxKind,
    cx: LowerCtx<'_>,
) -> Ir {
    let (open_ir, body, close_ir, spaced, has_comment) =
        match expl_group_pieces(node, open, close, cx) {
            ExplGroupPieces::Assembled(ir) => return ir,
            ExplGroupPieces::Pieces {
                open_ir,
                body,
                close_ir,
                spaced,
                forced,
            } => (open_ir, body, close_ir, spaced, forced),
        };
    // A forced block gets hard boundary separators *in-shape*, so
    // `propagate_breaks` marks the group `expand` and the printer lays the
    // body out in break mode — never the K&R hybrid of a flat-dispatched
    // concat (issue #97, `l3auxdata.dtx`). Otherwise the flat boundary is a
    // space (l3 house style) or nothing (tight); both break identically.
    let boundary = if has_comment {
        Ir::hard_line()
    } else if spaced {
        Ir::Line
    } else {
        Ir::SoftLine
    };
    Ir::group(Ir::concat([
        open_ir,
        Ir::indent(Ir::concat([boundary.clone(), body])),
        boundary,
        close_ir,
    ]))
}

/// The result of decomposing an expl3 brace group into its layout pieces; see
/// [`expl_group_pieces`].
enum ExplGroupPieces {
    /// A special-cased shape ([`expl_group_pieces`] resolved it fully): an empty
    /// body, with or without a glued lead comment. The caller uses the assembled
    /// `Ir` verbatim and does *not* get the three-candidate hang treatment.
    Assembled(Ir),
    /// A body-bearing group split into its head/body pieces so the caller can
    /// reassemble in flat, K&R, or Allman form.
    Pieces {
        /// The opening bracket, plus any glued lead comment.
        open_ir: Ir,
        /// The lowered body (leading/trailing breaks trimmed).
        body: Ir,
        /// The closing bracket.
        close_ir: Ir,
        /// Whether the flat boundary is a space (l3 house style) or tight.
        spaced: bool,
        /// Whether a comment, guard, or `.dtx` margin forces the broken form
        /// regardless of width.
        forced: bool,
    },
}

/// Decompose an expl3 brace `{…}` (or optional `[…]`) group into the pieces
/// [`lower_expl_group`] and the trailing-hang branch in [`lower_expl_code`] share.
/// The empty-body and glued-lead-comment shapes have bespoke, already-stable
/// assembly, so they are returned pre-assembled as [`ExplGroupPieces::Assembled`];
/// every body-bearing group is returned as [`ExplGroupPieces::Pieces`] for the
/// caller to lay out flat, K&R, or Allman.
fn expl_group_pieces(
    node: &SyntaxNode,
    open: SyntaxKind,
    close: SyntaxKind,
    cx: LowerCtx<'_>,
) -> ExplGroupPieces {
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
    // A comment opening the body (`{%`, the macro-code continuation idiom, with
    // inline whitespace tolerated) rides the opening bracket's line — it never
    // gets a line of its own. Inside a region the masked newline is inert
    // catcode-9 whitespace either way; this just keeps the authored shape.
    let mut lead_comment = Ir::Nil;
    {
        let mut i = 0;
        let mut spaced = false;
        while let Some(SyntaxElement::Token(t)) = body_elements.get(i) {
            match t.kind() {
                SyntaxKind::WHITESPACE => {
                    spaced = true;
                    i += 1;
                }
                SyntaxKind::COMMENT => {
                    lead_comment = if spaced {
                        Ir::concat([Ir::verbatim(" "), Ir::verbatim(t.text())])
                    } else {
                        Ir::verbatim(t.text())
                    };
                    body_elements.drain(..=i);
                    break;
                }
                _ => break,
            }
        }
    }
    // A body holding a `%` comment can never flatten: inline, everything after
    // the comment on the line — the closing bracket included — would be
    // swallowed into the comment on the next parse (`{ …% }`). The lead comment
    // glued to the opening bracket forces the broken form the same way.
    //
    // A docstrip guard (`%<…>`) or `.dtx` margin (`%`) is line-oriented the same
    // way, and worse: it is only recognized at line start, so flattening it into
    // `{ %<trace> … }` re-lexes it as an *ordinary* `%` comment that swallows the
    // rest of the line — braces included — unbalancing the enclosing group on the
    // next parse (issue #61). Force the broken form so each rides its own line,
    // where `lower_loose_token` pins it to column 0 and it stays a guard/margin.
    let has_lead_comment = !matches!(lead_comment, Ir::Nil);
    let has_comment = has_lead_comment
        || node
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| {
                matches!(
                    t.kind(),
                    SyntaxKind::COMMENT | SyntaxKind::GUARD | SyntaxKind::DOC_MARGIN
                )
            });
    let open_ir = Ir::concat([open_ir, lead_comment]);
    let body = trim_trailing_break(trim_leading_break(lower_expl_code(
        body_elements.into_iter(),
        cx,
        Statements::Structural,
    )));
    if matches!(body, Ir::Nil) {
        // A glued lead comment owns the rest of its line, so an empty body
        // still breaks before the closing bracket (`{%…` + `}` on its own
        // line), never `{%…}` with the bracket swallowed.
        return ExplGroupPieces::Assembled(if has_lead_comment {
            Ir::concat([open_ir, Ir::hard_line(), close_ir])
        } else {
            Ir::concat([open_ir, close_ir])
        });
    }
    // A glued lead comment on a body-bearing group still forces the broken form
    // (it owns the rest of the opening line) and, more to the point, means the
    // three-candidate trailing-hang treatment must not apply — the flat/K&R
    // candidates would put content after the comment on the same line. Route it
    // through the forced-break path (`forced` covers both the lead comment and
    // any interior comment/guard/margin).
    ExplGroupPieces::Pieces {
        open_ir,
        body,
        close_ir,
        spaced: expl_group_is_spaced(node),
        forced: has_comment,
    }
}

/// Lower a statement-leading expl3 conditional (`\…:nTF {c} {T} {F}`, its branch
/// count recognized by [`expl3::conditional_branches`]) to the l3styleguide's
/// exploded shape (R4/R5): the head and any leading arguments on one line, then
/// each of the `n` trailing brace branches on its own line hung one indent step
/// (+2 relative to the head, so +6 inside a +4 body; a multi-line branch nests its
/// interior +8). The break is **unconditional** — width-independent, so it is
/// pass-stable: the exploded output re-parses to the same greedy `COMMAND` (brace
/// arguments attach across the inserted newlines) in statement position and
/// re-explodes identically. Each branch is a *soft* [`lower_expl_group`], so a
/// short branch stays `{ … }` inline on its line and a long one breaks internally.
///
/// An annotated branch keeps its comment: a trailing `%` rides the branch's own
/// line, an own-line one stays on its own line between branches.
///
/// Returns `None` unless the command's own last `n` argument children are `GROUP`s
/// with only trivia and comments beyond them — i.e. the branches actually attach to
/// the conditional. An `:NTF`/`:nNnTF` whose single-token (`N`/`V`/operator)
/// argument breaks greedy attachment leaves the branch groups on a following
/// sibling, which this node-local scan cannot see;
/// [`lower_expl_conditional_unit`] picks those up from the resolved call unit, and
/// [`expl_conditional_at`] tries the two in order.
fn lower_expl_conditional(cmd: &SyntaxNode, cx: LowerCtx<'_>, n: usize) -> Option<Ir> {
    let children: Vec<SyntaxElement> = cmd.children_with_tokens().collect();
    let group_positions: Vec<usize> = children
        .iter()
        .enumerate()
        .filter(|(_, e)| e.as_node().is_some_and(|nd| nd.kind() == SyntaxKind::GROUP))
        .map(|(i, _)| i)
        .collect();
    if group_positions.len() < n {
        return None;
    }
    // The last `n` groups are the branches; everything before the first of them is
    // the head (control word, leading brace/operator args, and their trivia).
    let first_branch = group_positions[group_positions.len() - n];
    // The branches must be the *trailing* arguments: nothing but groups, trivia, and
    // comments may sit from the first branch onward, else this is not a clean
    // conditional call and the width path is safer. Comments are admitted (rather
    // than bailing to the width path) because annotated branches are ordinary l3
    // style — `{ \exp_not:n { equations~ } } % You might prefer \nobreakspace to ~`
    // — and the bail cost the whole exploded shape for one `%` (issue #101).
    for element in &children[first_branch..] {
        match element {
            SyntaxElement::Node(nd) if nd.kind() == SyntaxKind::GROUP => {}
            SyntaxElement::Token(t)
                if is_collapsible_trivia(t.kind()) || t.kind() == SyntaxKind::COMMENT => {}
            _ => return None,
        }
    }
    let branch_lines = expl_branch_lines(&children[first_branch..], cx)?;
    let head = trim_trailing_break(lower_expl_code(
        children[..first_branch].iter().cloned(),
        cx,
        Statements::Ignore,
    ));
    Some(Ir::concat(std::iter::once(head).chain(branch_lines)))
}

/// The exploded branch lines of a conditional: each `GROUP` in `tail` on its own
/// line hung one indent step, as a *soft* [`lower_expl_group`] so a short branch
/// stays `{ … }` inline on its line and a long one breaks internally.
///
/// `None` when `tail` holds anything but groups, collapsible trivia, and comments
/// — the branch list is then not clean and the width-driven path is safer. Shared
/// by [`lower_expl_conditional`] (branches attached to the head node) and
/// [`lower_expl_conditional_unit`] (branches greedy attachment gave to a sibling),
/// so the two spellings of the same layout cannot drift.
fn expl_branch_lines(tail: &[SyntaxElement], cx: LowerCtx<'_>) -> Option<Vec<Ir>> {
    // Comments are admitted (rather than bailing to the width path) because
    // annotated branches are ordinary l3 style — `{ \exp_not:n { equations~ } } %
    // You might prefer \nobreakspace to ~` — and the bail cost the whole exploded
    // shape for one `%` (issue #101).
    for element in tail {
        match element {
            SyntaxElement::Node(nd) if nd.kind() == SyntaxKind::GROUP => {}
            SyntaxElement::Token(t)
                if is_collapsible_trivia(t.kind()) || t.kind() == SyntaxKind::COMMENT => {}
            _ => return None,
        }
    }
    let mut parts = Vec::new();
    // Trivia seen since the last emitted branch or comment: `gap` renders as the one
    // space before a trailing comment, `own_line` (a newline in that run) keeps an
    // own-line comment on its own line. Own-line-ness is a *preserved* predicate, so
    // reading it is trivia-invariant and stable in both directions — a trailing
    // comment re-parses trailing, an own-line one re-parses own-line. Relocating
    // either way would change its attachment.
    let mut gap = false;
    let mut own_line = false;
    for element in tail {
        match element {
            SyntaxElement::Node(nd) => {
                let group = lower_expl_group(nd, SyntaxKind::L_BRACE, SyntaxKind::R_BRACE, cx);
                parts.push(Ir::indent(Ir::concat([Ir::hard_line(), group])));
                (gap, own_line) = (false, false);
            }
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::COMMENT => {
                let comment = Ir::verbatim(t.text());
                parts.push(if own_line {
                    Ir::indent(Ir::concat([Ir::hard_line(), comment]))
                } else if gap {
                    Ir::concat([Ir::verbatim(" "), comment])
                } else {
                    comment
                });
                (gap, own_line) = (false, false);
            }
            SyntaxElement::Token(t) => {
                gap = true;
                own_line |= t.kind() == SyntaxKind::NEWLINE;
            }
        }
    }
    Some(parts)
}

/// The exploded form of the expl3 conditional headed by `elements[idx]`, plus
/// the index of the unit's last element (the caller resumes at `last + 1`).
///
/// Node-local: arity attachment gives a recognized conditional its branches as
/// the head's own trailing groups, so [`lower_expl_conditional`] covers every
/// resolvable shape — including a head whose *arity* is underivable while its
/// branch count is not (`:wTF`), when greed happened to attach the branches.
/// Returns `None` to leave the call on the width-driven path.
fn expl_conditional_at(
    elements: &[SyntaxElement],
    idx: usize,
    cx: LowerCtx<'_>,
) -> Option<(Ir, usize)> {
    let node = elements.get(idx)?.as_node()?;
    if node.kind() != SyntaxKind::COMMAND {
        return None;
    }
    let n = expl3::conditional_branches(&command_name(node)?)?;
    let ir = lower_expl_conditional(node, cx, n)?;
    Some((ir, idx))
}

/// Lower a single loose token (one not collapsed into a trivia run) to inline IR.
/// A `.dtx` documentation margin (`DOC_MARGIN`) or docstrip guard (`GUARD`) pins
/// to column 0 via [`Ir::column_zero`] so docstrip's left-margin anchor survives
/// any surrounding LaTeX nesting; every other token splices verbatim. These tokens
/// only exist under the `.dtx` lexer config, so non-`.dtx` lowering is unaffected.
fn lower_loose_token(token: &SyntaxToken, cx: LowerCtx<'_>) -> Ir {
    if cx.in_dtx_doc_region && token.kind() == SyntaxKind::DOC_MARGIN {
        return Ir::Nil;
    }
    if matches!(token.kind(), SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD) {
        Ir::column_zero(token.text())
    } else if cx.absorbs_control_newline(token) {
        let before = token
            .text()
            .strip_suffix('\n')
            .expect("absorbed control symbol must end in a newline");
        Ir::verbatim(before)
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
            // A virtual documentation region owns its physical margins. Drop
            // each one together with the source padding after it; ordinary
            // lowering will regenerate indentation in virtual coordinates.
            SyntaxElement::Token(token)
                if cx.in_dtx_doc_region && token.kind() == SyntaxKind::DOC_MARGIN =>
            {
                while let Some(SyntaxElement::Token(next)) = iter.peek() {
                    if next.kind() == SyntaxKind::WHITESPACE {
                        iter.next();
                    } else {
                        break;
                    }
                }
            }
            // The floated leading `%` of a virtual `.dtx` documentation region
            // belongs to the region wrapper. Drop it and its padding before the
            // generic trivia arm can consume it as an ordinary gap; otherwise the
            // wrapper emits a second margin and comments out the region's `\begin`.
            SyntaxElement::Token(token) if margin_starts_dtx_doc_region(&token, cx) => {
                while let Some(SyntaxElement::Token(t)) = iter.peek() {
                    if t.kind() == SyntaxKind::WHITESPACE {
                        iter.next();
                    } else {
                        break;
                    }
                }
            }
            // Trivia inside a suppressed span is reproduced too, one token at a
            // time rather than collapsed into a `Gap`. Without this the *gaps
            // between* two suppressed siblings would still normalize, so an
            // `off`/`on` region spanning several top-level blocks would keep
            // every block byte-exact and quietly rewrite the seams between them.
            SyntaxElement::Token(token)
                if is_collapsible_trivia(token.kind()) && cx.suppressed(token.text_range()) =>
            {
                out.push(Ir::verbatim(token.text().to_string()));
            }
            SyntaxElement::Token(token) if is_collapsible_trivia(token.kind()) => {
                out.push(classify_trivia(
                    consume_gap_widened(&token, &mut iter),
                    cx.in_alignment_cell,
                ));
            }
            SyntaxElement::Token(token)
                if cx.wraps_prose()
                    && !cx.dtx_margin_probe
                    && token.kind() == SyntaxKind::DOC_MARGIN
                    && margin_floats_into_paragraph(&token, cx) =>
            {
                while let Some(SyntaxElement::Token(t)) = iter.peek() {
                    if t.kind() == SyntaxKind::WHITESPACE {
                        iter.next();
                    } else {
                        break;
                    }
                }
            }
            SyntaxElement::Token(token) => out.push(lower_loose_token(&token, cx)),
        }
    }
    out
}

/// Lower a *prose* element stream under [`WrapMode::Preserve`]: like
/// [`lower_element_stream`], but a newline-free whitespace run — inter-word spacing
/// on a single line — collapses to a single space instead of surviving verbatim.
/// `Preserve` governs *line breaks* only, so intra-line spacing normalizes exactly
/// as it does in every wrapping mode (runs of spaces/tabs are catcode-10 equivalent
/// to one space, so the collapse is meaning-preserving); a run carrying a newline
/// stays a break and the printer still owns the following indentation.
///
/// Reached only for genuine prose — a non-`.dtx` `PARAGRAPH` (see [`lower_node`]) or
/// a list item body (see [`preserve_chunks`]) — after [`flatten_inline_prose`] has
/// spliced inline-prose command bodies (`\emph{…}`) into the run. A child *node*
/// still recurses through [`lower_node`], so an *opaque* brace body (a `\newcommand`
/// definition, any non-inline argument group) keeps its inner spacing byte-for-byte,
/// exactly as under every other mode.
fn lower_prose_stream(elements: impl Iterator<Item = SyntaxElement>, cx: LowerCtx<'_>) -> Vec<Ir> {
    let mut out = Vec::new();
    let mut iter = elements.peekable();
    while let Some(element) = iter.next() {
        match element {
            SyntaxElement::Node(child) => out.push(lower_node(&child, cx)),
            // Tier 2, shared with [`classify_trivia`]: `Preserve` promises the
            // authored line structure survives, so this boundary reads the newline
            // count and reproduces it. Preservation-only, hence its own fixed point.
            SyntaxElement::Token(token) if is_collapsible_trivia(token.kind()) => {
                out.push(match consume_gap_widened(&token, &mut iter).newlines {
                    0 => Ir::verbatim(" "),
                    1 => Ir::hard_line(),
                    _ => Ir::empty_line(),
                });
            }
            SyntaxElement::Token(token) => out.push(lower_loose_token(&token, cx)),
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

/// The decomposition every environment lowering starts from: the elements before
/// `\begin` (lowered), the lowered `\begin` header, the raw body elements, and
/// the lowered `\end`. A `%` that trails the `\begin{…}` header on the same
/// source line belongs to that line (the space-suppression idiom), not the body,
/// so it is lifted onto `begin` *here* — once, for every layout path — rather
/// than in each lowering, where a path that forgot the lift would relocate the
/// comment onto its own body line and change its meaning (issue #38). The lifted
/// token still sits (nested) in `body`; consumers drop it by identity — the
/// stream path via [`lower_body_dropping_leading_comment`], the flattening paths
/// via [`is_lifted_comment`].
struct EnvParts {
    leading: Ir,
    begin: Ir,
    body: Vec<SyntaxElement>,
    end: Ir,
    lifted: Option<SyntaxToken>,
    /// How many of `body`'s leading elements came from [`lower_begin`]'s tail —
    /// content the greedy parser attached to `BEGIN` past the end of the header.
    /// They are *in* `body` so no consumer can drop them; the count is what lets
    /// [`lower_env_body`] splice them into the body's first paragraph.
    tail_len: usize,
    /// A shape-proved, unbraced argument that the grammar represents as the
    /// first body token. At present this is the parenthesized size tuple of the
    /// standard `picture` environment.
    body_header_token: Option<SyntaxToken>,
}

/// The body-final `\\<newline>` control symbol, ignoring only indentation on
/// the closer's line. The newline is part of this non-trivia token, not a
/// separate [`SyntaxKind::NEWLINE`], so an environment that unconditionally
/// adds its own closing break would otherwise create a blank paragraph.
fn trailing_control_newline(body: &[SyntaxElement]) -> Option<SyntaxToken> {
    let first = body.first()?.text_range().start();
    let last = body.last()?;
    let mut token = match last {
        SyntaxElement::Node(node) => node.last_token(),
        SyntaxElement::Token(token) => Some(token.clone()),
    }?;

    loop {
        if token.text_range().start() < first {
            return None;
        }
        match token.kind() {
            SyntaxKind::WHITESPACE => token = token.prev_token()?,
            SyntaxKind::CONTROL_SYMBOL if token.text().ends_with('\n') => return Some(token),
            _ => return None,
        }
    }
}

fn split_environment(node: &SyntaxNode, cx: LowerCtx<'_>) -> EnvParts {
    let leading = lower_environment_leading(node, cx);
    let mut begin = Ir::Nil;
    let mut end = Ir::Nil;
    let mut body: Vec<SyntaxElement> = Vec::new();
    let mut tail_len = 0usize;
    let mut seen_begin = false;
    for element in node.children_with_tokens() {
        match &element {
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::BEGIN => {
                seen_begin = true;
                // Content the greedy parser attached past the end of the header
                // leads the body (see [`lower_begin`]). `body` is still empty here
                // — everything before `BEGIN` is `leading` — so extending it now
                // keeps the tail in source order ahead of the real body.
                let parts = lower_begin(child, cx);
                begin = parts.header;
                tail_len = parts.tail.len();
                body.extend(parts.tail);
            }
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::END => {
                end = lower_node(child, cx);
            }
            _ if !seen_begin => {}
            _ => body.push(element),
        }
    }
    let lifted = leading_inline_comment(&body);
    if let Some(comment) = &lifted {
        begin = Ir::concat([begin, Ir::verbatim(comment.text())]);
    }
    let body_header_token = picture_header_token(node, &body);
    if let Some(token) = &body_header_token {
        begin = Ir::concat([begin, lower_loose_token(token, cx)]);
    }
    EnvParts {
        leading,
        begin,
        body,
        end,
        lifted,
        tail_len,
        body_header_token,
    }
}

/// Return the standard `picture` environment's glued `(width,height)` token.
/// TeX gives this environment an unbraced begin argument, so the generic grammar
/// necessarily places it in the body. The name, adjacency, and tuple shape make
/// the relocation text-falsifiable without claiming that arbitrary body text is
/// an environment argument.
fn picture_header_token(node: &SyntaxNode, body: &[SyntaxElement]) -> Option<SyntaxToken> {
    let environment = Environment::cast(node.clone())?;
    if environment.name().as_deref() != Some("picture") {
        return None;
    }
    let begin = environment.begin()?;
    let paragraph = body.first()?.as_node()?;
    if paragraph.kind() != SyntaxKind::PARAGRAPH {
        return None;
    }
    let token = paragraph.first_token()?;
    (token.kind() == SyntaxKind::WORD
        && begin.syntax().text_range().end() == token.text_range().start()
        && is_picture_tuple(token.text()))
    .then_some(token)
}

fn is_picture_tuple(text: &str) -> bool {
    let mut chars = text.chars().peekable();
    let mut groups = 0usize;
    while chars.peek().is_some() {
        if chars.next() != Some('(') {
            return false;
        }
        let mut comma = false;
        let mut content = false;
        loop {
            match chars.next() {
                Some(',') if !comma && content => {
                    comma = true;
                    content = false;
                }
                Some(')') if comma && content => break,
                Some('(' | ')') | None => return false,
                Some(_) => content = true,
            }
        }
        groups += 1;
    }
    matches!(groups, 1 | 2)
}

/// Whether `el` is the [`EnvParts::lifted`] `\begin`-line comment, compared by
/// token identity so the flattening body consumers (list, alignment grid, math
/// formula) drop exactly the token that was lifted and nothing else.
fn is_lifted_comment(el: &SyntaxElement, lifted: Option<&SyntaxToken>) -> bool {
    lifted.is_some_and(|l| el.as_token() == Some(l))
}

/// Whether `node` (a `PARAGRAPH`) takes the plain prose reflow in [`lower_node`] —
/// the one body path a spliced [`EnvParts::tail_len`] run can join. A `.dtx` doc
/// paragraph re-synthesizes its own `% ` margin per line and an expl3-overlapping
/// one segments into statements; neither admits foreign leading elements, so both
/// keep the concatenated form. Mirrors `lower_node`'s `PARAGRAPH` arms — keep the
/// two in step.
fn paragraph_reflows_as_prose(node: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    cx.wraps_prose()
        && !is_dtx_doc_paragraph(node)
        && !(cx.any_expl3() && cx.overlaps_expl3(node.text_range()))
}

/// Lower [`EnvParts::body`] through the generic element stream, dropping the
/// lifted `\begin`-line comment when one was taken.
///
/// The leading `tail_len` elements are content greedy attachment gave to `BEGIN`
/// past the end of its header (see [`lower_begin`]); where the body opens with a
/// prose paragraph they are *spliced into its reflow* rather than concatenated
/// ahead of it. That is what makes the relocation invisible to the layout: the
/// source lays out identically whether or not the parser happened to pull the
/// group into `BEGIN` — `\begin{center}\n{\bfseries A}\nmore` reflows onto one
/// line, exactly as it does with a word ahead of the group to keep it in the
/// paragraph.
///
/// Concatenating instead would abut the two with *no separator at all*: the
/// paragraph's own leading newline lives inside the node and its reflow trims it,
/// so `{\bfseries A}` and `more` would run together. That is a space TeX typesets,
/// silently deleted — and invisible to every CST oracle, since whitespace is trivia
/// to them and content to TeX.
fn lower_env_body(
    body: Vec<SyntaxElement>,
    tail_len: usize,
    lifted: bool,
    body_header_token: Option<&SyntaxToken>,
    cx: LowerCtx<'_>,
) -> Ir {
    if let Some(header_token) = body_header_token
        && let Some(SyntaxElement::Node(paragraph)) = body.first()
    {
        let elements = paragraph
            .children_with_tokens()
            .filter(|element| element.as_token() != Some(header_token));
        let first = if cx.wraps_prose() {
            reflow_elements(elements, cx, paragraph_reflow_kind(paragraph, cx))
        } else {
            Ir::concat(lower_element_stream(elements, cx))
        };
        return Ir::concat(
            std::iter::once(first).chain(lower_element_stream(body[1..].iter().cloned(), cx)),
        );
    }
    if tail_len > 0
        && let Some(SyntaxElement::Node(para)) = body.get(tail_len)
        && para.kind() == SyntaxKind::PARAGRAPH
        && paragraph_reflows_as_prose(para, cx)
    {
        let spliced = reflow_elements(
            body[..tail_len]
                .iter()
                .cloned()
                .chain(para.children_with_tokens()),
            cx,
            paragraph_reflow_kind(para, cx),
        );
        let rest = lower_element_stream(body[tail_len + 1..].iter().cloned(), cx);
        return Ir::concat(std::iter::once(spliced).chain(rest));
    }
    // A named math environment can likewise begin with content greedy attachment
    // left inside `BEGIN`, followed by the parser's ordinary `MATH` body wrapper.
    // The math-grid path flattens those siblings together, but a grid containing a
    // non-final multiline cell must fall back here. Keep the recovered prefix in
    // math mode on that fallback: lowering a comment-bearing group generically
    // would reset binary-operator context after the comment and indent its body a
    // full step rather than hanging it one column past `{`.
    if tail_len > 0
        && body
            .get(tail_len)
            .and_then(SyntaxElement::as_node)
            .is_some_and(|node| node.kind() == SyntaxKind::MATH)
    {
        let prefix = lower_math_seq(
            body[..tail_len].iter().cloned(),
            cx,
            MathSpacing::Normal,
            false,
        );
        let rest = lower_element_stream(body[tail_len..].iter().cloned(), cx);
        return Ir::concat(std::iter::once(prefix).chain(rest));
    }
    if lifted {
        lower_body_dropping_leading_comment(body, cx)
    } else {
        Ir::concat(lower_element_stream(body.into_iter(), cx))
    }
}

fn lower_environment(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    let EnvParts {
        leading,
        begin,
        body,
        end,
        lifted,
        tail_len,
        body_header_token,
    } = split_environment(node, cx);
    let body_cx = cx.absorbing_trailing_control_newline(&body);
    let body = lower_env_body(
        body,
        tail_len,
        lifted.is_some(),
        body_header_token.as_ref(),
        body_cx,
    );
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

/// Split a `CONDITIONAL_BRANCH`'s elements from the trailing collapsible-trivia
/// run that carries the gap to whatever follows it, classifying that gap as a
/// [`Gap`].
///
/// All inter-segment trivia belongs to the *preceding* branch — the grammar's
/// branch loop consumes it before it reaches the divider — so this is the only
/// place a boundary gap can live.
///
/// Only [`Gap::Glued`] is discriminated by the layout: it is the one case that is
/// not a break opportunity. [`Gap::Comment`] is distinguished purely so that a
/// comment-terminated boundary never lands in `Glued` — the `%` itself is not
/// collapsible trivia, so nothing is peeled behind it, and without its own variant
/// a branch ending `… % note` would read as glued and send the whole construct
/// down the byte-faithful path. It lays out exactly like [`Gap::Space`]; the flat
/// candidate is separately refused by [`collapse_conditional_elements`], which sees
/// the same `%`. Whether the peeled run held a newline is, as everywhere,
/// invisible here.
fn split_branch_gap(node: &SyntaxNode) -> (Vec<SyntaxElement>, Gap) {
    let mut elements: Vec<SyntaxElement> = node.children_with_tokens().collect();
    let mut peeled = false;
    while let Some(SyntaxElement::Token(t)) = elements.last() {
        if !is_collapsible_trivia(t.kind()) {
            break;
        }
        peeled = true;
        elements.pop();
    }
    let gap = match elements.last() {
        Some(SyntaxElement::Token(t)) if t.kind() == SyntaxKind::COMMENT => Gap::Comment,
        _ if peeled => Gap::space(),
        _ => Gap::Glued,
    };
    (elements, gap)
}

/// Whether a `CONDITIONAL`'s branch interiors should be reflowed as prose, or
/// `None` when the construct must not be relaid at all.
///
/// Read off the *enclosing* context, since a branch carries no `PARAGRAPH` of its
/// own to answer with (the gate keeps a conditional inside one paragraph, so one
/// never nests in a branch). A conditional whose nearest non-conditional ancestor
/// is a `PARAGRAPH` sits in running text and reflows like it; one inside a `GROUP`
/// or `ARGUMENT` is macro code — a `\def` body — where the enclosing group emits
/// the byte-faithful stream, so the branches do too. Nested conditionals inherit
/// the answer by walking past their parent branch.
///
/// `None` for a `.dtx` documentation paragraph: the broken candidate commits hard
/// lines, and a line committed inside a doc paragraph lands outside its `% `
/// margin. The `contains_doc_margin` guard on the dispatch arm only catches a
/// conditional carrying a margin *itself*, not one riding a margined line whose
/// `DOC_MARGIN` sits before the opener.
fn conditional_interior_reflows(node: &SyntaxNode) -> Option<bool> {
    let mut ancestor = node.parent();
    while let Some(parent) = ancestor {
        match parent.kind() {
            SyntaxKind::CONDITIONAL | SyntaxKind::CONDITIONAL_BRANCH => ancestor = parent.parent(),
            SyntaxKind::PARAGRAPH => return (!is_dtx_doc_paragraph(&parent)).then_some(true),
            _ => return Some(false),
        }
    }
    Some(false)
}

/// A `CONDITIONAL`'s children, decomposed for [`lower_conditional`]: the elements
/// before the first branch, the branches, and the closing `\fi`.
///
/// The leading run is **not** always empty, which is the whole reason this exists.
/// An own-line `%` run immediately before the opener binds forward as a
/// `DOC_COMMENT` and the grammar reparents it *inside* the `CONDITIONAL`
/// (`Parser::conditional`, and `parse_block` for a top-level one), so it is a
/// sibling of the branches. A lowering that walked only `CONDITIONAL_BRANCH`
/// children would drop it on the floor — and the non-trivia-content oracle cannot
/// see that, because a comment is trivia to the CST. Hence
/// [`comments_survive_formatting`] in `tests/format.rs`.
///
/// `None` when the node is not in the shape the all-or-nothing layout assumes (no
/// closer, no branch, or a stray element between two branches); the caller then
/// takes the byte-faithful stream, which emits every child by construction.
struct ConditionalParts {
    leading: Vec<SyntaxElement>,
    branches: Vec<SyntaxNode>,
    closer: SyntaxNode,
}

fn split_conditional(node: &SyntaxNode) -> Option<ConditionalParts> {
    let mut leading: Vec<SyntaxElement> = Vec::new();
    let mut branches: Vec<SyntaxNode> = Vec::new();
    let mut closer: Option<SyntaxNode> = None;
    for element in node.children_with_tokens() {
        match &element {
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::CONDITIONAL_BRANCH => {
                // The closer is last, positionally (`Conditional::closer`); a branch
                // after one means the walk stopped somewhere this layout cannot model.
                if closer.is_some() {
                    return None;
                }
                branches.push(child.clone());
            }
            SyntaxElement::Node(child)
                if child.kind() == SyntaxKind::COMMAND && !branches.is_empty() =>
            {
                if closer.replace(child.clone()).is_some() {
                    return None;
                }
            }
            _ if branches.is_empty() => leading.push(element),
            // Anything else between the branches would be dropped by the branch
            // walk, so decline the whole construct rather than lose it.
            _ => return None,
        }
    }
    Some(ConditionalParts {
        leading,
        branches,
        closer: closer?,
    })
}

/// Lower an `\if… … \else … \or … \fi` conditional **all-or-nothing**: flat when
/// the whole construct fits, else *every* divider opens a line.
///
/// The construct's extent is what makes this decidable, and it is why the node
/// exists. A per-divider rule at this layer has no coherent form: fired only
/// across a gap the author already wrote it *is* the lone-newline read; fired
/// unconditionally it manufactures a space token at the ~22% of glued sites, which
/// TeX contributes to the horizontal list; fired only where the author broke, it is
/// lopsided — one divider broken and its sibling not, decided by where the author
/// happened to glue.
///
/// Two things the node deliberately does **not** buy. There is no body indent and
/// no head/body split: the `\if` *test*'s extent is not statically resolvable
/// (`\ifnum\radius>5` scans ⟨number⟩⟨rel⟩⟨number⟩ by TeX's own scanner), so the
/// environment-shaped layout the corpus files are written in is out of reach even
/// with the node. And a construct with **any glued divider** takes the byte-faithful
/// path instead of the group: breaking one divider but not its glued sibling is the
/// lopsided form, and breaking the glued one is the typeset change, so the only
/// coherent option left is to relayout none of them. Those keep their authored
/// line structure, which is a fixed point (a hard line re-reads as a newline, glue
/// re-reads as glue) and never materializes a space.
///
/// The decision is offered to the printer as two whole candidates
/// ([`Ir::conditional_group_all_lines`]) rather than as one [`Ir::group`] of
/// `Ir::Line`s, and that is load-bearing. A group's break state is saturated from
/// whatever forced breaks its subtree carries, and a branch *interior* carries one
/// for every authored line the command-only-line rule
/// ([`line_is_command_only`]) keeps — so a group would decide the dividers from
/// the interior's authored newlines, which is precisely the predicate that must
/// not decide them. The flat candidate is collapsed from *content* alone
/// ([`collapse_conditional_elements`]), so its width — and therefore the choice
/// between the two — is a function of non-trivia content and the config only.
/// When no flat candidate exists (a `%` comment, a nested block), the broken form
/// is unconditional, which is a content fact and fair to read.
///
/// A branch *interior* is lowered the way the construct's **enclosing context**
/// would lower the same elements ([`conditional_interior_reflows`]). That is what
/// "as anywhere else" has to mean, and it cannot be read off the branch itself: the
/// gate keeps a `CONDITIONAL` inside one paragraph, so no `PARAGRAPH` node ever
/// nests in a branch to carry the prose lowering the way an environment body's
/// does. In prose the branch therefore reflows — its words wrap and its inter-word
/// spacing normalizes, exactly as they would outside the construct — while in a
/// `\def` body it takes the byte-faithful stream, because that is what the
/// enclosing `GROUP` does. Feeding macro code to the prose reflow is not merely
/// cosmetic: `pagesel.sty`'s `\ifx\\#2\\%` has the parser's `LINE_BREAK` node in an
/// `\ifx` operand slot, and the reflow's "a `\\` ends its line" rule oscillates on
/// it pass over pass.
///
/// The whole relayout is confined to the modes that lay prose out at all
/// ([`LowerCtx::wraps_prose`]). [`WrapMode::Preserve`] promises authored line
/// breaks are untouched, and the all-or-nothing choice would rejoin a conditional
/// the author spread over lines — so that mode takes the byte-faithful stream.
/// The other three rebuild every prose line from runs already, so the choice is
/// theirs to make.
fn lower_conditional(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    let generic = || Ir::concat(lower_element_stream(node.children_with_tokens(), cx));
    if !cx.wraps_prose() {
        return generic();
    }
    let Some(reflow_interior) = conditional_interior_reflows(node) else {
        return generic();
    };
    let Some(ConditionalParts {
        leading,
        branches,
        closer,
    }) = split_conditional(node)
    else {
        return generic();
    };

    let split: Vec<(Vec<SyntaxElement>, Gap)> = branches.iter().map(split_branch_gap).collect();
    if split.iter().any(|(_, gap)| matches!(gap, Gap::Glued)) {
        return generic();
    }

    // The bound `DOC_COMMENT` run, if any. Lowered outside the candidates (as
    // `lower_environment` does with its own leading run): it is not part of the
    // construct's width, and it must survive whichever candidate the printer picks.
    let leading = if leading.is_empty() {
        Ir::Nil
    } else {
        Ir::concat(lower_element_stream(leading.into_iter(), cx))
    };

    let closer_ir = lower_node(&closer, cx);
    let mut broken = Vec::with_capacity(split.len() * 2 + 1);
    for (elements, _) in &split {
        broken.push(if reflow_interior {
            reflow_elements(elements.iter().cloned(), cx, ReflowKind::Prose)
        } else {
            Ir::concat(lower_element_stream(elements.iter().cloned(), cx))
        });
        broken.push(Ir::hard_line());
    }
    broken.push(closer_ir.clone());
    let broken = Ir::concat(broken);

    let group = match collapse_conditional(&split, &closer_ir, cx) {
        Some(flat) => Ir::conditional_group_all_lines([flat, broken]),
        None => broken,
    };
    Ir::concat([leading, group])
}

/// The one-line candidate for [`lower_conditional`]: every branch collapsed to a
/// single line, dividers separated by one space.
///
/// `None` — no flat form exists, so the construct is unconditionally broken — when
/// any branch holds a `%` comment (which must end its line) or force-break content
/// (a nested environment, display math, `\\`). Both are *content* facts, so keying
/// the layout on them is sound; an authored newline is not, and collapses to a
/// space here exactly as it does in [`collapse_arg_group`].
fn collapse_conditional(
    split: &[(Vec<SyntaxElement>, Gap)],
    closer: &Ir,
    cx: LowerCtx<'_>,
) -> Option<Ir> {
    if closer.contains_forced_break() {
        return None;
    }
    let mut parts = Vec::new();
    for (elements, _) in split {
        parts.extend(collapse_conditional_elements(elements, cx)?);
        parts.push(Ir::verbatim(" "));
    }
    parts.push(closer.clone());
    Some(Ir::concat(parts))
}

/// Collapse one branch's elements to a single line, or `None` if it cannot be
/// collapsed. Mirrors [`collapse_arg_group`]'s body loop, without the delimiter
/// handling a branch has no equivalent of.
fn collapse_conditional_elements(elements: &[SyntaxElement], cx: LowerCtx<'_>) -> Option<Vec<Ir>> {
    let mut out = Vec::new();
    let mut iter = elements.iter().cloned().peekable();
    while let Some(element) = iter.next() {
        match element {
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {
                let gap = consume_gap(&t, &mut iter);
                if gap == Gap::Blank {
                    return None; // a blank-line `\par` (the gate should preclude it)
                }
                out.push(Ir::verbatim(gap.flat()));
            }
            // A `%` comment must terminate its line, so there is no flat form.
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::COMMENT => return None,
            SyntaxElement::Token(t) => out.push(Ir::verbatim(t.text())),
            SyntaxElement::Node(child) => {
                let ir = lower_node(&child, cx);
                if ir.contains_forced_break() {
                    return None; // nested block content: keep the broken form
                }
                out.push(ir);
            }
        }
    }
    Some(out)
}

/// Whether the environment is *margin-framed*: a `.dtx` documentation margin
/// (`DOC_MARGIN`) or docstrip guard (`GUARD`) sits immediately before its `\begin`
/// on the same physical line — `%␣␣␣␣\begin{macrocode}`, a documentation-layer
/// `% \begin{itemize}`. The `\begin`/`\end` are docstrip *frame lines* anchored at
/// column 0, so the body must not be indented (indenting would push the frame
/// margins off column 0 and split the closing `%␣␣␣␣\end{…}` frame — the corruption
/// this fixes). A pure CST-shape fact: it walks back over inline whitespace from
/// `\begin` and asks only "is the previous token a margin/guard on this line", with
/// no signature lookup, and covers `macrocode` and prose-layer environments
/// uniformly. `DOC_MARGIN`/
/// `GUARD` exist only under the `.dtx` config, so this is always false elsewhere.
fn is_margin_framed(node: &SyntaxNode) -> bool {
    let Some(begin) = Environment::cast(node.clone()).and_then(|e| e.begin()) else {
        return false;
    };
    // A frame header occupies its physical line. If body content follows the
    // `\begin` inline, the generic stream must keep it behind the existing `%`;
    // the framed layout would insert a break and turn it into live package code.
    if begin
        .syntax()
        .last_token()
        .and_then(|token| token.next_token())
        .is_some_and(|token| token.kind() != SyntaxKind::NEWLINE)
    {
        return false;
    }
    let mut tok = begin.syntax().first_token().and_then(|t| t.prev_token());
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
    let EnvParts {
        leading,
        begin,
        mut body,
        end,
        lifted,
        tail_len,
        body_header_token: _,
    } = split_environment(node, cx);

    // Pull the `%␣␣␣␣` that frames `\end` onto the `\end` line; what remains is the
    // real body.
    let frame = split_closing_frame(&mut body);
    let frame_ir = frame
        .map(|f| Ir::concat(lower_element_stream(f.into_iter(), cx)))
        .filter(|ir| !matches!(ir, Ir::Nil));

    let body_cx = cx.absorbing_trailing_control_newline(&body);
    let body = lower_env_body(body, tail_len, lifted.is_some(), None, body_cx);
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
                out.push(lower_loose_token(&token, cx));
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
    if node.kind() == SyntaxKind::PARAGRAPH && cx.wraps_prose() {
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
    cx.signatures
        .environment_at(node)
        .is_some_and(|sig| sig.no_indent)
}

/// Whether `node` — a `PARAGRAPH`, or an environment body about to be reflowed as
/// one — sits in the body of an environment the signature DB marks
/// `statementBody`: the TikZ/pgf picture family, whose content is a sequence of
/// `;`-terminated path statements rather than running prose
/// ([`crate::semantic::signature::EnvironmentSig::statement_body`]).
///
/// A prose fill is wrong for such a body in a way width alone cannot express: it
/// runs `\draw …;` and `\node …;` onto one line, and it breaks a `\foreach`
/// header away from its loop variables (issue #114). [`ReflowKind::Statement`]
/// routes the body to the statement layout: under [`WrapMode::Reflow`] the
/// parser's `STATEMENT` nodes are lowered structurally with hung continuations
/// ([`lower_statement`], Tier 1), and content no `;` terminates keeps the
/// authored-line fallback — the same posture a code-like brace-group body takes,
/// with the Tier-2 flush-continuation fixed point carrying over unchanged.
///
/// The **nearest** environment ancestor decides, never any of them. An `itemize`
/// or a `tabular` inside a `\node`'s label holds ordinary prose and must still
/// reflow, though a `tikzpicture` encloses it.
fn in_statement_body_env(node: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    node.ancestors()
        .skip(1)
        .find(|ancestor| ancestor.kind() == SyntaxKind::ENVIRONMENT)
        .and_then(|env| cx.signatures.environment_at(&env))
        .is_some_and(|sig| sig.statement_body)
}

/// A `\begin{…}` header, split from the content the greedy parser attached past
/// the end of that header. See [`lower_begin`].
struct BeginParts {
    header: Ir,
    /// Elements [`split_environment`] prepends to the environment's body, so they
    /// indent and reflow with it instead of riding the header line.
    tail: Vec<SyntaxElement>,
}

/// Lower a `\begin{name}` node into the header line and the body content the
/// greedy parser over-attached to it.
///
/// **The header ends at the last element glued to it.** Two rules decide where
/// that is. Groups matching the environment's *declared* argument slots are glued
/// to `\begin{name}` whatever the author wrote between them, so
/// `\begin{tabular}\n{cc}` renders as a single `\begin{tabular}{cc}` header.
/// Positional matching skips omitted optional slots. Once the supplied groups
/// exhaust the signature — and from the `{name}` group onwards for an environment
/// with no declared arguments — the header continues only while each boundary is
/// [`Gap::Glued`]; at the first gap it stops, and everything from there is body.
///
/// The slots come from the [`Signatures`] overlay (`cx.signatures`): a document's
/// own `\newenvironment{thm}[1]…` is honored just like a built-in `tabular`, with
/// the scanned definition shadowing a built-in of the same name. A delimiter
/// mismatch against a pending required slot invalidates the positional claim and
/// demotes the rest of the header to ordinary glue boundaries; this keeps an
/// incomplete curated signature from reclassifying source on the next parse.
///
/// Attachment past the declared slots is not an argument claim, so it must not be
/// rendered as one: leaving it in the header
/// stranded it at the `\begin` column, one level short of the body it belongs to
/// (`\begin{center}\n{\bfseries A heading}`). Gluing it up instead would dress body
/// content as an argument, so body is the honest destination.
///
/// Trivia-invariant: only `Glued`-versus-not is read, which the normalized [`Gap`]
/// boundary preserves — a lone newline, a space, and a blank line all fall in the
/// same bucket, so the unsafe predicate never reaches this decision. It is a fixed
/// point in both directions: a glued tail re-parses glued, and a tail sent to the
/// body re-parses separated.
///
/// A `%` that trailed the header on its own source line stays on it (own-line-ness
/// is a preserved predicate, and relocating a trailing comment rebinds it as the
/// next construct's `DOC_COMMENT`); one the author gave its own
/// line travels to the body with the rest of the tail, which is where it already
/// was. But a header comment *with* a declared arity keeps every argument in the
/// header, because both available moves are wrong there: gluing an argument across
/// the `%` would comment it out, and sending it to the body would take a `tabular`'s
/// colspec away from the grid. A mandatory argument after a trailing comment is
/// emitted on an indented continuation line; an optional must retain the generic
/// path, since inserting whitespace before `[…]` can change whether TeX recognizes
/// it. A `.dtx` doc margin or guard is likewise preserved wholesale — both must
/// open their own line.
///
/// Each declared argument is also matched to its signature slot ([`match_arg_slot`],
/// mirroring [`lower_command`]) so a [`ContentKind::Keyval`] argument reaches the
/// delimiter-appropriate segmented layout: `[…]` for `axis`/`tikzpicture`, and
/// `{…}` for tabularray's inner specification. Every other content kind lowers
/// exactly as the generic path would.
fn lower_begin(begin: &SyntaxNode, cx: LowerCtx<'_>) -> BeginParts {
    let sig = cx.signatures.environment_at(begin);
    let arity = sig.as_ref().map(|sig| sig.args.len()).unwrap_or(0);
    let mut has_comment = false;
    let mut has_margin = false;
    for token in begin
        .children_with_tokens()
        .filter_map(|element| element.into_token())
    {
        match token.kind() {
            SyntaxKind::COMMENT => has_comment = true,
            SyntaxKind::DOC_MARGIN if !cx.in_dtx_doc_region => has_margin = true,
            SyntaxKind::GUARD => has_margin = true,
            _ => {}
        }
    }
    if has_margin || (has_comment && arity > 0) {
        return BeginParts {
            header: if has_margin {
                lower_node(begin, cx)
            } else {
                lower_commented_begin(begin, cx, arity)
            },
            tail: Vec::new(),
        };
    }

    let args = sig.as_ref().map(|sig| &*sig.args).unwrap_or(&[]);
    let elements: Vec<SyntaxElement> = begin.children_with_tokens().collect();
    let mut head: Vec<Ir> = Vec::new();
    let mut slot = 0usize;
    let mut signature_matches = true;
    let mut i = 0usize;
    while let Some(element) = elements.get(i) {
        match element {
            SyntaxElement::Token(token)
                if cx.in_dtx_doc_region && token.kind() == SyntaxKind::DOC_MARGIN =>
            {
                // The region wrapper owns the canonical margin. Its source
                // padding is not a gap in the virtual LaTeX header.
                i += 1;
                while matches!(
                    elements.get(i),
                    Some(SyntaxElement::Token(next)) if next.kind() == SyntaxKind::WHITESPACE
                ) {
                    i += 1;
                }
            }
            SyntaxElement::Token(token) if is_collapsible_trivia(token.kind()) => {
                // Measure the run in place rather than consuming it: when it turns
                // out to be the split point it must travel to the body, so that
                // `leading_inline_comment` sees trivia — not a bare `%` — first and
                // declines to lift an own-line comment back onto the header.
                let (mut end, mut newlines, mut flat) = (i, 0usize, String::new());
                while let Some(SyntaxElement::Token(token)) = elements.get(end) {
                    if !is_collapsible_trivia(token.kind()) {
                        break;
                    }
                    newlines += usize::from(token.kind() == SyntaxKind::NEWLINE);
                    flat.push_str(token.text());
                    end += 1;
                }
                // A following group that matches the next signature slot glues
                // to `\begin{name}`, so the run is dropped. Match on a copy:
                // the group arm commits the slot only once it consumes the node.
                let mut next_slot = slot;
                let declared_arg_follows = signature_matches
                    && elements
                        .get(end)
                        .and_then(attached_arg_kind)
                        .and_then(|kind| match_arg_slot(args, &mut next_slot, kind))
                        .is_some();
                if declared_arg_follows {
                    i = end;
                    continue;
                }
                // A `%` authored on the header line rides it.
                let trails_comment = newlines == 0
                    && matches!(
                        elements.get(end),
                        Some(SyntaxElement::Token(token)) if token.kind() == SyntaxKind::COMMENT
                    );
                if trails_comment {
                    head.push(Ir::verbatim(flat));
                    i = end;
                    continue;
                }
                // Not glued: the header ends here and the rest is body.
                return BeginParts {
                    header: Ir::concat(head),
                    tail: elements[i..].to_vec(),
                };
            }
            SyntaxElement::Node(child)
                if matches!(child.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL) =>
            {
                let kind = attached_arg_kind(element).expect("group or optional argument");
                let spec = signature_matches
                    .then(|| match_arg_slot(args, &mut slot, kind))
                    .flatten();
                // A delimiter mismatch against a pending required slot means the
                // signature is incomplete for this source shape. Demote the rest
                // of the header to ordinary glue boundaries instead of making a
                // later group look declared by skipping over the mismatch.
                if spec.is_none() && slot < args.len() {
                    signature_matches = false;
                }
                let keyval = spec.is_some_and(|spec| spec.content == ContentKind::Keyval);
                let segmented = keyval.then(|| match child.kind() {
                    SyntaxKind::OPTIONAL => lower_optional(child, cx, true),
                    SyntaxKind::GROUP => lower_segmented_group(
                        child,
                        SyntaxKind::L_BRACE,
                        SyntaxKind::R_BRACE,
                        cx,
                        true,
                    ),
                    _ => unreachable!("argument kind checked above"),
                });
                head.push(segmented.flatten().unwrap_or_else(|| lower_node(child, cx)));
                i += 1;
            }
            // The `\begin` control word, the `{name}` group, and anything the
            // author glued past the declared arity stay on the header line.
            SyntaxElement::Node(child) => {
                head.push(lower_node(child, cx));
                i += 1;
            }
            SyntaxElement::Token(token) => {
                head.push(lower_loose_token(token, cx));
                i += 1;
            }
        }
    }
    BeginParts {
        header: Ir::concat(head),
        tail: Vec::new(),
    }
}

/// The positional signature delimiter represented by an attached syntax node.
fn attached_arg_kind(element: &SyntaxElement) -> Option<ArgKind> {
    match element.as_node()?.kind() {
        SyntaxKind::GROUP => Some(ArgKind::Brace),
        SyntaxKind::OPTIONAL => Some(ArgKind::Bracket),
        _ => None,
    }
}

/// Lower a declared `\begin` header containing a comment without letting the
/// comment detach or consume a later argument. When a comment trails one argument
/// and the next outstanding argument is mandatory, that group is a structural
/// header continuation and receives one indent. The brace gate matters: TeX skips
/// whitespace while scanning a mandatory argument, whereas inserting indentation
/// before an optional `[…]` can change argument recognition.
fn lower_commented_begin(begin: &SyntaxNode, cx: LowerCtx<'_>, arity: usize) -> Ir {
    let elements: Vec<SyntaxElement> = begin.children_with_tokens().collect();
    let mut args_seen = 0usize;

    for (i, element) in elements.iter().enumerate() {
        if matches!(element, SyntaxElement::Node(node) if matches!(node.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL))
            && args_seen < arity
        {
            args_seen += 1;
            continue;
        }
        let SyntaxElement::Token(comment) = element else {
            continue;
        };
        if comment.kind() != SyntaxKind::COMMENT || args_seen >= arity {
            continue;
        }

        let trails_argument = elements[..i]
            .iter()
            .rev()
            .find_map(|previous| match previous {
                SyntaxElement::Token(token) if token.kind() == SyntaxKind::WHITESPACE => None,
                SyntaxElement::Node(node)
                    if matches!(node.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL) =>
                {
                    Some(true)
                }
                _ => Some(false),
            })
            == Some(true);
        if !trails_argument {
            continue;
        }

        let mut next = i + 1;
        let mut has_newline = false;
        while let Some(SyntaxElement::Token(token)) = elements.get(next) {
            if !is_collapsible_trivia(token.kind()) {
                break;
            }
            has_newline |= token.kind() == SyntaxKind::NEWLINE;
            next += 1;
        }
        if !has_newline
            || !matches!(elements.get(next), Some(SyntaxElement::Node(node)) if node.kind() == SyntaxKind::GROUP)
        {
            continue;
        }

        let prefix = Ir::concat(lower_element_stream(elements[..=i].iter().cloned(), cx));
        let continuation = Ir::concat(lower_element_stream(elements[i + 1..].iter().cloned(), cx));
        return Ir::concat([prefix, Ir::indent(continuation)]);
    }

    lower_node(begin, cx)
}

/// True if `node` (an `ENVIRONMENT`) names a list environment the signature DB
/// marks `list` — `itemize`/`enumerate`/`description`, whose `\item`s the
/// formatter lays out one per line with a hanging indent (see
/// [`lower_list_environment`]).
fn is_list_env(node: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    cx.signatures
        .environment_at(node)
        .is_some_and(|sig| sig.list)
}

/// One `\item` of a list environment: the rendered marker (`\item`, an optional
/// `[label]`, and a bounded Beamer `<overlay>` suffix), the width to hang
/// continuation lines at (the rendered width of the control word plus a space —
/// `\item `, *not* the label or overlay, so a wide marker does not push the body's
/// left edge around), and the item's body split into paragraph *chunks* (a blank
/// line in the source starts a new chunk). `blank_before` records whether a blank
/// line separated this item from the previous one, so it is reproduced.
struct ListItem {
    /// The comment lines of a `DOC_COMMENT` bound leading into the `\item`
    /// (`% note` on its own line directly above), rendered one per line above
    /// the marker at the item indent.
    doc_lines: Vec<String>,
    marker: String,
    hang: usize,
    glue_body: bool,
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
/// configured [`ItemIndent`]. Under the default [`ItemIndent::Hang`], a
/// `description` item's wide `[label]` trails on the first line but does not deepen
/// the body indent. The framing (`\begin`/`\end`, the indented body with
/// leading/trailing `hard_line`) matches [`lower_environment`].
///
/// Under [`WrapMode::Preserve`] the body is *not* reflowed: the author's line breaks
/// and inner spacing are kept byte-faithful (see [`lower_item_chunks`]) and only the
/// continuation-line indentation is re-hung under the marker. Falls back to the plain
/// [`lower_environment`] when the body has no `\item` to anchor on, so an unusual
/// shape degrades to today's indented body rather than misformatting.
fn lower_list_environment(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    let EnvParts {
        leading,
        begin,
        body,
        end,
        lifted,
        // The `BEGIN` tail (see [`lower_begin`]) rides `body` as ordinary leading
        // elements: this path flattens the body itself, so it needs no splice.
        tail_len: _,
        body_header_token: _,
    } = split_environment(node, cx);

    let body_cx = cx.absorbing_trailing_control_newline(&body);
    let Some(body) = lower_list_body(&body, body_cx, lifted.as_ref()) else {
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
/// `\item`, collect a bounded Beamer overlay suffix into the marker, and render a
/// hanging-indented reflow of the content. Returns `None` (caller falls back) when
/// the body carries no `\item`.
fn lower_list_body(
    body_elements: &[SyntaxElement],
    cx: LowerCtx<'_>,
    lifted: Option<&SyntaxToken>,
) -> Option<Ir> {
    let flat = flatten_list_body(body_elements, lifted);

    // Content before the first `\item` (usually just trivia); kept as its own
    // leading segment so nothing is dropped.
    let mut preamble: Vec<Vec<SyntaxElement>> = vec![Vec::new()];
    let mut items: Vec<ListItem> = Vec::new();
    let mut blank_pending = false;
    let mut index = 0;
    while index < flat.len() {
        match &flat[index] {
            FlatItem::Blank => {
                // A paragraph boundary: it separates items (recorded on the next
                // item) and, within an item, starts a fresh content chunk.
                blank_pending = true;
                match items.last_mut() {
                    Some(item) => item.chunks.push(Vec::new()),
                    None => preamble.push(Vec::new()),
                }
                index += 1;
            }
            FlatItem::El(el) if is_item_command(el) => {
                let mut item = split_item_marker(el, cx);
                index += 1;
                if let Some((suffix, end)) = item_overlay_marker_suffix(&flat, index) {
                    item.marker.push_str(&suffix);
                    item.glue_body = flat.get(end).is_some_and(
                        |next| matches!(next, FlatItem::El(el) if el.kind() == SyntaxKind::COMMENT),
                    );
                    index = end;
                }
                item.blank_before = blank_pending;
                items.push(item);
                blank_pending = false;
            }
            FlatItem::El(el) => {
                match items.last_mut() {
                    Some(item) => item.chunks.last_mut().unwrap().push(el.clone()),
                    None => preamble.last_mut().unwrap().push(el.clone()),
                }
                blank_pending = false;
                index += 1;
            }
        }
    }

    if items.is_empty() {
        return None;
    }

    let mut segments: Vec<Ir> = Vec::new();
    let mut seps: Vec<Ir> = Vec::new();
    let preamble_ir = lower_item_chunks(&preamble, cx);
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

/// Render one [`ListItem`]: any bound doc-comment lines on their own lines above,
/// then the marker, then a space and the item's body reflowed inside an
/// [`Ir::align`] whose width comes from [`LowerCtx::item_indent`]. The `hang`
/// width is the control word plus its separating space (`\item `), deliberately
/// excluding a label or overlay. A comment glued to an overlay receives no
/// separating space; an empty item (marker with no body) renders as the bare
/// marker.
fn render_list_item(item: &ListItem, cx: LowerCtx<'_>) -> Ir {
    let content = lower_item_chunks(&item.chunks, cx);
    let marker = Ir::verbatim(item.marker.clone());
    let body = if matches!(content, Ir::Nil) {
        marker
    } else {
        let separator = if item.glue_body {
            Ir::Nil
        } else {
            Ir::verbatim(" ")
        };
        let continuation_indent = match cx.item_indent {
            ItemIndent::Hang => item.hang,
            ItemIndent::Indent => cx.indent_width,
            ItemIndent::None => 0,
        };
        Ir::concat([marker, separator, Ir::align(continuation_indent, content)])
    };
    if item.doc_lines.is_empty() {
        return body;
    }
    let doc = item.doc_lines.iter().map(|line| Ir::verbatim(line.clone()));
    Ir::concat([Ir::join(Ir::hard_line(), doc), Ir::hard_line(), body])
}

/// Lower an item body's paragraph chunks, dispatching on the wrap mode: a
/// prose-wrapping mode reflows each chunk to width ([`reflow_chunks`]), while
/// [`WrapMode::Preserve`] keeps the author's breaks and inner spacing byte-faithful
/// ([`preserve_chunks`]). Either way the result sits inside the item's hanging
/// [`Ir::align`], so continuation lines indent under the marker.
fn lower_item_chunks(chunks: &[Vec<SyntaxElement>], cx: LowerCtx<'_>) -> Ir {
    if cx.wraps_prose() {
        reflow_chunks(chunks, cx)
    } else {
        preserve_chunks(chunks, cx)
    }
}

/// Preserve-mode analogue of [`reflow_chunks`]: lower each paragraph chunk as prose
/// under [`WrapMode::Preserve`] ([`lower_prose_stream`]) so the author's line breaks
/// become `hard_line`s while inter-word spacing collapses to a single space, then
/// join the (non-empty) chunks with an [`Ir::empty_line`]. Inline-prose command
/// bodies are flattened in first (matching the paragraph path), so an `\emph{…}`
/// body collapses too while an opaque argument group stays verbatim. Each chunk's
/// own edge breaks are trimmed — the leading whitespace after `\item ` and any
/// trailing break — so the first line glues after the marker and no blank line
/// leaks; the interior newlines survive as `hard_line`s that hang under the marker
/// via the enclosing [`Ir::align`].
fn preserve_chunks(chunks: &[Vec<SyntaxElement>], cx: LowerCtx<'_>) -> Ir {
    let parts = chunks
        .iter()
        .map(|chunk| {
            let flat = flatten_inline_prose(chunk.clone(), cx, false);
            let ir = Ir::concat(lower_prose_stream(flat.into_iter(), cx));
            let (_, ir) = peel_leading_break(ir);
            let (_, ir) = peel_trailing_break(ir);
            ir
        })
        .filter(|ir| !matches!(ir, Ir::Nil));
    Ir::join(Ir::empty_line(), parts)
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
/// paragraph boundary as a [`FlatItem::Blank`]. Body-level trivia is classified by
/// its newline count: a blank-line run (`≥2` newlines) becomes the `Blank` (its
/// tokens are dropped — the boundary carries them), while a single-newline run is
/// kept in the stream so [`reflow_elements`] still sees the line break — dropping
/// it would glue a body-level own-line `%` onto the preceding content or a nested
/// `\end{…}` (issue #48). Leading and trailing runs (against `\begin`/`\end`) are
/// dropped either way; the list framing re-supplies those breaks.
fn flatten_list_body(
    body_elements: &[SyntaxElement],
    lifted: Option<&SyntaxToken>,
) -> Vec<FlatItem> {
    let mut out: Vec<FlatItem> = Vec::new();
    let mut started = false;
    // Pending body-level trivia run: its tokens and how many newlines it spans.
    let mut run: Vec<SyntaxElement> = Vec::new();
    let mut run_newlines = 0usize;
    for element in body_elements {
        match element {
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {
                if t.kind() == SyntaxKind::NEWLINE {
                    run_newlines += 1;
                }
                run.push(element.clone());
                continue;
            }
            other if is_lifted_comment(other, lifted) => continue,
            _ => {}
        }
        if started {
            if run_newlines >= 2 {
                out.push(FlatItem::Blank);
            } else {
                out.extend(run.drain(..).map(FlatItem::El));
            }
        }
        run.clear();
        run_newlines = 0;
        match element {
            SyntaxElement::Node(p) if p.kind() == SyntaxKind::PARAGRAPH => {
                out.extend(
                    p.children_with_tokens()
                        .filter(|e| !is_lifted_comment(e, lifted))
                        .map(FlatItem::El),
                );
            }
            other => out.push(FlatItem::El(other.clone())),
        }
        started = true;
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

/// Read Beamer's bounded `\item<overlay>[label]<overlay>` suffix from the flat
/// list stream. The parser deliberately leaves angle-delimited syntax generic,
/// so list lowering recognizes only the complete, immediately following shape;
/// incomplete angle text remains ordinary item content.
fn item_overlay_marker_suffix(flat: &[FlatItem], start: usize) -> Option<(String, usize)> {
    let (mut suffix, mut end) = angle_suffix(flat, start)?;
    if let Some((label, label_end)) = bracket_suffix(flat, end) {
        suffix.push_str(&label);
        end = label_end;
        if let Some((overlay, overlay_end)) = angle_suffix(flat, end) {
            suffix.push_str(&overlay);
            end = overlay_end;
        }
    }
    Some((suffix, end))
}

fn angle_suffix(flat: &[FlatItem], start: usize) -> Option<(String, usize)> {
    let mut index = skip_flat_trivia(flat, start);
    let first = flat_element(flat.get(index)?)?;
    let first_text = element_source_text(first);
    if !first_text.starts_with('<') {
        return None;
    }

    let mut suffix = String::new();
    loop {
        let element = flat_element(flat.get(index)?)?;
        if element.kind() == SyntaxKind::COMMENT {
            return None;
        }
        if is_collapsible_trivia(element.kind()) {
            if !suffix.ends_with(' ') {
                suffix.push(' ');
            }
        } else {
            let text = element_source_text(element);
            suffix.push_str(&text);
            if text.ends_with('>') {
                return Some((suffix, index + 1));
            }
        }
        index += 1;
    }
}

fn bracket_suffix(flat: &[FlatItem], start: usize) -> Option<(String, usize)> {
    let mut index = skip_flat_trivia(flat, start);
    let first = flat_element(flat.get(index)?)?;
    if first.kind() != SyntaxKind::L_BRACKET {
        return None;
    }

    let mut depth = 0usize;
    let mut suffix = String::new();
    loop {
        let element = flat_element(flat.get(index)?)?;
        match element.kind() {
            SyntaxKind::L_BRACKET => depth += 1,
            SyntaxKind::R_BRACKET => depth = depth.checked_sub(1)?,
            SyntaxKind::COMMENT => return None,
            _ => {}
        }
        if is_collapsible_trivia(element.kind()) {
            if !suffix.ends_with(' ') {
                suffix.push(' ');
            }
        } else {
            suffix.push_str(&element_source_text(element));
        }
        index += 1;
        if depth == 0 {
            return Some((suffix, index));
        }
    }
}

fn skip_flat_trivia(flat: &[FlatItem], mut index: usize) -> usize {
    while let Some(FlatItem::El(element)) = flat.get(index) {
        if !is_collapsible_trivia(element.kind()) {
            break;
        }
        index += 1;
    }
    index
}

fn flat_element(item: &FlatItem) -> Option<&SyntaxElement> {
    match item {
        FlatItem::El(element) => Some(element),
        FlatItem::Blank => None,
    }
}

fn element_source_text(element: &SyntaxElement) -> String {
    match element {
        SyntaxElement::Node(node) => node.text().to_string(),
        SyntaxElement::Token(token) => token.text().to_string(),
    }
}

/// Split a `\item` command node into a [`ListItem`] (`blank_before` is the
/// caller's to set): the rendered marker string (the control word plus any leading
/// optional `[label]`, the only argument an item marker takes), the *hang* width
/// for continuation lines (the control word's rendered width plus one for the
/// separating space — deliberately excluding the `[label]` so a wide `description`
/// label does not deepen the body indent), and the trailing elements that are
/// really body content — a `{…}` group the greedy parser over-attached, which
/// belongs to the item body, not the marker. A `DOC_COMMENT` bound leading into
/// the `\item` yields the item's `doc_lines`, never marker or content.
fn split_item_marker(el: &SyntaxElement, cx: LowerCtx<'_>) -> ListItem {
    let node = el.as_node().expect("item command is a node");
    let mut doc_lines: Vec<String> = Vec::new();
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
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::DOC_COMMENT => {
                doc_lines.extend(
                    n.children_with_tokens()
                        .filter_map(|e| e.into_token())
                        .filter(|t| t.kind() == SyntaxKind::COMMENT)
                        .map(|t| t.text().to_string()),
                );
            }
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
    ListItem {
        doc_lines,
        marker,
        hang,
        glue_body: false,
        chunks: vec![content],
        blank_before: false,
    }
}

/// True if `node` (an `ENVIRONMENT`) names an environment the signature DB marks
/// `align` — an `align`/matrix-family environment whose `&` columns the formatter
/// lays out into a grid (see [`lower_aligned_environment`]).
fn is_alignment_env(node: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    cx.signatures
        .environment_at(node)
        .is_some_and(|sig| sig.align)
}

/// True if `node` (an `ENVIRONMENT`) carries a **top-level `&`** — an alignment tab
/// that is a direct child of the body (or of its single wrapping `PARAGRAPH`), not
/// nested inside a group or sub-environment. A `&` at catcode 4 reading as a column
/// tab is a static CST-shape fact, so an environment the signature DB cannot name
/// (`myaligned`, issue #84) still routes to the `&`-column grid when it is shaped
/// like one. It mirrors the cell boundary [`build_alignment_grid`]/`flatten_alignment_body`
/// use, so the routing decision and the grid it enables agree on what a top-level
/// `&` is; a nested `&` lives in a child node and is correctly invisible. Deliberately
/// keyed on `&` alone (not `\\`): a `\\`-only body is a line stack, not a column
/// alignment, and gridding an arbitrary `\begin{center}a \\ b\end{center}` would
/// reflow it.
fn body_has_top_level_ampersand(node: &SyntaxNode) -> bool {
    node.children_with_tokens().any(|el| match el {
        SyntaxElement::Token(t) => t.kind() == SyntaxKind::AMPERSAND,
        SyntaxElement::Node(p) if p.kind() == SyntaxKind::PARAGRAPH => p
            .children_with_tokens()
            .any(|g| g.kind() == SyntaxKind::AMPERSAND),
        _ => false,
    })
}

/// True if `node` (an `ENVIRONMENT`) names an environment the signature DB marks
/// `math` — `equation`, `align`, `gather`, matrix, … The parser wraps such a body
/// in a `MATH` node (it entered math mode); [`lower_math_environment`] lays it out
/// with the math-aware paths.
fn is_math_env(node: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    cx.signatures
        .environment_at(node)
        .is_some_and(|sig| sig.math)
}

/// One rendered grid cell: its flat, trimmed text, the number of columns it spans
/// (`1` for an ordinary cell, `n` for `\multicolumn{n}{…}{…}`), and an optional
/// alignment override (a `\multicolumn`'s own `{spec}`; `None` means "use the
/// column's declared alignment").
///
/// A *block* cell (`block` is `Some`, math grids only) is a cell that cannot
/// collapse to one line because it holds a nested multi-line construct — a block
/// environment (`\begin{aligned}…`, `\begin{cases}…`, a matrix), possibly inside
/// a `\left…\right` pair or a group. Its IR replaces `text` (which stays empty):
/// the first line continues the row and every later line hangs at the breaking
/// node's start column ([`Ir::Align`]), so a bare nested environment gets its
/// `\end{…}` directly under its `\begin{…}` and the body one indent step deeper.
/// A block cell is only ever the last cell of its row, never defines a column
/// width, and simply overflows — the same posture as a spanning cell.
struct Cell {
    text: String,
    span: usize,
    align: Option<ColAlign>,
    block: Option<BlockCell>,
}

/// The rendered IR of a block cell (see [`Cell`]) plus its hang offset: the flat
/// width of the cell content *before* the breaking node (`= ` in
/// `= \begin{aligned}…`), which the renderer adds to the cell's start column so
/// the hanging lines anchor at the node itself.
struct BlockCell {
    hang: usize,
    ir: Ir,
}

/// One row of an alignment grid: its rendered cells, the flat text of the `\\` that
/// terminated the row (`None` for a final row written without a trailing line
/// break), and an optional end-of-line comment that trails the row (rendered
/// *after* the `\\`, so the break is never commented out).
struct AlignRow {
    cells: Vec<Cell>,
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
    let EnvParts {
        leading,
        begin,
        body,
        end,
        lifted,
        // The `BEGIN` tail (see [`lower_begin`]) rides `body` as ordinary leading
        // elements: this path flattens the body itself, so it needs no splice.
        tail_len: _,
        body_header_token: _,
    } = split_environment(node, cx);

    let body_cx = cx.absorbing_trailing_control_newline(&body);
    let soften_nested_newlines = cx.in_dtx_doc_region && !is_math_env(node, cx);
    let Some(items) = build_alignment_grid(
        &body,
        body_cx,
        false,
        soften_nested_newlines,
        lifted.as_ref(),
    ) else {
        return lower_environment(node, cx);
    };
    if !items.iter().any(|item| matches!(item, GridItem::Row(_))) {
        // A body with no actual rows (empty, `\\`-only, or comment-only) has no
        // grid; let the generic path render it.
        return lower_environment(node, cx);
    }

    let aligns = column_alignments(node, cx).unwrap_or_default();
    let body = render_alignment_rows(&items, &aligns);
    Ir::concat([
        leading,
        begin,
        Ir::indent(Ir::concat([Ir::hard_line(), body])),
        Ir::hard_line(),
        end,
    ])
}

/// Lower a named **math** environment (`equation`, `align`, `gather`, matrix, …),
/// whose body the parser wrapped in a `MATH` node. Two layouts, chosen by the body's
/// shape:
///
/// - **Grid** (a top-level `&` or `\\`): `align`/matrix column-and-row grids, and
///   `gather`/`multline` row stacks (a single column). Reuses [`build_alignment_grid`]
///   in `math` mode, so cells get role-aware math spacing.
/// - **Single formula** (neither): `equation`/`displaymath`. Routes the `MATH` body
///   through [`lower_display_math_body`], the relation-aware amsmath-style breaker,
///   so a too-long formula breaks at its top-level relations/operators.
///
/// Framing (leading, `\begin` header, indented body, `\end`) mirrors
/// [`lower_display_math`] and [`lower_aligned_environment`]. If the body is not a
/// `MATH` node — which only happens if the formatter's `math` signature view diverges
/// from the parser's built-in one — it falls back to [`lower_environment`] rather than
/// mislaying the body.
fn lower_math_environment(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    let EnvParts {
        leading,
        begin,
        body: body_elements,
        end,
        lifted,
        // The `BEGIN` tail (see [`lower_begin`]) rides `body` as ordinary leading
        // elements: this path flattens the body itself, so it needs no splice.
        tail_len: _,
        body_header_token: _,
    } = split_environment(node, cx);
    let body_cx = cx.absorbing_trailing_control_newline(&body_elements);

    let Some(math_node) = body_elements
        .iter()
        .filter_map(|e| e.as_node())
        .find(|n| n.kind() == SyntaxKind::MATH)
    else {
        // The parser did not enter math mode for this environment (its built-in
        // signature is not `math`); render it generically.
        return lower_environment(node, cx);
    };

    // A top-level `&` or `\\` inside the `MATH` body means a grid; otherwise it is a
    // single formula.
    let is_grid = math_node
        .children_with_tokens()
        .any(|e| matches!(e.kind(), SyntaxKind::AMPERSAND | SyntaxKind::LINE_BREAK));

    let body = if is_grid {
        match build_alignment_grid(&body_elements, body_cx, true, false, lifted.as_ref()) {
            Some(items) if items.iter().any(|item| matches!(item, GridItem::Row(_))) => {
                let aligns = column_alignments(node, cx).unwrap_or_default();
                render_alignment_rows(&items, &aligns)
            }
            // A grid we cannot lay out on aligned rows (a mid-row comment, a blank
            // line, a nested block that is not the last cell of its row): fall back
            // to the generic environment lowering, exactly as
            // [`lower_aligned_environment`] does. A *trailing* nested block keeps
            // the grid — it renders as a hanging block cell (see [`Cell::block`]).
            _ => return lower_environment(node, cx),
        }
    } else {
        // Flatten the whole body, not only the parser's `MATH` child. Greedy
        // `\begin` attachment can leave a detached leading group in `body`
        // before that node (`\begin{equation}\n{\foo} : T`); selecting only
        // `math_node` would silently delete the group. The siblings are math
        // content too, and the display-formula lowerer handles them directly.
        let mut elements: Vec<SyntaxElement> = Vec::new();
        for element in &body_elements {
            if element.as_node() == Some(math_node) {
                elements.extend(math_node.children_with_tokens());
            } else {
                elements.push(element.clone());
            }
        }
        // Drop the lifted `\begin`-line comment (the `MATH` node's first token)
        // before the formula lowering, which would otherwise re-emit it.
        elements.retain(|e| !is_lifted_comment(e, lifted.as_ref()));
        if elements.iter().all(|e| {
            e.as_token()
                .is_some_and(|t| is_collapsible_trivia(t.kind()))
        }) {
            // Empty body (possibly only after the lift): `\begin` and `\end` on
            // adjacent lines, as [`lower_environment`]'s empty-body branch does,
            // rather than framing an empty indented line.
            return Ir::concat([leading, begin, Ir::hard_line(), end]);
        }
        trim_trailing_break(lower_display_formula_elements(&elements, body_cx))
    };

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
/// is attached as the row's `trailing_comment`; comment-only lines after it are
/// passthrough lines like any other. A comment in the *middle* of a row (with more
/// cells after it) cannot sit on an aligned line — its text runs to end of line,
/// commenting out the rest — so it returns `None` and falls back.
///
/// Returns `None` when [`flatten_alignment_body`] rejects the body (a blank-line
/// break), when a cell carries a forced break that cannot collapse to one line (a
/// nested block, or a blank line inside the cell — a lone continuation newline is
/// joined, not a fallback), or on a mid-row comment. Exception: in a *math* grid, a
/// cell whose forced break comes from a nested block environment (`aligned`,
/// `cases`, a matrix) becomes a multi-line *block* cell ([`Cell::block`]) instead of
/// a fallback, provided it is the last cell of its row — the grid survives and the
/// nested environment's lines hang at its `\begin{…}` column.
///
/// `math` is `true` for math grids (`align`, `pmatrix`, …) whose body the parser
/// wrapped in a `MATH` node: the flattener descends that node and each cell lowers
/// through the role-aware math sequencer ([`lower_math_seq`]) so operator spacing
/// and tight scripts apply. It is `false` for a non-math grid (`tabular`),
/// where the body is a prose block and cells lower through [`lower_element_stream`]
/// exactly as before. `soften_nested_newlines` is restricted to non-math cells in
/// fully owned virtual `.dtx` regions: it makes a margin-framed continuation inside
/// a parser-attached command behave like the same continuation after margin
/// normalization, while ordinary tables and math-grid fallbacks retain their
/// existing forced-break gates.
fn build_alignment_grid(
    body_elements: &[SyntaxElement],
    cx: LowerCtx<'_>,
    math: bool,
    soften_nested_newlines: bool,
    lifted: Option<&SyntaxToken>,
) -> Option<Vec<GridItem>> {
    let mut inline = flatten_alignment_body(body_elements, cx, math)?;
    // The `\begin`-line trailing comment was lifted onto the header
    // ([`split_environment`]); drop it here so it is not re-emitted as a
    // passthrough line of its own.
    inline.retain(|e| !is_lifted_comment(e, lifted));
    let printer = Printer::new(FormatStyle::default());
    let cell_cx = LowerCtx {
        in_alignment_cell: soften_nested_newlines,
        ..cx
    };

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
        cells: &mut Vec<Cell>,
        printer: &Printer,
        cx: LowerCtx<'_>,
        math: bool,
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
        // Read the `\multicolumn` span/alignment before the cell is drained below.
        let (span, align) = detect_multicolumn(cell);
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
        //
        // A math cell lowers through the role-aware sequencer, which already collapses
        // an interior whitespace/newline run to a single space (so a continuation line
        // joins) and applies operator spacing; a blank line still surfaces as a forced
        // break and (correctly) falls back below, but a nested block environment's
        // break yields a *block* cell instead (see [`Cell::block`]) so the grid
        // survives a `\begin{aligned}…`/`\begin{cases}…`/matrix cell.
        //
        // Block eligibility is read off the elements before they are drained. The
        // anchor is the first *node* child whose own lowering cannot stay flat —
        // a nested block environment, possibly wrapped in `\left…\right` or a
        // group. Only node children can carry a forced break here (comments
        // bailed above, a `\\` never lands inside a cell), so any break in the
        // full cell IR below is that node's structured layout, safe to hang at
        // its column. A verbatim-bodied environment anywhere in the cell's
        // subtree, or a blank line of the cell's own, still falls back.
        let first_block = if math {
            cell.iter().position(|e| {
                e.as_node().is_some()
                    && lower_math_element(e.clone(), cx, MathSpacing::Normal)
                        .contains_forced_break()
            })
        } else {
            None
        };
        let block_eligible = first_block.is_some()
            && !cell.iter().filter_map(|e| e.as_node()).any(|n| {
                n.descendants()
                    .any(|d| d.kind() == SyntaxKind::ENVIRONMENT && has_verbatim_body(&d))
            })
            && !cell_has_blank_line(cell);
        // The hang offset anchors a block cell's continuation lines at the
        // breaking node's start column: the flat width of the cell content before
        // it, plus the one joining space the sequencer places before it (a
        // relation/operator prefix like `= ` always gets one; the tight operand
        // juxtaposition `2\begin{…}` would not, costing one cosmetic column in
        // that unwritten shape). Computed before the drain below.
        let hang = match first_block.filter(|_| block_eligible) {
            None | Some(0) => 0,
            Some(i) => {
                let prefix =
                    lower_math_seq(cell[..i].iter().cloned(), cx, MathSpacing::Normal, false);
                let width = printer.print_flat(&prefix).trim().chars().count();
                if width == 0 { 0 } else { width + 1 }
            }
        };
        let ir = if math {
            lower_math_seq(cell.drain(..), cx, MathSpacing::Normal, false)
        } else {
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
            Ir::concat(joined)
        };
        if ir.contains_forced_break() {
            if !block_eligible {
                return None;
            }
            cells.push(Cell {
                text: String::new(),
                span,
                align,
                block: Some(BlockCell { hang, ir }),
            });
            return Some(());
        }
        cells.push(Cell {
            text: printer.print_flat(&ir).trim().to_string(),
            span,
            align,
            block: None,
        });
        Some(())
    }

    /// Inspect a not-yet-drained cell for a lone `\multicolumn{n}{spec}{body}`,
    /// returning its column span and the alignment from its `{spec}` (`(1, None)`
    /// for any ordinary cell). The greedy parser attaches all three `{…}` groups to
    /// the `\multicolumn` `COMMAND`, so the cell is a single command node; a
    /// non-integer span or unparsable spec degrades to span 1 / no override.
    fn detect_multicolumn(cell: &[SyntaxElement]) -> (usize, Option<ColAlign>) {
        let mut content = cell.iter().filter(|e| {
            !e.as_token()
                .is_some_and(|t| is_collapsible_trivia(t.kind()))
        });
        let Some(first) = content.next() else {
            return (1, None);
        };
        if content.next().is_some() {
            return (1, None);
        }
        let Some(node) = first.as_node() else {
            return (1, None);
        };
        if node.kind() != SyntaxKind::COMMAND
            || command_name(node).as_deref() != Some("multicolumn")
        {
            return (1, None);
        }
        let span = crate::ast::nth_group_text(node, 0)
            .and_then(|t| t.trim().parse::<usize>().ok())
            .filter(|&n| n >= 1);
        let align = crate::ast::nth_group(node, 1)
            .map(|g| crate::ast::group_inner_source(&g))
            .and_then(|s| colspec::parse_column_spec(&s))
            .and_then(|v| v.first().copied());
        (span.unwrap_or(1), align)
    }

    let mut items: Vec<GridItem> = Vec::new();
    let mut cells: Vec<Cell> = Vec::new();
    let mut cell: Vec<SyntaxElement> = Vec::new();

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
            // Not on its own line: it directly follows the previous row's `\\`.
            if line.has_rule {
                // The `\\ \hline` form — a rule sharing the physical line with the
                // preceding row's `\\`. Normalize it onto its own passthrough line
                // (idempotent: on re-parse it reads as an own-line rule).
                items.push(GridItem::Passthrough(line.text));
            } else if let Some(GridItem::Row(row)) = items.last_mut() {
                // A pure comment there trails that row.
                row.trailing_comment = Some(line.text);
            }
            cell.clear();
            idx = line.next;
            continue;
        }

        match &inline[idx] {
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::AMPERSAND => {
                finish_cell(&mut cell, &mut cells, &printer, cell_cx, math)?;
                // A block cell may only end its row: a `&` after one would need
                // the next cell to align past the block's last line, which the
                // grid cannot lay out — fall back.
                if cells.last().is_some_and(|c| c.block.is_some()) {
                    return None;
                }
            }
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::LINE_BREAK => {
                finish_cell(&mut cell, &mut cells, &printer, cell_cx, math)?;
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
                // clean only when nothing more of the row can follow: trivia and
                // own-line comments (each a passthrough line, handled by the
                // boundary branch above on later iterations) may remain; anything
                // else would be commented out by joining onto this line — fall
                // back.
                if !rest_is_trivia_and_comment_lines(&inline, idx + 1) {
                    return None;
                }
                let text = token.text().trim_end().to_string();
                finish_cell(&mut cell, &mut cells, &printer, cell_cx, math)?;
                items.push(GridItem::Row(AlignRow {
                    cells: std::mem::take(&mut cells),
                    line_break: None,
                    trailing_comment: Some(text),
                }));
            }
            _ => cell.push(inline[idx].clone()),
        }
        idx += 1;
    }

    // The final segment (content after the last `\\` or trailing comment). Drop it
    // when it is a single empty cell — the "body ended in `\\`" (or in a
    // trailing-comment row) case — so the trailing break stays on the prior row
    // without adding a blank line; otherwise it is a real last row.
    finish_cell(&mut cell, &mut cells, &printer, cell_cx, math)?;
    let final_is_empty = cells.len() == 1
        && cells[0].text.is_empty()
        && cells[0].block.is_none()
        && cells[0].span == 1;
    if !final_is_empty {
        items.push(GridItem::Row(AlignRow {
            cells,
            line_break: None,
            trailing_comment: None,
        }));
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
            // A bare rule control word — the peeled prefix of an over-attaching rule
            // command ([`rule_overattaches_cell`]), whose trailing `{…}` cell now
            // follows as its own element.
            SyntaxElement::Token(t) if token_is_rule_word(t, cx) => {
                has_rule = true;
                i += 1;
                content_end = i;
                continue;
            }
            // The booktabs `\cmidrule(lr){2-3}` paren trim spec. `(lr)` is generic
            // catcode-12 text (a `WORD`) that breaks the greedy argument attach, so
            // it and the following detached `{2-3}` range group arrive as loose
            // siblings after the rule command. Recognizing them as part of the rule
            // line is a layout decision (the rule-line concept is the formatter's).
            SyntaxElement::Token(t) if has_rule && is_paren_trim_word(t) => {
                i += 1;
                content_end = i;
                continue;
            }
            SyntaxElement::Node(n) if has_rule && n.kind() == SyntaxKind::GROUP => {
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
        SyntaxElement::Token(t) => t.kind() == SyntaxKind::COMMENT || token_is_rule_word(t, cx),
        SyntaxElement::Node(n) => n.kind() == SyntaxKind::COMMAND && is_rule_command(n, cx),
    }
}

/// Whether `token` is a bare control word naming a horizontal-rule command per the
/// signature DB — the peeled prefix of an over-attaching rule command (see
/// [`rule_overattaches_cell`]), recognized as a rule the same way an intact rule
/// `COMMAND` node is ([`is_rule_command`]).
fn token_is_rule_word(token: &SyntaxToken, cx: LowerCtx<'_>) -> bool {
    token.kind() == SyntaxKind::CONTROL_WORD
        && cx
            .signatures
            .command(token.text().trim_start_matches('\\'))
            .is_some_and(|sig| sig.rule)
}

/// Whether `token` is a booktabs `\cmidrule` paren trim spec — a `WORD` of the form
/// `(l)`, `(r)`, `(lr)`, or `(rl)` (catcode-12 text the lexer globs into one token).
/// `pub` because the linter's rule-span gate (`in_rule_span_argument`, in the
/// `badness` crate) recognizes the same shape; single-sourced so the two never
/// drift.
pub fn is_paren_trim_word(token: &SyntaxToken) -> bool {
    if token.kind() != SyntaxKind::WORD {
        return false;
    }
    let t = token.text();
    t.len() >= 3
        && t.starts_with('(')
        && t.ends_with(')')
        && t[1..t.len() - 1].chars().all(|c| c == 'l' || c == 'r')
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

/// Whether the cell's own top-level trivia contains a blank line (two newlines
/// with only inline whitespace between them, the `\par` boundary). A blank line
/// inside a *child* node (a nested environment's body) is that node's own
/// business and is not seen here.
fn cell_has_blank_line(cell: &[SyntaxElement]) -> bool {
    let mut run_newlines = 0;
    for e in cell {
        match e.as_token().map(SyntaxToken::kind) {
            Some(SyntaxKind::NEWLINE) => {
                run_newlines += 1;
                if run_newlines >= 2 {
                    return true;
                }
            }
            Some(SyntaxKind::WHITESPACE) => {}
            _ => run_newlines = 0,
        }
    }
    false
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

/// Whether everything from `from` onward is collapsible trivia or `%` comments
/// that each start their own physical line — nothing more of the *row* remains,
/// so a comment at the current position is a clean trailing comment, and any
/// following comment-only lines render as passthrough lines. A comment that does
/// not sit on its own line, or any non-trivia element, disqualifies the rest
/// (joining it onto the row's line would comment it out).
fn rest_is_trivia_and_comment_lines(inline: &[SyntaxElement], from: usize) -> bool {
    let mut own_line = false;
    for element in &inline[from..] {
        let Some(token) = element.as_token() else {
            return false;
        };
        match token.kind() {
            SyntaxKind::NEWLINE => own_line = true,
            SyntaxKind::WHITESPACE => {}
            SyntaxKind::COMMENT if own_line => own_line = false,
            _ => return false,
        }
    }
    true
}

/// Flatten an alignment environment's body into a single stream of inline
/// elements, descending one level into the lone body wrapper node (where the `&`
/// and `\\` separators live) — a `PARAGRAPH` for a prose grid (`tabular`), or a
/// `MATH` node for a `math` grid (`align`/matrix, parsed in math mode). Trivia
/// outside the wrapper is dropped (it is just the body's own leading/trailing break,
/// which the indenter re-supplies).
///
/// Returns `None` when the body holds more than one wrapper node — a blank-line
/// break, which the single grid does not model — so the caller falls back.
///
/// A rule command that the greedy parser saddled with the next line's first cell
/// as a bogus `{…}` argument ([`rule_overattaches_cell`]) is expanded into its own
/// children, so the rule lands on its own passthrough line and the `{…}` is handed
/// back to the grid as cell content.
fn flatten_alignment_body(
    body_elements: &[SyntaxElement],
    cx: LowerCtx<'_>,
    math: bool,
) -> Option<Vec<SyntaxElement>> {
    let wrapper = if math {
        SyntaxKind::MATH
    } else {
        SyntaxKind::PARAGRAPH
    };
    let mut inline: Vec<SyntaxElement> = Vec::new();
    let mut paragraphs = 0;
    for element in strip_virtual_dtx_framing(body_elements.iter().cloned(), cx) {
        match element {
            SyntaxElement::Node(child) if child.kind() == wrapper => {
                paragraphs += 1;
                if paragraphs > 1 {
                    return None;
                }
                extend_alignment_elements(&mut inline, child.children_with_tokens(), cx);
            }
            SyntaxElement::Token(token) if is_collapsible_trivia(token.kind()) => {}
            other => push_alignment_element(&mut inline, other, cx),
        }
    }
    Some(inline)
}

/// Extend a flattened grid stream through the virtual-document framing adapter.
/// The adapter is applied at every level the grid flattener descends, so a source
/// margin and its padding cannot become cell content merely because the parser
/// attached that physical line inside a wrapper or an over-attaching rule node.
fn extend_alignment_elements(
    inline: &mut Vec<SyntaxElement>,
    elements: impl IntoIterator<Item = SyntaxElement>,
    cx: LowerCtx<'_>,
) {
    for element in strip_virtual_dtx_framing(elements, cx) {
        push_alignment_element(inline, element, cx);
    }
}

/// Push one flattened-body element, expanding an over-attaching rule command
/// ([`rule_overattaches_cell`]) into its children so its trailing `{…}` cell is
/// exposed to the grid rather than glued to the rule.
fn push_alignment_element(
    inline: &mut Vec<SyntaxElement>,
    element: SyntaxElement,
    cx: LowerCtx<'_>,
) {
    if let SyntaxElement::Node(node) = &element
        && rule_overattaches_cell(node, cx)
    {
        extend_alignment_elements(inline, node.children_with_tokens(), cx);
    } else {
        inline.push(element);
    }
}

/// Whether `node` is a horizontal-rule `COMMAND` (`\midrule`, `\toprule`, …) onto
/// which the greedy parser attached the next line's first
/// cell as a spurious `{…}` argument. Booktabs rules take at most an optional
/// `[width]`, never a mandatory brace argument, so a leading `{…}` is never a real
/// argument — it is cell content the arity refinement peels back off.
///
/// Restricted to a *leading* `{…}` (no real argument consumed first): the rare
/// `\toprule[2pt]{…}` shape keeps the generic fallback rather than being split.
fn rule_overattaches_cell(node: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    if node.kind() != SyntaxKind::COMMAND || !is_rule_command(node, cx) {
        return false;
    }
    let Some(first_arg) = node
        .children()
        .find(|child| matches!(child.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL))
    else {
        return false;
    };
    if first_arg.kind() != SyntaxKind::GROUP {
        return false;
    }
    // A leading `{…}` is a real argument only when the signature's first slot is a
    // mandatory brace argument (`\cline{2-3}`, `\specialrule{…}`).
    let first_slot_is_brace = command_name(node)
        .and_then(|name| cx.signatures.command(&name))
        .and_then(|sig| sig.args.first())
        .is_some_and(|arg| arg.kind == ArgKind::Brace);
    !first_slot_is_brace
}

/// The per-column alignments declared by a `tabular`/`array` environment's column
/// specification (`\begin{tabular}{lcr}` → `[Left, Center, Right]`), or `None` to
/// signal the caller to fall back to all-left.
///
/// Only environments the signature DB marks `align` carry a user column spec; a math
/// grid's `\begin` has no argument `GROUP` at all (the environment name is a separate
/// `NAME_GROUP`), so those return `None` and fall through. The spec is the *last*
/// `{…}` `GROUP` on the `\begin` — uniform across `tabular`'s `{spec}`, `array`'s
/// `{spec}`, and `tabular*`'s `{width}[pos]{spec}` (the `[pos]` is an `OPTIONAL`, not
/// a `GROUP`). The raw inner source is read via [`Group::inner_source`] rather than
/// `nth_group_text`, which would bail on the nested `{…}` of a `p{3cm}` column.
fn column_alignments(env: &SyntaxNode, cx: LowerCtx<'_>) -> Option<Vec<ColAlign>> {
    let begin = Environment::cast(env.clone())?.begin()?;
    if !cx
        .signatures
        .environment_at(begin.syntax())
        .is_some_and(|sig| sig.align)
    {
        return None;
    }
    let spec = crate::ast::children::<Group>(begin.syntax()).last()?;
    colspec::parse_column_spec(&spec.inner_source())
}

/// Render the grid to IR: align each cell within its column to the column's declared
/// alignment (`aligns`, empty = all-left), join cells with `" & "`, append the row's
/// `\\` and any trailing comment, and join all items with [`Ir::hard_line`]. A row is
/// one [`Ir::text`] (no newline; cells are flat), which the caller indents one step.
/// Rows terminated by `\\` pad to the full grid width, aligning their terminators;
/// unterminated rows omit that final padding so they never carry trailing whitespace.
/// [`GridItem::Passthrough`] lines (comments, `\hline`/`\midrule`, …) are emitted
/// verbatim between rows and never counted toward column widths.
///
/// A `\multicolumn{n}{…}{…}` cell spans `n` columns: it never defines a single
/// column's width, and its rendered field is the sum of the spanned column widths
/// plus the `" & "` separators it absorbs. The spanned columns keep their
/// data-derived widths (the `\multicolumn`'s *source* markup is usually wider than a
/// few narrow columns; growing them to fit would balloon the data rows), so when the
/// markup exceeds its span it simply overflows that one row — matching how such a row
/// is written by hand. When the span is instead *wider* than the markup, the cell is
/// aligned within it per the `\multicolumn`'s own `{spec}`.
fn render_alignment_rows(items: &[GridItem], aligns: &[ColAlign]) -> Ir {
    const SEP: &str = " & ";

    // Column width = the max char-count over every *span-1* cell in that column
    // (including last cells, so a long final cell still widens the column above it).
    // Char count matches the printer's own column metric. Spanning cells and
    // passthrough lines do not participate here.
    let mut col_widths: Vec<usize> = Vec::new();
    let mut widen = |c: usize, width: usize| {
        while c >= col_widths.len() {
            col_widths.push(0);
        }
        if width > col_widths[c] {
            col_widths[c] = width;
        }
    };
    for item in items {
        let GridItem::Row(row) = item else { continue };
        let mut c = 0;
        for cell in &row.cells {
            if cell.span == 1 && cell.block.is_none() {
                widen(c, cell.text.chars().count());
            }
            c += cell.span;
        }
    }

    // The combined field width of the `span` columns starting at `c`: their widths
    // plus the `" & "` separators the spanning cell absorbs.
    let field_width = |c: usize, span: usize| -> usize {
        let sum: usize = (c..c + span)
            .map(|i| col_widths.get(i).copied().unwrap_or(0))
            .sum();
        sum + SEP.len() * (span - 1)
    };
    let grid_width =
        col_widths.iter().sum::<usize>() + SEP.len() * col_widths.len().saturating_sub(1);
    let grid_width = grid_width.saturating_sub(usize::from(
        col_widths.len() > 1 && col_widths.first() == Some(&0),
    ));

    let lines = items.iter().map(|item| {
        let row = match item {
            GridItem::Passthrough(text) => {
                // A passthrough spans physical lines when a comment-only line
                // binds into a rule command as its `DOC_COMMENT` (issue #49):
                // flat-printing keeps the doc comment's break as a raw newline,
                // which a single `Ir::text` would emit without re-applying the
                // grid indent. Split so every physical line is indented.
                return Ir::join(
                    Ir::hard_line(),
                    text.lines().map(|line| Ir::text(line.trim().to_string())),
                );
            }
            GridItem::Row(row) => row,
        };
        let mut line = String::new();
        // A block cell (always the row's last, see [`Cell::block`]): its later
        // lines hang at the nested environment's start column, so its IR goes
        // inside an [`Ir::align`] whose width is the flat prefix already on the
        // line plus the cell's own hang offset. It takes no padding and no
        // width — like a spanning cell, it overflows.
        let mut block: Option<Ir> = None;
        let last = row.cells.len().saturating_sub(1);
        let mut c = 0;
        for (idx, cell) in row.cells.iter().enumerate() {
            if idx > 0 {
                // A row never opens with the separator's leading space: when
                // everything before this `&` is empty (an `aligned`/`split` body
                // whose rows all start at `&`, so the leading column's width is
                // 0), there is nothing to separate and the `&` is the line's
                // first character. A *padded* empty cell (its column is nonzero
                // elsewhere) has already pushed its pad, keeping the `&` aligned.
                line.push_str(if line.is_empty() { "& " } else { SEP });
            }
            if let Some(block_cell) = &cell.block {
                block = Some(Ir::align(
                    line.chars().count() + block_cell.hang,
                    block_cell.ir.clone(),
                ));
                break;
            }
            let field = field_width(c, cell.span);
            let text_width = cell.text.chars().count();
            let pad = field.saturating_sub(text_width);
            // A `\multicolumn`'s own `{spec}` overrides the column alignment.
            let align = cell
                .align
                .unwrap_or_else(|| aligns.get(c).copied().unwrap_or(ColAlign::Left));
            let (leading, trailing) = match align {
                ColAlign::Left => (0, pad),
                ColAlign::Right => (pad, 0),
                ColAlign::Center => (pad / 2, pad - pad / 2),
            };
            // The last cell never carries trailing whitespace (leading pad is fine).
            let trailing = if idx == last { 0 } else { trailing };
            line.push_str(&" ".repeat(leading));
            line.push_str(&cell.text);
            line.push_str(&" ".repeat(trailing));
            c += cell.span;
        }
        let mut tail = String::new();
        if let Some(line_break) = &row.line_break {
            if block.is_none() {
                line.push_str(&" ".repeat(grid_width.saturating_sub(line.chars().count())));
            }
            tail.push(' ');
            tail.push_str(line_break);
        }
        // The trailing comment always follows the `\\` so the break is never
        // commented out.
        if let Some(comment) = &row.trailing_comment {
            tail.push(' ');
            tail.push_str(comment);
        }
        match block {
            Some(block) => Ir::concat([Ir::text(line), block, Ir::text(tail)]),
            None => {
                line.push_str(&tail);
                Ir::text(line)
            }
        }
    });
    Ir::join(Ir::hard_line(), lines)
}

/// Lower a delimited group — a brace group `{…}` (`open`/`close` =
/// `L_BRACE`/`R_BRACE`) or an optional-argument group `[…]`
/// (`L_BRACKET`/`R_BRACKET`) — indenting its body one step, exactly like
/// [`lower_environment`] but with token delimiters instead of `BEGIN`/`END`
/// nodes. Under the Tier-2 wrap modes it is called for multi-line groups only
/// (see [`spans_multiple_lines`]); under [`WrapMode::Reflow`] it is the block
/// form a group or optional falls back to when the width-driven paths
/// ([`lower_opaque_group`], [`lower_optional`]) decline — a blank line, a
/// comment, nested block content — where the node may also be single-line
/// (`\baz[{c% x\nd}]`).
///
/// Inside a group the parser emits body tokens directly (no `PARAGRAPH`
/// wrapping), so the only `open` token is the first child and the only `close`
/// token is the last — but an `OPTIONAL` body may contain a stray `[` (TeX does
/// not nest `[`), so the opener is captured only once (`open_ir` still `Nil`).
fn lower_bracketed(
    node: &SyntaxNode,
    open: SyntaxKind,
    close: SyntaxKind,
    cx: LowerCtx<'_>,
    keyval: bool,
) -> Ir {
    let mut open_ir = Ir::Nil;
    let mut close_ir = Ir::Nil;
    let mut body_elements: Vec<SyntaxElement> = Vec::new();
    for element in strip_virtual_dtx_framing(node.children_with_tokens(), cx) {
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

    // Whether the source glued the open delimiter directly to the first body
    // token, with no whitespace between them (`{\let…`). In normal catcodes an
    // end-of-line after a non-control-word token reads as a *space* (TeX state M
    // holds right after a `{`), so a break the formatter synthesizes here would
    // inject a space token the author never wrote — a silent meaning change
    // wherever space tokens matter (horizontal mode). We therefore only break
    // after the opener when the source already had whitespace there, where
    // whitespace ↔ newline is TeX-identical. The parser emits leading
    // whitespace/newlines as their own trivia tokens, so a whitespace boundary
    // shows up as a leading `WHITESPACE`/`NEWLINE` element; anything else (real
    // content, a nested node) means the opener was glued. This path is never
    // reached inside an expl3 region (routed to `lower_expl_group` earlier),
    // where source whitespace is catcode-9 and the synthesized break is sound.
    // Two things lift the guard. A `[…]` always did (`collapse_arg_group`, issue
    // #47): it freely swaps its interior newlines for spaces, so the Allman break
    // after `[` is by design. And a *proven keyval* body does, whatever its
    // delimiter — `ContentKind::Keyval` asserts the processor strips spaces around
    // entries, which is the same license under a different name. Reading `open`
    // alone was the proxy for that second one, sound only while keyval lived on
    // brackets; left in place it glued a `\pgfkeys{a=1,` opener while its closer
    // still took its own line, an asymmetry nothing justified.
    let open_glued = open == SyntaxKind::L_BRACE
        && !keyval
        && body_elements
            .first()
            .and_then(SyntaxElement::as_token)
            .is_none_or(|t| !is_collapsible_trivia(t.kind()));

    // Mirror the opener rule at the other edge. If the final body element was
    // glued to a meaningful closer, inserting a line break before that closer
    // would add a space token to the group. This is observable in ordinary text
    // and in a `\def` replacement body. Proven keyval processors are exempt: the
    // signature guarantees that surrounding entry whitespace is insignificant.
    let close_glued = !keyval
        && body_elements.last().is_some_and(|element| {
            element
                .as_token()
                .is_none_or(|token| !is_collapsible_trivia(token.kind()))
        });

    // A brace-group body under reflow is laid out as code-like statements: each
    // source line stays its own logical line, but an over-long one wraps to the
    // width instead of forcing the printer to break the innermost nested prose
    // group (the only soft break a rigid `lower_element_stream` body would expose).
    // Optional `[…]` bodies and the non-reflow modes keep the generic stream.
    let body =
        if matches!(cx.wrap, WrapMode::Reflow | WrapMode::Stable) && open == SyntaxKind::L_BRACE {
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
        // A glued opener keeps the first body line on the opener's line; the
        // `Ir::indent` still indents the body's *interior* breaks one step, so
        // only the first line rides the opener (`{\aaa` / `␣␣\bbb`).
        let lead = if open_glued { Ir::Nil } else { Ir::hard_line() };
        let trail = if close_glued {
            Ir::Nil
        } else {
            Ir::hard_line()
        };
        Ir::concat([
            open_ir,
            Ir::indent(Ir::concat([lead, body])),
            trail,
            close_ir,
        ])
    }
}

/// Lower a brace [`SyntaxKind::GROUP`] under [`WrapMode::Reflow`]: a
/// width-driven fill over its body, so block-vs-inline is decided by width,
/// content, and preserved predicates — never by whether the author happened to
/// break the line ([`spans_multiple_lines`], the unsafe lone-newline
/// predicate; see the trivia-invariant-layout section of `formatter.md`).
///
/// The flat rendering is byte-identical to the generic inline path except that
/// a lone-newline run renders as one space — the newline ↔ space exchange that
/// is TeX-identical. Break opportunities are exactly the perturbation-eligible
/// gaps ([`crate::formatter::perturb`]): a lone-newline run and a single-space
/// gap, both of which render `" "` flat, so `fmt(perturbed) == fmt(original)`
/// holds by construction. Any other gap spelling (`a␣␣b`, a tab) glues
/// verbatim into its atom, and a glued junction never gains a break — breaking
/// where the author glued would inject a space token TeX typesets (the same
/// rationale as [`lower_bracketed`]'s `open_glued`). Edge padding rides the
/// flat rendering and vanishes broken, where the delimiter's own newline
/// supplies the space token; an empty body keeps its padding flat (`{ }` and
/// `{\n}` both render `{ }` — deleting it would delete a space token).
///
/// A body the fill cannot own takes today's indented block form
/// ([`lower_bracketed`]) instead, keyed on preserved predicates and content
/// only: an *interior* blank line, a direct `%` comment (which must end its
/// line; the glued `{%` case included), a token embedding a newline (a
/// multi-line brace `\verb`), or a child whose IR carries a forced break —
/// nested block content, itself decided by preserved predicates and content
/// under this policy, so the read stays Tier-1-clean. A blank run at the
/// body's *edge* does not decline: the block form trims it away
/// ([`trim_leading_break`]/[`trim_trailing_break`]), so edge-blank presence is
/// not a predicate the block form preserves — it erases to padding here,
/// matching the deletion the block form already performed.
///
/// Known residuals, argued safe by catcode rather than by oracle: a
/// multi-space run stays authored (`{a  b}` and `{a  \nb}` differ, but neither
/// is an eligible perturbation and TeX collapses a catcode-10 run to one space
/// token); a lone newline beside a `\verb` is erased though the oracle
/// excludes VERB-adjacent gaps (a complete `VERB` token carries its
/// delimiters); and an `\obeylines` body joins — unresolvable macro meaning is
/// out of scope, as it is for paragraph reflow generally.
fn lower_opaque_group(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    /// Resolve the gap read but not yet committed: a `" "` flat gap is a break
    /// opportunity (the fill's own separator renders it), anything else glues
    /// verbatim into the atom in progress.
    fn commit_gap(atoms: &mut Vec<Ir>, atom: &mut Vec<Ir>, pending: &mut Option<(String, bool)>) {
        if let Some((gap, _)) = pending.take() {
            if gap == " " {
                if !atom.is_empty() {
                    atoms.push(Ir::concat(std::mem::take(atom)));
                }
            } else {
                atom.push(Ir::verbatim(gap));
            }
        }
    }
    let block = || lower_bracketed(node, SyntaxKind::L_BRACE, SyntaxKind::R_BRACE, cx, false);
    let mut open = Ir::Nil;
    let mut close = Ir::Nil;
    let mut lead: Option<String> = None;
    let mut atoms: Vec<Ir> = Vec::new();
    let mut atom: Vec<Ir> = Vec::new();
    // The flat spelling of the gap read but not yet committed, and whether it
    // was a blank-line run. Only an *interior* blank line declines — the block
    // form trims a blank at the body's edge away (`trim_leading_break` /
    // `trim_trailing_break`), so declining on one would key on a predicate the
    // emitter then destroys: pass 2 would see no blank and flatten (the
    // latexindent `poly-switch-blank-line` family). An edge blank erases to
    // padding, exactly the deletion the block form already performed.
    let mut pending: Option<(String, bool)> = None;
    let mut iter = alignment_cell_elements(node.children_with_tokens(), cx)
        .into_iter()
        .peekable();
    while let Some(element) = iter.next() {
        match element {
            SyntaxElement::Token(t)
                if t.kind() == SyntaxKind::L_BRACE && matches!(open, Ir::Nil) =>
            {
                open = Ir::verbatim(t.text());
            }
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::R_BRACE => {
                close = Ir::verbatim(t.text());
            }
            SyntaxElement::Token(t)
                if cx.in_dtx_doc_region && t.kind() == SyntaxKind::DOC_MARGIN =>
            {
                // The virtual region owns both the physical margin and its source
                // padding. Retaining the padding makes a wrapped group add its
                // indentation again on every pass.
                while matches!(
                    iter.peek(),
                    Some(SyntaxElement::Token(next)) if next.kind() == SyntaxKind::WHITESPACE
                ) {
                    iter.next();
                }
            }
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {
                let gap = consume_gap(&t, &mut iter);
                let blank = gap == Gap::Blank;
                let flat = gap.flat().to_string();
                if atoms.is_empty() && atom.is_empty() {
                    lead = Some(flat);
                } else {
                    pending = Some((flat, blank));
                }
            }
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::COMMENT => return block(),
            SyntaxElement::Token(t) if t.text().contains('\n') => return block(),
            SyntaxElement::Token(t) => {
                if matches!(pending, Some((_, true))) {
                    return block(); // an interior blank line: preserved predicate
                }
                commit_gap(&mut atoms, &mut atom, &mut pending);
                atom.push(lower_loose_token(&t, cx));
            }
            SyntaxElement::Node(child) => {
                let ir = lower_node(&child, cx);
                if ir.contains_forced_break() {
                    return block(); // nested block content
                }
                if matches!(pending, Some((_, true))) {
                    return block(); // an interior blank line: preserved predicate
                }
                commit_gap(&mut atoms, &mut atom, &mut pending);
                atom.push(ir);
            }
        }
    }
    let trail: Option<String> = pending.take().map(|(gap, _)| gap);
    if !atom.is_empty() {
        atoms.push(Ir::concat(atom));
    }
    if atoms.is_empty() {
        // `{}` / `{ }` / `{\n}`: nothing to lay out; the padding survives flat.
        let lead = lead.map(Ir::verbatim).unwrap_or(Ir::Nil);
        return Ir::concat([open, lead, close]);
    }
    // An edge gap joins the vanish-when-broken protocol only when its flat
    // spelling is `" "` — the one spelling a break reproduces (the broken
    // form's newline re-reads as a lone-newline gap, whose flat is `" "`), and
    // the same criterion the interior break opportunities use. Any other
    // spelling (`{0    }`) must ride verbatim and never break: vanishing it
    // would hand pass 2 a `" "` gap where pass 1 measured four spaces, and the
    // layout oscillates (pgf's `\pgfpoint@oncoil{0    }` coil tables).
    let mut parts: Vec<Ir> = vec![open];
    let mut inner: Vec<Ir> = Vec::new();
    match lead.as_deref() {
        None => {}
        Some(" ") => {
            inner.push(Ir::soft_line());
            inner.push(Ir::if_break(Ir::verbatim(" "), Ir::Nil));
        }
        Some(other) => parts.push(Ir::verbatim(other)),
    }
    inner.push(Ir::fill(atoms));
    parts.push(Ir::indent(Ir::concat(inner)));
    match trail.as_deref() {
        None => {}
        Some(" ") => {
            parts.push(Ir::if_break(Ir::verbatim(" "), Ir::Nil));
            parts.push(Ir::soft_line());
        }
        Some(other) => parts.push(Ir::verbatim(other)),
    }
    parts.push(close);
    Ir::group(Ir::concat(parts))
}

/// Lower a [`SyntaxKind::OPTIONAL`] argument group, or `None` to leave it on the
/// generic inline path. The bracket entry point to [`lower_segmented_group`].
fn lower_optional(node: &SyntaxNode, cx: LowerCtx<'_>, keyval: bool) -> Option<Ir> {
    lower_segmented_group(
        node,
        SyntaxKind::L_BRACKET,
        SyntaxKind::R_BRACKET,
        cx,
        keyval,
    )
}

/// Lower a delimited argument group as a comma-segmented Wadler group, or `None` to
/// leave it on the generic inline path.
///
/// The body is a plain Wadler group over its top-level comma-separated entries: flat
/// when it fits the width, one key per line when it does not. The flat rendering is
/// exactly the old collapsed form, so `\foo[a=1,\nb=2]` still formats as
/// `\foo[a=1, b=2]` (issue #47) — a source line break inside `[…]` is incidental —
/// while an over-long bracket now *expands* instead of silently overflowing, and the
/// choice no longer depends on whether the author happened to break the line
/// (`spans_multiple_lines` was the unsafe lone-newline predicate; see the
/// trivia-invariant-layout section of `formatter.md`). The fit decision is this
/// group's rest-aware measurement, so trailing same-line content (`]{c}`) counts
/// toward it.
///
/// `keyval` reports that the signature DB proved this argument a `key=value` list
/// (see [`ContentKind::Keyval`]), which additionally licenses splitting a comma the
/// author *glued*. Without it only gaps the author already wrote are break
/// opportunities, so a textual optional (`\item[red,green]`, a `\newcommand`
/// default) can never gain a space that would be typeset.
///
/// `{…}` reaches here only *through* that proof. A mandatory group is the ordinary
/// home of typeset text, so the generic opaque lowering owns it by default; the
/// keyval-family setters (`\pgfkeys`, `\tikzset`, `\lstset`, …) opt in through the
/// curated signature DB, and there segmenting at commas is the whole point — the
/// alternative is reflowing a key list as prose, which wraps mid-key.
///
/// A body that is not safely segmentable — a blank line, a `%` comment, nested
/// block content — takes the indented block form ([`lower_bracketed`])
/// unconditionally, so both spellings of the same content land on it (the
/// choice reads content and preserved predicates, never a lone newline). With
/// no split point at all the group collapses to one atom rather than
/// uselessly detonating into `[\n!htb\n]`. Inert under [`WrapMode::Preserve`]
/// and the other non-prose-wrapping modes, which keep the pre-existing block
/// layout.
fn lower_segmented_group(
    node: &SyntaxNode,
    open_kind: SyntaxKind,
    close_kind: SyntaxKind,
    cx: LowerCtx<'_>,
    keyval: bool,
) -> Option<Ir> {
    // A captured xparse `v` argument is same-line by construction. Breaking a
    // preceding optional makes the next parse lose the VERB capture and exposes
    // raw name bytes as ordinary LaTeX syntax.
    if node.next_sibling().is_some_and(|sibling| {
        sibling.kind() == SyntaxKind::GROUP
            && sibling
                .descendants_with_tokens()
                .filter_map(SyntaxElement::into_token)
                .any(|token| token.kind() == SyntaxKind::VERB)
    }) {
        return None;
    }
    // A `[…]` continuing across `.dtx` doc-margined lines keeps its authored
    // margins: `lower_node` already gates on this, but the signature-aware callers
    // reach here directly, and relaying such a body would move content off its `%`.
    //
    // The second gate is the one that bites: a bracket *on* a doc line
    // (`% \begin{function}[EXP, pTF]{…}`) holds no margin token of its own, so
    // `contains_doc_margin` says nothing, yet every line a break here creates would
    // land unmargined — silently promoting documentation to live code. The old
    // lowering was safe by accident (it only ever broke an already-multi-line
    // bracket); a width-driven group has to say so.
    if !cx.wraps_prose()
        || (!cx.in_dtx_doc_region
            && (contains_doc_margin(node, cx) || doc_margin_opens_line(node, cx)))
    {
        // Tier-2 residue: under a mode that does not wrap prose (or on a `.dtx`
        // doc line) the pre-existing behaviour is kept byte for byte — block
        // form when the author broke the line, generic inline path otherwise.
        // Fixed-point argument on [`spans_multiple_lines`].
        return spans_multiple_lines(node)
            .then(|| lower_bracketed(node, open_kind, close_kind, cx, keyval));
    }
    let Some(segments) = segment_delimited_body(node, open_kind, close_kind, cx, keyval) else {
        // Not safely segmentable: blank line, comment, or a child carrying a
        // forced break. The first two put a NEWLINE directly in the node, but
        // the third occurs in single-line spellings too (`\baz[{c\n\nd}]`), so
        // the block form applies unconditionally — both spellings of the same
        // content take it, keyed on content and preserved predicates alone.
        return Some(lower_bracketed(node, open_kind, close_kind, cx, keyval));
    };
    let GroupSegments {
        open,
        mut parts,
        close,
        splits,
    } = segments;
    // Padding at the body's edges rides the flat rendering but must vanish when the
    // delimiters take their own lines, or the first key lands at indent + 1.
    let lead = peel_padding(&mut parts, Edge::Leading);
    let trail = peel_padding(&mut parts, Edge::Trailing);
    let body = Ir::concat(parts);
    if splits == 0 {
        // Nothing to break at: emit the collapsed atom and let it overflow. A
        // breakable group here would push `[!htb]` onto three lines to no gain.
        return Some(Ir::concat([open, lead, body, trail, close]));
    }
    Some(Ir::group(Ir::concat([
        open,
        Ir::indent(Ir::concat([
            Ir::soft_line(),
            Ir::if_break(lead, Ir::Nil),
            body,
        ])),
        Ir::if_break(trail, Ir::Nil),
        Ir::soft_line(),
        close,
    ])))
}

/// Which end of an `[…]` body [`peel_padding`] works on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Edge {
    Leading,
    Trailing,
}

/// Remove and return the whitespace padding at one end of a segmented `[…]` body
/// (see [`is_trimmable_break`]). A [`Ir::Nil`] result means there was none.
fn peel_padding(parts: &mut Vec<Ir>, edge: Edge) -> Ir {
    let mut padding = Ir::Nil;
    loop {
        let at = match edge {
            Edge::Leading => 0,
            Edge::Trailing => parts.len().saturating_sub(1),
        };
        match parts.get(at) {
            Some(ir) if is_trimmable_break(ir) => {
                let taken = parts.remove(at);
                if matches!(padding, Ir::Nil) {
                    padding = taken;
                }
            }
            _ => return padding,
        }
    }
}

/// A delimited body cut into its top-level entries: the delimiters, the entry IR
/// with [`Gap::separator`] split points already interleaved, and how many there are.
struct GroupSegments {
    open: Ir,
    parts: Vec<Ir>,
    close: Ir,
    splits: usize,
}

/// Segment an `OPTIONAL` or `GROUP` body at its top-level commas, or `None` when the
/// body is not safely segmentable — the same three bail conditions as
/// [`collapse_arg_group`]: a blank-line `\par`, a `%` comment (which must end its
/// line), or nested content carrying a forced break.
///
/// A comma is a split point only at bracket depth 0. The parser closes an
/// `OPTIONAL` at its first `]`, so a stray `[` inside the body (TeX does not nest
/// `[`) opens a region that never closes — everything after it stays glued, which
/// is the conservative reading: `\foo[a=[1,2]` must not break at the `1,2`. A `{…}`
/// body needs no matching rule for braces: the parser gives every nested brace group
/// its own `GROUP` node, so the only `L_BRACE`/`R_BRACE` *tokens* here are this
/// body's own delimiters, and a nested comma arrives already sealed inside a child.
fn segment_delimited_body(
    node: &SyntaxNode,
    open_kind: SyntaxKind,
    close_kind: SyntaxKind,
    cx: LowerCtx<'_>,
    split_glued_commas: bool,
) -> Option<GroupSegments> {
    let mut open = Ir::Nil;
    let mut close = Ir::Nil;
    let mut parts: Vec<Ir> = Vec::new();
    let mut splits = 0usize;
    let mut depth = 0usize;
    // Whether the last content token was a `WORD` ending in `,` at depth 0, so the
    // *next* gap is a break opportunity.
    let mut open_entry = false;
    // Whether the entry currently being accumulated holds any content yet — what
    // tells [`push_entry_word`] a leading comma closes a real entry rather than an
    // empty one.
    let mut entry_open = false;
    let mut iter = strip_virtual_dtx_framing(node.children_with_tokens(), cx)
        .into_iter()
        .peekable();
    while let Some(element) = iter.next() {
        match element {
            SyntaxElement::Token(t) if t.kind() == open_kind && matches!(open, Ir::Nil) => {
                open = Ir::verbatim(t.text());
            }
            SyntaxElement::Token(t) if t.kind() == close_kind => {
                close = Ir::verbatim(t.text());
            }
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::L_BRACKET => {
                depth += 1;
                open_entry = false;
                entry_open = true;
                parts.push(Ir::verbatim(t.text()));
            }
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {
                let gap = consume_gap(&t, &mut iter);
                if gap == Gap::Blank {
                    return None; // a blank-line `\par`: keep the block form
                }
                if open_entry && depth == 0 {
                    parts.push(gap.separator());
                    splits += 1;
                    entry_open = false;
                } else {
                    // Not a split point: the gap rides at its flat spelling, which
                    // collapses a lone newline to a single space and keeps pure
                    // inline whitespace verbatim, matching the generic lowering.
                    parts.push(Ir::verbatim(gap.flat()));
                }
                open_entry = false;
            }
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::COMMENT => return None,
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::WORD && depth == 0 => {
                splits += push_entry_word(t.text(), split_glued_commas, &mut parts, entry_open);
                // A word ending in `,` closes its entry and opens an empty one.
                open_entry = t.text().ends_with(',');
                entry_open = !open_entry;
            }
            SyntaxElement::Token(t) => {
                parts.push(lower_loose_token(&t, cx));
                open_entry = false;
                entry_open = true;
            }
            SyntaxElement::Node(child) => {
                let ir = lower_node(&child, cx);
                if ir.contains_forced_break() {
                    return None; // nested block content: keep the block form
                }
                parts.push(ir);
                open_entry = false;
                entry_open = true;
            }
        }
    }
    // A trailing separator (`[a, b, ]`) would put the closing `]` two lines down.
    // Drop it — but an `Ir::Line` replaced authored whitespace above, and an
    // optional is textual, so that space token must survive as trailing padding
    // (`[a, ]` and `[a,\n]` both keep it); a glued-comma `Ir::SoftLine` stood
    // for nothing and restores nothing.
    let mut dropped_gap = false;
    while matches!(parts.last(), Some(Ir::Line | Ir::SoftLine)) {
        dropped_gap |= matches!(parts.last(), Some(Ir::Line));
        parts.pop();
        splits = splits.saturating_sub(1);
    }
    if dropped_gap {
        parts.push(Ir::verbatim(" "));
    }
    Some(GroupSegments {
        open,
        parts,
        close,
        splits,
    })
}

/// Push one body `WORD` onto `parts`, cutting it at each *interior* comma when
/// `keyval` licenses it, and return how many separators were emitted. The comma
/// stays on the piece it terminates (`xmin=-5,` / `xmax=5,`), since it belongs to
/// the key before it.
///
/// A comma whose entry holds nothing at all is an empty entry (a doubled `,`), not
/// a key: it never earns a line of its own and rides along on the next piece
/// instead. That emptiness is *not* a property of this word alone — the lexer ends
/// a `WORD` at every control sequence, so a key list routinely hands us a word that
/// opens with the comma closing the previous token's entry
/// (`width=` `\figurewidth` `,xmin=-5,…`). Hence `entry_open`: whether the caller
/// has already emitted content for the entry this word continues. A glued comma
/// uses `Line`, not `SoftLine`: the broken spelling necessarily reparses its
/// newline as a space gap, so using the same flat spelling before and after the
/// break keeps width decisions at a fixed point (issue #121). The caller's
/// whitespace-insensitivity proof (`Keyval` or `TokenList`) is what licenses
/// introducing that otherwise-significant flat space.
fn push_entry_word(
    text: &str,
    split_glued_commas: bool,
    parts: &mut Vec<Ir>,
    entry_open: bool,
) -> usize {
    if !split_glued_commas || !text.contains(',') {
        parts.push(Ir::verbatim(text));
        return 0;
    }
    let mut splits = 0usize;
    let mut start = 0usize;
    let mut pushed = 0usize;
    let mut has_content = entry_open;
    for (i, _) in text.match_indices(',') {
        let piece = &text[start..=i];
        if piece.len() == 1 && !has_content {
            continue; // an empty entry: let the comma ride on the next piece
        }
        if pushed > 0 {
            parts.push(Ir::line());
            splits += 1;
        }
        parts.push(Ir::verbatim(piece));
        pushed += 1;
        start = i + 1;
        has_content = false;
    }
    if start < text.len() {
        if pushed > 0 {
            parts.push(Ir::line());
            splits += 1;
        }
        parts.push(Ir::verbatim(&text[start..]));
    }
    splits
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

fn command_has_math_arg(command: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    command_name(command)
        .and_then(|name| cx.signatures.command(&name))
        .is_some_and(|sig| {
            sig.args
                .iter()
                .any(|spec| spec.domain == ArgumentDomain::Math)
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

/// Whether `command` is a *sectioning* command (`\part` … `\subparagraph`), per the
/// signature DB's [`CommandSig::sectioning`] level. Prose reflow treats such a
/// command as a block-level statement. When the command is a direct child of a
/// prose paragraph, it becomes a paragraph-separated block with one blank line on
/// each side (see [`reflow_elements`]).
///
/// Read from the semantic layer, never from a name list in the formatter (decision
/// #2): sectioning level is exactly the kind of meaning the signature DB owns, and
/// `\section` is only a heading because the DB says so.
fn command_is_sectioning(command: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    command_name(command)
        .and_then(|name| cx.signatures.command(&name))
        .is_some_and(|sig| sig.sectioning.is_some())
}

/// Whether `command` is the canonical label command that may remain attached to a
/// preceding section heading.
fn command_is_label(command: &SyntaxNode) -> bool {
    command_name(command).as_deref() == Some("label")
}

/// Whether `command` is a curated *block-level* command (`\usepackage`,
/// `\newcommand`, `\maketitle`, …), per the signature DB's [`CommandSig::block`]
/// flag. Prose reflow treats such a command like a sectioning one — a block-level
/// statement on its own line, whatever trivia the author wrote — except that a
/// block command **glued** to adjacent non-trivia keeps its authored adjacency
/// (see [`reflow_elements`]): breaking there materializes a space token TeX
/// typesets (`\ProcessOptions\relax`), where a heading's own `\par` makes the
/// materialized glue provably inert.
///
/// Read from the semantic layer rather than a formatter-owned name list. The
/// flag is positive and curated-only, so an un-signatured or
/// scanned-definition command is *not* block here and falls back to the residual
/// authored-break rule in [`line_is_command_only`].
///
/// A command whose signature declares a *required* argument must actually carry
/// an attached argument node. A **bare** head — `\newcommand` in
/// `\newcommand\foo{…}`, where the control-word run break leaves every argument
/// unattached — is a shape the attachment model did not capture, and
/// intercepting it is not pass-stable: glued to a forced-break sibling it is
/// stranded at end-of-line by the ride path (glue the engine itself breaks), so
/// the adjacency this gate reads differs between passes
/// (`pgfcomp-version-0-65.sty`). A bare head falls to the residual rule, whose
/// authored-break preservation *is* the fixed point of that stranding.
fn command_is_block(command: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    command_name(command)
        .and_then(|name| cx.signatures.command(&name))
        .is_some_and(|sig| {
            sig.block
                && (sig.args.iter().all(|arg| !arg.required) || command.children().next().is_some())
        })
}

/// Lower one `STATEMENT` node (a `;`-terminated statement in a curated
/// `statementBody` environment body) under [`WrapMode::Reflow`] — the
/// structural statement lowering, entered from `reflow_elements_checked`'s
/// `STATEMENT` arm.
///
/// The interior reflows under [`ReflowKind::StatementInterior`]: a lone
/// newline is a plain atom boundary the width fill re-decides, the
/// command-only residue is off (a width-owned body must not mint forced
/// breaks), a comment still rides and ends its line, and a forced-break child
/// (a `{label}` holding an environment) commits as its own segment with a
/// glued `;` riding its last line. Gaps additionally consult the TikZ unit
/// model (`semantic::tikz::statement_glue`): a unit-internal gap — an operator
/// and what it connects, `at` and its coordinate, a coordinate and its
/// operation, an operation and its argument — renders as a single space and
/// never breaks, so a width wrap lands only at unit boundaries (idiomatically,
/// before a path operator). The enclosing [`Ir::indent`] then hangs **every**
/// continuation line — width wraps, post-comment lines, and block segments
/// alike — one step under the statement head, so a wrapped `\node[…] at (2,3)`
/// / `{…};` reads as a continuation rather than a sibling.
///
/// Fixed point (Tier 1): the hang is *emitted*, never read. Statement extent
/// re-derives from the terminating `;` on every parse, however the emitted
/// layout breaks — an emitted wrap is leading line trivia to the next parse —
/// and the interior reads only width, gluedness, comment presence, and
/// non-trivia token text (the unit model), all preserved or content-derived.
/// So `fmt(fmt(x)) == fmt(x)` holds by structure, where the
/// flush-continuation contract this replaces had to *forbid* the hang.
fn lower_statement(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    Ir::indent(reflow_elements(
        node.children_with_tokens(),
        cx,
        ReflowKind::StatementInterior,
    ))
}

/// Pre-pass over a paragraph element stream: splice each `STATEMENT` wrapper's
/// children into the stream, restoring the sibling layout a pre-statement parse
/// produced. Taken by every path that lays a statement-body paragraph out as a
/// *line stream* — the non-`Reflow` prose modes and the `Preserve` paragraph arm
/// — so the wrapper changes no bytes there; only the structural `Reflow`
/// lowering ([`lower_statement`]) reads the node itself.
fn flatten_statements(elements: Vec<SyntaxElement>) -> Vec<SyntaxElement> {
    if !elements.iter().any(|e| {
        e.as_node()
            .is_some_and(|n| n.kind() == SyntaxKind::STATEMENT)
    }) {
        return elements;
    }
    let mut out = Vec::with_capacity(elements.len());
    for element in elements {
        match &element {
            SyntaxElement::Node(node) if node.kind() == SyntaxKind::STATEMENT => {
                out.extend(node.children_with_tokens());
            }
            _ => out.push(element),
        }
    }
    out
}

/// Pre-pass over a reflow element stream: replace each *inline* prose command
/// ([`command_is_inline_prose`]) with its surface tokens, splicing its prose
/// argument's body directly into the stream. The body's inter-word whitespace then
/// becomes break opportunities in the surrounding paragraph fill, and the prose
/// `{`/`}` glue onto the adjacent words — so an inline footnote wraps as running
/// text instead of exploding into a block. Non-prose arguments and the control
/// word are kept verbatim; nested inline prose commands are expanded recursively.
/// `glue_matched_args` is enabled only in ordinary prose and prose-argument
/// reflow. A code-like statement or preserve-mode stream keeps its
/// argument-boundary trivia because flattening an inner command must not make an
/// opaque parent group newly flat on the next pass. Virtual `.dtx` margin prose
/// and other margin-carrying streams also keep it: the corpus gate shows that
/// deleting the boundary there is not yet fixed-point stable.
fn flatten_inline_prose(
    elements: Vec<SyntaxElement>,
    cx: LowerCtx<'_>,
    glue_matched_args: bool,
) -> Vec<SyntaxElement> {
    let mut out = Vec::new();
    for element in elements {
        match &element {
            SyntaxElement::Node(node)
                if node.kind() == SyntaxKind::COMMAND && command_is_inline_prose(node, cx) =>
            {
                expand_inline_prose(node, cx, glue_matched_args, &mut out);
            }
            _ => out.push(element),
        }
    }
    out
}

/// Expand one inline prose command into `out` (see [`flatten_inline_prose`]): the
/// control word and any non-prose argument are emitted verbatim, while each prose
/// argument is spliced delimiter-and-body via [`splice_prose_group`]. At
/// prose-reflow altitude, collapsible trivia before a matched argument slot is
/// dropped: TeX's undelimited argument scanner already ignores it, and retaining
/// it would let an authored space versus newline choose the command's layout. A
/// comment remains a barrier because the line ending it consumes cannot be
/// removed. Slot matching mirrors [`lower_command`] so an omitted optional does
/// not misalign positions.
fn expand_inline_prose(
    node: &SyntaxNode,
    cx: LowerCtx<'_>,
    glue_matched_args: bool,
    out: &mut Vec<SyntaxElement>,
) {
    let Some(sig) = command_name(node).and_then(|name| cx.signatures.command(&name)) else {
        out.push(SyntaxElement::Node(node.clone()));
        return;
    };
    let mut slot = 0usize;
    let mut pending_trivia = Vec::new();
    for child in node.children_with_tokens() {
        if is_collapsible_trivia_element(&child) {
            pending_trivia.push(child);
            continue;
        }
        match child {
            SyntaxElement::Node(group)
                if matches!(group.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL) =>
            {
                let is_bracket = group.kind() == SyntaxKind::OPTIONAL;
                let kind = if is_bracket {
                    ArgKind::Bracket
                } else {
                    ArgKind::Brace
                };
                let spec = match_arg_slot(&sig.args, &mut slot, kind);
                let follows_comment = out.last().is_some_and(
                    |element| matches!(element, SyntaxElement::Token(t) if t.kind() == SyntaxKind::COMMENT),
                );
                if spec.is_none() || follows_comment || !glue_matched_args {
                    out.append(&mut pending_trivia);
                } else {
                    pending_trivia.clear();
                }
                let prose = spec.is_some_and(|spec| spec.content == ContentKind::Prose);
                if prose {
                    let (open, close) = if is_bracket {
                        (SyntaxKind::L_BRACKET, SyntaxKind::R_BRACKET)
                    } else {
                        (SyntaxKind::L_BRACE, SyntaxKind::R_BRACE)
                    };
                    splice_prose_group(&group, open, close, cx, glue_matched_args, out);
                } else {
                    out.push(SyntaxElement::Node(group));
                }
            }
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::VERB => {
                out.append(&mut pending_trivia);
                if token.text().starts_with('{') {
                    match_verbatim_arg_slot(&sig.args, &mut slot);
                }
                out.push(SyntaxElement::Token(token));
            }
            other => {
                out.append(&mut pending_trivia);
                out.push(other);
            }
        }
    }
    out.append(&mut pending_trivia);
}

/// Splice a prose group's delimiters and body into `out` (see
/// [`flatten_inline_prose`]). The group's own `open`/`close` tokens are emitted
/// around the body; the body's leading and trailing whitespace is dropped so the
/// delimiters glue tight to the first and last words, and nested inline prose
/// commands inside the body are expanded recursively.
///
/// The delimiter kinds are the node's *own* pair — `{`/`}` for a `GROUP`, `[`/`]`
/// for an `OPTIONAL` — never "any closer". A bracket is ordinary prose content
/// inside a brace group (`\emph{a [b] c}`), and matching it as a delimiter dropped
/// it from the body: the `open` arm is guarded by `open.is_none()`, but a `close`
/// arm matching both kinds is overwritten by the group's real closer, so the `]`
/// vanished from the output entirely — the whitespace-only invariant broken at
/// default settings. Kind-matching is sufficient without also demanding the *last*
/// such token: the formatter only runs on clean parses, where a `GROUP` holds
/// exactly one `R_BRACE` (a second would have closed it) and the parser ends an
/// `OPTIONAL` at its first `]`.
fn splice_prose_group(
    group: &SyntaxNode,
    open_kind: SyntaxKind,
    close_kind: SyntaxKind,
    cx: LowerCtx<'_>,
    glue_matched_args: bool,
    out: &mut Vec<SyntaxElement>,
) {
    let mut open: Option<SyntaxElement> = None;
    let mut close: Option<SyntaxElement> = None;
    let mut body: Vec<SyntaxElement> = Vec::new();
    for element in group.children_with_tokens() {
        match &element {
            SyntaxElement::Token(t) if t.kind() == open_kind && open.is_none() => {
                open = Some(element);
            }
            SyntaxElement::Token(t) if t.kind() == close_kind => {
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
    out.extend(flatten_inline_prose(body, cx, glue_matched_args));
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

/// Split an inline command's token-list argument into paragraph-fill atoms.
/// Everything before the first entry (the command head and any optional arguments)
/// stays on that atom; everything after the final entry (the closing delimiter and
/// any attached suffix) stays on the last. Returns `None` when there is no useful
/// top-level comma split or the body carries a preserved predicate that forbids
/// segmentation.
fn inline_token_list_atoms(node: &SyntaxNode, cx: LowerCtx<'_>) -> Option<Vec<Ir>> {
    let sig = command_name(node).and_then(|name| cx.signatures.command(&name))?;
    let mut slot = 0usize;
    let mut found = false;
    let mut atoms: Vec<Vec<Ir>> = vec![Vec::new()];

    for element in alignment_cell_elements(node.children_with_tokens(), cx) {
        match element {
            SyntaxElement::Node(group)
                if matches!(group.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL) =>
            {
                let is_bracket = group.kind() == SyntaxKind::OPTIONAL;
                let (open_kind, close_kind, kind) = if is_bracket {
                    (
                        SyntaxKind::L_BRACKET,
                        SyntaxKind::R_BRACKET,
                        ArgKind::Bracket,
                    )
                } else {
                    (SyntaxKind::L_BRACE, SyntaxKind::R_BRACE, ArgKind::Brace)
                };
                let spec = match_arg_slot(&sig.args, &mut slot, kind);
                if spec.is_some_and(|spec| spec.content == ContentKind::TokenList) {
                    let GroupSegments {
                        open,
                        mut parts,
                        close,
                        splits,
                    } = segment_delimited_body(&group, open_kind, close_kind, cx, true)?;
                    if splits == 0 {
                        return None;
                    }
                    let _ = peel_padding(&mut parts, Edge::Leading);
                    let _ = peel_padding(&mut parts, Edge::Trailing);
                    atoms.last_mut().unwrap().push(open);
                    for part in parts {
                        if matches!(part, Ir::Line | Ir::SoftLine) {
                            atoms.push(Vec::new());
                        } else {
                            atoms.last_mut().unwrap().push(part);
                        }
                    }
                    atoms.last_mut().unwrap().push(close);
                    found = true;
                } else {
                    atoms.last_mut().unwrap().push(lower_node(&group, cx));
                }
            }
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::VERB => {
                if token.text().starts_with('{') {
                    match_verbatim_arg_slot(&sig.args, &mut slot);
                }
                atoms
                    .last_mut()
                    .unwrap()
                    .push(lower_loose_token(&token, cx));
            }
            SyntaxElement::Node(child) => atoms.last_mut().unwrap().push(lower_node(&child, cx)),
            SyntaxElement::Token(token) => {
                atoms
                    .last_mut()
                    .unwrap()
                    .push(lower_loose_token(&token, cx));
            }
        }
    }

    found.then(|| atoms.into_iter().map(Ir::concat).collect())
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
    lower_command_with_math_spacing(node, cx, MathSpacing::Normal)
}

fn lower_command_with_math_spacing(
    node: &SyntaxNode,
    cx: LowerCtx<'_>,
    math_spacing: MathSpacing,
) -> Ir {
    let Some(sig) = command_name(node).and_then(|name| cx.signatures.command(&name)) else {
        // Defensive: the guard already proved a prose signature exists.
        return Ir::concat(lower_element_stream(node.children_with_tokens(), cx));
    };
    let math_only = sig
        .args
        .iter()
        .any(|spec| spec.domain == ArgumentDomain::Math);

    let mut out: Vec<Ir> = Vec::new();
    let mut slot = 0usize;
    let mut iter = alignment_cell_elements(node.children_with_tokens(), cx)
        .into_iter()
        .peekable();
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
                let kind = if is_bracket {
                    ArgKind::Bracket
                } else {
                    ArgKind::Brace
                };
                let spec = match_arg_slot(&sig.args, &mut slot, kind);
                if math_only {
                    if spec.is_some_and(|spec| spec.domain == ArgumentDomain::Math) {
                        out.push(lower_math_argument_group(&child, cx, math_spacing));
                    } else {
                        out.push(Ir::verbatim(child.text().to_string()));
                    }
                    continue;
                }
                match spec.map(|s| s.content) {
                    Some(ContentKind::Prose) => {
                        out.push(lower_prose_group(&child, open, close, cx));
                    }
                    Some(ContentKind::TokenList) => {
                        // Outside a paragraph fill, keep a token list as one inline
                        // atom. The paragraph path exposes its top-level entries as
                        // fill atoms before reaching this lowering.
                        out.push(
                            collapse_arg_group(&child, open, close, cx)
                                .unwrap_or_else(|| lower_node(&child, cx)),
                        );
                    }
                    // A proven `key=value` list: its processor strips spaces around
                    // entries, so the layout may also break at a comma the author
                    // glued (see [`ContentKind::Keyval`]). A `{…}` reaches this only
                    // through the curated tier — the setters (`\pgfkeys`, `\tikzset`)
                    // whose whole mandatory argument is the key list.
                    Some(ContentKind::Keyval) => {
                        out.push(
                            lower_segmented_group(&child, open, close, cx, true)
                                .unwrap_or_else(|| lower_node(&child, cx)),
                        );
                    }
                    _ => out.push(lower_node(&child, cx)),
                }
            }
            SyntaxElement::Node(child) if math_only => {
                out.push(Ir::verbatim(child.text().to_string()))
            }
            SyntaxElement::Node(child) => out.push(lower_node(&child, cx)),
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::VERB => {
                if token.text().starts_with('{') {
                    match_verbatim_arg_slot(&sig.args, &mut slot);
                }
                out.push(lower_loose_token(&token, cx));
            }
            SyntaxElement::Token(token) if math_only => out.push(Ir::verbatim(token.text())),
            SyntaxElement::Token(token) if is_collapsible_trivia(token.kind()) => {
                out.push(classify_trivia(
                    consume_gap_widened(&token, &mut iter),
                    cx.in_alignment_cell,
                ));
            }
            SyntaxElement::Token(token) => out.push(lower_loose_token(&token, cx)),
        }
    }
    Ir::concat(out)
}

/// Lower a prose argument group: like [`lower_bracketed`], but the body is reflowed
/// to the line width ([`reflow_elements`]) and the whole thing is wrapped in a soft
/// [`Ir::group`] so it stays on one line when it fits (`\footnote{short}`) and
/// breaks the delimiters onto their own lines, indenting and word-wrapping the body,
/// when it does not. Empty bodies collapse to the bare delimiters.
///
/// A `%` comment at either edge of the body takes [`lower_bracketed`]'s two
/// guards, for the same reasons — the soft group is the only thing that makes
/// them look different here:
///
/// - A comment **glued to the open delimiter** rides the opener's line. Pushing
///   it to its own indented line would turn the newline the formatter writes
///   after `{` into a real space token inside the group, changing `\caption{%\n}`
///   (empty — the `%` eats the source newline) into `\caption{ }`.
/// - A comment the body **ends** with forces the group open, so the close
///   delimiter takes its own line. Flat, the group renders `\caption{x%}` and the
///   `%` comments the closing brace out — a content deletion, and one the
///   whitespace-only oracle sees only as a comment growing a `}`.
///
/// Both bite exactly when the whole body reflows to a *single* line: any second
/// line puts a hard separator between them, which already forces the group.
fn lower_prose_group(
    node: &SyntaxNode,
    open: SyntaxKind,
    close: SyntaxKind,
    cx: LowerCtx<'_>,
) -> Ir {
    let mut open_ir = Ir::Nil;
    let mut close_ir = Ir::Nil;
    let mut body_elements: Vec<SyntaxElement> = Vec::new();
    for element in alignment_cell_elements(node.children_with_tokens(), cx) {
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

    // The parser emits leading whitespace/newlines as their own trivia tokens, so
    // the first body element is a `COMMENT` iff it was glued to the opener.
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
    let has_trailing_comment = body_ends_with_comment(node, close);

    let body = reflow_elements(body_elements.into_iter(), cx, ReflowKind::ProseArg);
    if matches!(body, Ir::Nil) {
        if has_leading_comment {
            // `\caption{%\n}`: the comment already rode the open delimiter, so
            // the close must still drop to its own line.
            Ir::concat([open_ir, Ir::hard_line(), close_ir])
        } else {
            Ir::concat([open_ir, close_ir])
        }
    } else {
        let brk: fn() -> Ir = if has_leading_comment || has_trailing_comment {
            Ir::hard_line
        } else {
            Ir::soft_line
        };
        Ir::group(Ir::concat([
            open_ir,
            Ir::indent(Ir::concat([brk(), body])),
            brk(),
            close_ir,
        ]))
    }
}

/// Whether the last content token of `node`'s body — the `close` delimiter and
/// any trailing collapsible trivia skipped — is a `%` comment, i.e. whatever the
/// body lowers to ends a line and nothing may follow it there.
///
/// Read at any depth, because a comment nested in the last child is still the
/// last thing emitted *unless* that child's own lowering already put a break
/// after it, which is exactly what the delimiter-bearing lowerings do
/// (`\caption{\emph{a%\n}}` ends on `\emph`'s `}`, not on the comment).
fn body_ends_with_comment(node: &SyntaxNode, close: SyntaxKind) -> bool {
    let mut token = node.last_token();
    while let Some(t) = token {
        if !node.text_range().contains_range(t.text_range()) {
            return false; // walked out of the group (an unclosed body)
        }
        match t.kind() {
            SyntaxKind::COMMENT => return true,
            k if k == close || is_collapsible_trivia(k) => token = t.prev_token(),
            _ => return false,
        }
    }
    false
}

/// Collapse a signature-marked [`ContentKind::TokenList`] to a single inline atom.
/// This is the fallback outside paragraph reflow and for lists that cannot expose
/// safe comma boundaries there. Interior newlines collapse to spaces, so a citation
/// list written across lines (`\citep{\n  a,\n  b\n}`) formats identically to its
/// one-line form (`\citep{a, b}`).
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
                let gap = consume_gap(&t, &mut iter);
                if gap == Gap::Blank {
                    return None; // a blank-line `\par`: keep the block form
                }
                // The gap's flat spelling: a lone newline collapses to a single
                // space, pure inline whitespace stays verbatim, matching the
                // one-line generic lowering.
                body.push(Ir::verbatim(gap.flat()));
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

/// Present a specialized lowerer with the virtual document's LaTeX stream rather
/// than the physical `.dtx` framing stored in the lossless CST. A documentation
/// margin and its following padding belong to the region wrapper; every other
/// element, including the preceding newline, remains available to the layout.
fn strip_virtual_dtx_framing(
    elements: impl IntoIterator<Item = SyntaxElement>,
    cx: LowerCtx<'_>,
) -> Vec<SyntaxElement> {
    if !cx.in_dtx_doc_region {
        return elements.into_iter().collect();
    }

    let mut stripped = Vec::new();
    let mut after_margin = false;
    for element in elements {
        match &element {
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::DOC_MARGIN => {
                after_margin = true;
            }
            SyntaxElement::Token(token)
                if after_margin && token.kind() == SyntaxKind::WHITESPACE => {}
            _ => {
                after_margin = false;
                stripped.push(element);
            }
        }
    }
    stripped
}

/// Present a nested non-math alignment-cell lowerer with the same virtual stream
/// as the grid itself. Outside that narrow context, retain the node's ordinary
/// stream so enabling virtual grids does not alter unrelated document lowering.
fn alignment_cell_elements(
    elements: impl IntoIterator<Item = SyntaxElement>,
    cx: LowerCtx<'_>,
) -> Vec<SyntaxElement> {
    if cx.in_alignment_cell {
        strip_virtual_dtx_framing(elements, cx)
    } else {
        elements.into_iter().collect()
    }
}

/// Lower inline `$…$`/`\(…\)` or display `$$…$$`/`\[…\]` math. The delimiter
/// tokens are direct children of the math node and are emitted verbatim; the
/// `MATH` child (the body) is formatted by [`lower_math_body`].
fn lower_math(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    Ir::concat(
        strip_virtual_dtx_framing(node.children_with_tokens(), cx)
            .into_iter()
            .map(|el| match el {
                SyntaxElement::Node(n) if n.kind() == SyntaxKind::MATH => lower_math_body(&n, cx),
                SyntaxElement::Node(n) => lower_node(&n, cx),
                SyntaxElement::Token(t) => Ir::verbatim(t.text()),
            }),
    )
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
    let mut open_has_comment = false;
    for element in strip_virtual_dtx_framing(node.children_with_tokens(), cx) {
        match element {
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::MATH => {
                // A `%` trailing the opening delimiter on the same source line
                // rides that line, exactly as an environment's `\begin`-line
                // comment does ([`split_environment`]), and is dropped from the
                // body by identity.
                let lifted = leading_inline_comment(&[SyntaxElement::Node(n.clone())]);
                if let Some(comment) = &lifted {
                    open.push_str(comment.text());
                    open_has_comment = true;
                }
                let elements: Vec<SyntaxElement> = n
                    .children_with_tokens()
                    .filter(|e| !is_lifted_comment(e, lifted.as_ref()))
                    .collect();
                let body_cx = cx.absorbing_trailing_control_newline(&elements);
                body_empty = elements.iter().all(|e| {
                    e.as_token()
                        .is_some_and(|t| is_collapsible_trivia(t.kind()))
                });
                body = trim_trailing_break(lower_display_formula_elements(&elements, body_cx));
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
        if open_has_comment {
            // The lifted comment runs to end of line, so the closing delimiter
            // must not collapse onto it (it would be commented out).
            Ir::concat([Ir::verbatim(open), Ir::hard_line(), Ir::verbatim(close)])
        } else {
            Ir::concat([Ir::verbatim(open), Ir::verbatim(close)])
        }
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
    lower_math_seq(node.children_with_tokens(), cx, MathSpacing::Normal, false)
}

/// Lower a single-formula display-math body per the resolved [`MathWrap`]
/// policy (`LowerCtx::math_wrap`): `Break` routes through the amsmath-style
/// breaker, `SingleLine` through the plain collapsing body (overflowing if too
/// long, like inline math), and `Preserve` keeps authored newlines as hard
/// breaks. `Auto` is resolved away in [`format_root`] and cannot reach here;
/// map it to the breaker defensively rather than panic. Takes the `MATH` node's
/// elements rather than the node itself so a caller can drop the lifted
/// opener-line comment ([`split_environment`]) before the lowering.
fn lower_display_formula_elements(elements: &[SyntaxElement], cx: LowerCtx<'_>) -> Ir {
    // A leading `\label{…}` is equation bookkeeping, not part of the formula: give
    // it its own line so the math starts fresh below it, under every wrap policy.
    // Grids (`align`, `\\`) never reach here. The split recurses so the remaining
    // formula lowers under its normal `MathWrap` policy (and a `\label\label` run
    // peels one label per level).
    if let Some((label, rest)) = split_leading_label(elements) {
        return Ir::concat([
            lower_math_element(label, cx, MathSpacing::Normal),
            Ir::hard_line(),
            lower_display_formula_elements(rest, cx),
        ]);
    }
    match cx.math_wrap {
        MathWrap::Auto | MathWrap::Break => lower_display_math_body(elements, cx),
        MathWrap::SingleLine => {
            lower_math_seq(elements.iter().cloned(), cx, MathSpacing::Normal, false)
        }
        MathWrap::Preserve => {
            lower_math_seq(elements.iter().cloned(), cx, MathSpacing::Normal, true)
        }
    }
}

/// Split a display-math body whose first non-trivia atom is a `\label{…}` into
/// that label and the remaining formula elements. Returns `None` when the body
/// does not lead with a label, or when nothing but trivia follows it (a body that
/// is only a label stays on one line rather than gaining a dangling break). Scoped
/// to the single `\label` command by name — a trailing label, or any other
/// bookkeeping command, is deliberately left in place (see the formatter book).
fn split_leading_label(elements: &[SyntaxElement]) -> Option<(SyntaxElement, &[SyntaxElement])> {
    let idx = elements
        .iter()
        .position(|e| !is_collapsible_trivia_element(e))?;
    let node = elements[idx].as_node()?;
    if node.kind() != SyntaxKind::COMMAND
        || crate::ast::command_name(node).as_deref() != Some("label")
    {
        return None;
    }
    let rest = &elements[idx + 1..];
    if rest.iter().all(is_collapsible_trivia_element) {
        return None;
    }
    Some((elements[idx].clone(), rest))
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

/// Source-spacing policy for a math list. Script arguments use TeX's compact
/// convention, while ordinary math lists receive explicit binary/relation padding.
#[derive(Clone, Copy, PartialEq)]
enum MathSpacing {
    Normal,
    Script,
}

/// One top-level atom of a display-math body, paired with its [`MathRole`].
struct MathPiece {
    ir: Ir,
    role: MathRole,
    colon_relation_prefix: bool,
    spaced_slash: bool,
    slash: bool,
    /// Whether authored whitespace preceded this atom. Drives operand-operand
    /// spacing exactly as [`lower_math_seq`] does, so a tight command boundary
    /// (`\gamma)`, `}.`) stays tight rather than gaining a spurious space.
    space_before: bool,
    /// Net change in bracket nesting (`(`/`[`/`\{`/named delimiters vs their
    /// closers) contributed by this atom's own text. Used to suppress operator
    /// breaks inside a bracketed subexpression (`(1 - \gamma)` must not break at
    /// `-`, and a relation inside a set-builder `\{ … \}` is not an anchor).
    bracket_delta: i32,
}

struct MathSurfaceAtom {
    ir: Ir,
    class: MathClass,
    delimiter: Option<DelimiterRole>,
    colon_relation_prefix: bool,
    starts_equals_relation: bool,
    spaced_slash: bool,
    slash: bool,
    control_word_operator: bool,
    starts_control_word_letter: bool,
    ends_control_word: bool,
}

/// Lower one CST element into the semantic atoms that its source surface
/// contains. Structural nodes stay indivisible; a coalesced `WORD` is sliced at
/// Unicode scalar boundaries. Consecutive relation scalars and a colon run
/// followed by `=` remain one surface atom, so authored compound spellings such
/// as `<=`, `:=`, and `::=` are not separated.
fn lower_math_atoms(
    el: SyntaxElement,
    cx: LowerCtx<'_>,
    spacing: MathSpacing,
) -> Vec<MathSurfaceAtom> {
    let atoms: Vec<_> = math_atoms(&el).collect();
    let SyntaxElement::Token(token) = &el else {
        let starts_control_word_letter = element_starts_control_word_letter(&el);
        let ends_control_word = element_ends_control_word(&el);
        let starts_with_equals = element_starts_with_token_text(&el, "=");
        let atom = atoms
            .into_iter()
            .next()
            .expect("a structural math element has one semantic atom");
        let control_word_operator =
            ends_control_word && matches!(atom.class, MathClass::Bin | MathClass::Rel);
        return vec![MathSurfaceAtom {
            ir: lower_math_element(el, cx, spacing),
            class: atom.class,
            delimiter: atom.delimiter,
            colon_relation_prefix: false,
            starts_equals_relation: atom.class == MathClass::Rel && starts_with_equals,
            spaced_slash: false,
            slash: false,
            control_word_operator,
            starts_control_word_letter,
            ends_control_word,
        }];
    };
    if token.kind() != SyntaxKind::WORD {
        let starts_control_word_letter = token
            .text()
            .chars()
            .next()
            .is_some_and(is_control_word_letter);
        let ends_control_word = token.kind() == SyntaxKind::CONTROL_WORD;
        let atom = atoms
            .into_iter()
            .next()
            .expect("a math token has one semantic atom");
        let control_word_operator =
            ends_control_word && matches!(atom.class, MathClass::Bin | MathClass::Rel);
        return vec![MathSurfaceAtom {
            ir: lower_math_element(el, cx, spacing),
            class: atom.class,
            delimiter: atom.delimiter,
            colon_relation_prefix: false,
            starts_equals_relation: false,
            spaced_slash: false,
            slash: false,
            control_word_operator,
            starts_control_word_letter,
            ends_control_word,
        }];
    }

    let token_start = token.text_range().start();
    let mut surface: Vec<MathSurfaceAtom> = Vec::with_capacity(atoms.len());
    for atom in atoms {
        let start = usize::from(atom.range.start() - token_start);
        let end = usize::from(atom.range.end() - token_start);
        let text = &token.text()[start..end];
        if let Some(previous) = surface.last_mut() {
            let extends_colon_prefix =
                previous.colon_relation_prefix && atom.class == MathClass::Punct && text == ":";
            let completes_colon_relation =
                previous.colon_relation_prefix && atom.class == MathClass::Rel && text == "=";
            if previous.class == MathClass::Rel && atom.class == MathClass::Rel
                || extends_colon_prefix
                || completes_colon_relation
            {
                previous.ir = Ir::concat([previous.ir.clone(), Ir::verbatim(text)]);
                if completes_colon_relation {
                    previous.class = MathClass::Rel;
                    previous.colon_relation_prefix = false;
                }
                continue;
            }
        }
        surface.push(MathSurfaceAtom {
            ir: Ir::verbatim(text),
            class: atom.class,
            delimiter: atom.delimiter,
            colon_relation_prefix: atom.class == MathClass::Punct && text == ":",
            starts_equals_relation: false,
            spaced_slash: atom.class == MathClass::Ord
                && text == "/"
                && (start == 0
                    && token
                        .prev_token()
                        .is_some_and(|token| is_collapsible_trivia(token.kind()))
                    || end == token.text().len()
                        && token
                            .next_token()
                            .is_some_and(|token| is_collapsible_trivia(token.kind()))),
            slash: atom.class == MathClass::Ord && text == "/",
            control_word_operator: false,
            starts_control_word_letter: text.chars().next().is_some_and(is_control_word_letter),
            ends_control_word: false,
        });
    }
    // Anticipate the gap the sequencer will add before a following operator;
    // otherwise that gap makes only the next pass recognize the slash as spaced.
    for index in 0..surface.len() {
        if !surface[index].slash || surface[index].spaced_slash {
            continue;
        }
        let next_operator = surface.get(index + 1).is_some_and(|next| {
            spacing == MathSpacing::Normal && matches!(next.class, MathClass::Bin | MathClass::Rel)
        }) || index + 1 == surface.len()
            && token.next_token().is_some_and(|next| {
                let class = math_atoms(&SyntaxElement::Token(next.clone()))
                    .next()
                    .map(|atom| atom.class);
                class.is_some_and(|class| {
                    matches!(class, MathClass::Bin | MathClass::Rel)
                        && (spacing == MathSpacing::Normal
                            || next.kind() == SyntaxKind::CONTROL_WORD)
                })
            });
        surface[index].spaced_slash = next_operator;
    }
    surface
}

fn is_control_word_letter(character: char) -> bool {
    character.is_alphabetic()
}

fn element_starts_control_word_letter(element: &SyntaxElement) -> bool {
    let first = match element {
        SyntaxElement::Node(node) => node
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .find(|token| !is_collapsible_trivia(token.kind())),
        SyntaxElement::Token(token) => Some(token.clone()),
    };
    first
        .as_ref()
        .and_then(|token| token.text().chars().next())
        .is_some_and(is_control_word_letter)
}

fn element_starts_with_token_text(element: &SyntaxElement, expected: &str) -> bool {
    match element {
        SyntaxElement::Node(node) => node
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .find(|token| !is_collapsible_trivia(token.kind())),
        SyntaxElement::Token(token) => Some(token.clone()),
    }
    .is_some_and(|token| token.text() == expected)
}

fn element_ends_control_word(element: &SyntaxElement) -> bool {
    let last = match element {
        SyntaxElement::Node(node) => node
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .filter(|token| !is_collapsible_trivia(token.kind()))
            .last(),
        SyntaxElement::Token(token) => Some(token.clone()),
    };
    last.is_some_and(|token| token.kind() == SyntaxKind::CONTROL_WORD)
}

/// The [`MathRole`] of a top-level math atom. `prev_class` is the TeX class of the
/// preceding atom and `prev_opener` whether it ended with an opening delimiter: a
/// `+`/`-` (or any binary operator) with no operand to its left — either the first
/// atom, one after a binary, large operator, relation, opener, or punctuation —
/// is unary, so it glues to its operand and is *not* a break point, degrading to
/// an [`MathRole::Operand`]. The full class is needed here because [`MathRole`]
/// deliberately collapses operators and punctuation into `Operand`.
fn math_atom_role(class: MathClass, prev_class: MathClass, prev_opener: bool) -> MathRole {
    let raw = match class {
        MathClass::Bin => MathRole::Binary,
        MathClass::Rel => MathRole::Relation,
        _ => MathRole::Operand,
    };
    if raw == MathRole::Binary
        && (matches!(
            prev_class,
            MathClass::Op | MathClass::Bin | MathClass::Rel | MathClass::Punct
        ) || prev_opener)
    {
        MathRole::Operand
    } else {
        raw
    }
}

/// Collect the top-level atoms of a display-math `MATH` body as [`MathPiece`]s,
/// collapsing trivia runs exactly as [`lower_math_seq`] does. Returns `None` —
/// signalling the caller to take the plain non-breaking path — when the body
/// holds a comment or explicit line break (either forces its own break, which
/// does not compose with the operator-break layout), or has fewer than two atoms
/// (nothing to break).
fn collect_math_pieces(elements: &[SyntaxElement], cx: LowerCtx<'_>) -> Option<Vec<MathPiece>> {
    let mut pieces: Vec<MathPiece> = Vec::new();
    // Start as a non-operand so a leading `+`/`-` (no left operand) reads as unary
    // and glues to its operand rather than becoming a break point — e.g. `-x`.
    let mut prev_class = MathClass::Rel;
    let mut prev_opener = false;
    let mut pending_space = false;
    let mut iter = strip_virtual_dtx_framing(elements.iter().cloned(), cx)
        .into_iter()
        .peekable();
    while let Some(el) = iter.next() {
        match el {
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {
                consume_gap(&t, &mut iter);
                if !pieces.is_empty() {
                    pending_space = true;
                }
            }
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::COMMENT => return None,
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::LINE_BREAK => return None,
            other => {
                for atom in lower_math_atoms(other, cx, MathSpacing::Normal) {
                    let role = math_atom_role(atom.class, prev_class, prev_opener);
                    let completes_colon_relation = !pending_space
                        && pieces
                            .last()
                            .is_some_and(|piece| piece.colon_relation_prefix)
                        && atom.starts_equals_relation;
                    if completes_colon_relation {
                        let previous = pieces.last_mut().expect("checked above");
                        previous.ir = Ir::concat([previous.ir.clone(), atom.ir]);
                        previous.role = MathRole::Relation;
                        previous.colon_relation_prefix = false;
                        previous.bracket_delta += match atom.delimiter {
                            Some(DelimiterRole::Open) => 1,
                            Some(DelimiterRole::Close) => -1,
                            Some(DelimiterRole::Fence) | None => 0,
                        };
                        prev_class = MathClass::Rel;
                        prev_opener = atom.delimiter == Some(DelimiterRole::Open);
                        continue;
                    }
                    prev_class = atom.class;
                    prev_opener = atom.delimiter == Some(DelimiterRole::Open);
                    let bracket_delta = match atom.delimiter {
                        Some(DelimiterRole::Open) => 1,
                        Some(DelimiterRole::Close) => -1,
                        Some(DelimiterRole::Fence) | None => 0,
                    };
                    pieces.push(MathPiece {
                        ir: atom.ir,
                        role,
                        colon_relation_prefix: atom.colon_relation_prefix,
                        spaced_slash: atom.spaced_slash,
                        slash: atom.slash,
                        space_before: pending_space,
                        bracket_delta,
                    });
                    pending_space = false;
                }
            }
        }
    }
    // The display breaker computes every effective role up front, so it can make
    // operator-created slash gaps symmetric before building either layout.
    for index in 0..pieces.len() {
        if pieces[index].slash
            && (index > 0 && pieces[index - 1].role != MathRole::Operand
                || pieces
                    .get(index + 1)
                    .is_some_and(|next| next.role != MathRole::Operand))
        {
            pieces[index].spaced_slash = true;
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
/// otherwise. Each segment's right-hand side is its own nested group: breaking
/// the body at its relations does not also break a segment at its binary
/// operators unless that segment overflows its own line. If the LHS-derived
/// relation column would make a continuation overflow, the printer breaks before
/// the first relation and hangs the relation stack at the display body's base
/// indent instead.
fn lower_display_math_body(elements: &[SyntaxElement], cx: LowerCtx<'_>) -> Ir {
    let Some(pieces) = collect_math_pieces(elements, cx) else {
        return lower_math_seq(elements.iter().cloned(), cx, MathSpacing::Normal, false);
    };

    let flat_width = |ir: &Ir| {
        Printer::new(FormatStyle::default())
            .print_flat(ir)
            .chars()
            .count()
    };

    // Bracket depth entering each atom (running sum of the preceding atoms'
    // deltas). A top-level break/relation is one seen at depth 0; operators
    // inside a parenthesized subexpression are structurally interior.
    let depth_before: Vec<i32> = {
        let mut acc = 0;
        let mut v = Vec::with_capacity(pieces.len());
        for p in &pieces {
            v.push(acc);
            acc += p.bracket_delta;
        }
        v
    };

    // Non-breaking separator between atoms `k-1` and `k`, mirroring
    // [`lower_math_seq`]: a space around any operator (either side) or across an
    // authored gap, nothing between two operands authored tight.
    let space_sep = |k: usize| -> Ir {
        if pieces[k].spaced_slash || pieces[k - 1].spaced_slash {
            Ir::verbatim(" ")
        } else if pieces[k].role != MathRole::Operand
            || pieces[k - 1].role != MathRole::Operand
            || pieces[k].space_before
        {
            Ir::text(" ")
        } else {
            Ir::Nil
        }
    };
    // Whether a break may be inserted before atom `k`: a top-level binary
    // operator with an operand to its left (a genuine infix `+`, not a unary
    // sign, and not one nested in parentheses).
    let breakable = |k: usize| -> bool {
        pieces[k].role == MathRole::Binary
            && pieces[k - 1].role == MathRole::Operand
            && depth_before[k] == 0
    };
    // The first top-level relation (a relation nested in parentheses does not
    // anchor the alignment or split a segment).
    let is_anchor = |k: usize| pieces[k].role == MathRole::Relation && depth_before[k] == 0;

    // With no top-level relation, continuation lines hang at the base indent: the
    // body simply breaks before each top-level binary operator.
    let Some(anchor) = (0..pieces.len()).find(|&k| is_anchor(k)) else {
        let mut parts: Vec<Ir> = Vec::with_capacity(pieces.len() * 2);
        for (i, piece) in pieces.iter().enumerate() {
            if i > 0 {
                parts.push(if breakable(i) {
                    Ir::line()
                } else {
                    space_sep(i)
                });
            }
            parts.push(piece.ir.clone());
        }
        return Ir::group(Ir::concat(parts));
    };

    let mut lhs: Vec<Ir> = Vec::new();
    // Left-hand side, flat on the opening line.
    for (i, piece) in pieces[..anchor].iter().enumerate() {
        if i > 0 {
            lhs.push(space_sep(i));
        }
        lhs.push(piece.ir.clone());
    }
    // A multi-line left-hand side (a nested matrix/`aligned`/`cases` environment)
    // has no meaningful flat width — using it as the relation column would push
    // every hanging line dozens of columns right. Anchor the relations at the
    // base indent instead, and break before the first relation so the segment's
    // hanging indent still corresponds to real columns.
    let lhs_multiline = pieces[..anchor]
        .iter()
        .any(|p| p.ir.contains_forced_break());
    // The relation column: the left-hand side sits flat on the opening line, and
    // the first relation follows one space later. Every top-level relation aligns
    // here.
    let rel_col = if anchor == 0 || lhs_multiline {
        0
    } else {
        flat_width(&Ir::concat(lhs.clone())) + 1
    };

    let build_relation_layout = |relation_indent: usize, break_before_first: bool| {
        let mut parts = lhs.clone();
        // Each relation opens a segment running to the next relation. In the
        // aligned layout, the first relation stays beside the LHS and later
        // relations return to `relation_indent`. The base-indent fallback also
        // breaks before the first relation, so an over-deep LHS cannot dictate
        // the width of every continuation line.
        let mut i = anchor;
        let mut first_segment = true;
        while i < pieces.len() {
            if first_segment {
                if lhs_multiline {
                    parts.push(Ir::hard_line());
                } else if break_before_first {
                    parts.push(Ir::line());
                } else if anchor > 0 {
                    parts.push(space_sep(anchor));
                }
            } else {
                parts.push(Ir::line());
            }
            parts.push(pieces[i].ir.clone());
            let relw = flat_width(&pieces[i].ir);

            let start = i + 1;
            let mut j = start;
            while j < pieces.len() && !is_anchor(j) {
                j += 1;
            }
            let mut rhs: Vec<Ir> = Vec::with_capacity((j - start) * 2);
            for (offset, piece) in pieces[start..j].iter().enumerate() {
                let k = start + offset;
                rhs.push(if breakable(k) {
                    Ir::line()
                } else {
                    space_sep(k)
                });
                rhs.push(piece.ir.clone());
            }
            parts.push(Ir::group(Ir::align(relw + 1, Ir::concat(rhs))));

            first_segment = false;
            i = j;
        }

        Ir::align(relation_indent, Ir::concat(parts))
    };

    let body = if rel_col == 0 {
        build_relation_layout(0, lhs_multiline)
    } else {
        Ir::bounded_align(
            build_relation_layout(rel_col, false),
            build_relation_layout(0, true),
        )
    };
    Ir::group(body)
}

/// The shared math-atom sequencer (see [`lower_math_body`]). Ordinary math spacing
/// is *role-aware*: a single space is placed around every binary/relation operator
/// (`a+b` → `a + b`, `x=-b` → `x = -b`), reusing [`math_atom_role`]'s unary
/// detection so a `+`/`-` with no left operand stays glued to its operand
/// (`-x`, `2^{-5}`). Script-size lists suppress padding around punctuation
/// operators recursively, but retain spaces around control-word operators such as
/// `\in`; function application remains tight to its opener (`\Gamma(x)`). Both
/// modes preserve a fully glued slash but symmetrize a gap on either side, and
/// retain any separator required to avoid merging a control word with a following
/// letter. Plain operand juxtaposition keeps its authored spacing (a gap collapses
/// to one space, no gap stays tight). A `%`
/// comment forces a hard line break, and an authored own-line comment remains
/// own-line under every wrap mode so its association cannot change. A trailing
/// break (a comment at the body's end) is emitted rather than trimmed so the
/// caller's closing delimiter lands on its own line, while a trailing space is
/// dropped.
///
/// With `preserve_newlines` ([`MathWrap::Preserve`]) a trivia run spanning at
/// least one newline becomes a hard break instead of a space — the author's
/// line structure survives while in-line spacing is still normalized. A blank
/// run (≥2 newlines, invalid inside math anyway) also collapses to a single
/// break, and an edge run is still trimmed (the caller's delimiters own their
/// lines).
fn lower_math_seq(
    elements: impl Iterator<Item = SyntaxElement>,
    cx: LowerCtx<'_>,
    spacing: MathSpacing,
    preserve_newlines: bool,
) -> Ir {
    let mut out: Vec<Ir> = Vec::new();
    let mut started = false;
    // Start as a non-operand so a leading `+`/`-` reads as unary (see
    // [`collect_math_pieces`]).
    let mut prev_role = MathRole::Relation;
    let mut prev_class = MathClass::Rel;
    let mut prev_opener = false; // the previous atom ended with an opening delimiter
    let mut prev_delimiter_edge = false;
    let mut prev_spaced_slash = false;
    let mut prev_control_word_operator = false;
    let mut prev_ends_control_word = false;
    let mut prev_colon_relation_prefix = false;
    let mut prev_colon_needs_left_space = false;
    let mut prev_atom_ir_index = 0;
    let mut pending_space = false; // authored whitespace since the last atom
    let mut pending_break = false; // a comment forced a hard line break
    let mut pending_newline = false; // a preserved authored line break
    let mut pending_comment_own_line = false; // the next comment must retain its association
    let mut iter = strip_virtual_dtx_framing(elements, cx)
        .into_iter()
        .peekable();
    while let Some(el) = iter.next() {
        match el {
            // Tier 2 under `preserve_newlines` ([`MathWrap::Preserve`]) only: that
            // mode's contract is the author's line structure, and reproducing a
            // break as a break is preservation-only, hence its own fixed point.
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {
                let gap = consume_gap_widened(&t, &mut iter);
                if started {
                    pending_space = true;
                    pending_newline = preserve_newlines && gap.newlines > 0;
                    pending_comment_own_line = gap.newlines > 0;
                }
            }
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::COMMENT => {
                if pending_break || pending_newline || pending_comment_own_line {
                    out.push(Ir::hard_line());
                } else if pending_space {
                    out.push(Ir::verbatim(" "));
                }
                out.push(Ir::verbatim(t.text()));
                started = true;
                pending_space = false;
                pending_newline = false;
                pending_comment_own_line = false;
                pending_break = true;
            }
            other => {
                // A top-level `\\` (a `LINE_BREAK` node) ends its line: emit it, then
                // force a hard break before the next atom. This is how a row stack
                // (`\[ a \\ b \]`, or an aligned body that fell back off the grid)
                // keeps each row on its own line. A `&` in a cell never reaches here,
                // so cells are unaffected.
                let is_line_break = matches!(
                    &other,
                    SyntaxElement::Node(n) if n.kind() == SyntaxKind::LINE_BREAK
                );
                for atom in lower_math_atoms(other, cx, spacing) {
                    let completes_colon_relation = prev_colon_relation_prefix
                        && atom.starts_equals_relation
                        && !pending_space
                        && !pending_break
                        && !pending_newline;
                    if completes_colon_relation && prev_colon_needs_left_space {
                        out[prev_atom_ir_index] =
                            Ir::concat([Ir::verbatim(" "), out[prev_atom_ir_index].clone()]);
                    }
                    let role = math_atom_role(atom.class, prev_class, prev_opener);
                    let spaced_slash = atom.spaced_slash
                        || atom.slash
                            && (spacing == MathSpacing::Normal && prev_role != MathRole::Operand
                                || spacing == MathSpacing::Script && prev_control_word_operator);
                    let touches_spaced_slash = prev_spaced_slash || spaced_slash;
                    let delimiter_edge = matches!(
                        atom.delimiter,
                        Some(DelimiterRole::Open | DelimiterRole::Close)
                    );
                    let touches_delimiter = prev_delimiter_edge || delimiter_edge;
                    let spaced_operands = role == MathRole::Operand
                        && prev_role == MathRole::Operand
                        && pending_space
                        && !touches_delimiter;
                    let script_operator_spacing =
                        atom.control_word_operator || prev_control_word_operator;
                    let normal_spacing = spacing == MathSpacing::Normal
                        && (role != MathRole::Operand
                            || prev_role != MathRole::Operand
                            || pending_space);
                    let separator_start = out.len();
                    if !started {
                        // no separator before the first atom
                    } else if pending_break || pending_newline {
                        out.push(Ir::hard_line());
                    } else if completes_colon_relation {
                        // The preceding colon and this scripted equals form one
                        // relation atom, so their boundary stays tight.
                    } else if prev_ends_control_word && atom.starts_control_word_letter {
                        // Tight script spacing must not merge `\in A` into the
                        // distinct control word `\inA`.
                        out.push(Ir::verbatim(" "));
                    } else if touches_spaced_slash
                        || normal_spacing
                        || spacing == MathSpacing::Script
                            && (script_operator_spacing || spaced_operands)
                    {
                        // Space around a binary/relation operator (either side), or a
                        // collapsed authored gap between ordinary operands. Script-size
                        // lists suppress incidental gaps around operators.
                        out.push(Ir::verbatim(" "));
                    }
                    prev_opener = atom.delimiter == Some(DelimiterRole::Open);
                    prev_delimiter_edge = delimiter_edge;
                    prev_spaced_slash = spaced_slash;
                    prev_control_word_operator = atom.control_word_operator;
                    prev_ends_control_word = atom.ends_control_word;
                    prev_colon_relation_prefix = atom.colon_relation_prefix;
                    prev_colon_needs_left_space = atom.colon_relation_prefix
                        && spacing == MathSpacing::Normal
                        && started
                        && separator_start == out.len();
                    prev_atom_ir_index = out.len();
                    out.push(atom.ir);
                    started = true;
                    pending_space = false;
                    pending_newline = false;
                    pending_comment_own_line = false;
                    pending_break = is_line_break;
                    prev_role = role;
                    prev_class = atom.class;
                }
            }
        }
    }
    if pending_break {
        out.push(Ir::hard_line());
    }
    Ir::concat(out)
}

/// Lower one math atom (a non-trivia element of a math body).
fn lower_math_element(el: SyntaxElement, cx: LowerCtx<'_>, spacing: MathSpacing) -> Ir {
    match el {
        SyntaxElement::Node(n) => match n.kind() {
            SyntaxKind::SCRIPTED => lower_scripted(&n, cx, spacing),
            SyntaxKind::SUBSCRIPT | SyntaxKind::SUPERSCRIPT => lower_script(&n, cx),
            SyntaxKind::GROUP => lower_math_group(&n, cx, spacing),
            SyntaxKind::LEFT_RIGHT => lower_left_right(&n, cx, spacing),
            // Only signature-proven math slots recurse. A scanned redefinition
            // shadows the built-in with unknown domains and restores the
            // whole-command preservation fallback.
            SyntaxKind::COMMAND if command_has_math_arg(&n, cx) => {
                lower_command_with_math_spacing(&n, cx, spacing)
            }
            SyntaxKind::COMMAND => Ir::verbatim(n.text().to_string()),
            // A block environment is an indivisible math atom whose continuation
            // lines hang from its rendered start column. Generic indentation alone
            // would instead return to the enclosing math body's base indentation.
            SyntaxKind::ENVIRONMENT => Ir::align_current(lower_node(&n, cx)),
            // Anything unexpected: defer to generic lowering.
            _ => lower_node(&n, cx),
        },
        SyntaxElement::Token(t) => lower_loose_token(&t, cx),
    }
}

/// Lower a `{…}` math group: keep the braces, format the body in math mode. The
/// body sits one column past the `{` ([`Ir::align`]), so a multi-line body (a
/// nested block environment) hangs at its own start column — its `\end{…}` under
/// its `\begin{…}` — instead of at the `{`. A single-line body is unaffected.
fn lower_math_group(node: &SyntaxNode, cx: LowerCtx<'_>, spacing: MathSpacing) -> Ir {
    let inner = node
        .children_with_tokens()
        .filter(|el| !matches!(el.kind(), SyntaxKind::L_BRACE | SyntaxKind::R_BRACE));
    Ir::concat([
        Ir::verbatim("{"),
        Ir::align(1, lower_math_seq(inner, cx, spacing, false)),
        Ir::verbatim("}"),
    ])
}

/// Lower a signature-proven math argument while retaining its authored brace or
/// bracket delimiters. Only the body enters recursive math lowering.
fn lower_math_argument_group(node: &SyntaxNode, cx: LowerCtx<'_>, spacing: MathSpacing) -> Ir {
    let (open_kind, close_kind, open, close) = match node.kind() {
        SyntaxKind::GROUP => (SyntaxKind::L_BRACE, SyntaxKind::R_BRACE, "{", "}"),
        SyntaxKind::OPTIONAL => (SyntaxKind::L_BRACKET, SyntaxKind::R_BRACKET, "[", "]"),
        _ => return Ir::verbatim(node.text().to_string()),
    };
    let inner = node.children_with_tokens().filter(
        |element| !matches!(element.kind(), kind if kind == open_kind || kind == close_kind),
    );
    Ir::concat([
        Ir::verbatim(open),
        Ir::align(1, lower_math_seq(inner, cx, spacing, false)),
        Ir::verbatim(close),
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
///
/// The body sits just inside the opening delimiter, and its [`Ir::align`] width is
/// that flat opening width — so a multi-line body (a nested block environment)
/// hangs at its own start column, its `\end{…}` under its `\begin{…}` rather than
/// under the `\left`. A single-line body is unaffected.
fn lower_left_right(node: &SyntaxNode, cx: LowerCtx<'_>, spacing: MathSpacing) -> Ir {
    let mut parts: Vec<Ir> = Vec::new();
    // The flat width of the opening run (`\left(`, `\left\langle`, …): every
    // delimiter token seen before the body.
    let mut open_width = 0usize;
    let mut seen_body = false;
    for el in node.children_with_tokens() {
        match el {
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {}
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::MATH => {
                seen_body = true;
                if !math_body_is_empty(&n) {
                    parts.push(Ir::align(
                        open_width + 1,
                        Ir::concat([
                            Ir::verbatim(" "),
                            lower_math_seq(n.children_with_tokens(), cx, spacing, false),
                            Ir::verbatim(" "),
                        ]),
                    ));
                }
            }
            SyntaxElement::Token(t) => {
                if !seen_body {
                    open_width += t.text().chars().count();
                }
                parts.push(Ir::verbatim(t.text()));
            }
            SyntaxElement::Node(n) => parts.push(lower_node(&n, cx)),
        }
    }
    Ir::concat(parts)
}

/// Whether a math body has no visible content (only whitespace/newlines), so a
/// `\left( … \right)` around it should not gain inner spaces.
fn math_body_is_empty(node: &SyntaxNode) -> bool {
    node.text().to_string().trim().is_empty()
}

/// Lower a `SCRIPTED` atom: the base then its `^`/`_` scripts, all tight (the
/// trivia the parser kept inside the node for losslessness is dropped here).
fn lower_scripted(node: &SyntaxNode, cx: LowerCtx<'_>, spacing: MathSpacing) -> Ir {
    Ir::concat(
        strip_virtual_dtx_framing(node.children_with_tokens(), cx)
            .into_iter()
            .filter_map(|el| match el {
                SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => None,
                SyntaxElement::Node(n)
                    if matches!(n.kind(), SyntaxKind::SUBSCRIPT | SyntaxKind::SUPERSCRIPT) =>
                {
                    Some(lower_script(&n, cx))
                }
                other => Some(lower_math_element(other, cx, spacing)),
            }),
    )
}

/// Lower a `SUBSCRIPT`/`SUPERSCRIPT`: the `_`/`^` glued tightly to its argument.
/// Braces are kept verbatim — dropping redundant single-token braces (`x^{2}` ->
/// `x^2`) is a *content* rewrite, not layout, so it lives in the linter's
/// `redundant-script-braces` autofix, keeping this layout engine whitespace-only.
fn lower_script(node: &SyntaxNode, cx: LowerCtx<'_>) -> Ir {
    Ir::concat(
        strip_virtual_dtx_framing(node.children_with_tokens(), cx)
            .into_iter()
            .filter_map(|el| match el {
                SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => None,
                SyntaxElement::Token(t)
                    if matches!(t.kind(), SyntaxKind::CARET | SyntaxKind::UNDERSCORE) =>
                {
                    Some(Ir::verbatim(t.text()))
                }
                other => Some(lower_math_element(other, cx, MathSpacing::Script)),
            }),
    )
}

/// True if `node` directly contains a `NEWLINE` token — **the unsafe
/// lone-newline predicate** (trivia-invariant layout, `formatter.md`).
///
/// Its two surviving readers — the `GROUP` arm's non-[`WrapMode::Reflow`] /
/// doc-margined branch in [`lower_node`] and [`lower_optional`]'s
/// non-`wraps_prose` / doc-margined early return — decide block-vs-inline for
/// a delimited group and are sanctioned **Tier 2**, on this fixed-point
/// argument: the block form ([`lower_bracketed`]) always ends with a newline
/// before its closing delimiter, so its output re-reads as multi-line and
/// takes the block form again, byte-stably (its body renderers —
/// [`ReflowKind::Statement`], the generic stream — carry their own fixed-point
/// contracts); an empty multi-line body collapses to the bare delimiters,
/// which re-read single-line and *stay* on the inline path; and the inline
/// path emits no newline inside the group, so a single-line group re-reads
/// single-line. Every layout either reader can emit re-reads to itself.
///
/// Both readers are reachable only under the Tier-2 wrap modes
/// (`Preserve`/`Stable`/`Sentence`/`Semantic`, modes defined by authored
/// breaks) or behind `doc_margin_opens_line` (a preserved column-0 predicate).
/// Under the default `Reflow`, [`lower_opaque_group`] and [`lower_optional`]
/// decide from width, content, and preserved predicates only and never
/// consult this. **Don't add a reader in a Tier-1 position.**
fn spans_multiple_lines(node: &SyntaxNode) -> bool {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .any(|t| t.kind() == SyntaxKind::NEWLINE)
}

/// True if `node` contains a `.dtx` documentation margin or docstrip guard at
/// any depth — a construct continuing across margined or guarded lines. Both
/// tokens are line-oriented column-0 facts (a `%` or `%<…>` recognized at line
/// start only), so any relayout that merges or re-indents their lines silently
/// turns them into ordinary comments on the next parse. Always false outside
/// the `.dtx` lexer mode (only it emits these kinds), which is why `cx.is_dtx`
/// short-circuits it — the same gate [`doc_margin_opens_line`] carries, and not
/// merely an optimization at this size. This is a *match guard* on most of
/// [`lower_node`]'s relayout arms, so it runs for every group, environment, and
/// math node; the walk is `O(subtree)`, and a nested construct re-walks at each
/// level. Ungated it was ~52% of the run on `{{{…}}}` nested 4000 deep, and made
/// lowering quadratic in nesting depth for every file, `.dtx` or not.
fn contains_doc_margin(node: &SyntaxNode, cx: LowerCtx<'_>) -> bool {
    if !cx.is_dtx || cx.in_dtx_doc_region {
        return false;
    }
    node.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .any(|t| matches!(t.kind(), SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD))
}

/// Whether a forced-break block's interior lines all ride their own column-0
/// margins: the subtree spans at least one newline, every `NEWLINE` token is
/// immediately followed (still inside the node) by a `DOC_MARGIN` or `GUARD`,
/// and no other token embeds a newline (a multi-line `VERB`, a `\`-newline
/// control symbol). Every relayout arm of [`lower_node`] refuses a doc-margined
/// subtree, so such a block lowers through the byte-faithful stream and
/// reproduces its margins exactly when committed raw — only its first line
/// needs the canonical margin re-attached
/// ([`LineBuilder::push_margined_block`]). A node-final `NEWLINE` (nothing
/// follows it inside the node) fails conservatively: the check cannot see what
/// the next line carries.
fn block_rides_own_margins(node: &SyntaxNode) -> bool {
    let tokens: Vec<SyntaxToken> = node
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .collect();
    let mut saw_newline = false;
    for (i, token) in tokens.iter().enumerate() {
        if !token.text().contains('\n') {
            continue;
        }
        if token.kind() != SyntaxKind::NEWLINE {
            return false;
        }
        saw_newline = true;
        if !tokens
            .get(i + 1)
            .is_some_and(|next| matches!(next.kind(), SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD))
        {
            return false;
        }
    }
    saw_newline
}

/// True if `node` directly contains a `VERBATIM_BODY` token — i.e. it is a
/// verbatim-like environment whose body must be emitted byte-for-byte.
fn has_verbatim_body(node: &SyntaxNode) -> bool {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .any(|t| t.kind() == SyntaxKind::VERBATIM_BODY)
}

/// A **normalized** trivia boundary: everything the layout is allowed to know
/// about the gap between two neighbouring elements.
///
/// What this type cannot say is the point of it. There is deliberately no
/// `Newline` variant — inline whitespace and a lone newline both arrive as
/// [`Self::Space`] — because the formatter converts freely between those two
/// spellings in *both* directions (`alpha\nbeta` → `alpha beta`, and a width wrap
/// back again). A layout decision keyed on which one the author wrote is
/// therefore a latent idempotency bug, and it is the root cause of the whole
/// K&R/Allman family (issues #71, #94, #96, #97). Discipline caught those one at
/// a time; deleting the information at the boundary is the enforcement, because a
/// rule cannot key on what it cannot see.
///
/// Glued-ness, blank-line presence, and comment presence *are* predicates the
/// formatter preserves (`P(fmt(x)) == P(x)`), so they keep their own variants and
/// layout may read them freely.
///
/// A Tier-2 site — one whose contract *is* the authored line structure — reads
/// the newline count through [`WideGap`] instead, and owes the written
/// fixed-point argument that goes with it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Gap {
    /// No trivia at all: the neighbours abut (`\ifmmode y\else`,
    /// `xmin=-5,xmax=5`). Breaking here would materialize a space token TeX
    /// contributes to the horizontal list — a typeset change the CST oracles
    /// cannot see, since whitespace is trivia to them and content to TeX — so it
    /// is a break opportunity only where a processor is proven to discard that
    /// space (see [`ContentKind::Keyval`]).
    Glued,
    /// Collapsible trivia carrying no blank line: inline whitespace, a lone
    /// newline, or any mix of the two. A break opportunity, and the one variant
    /// that must never be split back into its two spellings.
    ///
    /// `flat` is what a one-line rendering writes here, from [`Gap::flat`].
    Space { flat: String },
    /// Two or more newlines: an authored `\par`.
    Blank,
    /// The boundary ends at a `%` comment. The comment must terminate its line, so
    /// a break here is forced — and costs nothing, because the `%` already absorbs
    /// the line end.
    Comment,
}

impl Gap {
    /// A gap whose flat spelling is a single space: what every gap the layout is
    /// free to break renders as when it does not.
    fn space() -> Gap {
        Gap::Space {
            flat: " ".to_string(),
        }
    }

    /// Normalize a consumed trivia run. A run holding *any* newline flattens to one
    /// space — the only spelling a break reproduces — and its leading indentation is
    /// dropped, since the printer owns indentation and recreates it.
    fn from_run(newlines: usize, trailing_ws: String) -> Gap {
        match newlines {
            0 => Gap::Space { flat: trailing_ws },
            1 => Gap::space(),
            _ => Gap::Blank,
        }
    }

    /// What a *flat* rendering writes at this boundary: nothing where the author
    /// glued, the authored whitespace verbatim for a newline-free run, and a single
    /// space wherever the run carried a newline (blank line included).
    ///
    /// So a lone newline and a single authored space are indistinguishable here —
    /// that is the whole point — while a wider run (`\pgfpoint@oncoil{0    }`) still
    /// rides verbatim. That is not a leak of the unsafe predicate: every reader of
    /// `flat` emits it unchanged, so "the run was wider than one space" is a
    /// predicate they all preserve.
    fn flat(&self) -> &str {
        match self {
            Gap::Space { flat } => flat,
            Gap::Blank => " ",
            Gap::Glued | Gap::Comment => "",
        }
    }

    /// How a split point at this boundary renders in each mode: an [`Ir::Line`] (a
    /// space flat, a newline broken) wherever the author already wrote whitespace —
    /// the whitespace ↔ newline exchange that is TeX-identical anywhere, so it needs
    /// no permission — and an [`Ir::SoftLine`] (*nothing* flat) where they glued, so
    /// a fitting line stays byte-identical to the source and only the broken form
    /// materializes a space token.
    fn separator(&self) -> Ir {
        match self {
            Gap::Glued => Ir::soft_line(),
            _ => Ir::line(),
        }
    }
}

/// A [`Gap`] with the **unsafe** predicate — how many newlines the run spanned —
/// still readable.
///
/// Handed only to the Tier-2 sites whose contract *is* the authored line
/// structure: the byte-faithful stream ([`classify_trivia`]), the preserve-shaped
/// wrap modes, [`ReflowKind::Statement`]'s fallback content (a picture body's
/// `STATEMENT` nodes are structural, [`lower_statement`]), the expl3 fallback
/// statement, and the
/// command-only-line residue. Each owes a written fixed-point argument showing
/// every layout it can emit re-reads to itself; preservation-only rules have the
/// easy one (a hard line prints a newline, which re-reads as a newline, and is
/// kept again in place). Everything else takes [`Self::narrow`].
struct WideGap {
    gap: Gap,
    /// **Tier 2.** Reading this is reading the predicate the formatter does not
    /// preserve. Do not add a read without the fixed-point argument.
    newlines: usize,
}

impl WideGap {
    fn narrow(self) -> Gap {
        self.gap
    }
}

/// Consume the maximal run of collapsible trivia beginning at `first` and
/// normalize it to a [`Gap`] — the boundary every width-driven lowering takes,
/// and the reason none of them *can* key on a lone newline.
fn consume_gap(
    first: &SyntaxToken,
    iter: &mut Peekable<impl Iterator<Item = SyntaxElement>>,
) -> Gap {
    consume_gap_widened(first, iter).narrow()
}

/// The Tier-2 form of [`consume_gap`]: the same run, with the newline count left
/// readable. See [`WideGap`] for who may call this and what they owe.
///
/// The run's newline count is taken over the whole run; the whitespace following
/// the *last* newline is the run's leading indentation, which the printer owns, and
/// whitespace *before* a newline is trailing whitespace — both are dropped by
/// [`Gap::from_run`]. For a run with no newline the whole run is the gap's flat
/// spelling.
fn consume_gap_widened(
    first: &SyntaxToken,
    iter: &mut Peekable<impl Iterator<Item = SyntaxElement>>,
) -> WideGap {
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
    WideGap {
        gap: Gap::from_run(newlines, trailing_ws),
        newlines,
    }
}

/// Consume the maximal run of collapsible trivia in `elements` beginning at
/// `*i`, advancing `*i` past it and returning the number of newlines it spans.
/// The index-based analogue of [`consume_gap_widened`], used by the two reflow
/// drivers, which need to look ahead past the run (the peekable iterator form
/// cannot).
///
/// **Tier 2**, like everything else that reads a newline count: both callers are
/// line-structure-preserving sites carrying their own fixed-point arguments (see
/// [`WideGap`]). Neither needs the flat spelling — a reflow re-derives spacing
/// from the fill — so this returns the count bare rather than a [`WideGap`].
fn consume_widened_gap_slice(elements: &[SyntaxElement], i: &mut usize) -> usize {
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
/// solely of non-inline command(s) and inline whitespace — the unit
/// [`reflow_elements`]'s *residual* rule keeps on its own line rather than
/// reflowing into its neighbours. The line runs until the next newline, comment,
/// or end of the stream; any non-trivia element that is not such a command (a
/// word, a control symbol, a group, math, a `\\`, a block, or an *inline* command
/// like `\citep`/`\ref` — see [`command_is_inline`]) disqualifies it. A line with
/// no command (e.g. an empty or comment-only line) is not a command line.
///
/// Residual: a curated block command ([`command_is_block`]) is intercepted by the
/// block-statement arm and gets its own line regardless of this test, so the
/// authored-break preservation decided here matters only for un-signatured and
/// scanned-definition commands (block-ness undecidable without meaning) and for
/// block commands glued to adjacent content.
///
/// That read of the lone-newline predicate is sanctioned **Tier 2**, on this
/// fixed-point argument: the rule is preservation-only — its entire effect is to
/// harden a gap that already holds a newline, never to write a break where none
/// was or to move content across one. So a newline in the output is either (a) a
/// break this rule kept, which re-reads as the same command-only line at the same
/// gap (nothing moves non-trivia across a hard line end, so command-only-ness is
/// itself preserved) and is kept again in place; or (b) a break the fill emitted,
/// which the next pass may *harden* when the printed line it opened or closed
/// happens to be command-only — a width wrap stranding an un-signatured command
/// alone. Hardening a break the greedy fill already chose is layout-neutral:
/// filling is first-fit, so the lines before the hardened break refill unchanged
/// and the fill after it restarts from column 0 exactly as the soft break did
/// (`reflow_command_stranded_by_width` pins this corner; the Tier-2 render modes'
/// own contracts cover their breaks the same way). Strict trivia invariance is
/// deliberately not claimed — preserving the authored break *is* the rule — so
/// `--checks trivia-strict` still reports these shapes
/// (`trivia_strict_check_fires_where_an_authored_break_is_preserved`), and the
/// [`Gap`] normalization carries this read as a [`WideGap`].
/// Retiring the rule outright would glue every authored `\mymacro`-on-its-own-line
/// into the fill — a policy change, not a fix.
///
/// A `CONDITIONAL` reaches this as a single non-`COMMAND` element and so
/// disqualifies its line, which is correct: [`lower_conditional`] owns where its
/// dividers fall, and the divider commands never appear in a reflow stream of
/// their own.
fn line_is_command_only(elements: &[SyntaxElement], start: usize, cx: LowerCtx<'_>) -> bool {
    let mut saw_command = false;
    for element in &elements[start..] {
        match element {
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::NEWLINE => break,
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::COMMENT => break,
            SyntaxElement::Token(t) if cx.is_dtx && t.kind() == SyntaxKind::DOC_MARGIN => {
                continue;
            }
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

/// Whether the element after `idx` leaves a break opportunity: the stream ends, or
/// the next element is collapsible trivia (whitespace, a newline) or a `COMMENT`
/// (a glued `%` is the line-continuation idiom and rides the committed line via
/// the `after_block` path). Anything else is glued to `elements[idx]`, and the
/// engine's rule is that adjacent non-whitespace elements form one unbreakable
/// atom — splitting there would materialize a space token TeX typesets. Keyed on
/// adjacency alone, a predicate the formatter preserves (it never converts
/// glued↔spaced), so the block-statement gate may read it.
fn next_is_separated(elements: &[SyntaxElement], idx: usize) -> bool {
    match elements.get(idx + 1) {
        None => true,
        Some(SyntaxElement::Token(t)) => matches!(
            t.kind(),
            SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
        ),
        Some(SyntaxElement::Node(_)) => false,
    }
}

/// Whether the next non-trivia element in the paragraph is a `\label` command.
fn next_nontrivia_is_label(elements: &[SyntaxElement], idx: usize) -> bool {
    elements[idx + 1..]
        .iter()
        .find(|element| !is_collapsible_trivia_element(element))
        .is_some_and(|element| {
            matches!(element, SyntaxElement::Node(node) if node.kind() == SyntaxKind::COMMAND && command_is_label(node))
        })
}

/// Whether `elements[idx]` is in a label run immediately after a sectioning
/// command. The paragraph boundary supplies the outer structural gate; scanning
/// only within this flattened sibling stream cannot attach a label across
/// intervening prose, comments, or another construct. Glued seams are admitted
/// because the sectioning rule itself puts the heading and first label on separate
/// lines; accepting that source shape on pass one is required for idempotence.
fn label_follows_sectioning_run(elements: &[SyntaxElement], idx: usize, cx: LowerCtx<'_>) -> bool {
    for element in elements[..idx].iter().rev() {
        match element {
            SyntaxElement::Token(token) if is_collapsible_trivia(token.kind()) => {}
            SyntaxElement::Node(node)
                if node.kind() == SyntaxKind::COMMAND && command_is_label(node) => {}
            SyntaxElement::Node(node)
                if node.kind() == SyntaxKind::COMMAND && command_is_sectioning(node, cx) =>
            {
                return node
                    .parent()
                    .is_some_and(|parent| parent.kind() == SyntaxKind::PARAGRAPH);
            }
            _ => return false,
        }
    }
    false
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
/// two or more → a single [`Ir::empty_line`] (one blank line). A non-math grid
/// cell softens the one-newline case to [`Ir::line`] so continuation lines join
/// even when parser attachment nests the trivia inside a command. Whitespace
/// that followed the last newline is *indentation*, which the printer owns and
/// recreates, so it is dropped by [`Gap::from_run`] — keeping it would
/// double-indent on reformat.
///
/// This is normally the byte-faithful stream's boundary, and the reason it takes a
/// [`WideGap`]: reproducing the author's line structure is its entire contract, so
/// it reads the newline count by definition. **Tier 2**, with the trivial
/// fixed-point argument — a `hard_line` re-reads as one newline, an `empty_line`
/// re-reads as two, and verbatim whitespace re-reads as itself. The alignment-cell
/// exception inherits the grid's existing continuation rule: it emits the cell on
/// one line, which reparses without the softened newline and remains on that same
/// flat path; a blank line is never softened.
fn classify_trivia(gap: WideGap, soften_newline: bool) -> Ir {
    match gap.newlines {
        0 => Ir::verbatim(gap.gap.flat()),
        1 if soften_newline => Ir::line(),
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

    /// Run [`head_command_has_grouped_sibling_arg`] on the *innermost* `GROUP`
    /// descendant of `input` whose text contains `marker`.
    fn grouped_sibling_walk(input: &str, marker: &str) -> bool {
        let parsed = parse(input);
        assert!(parsed.errors.is_empty(), "test input should parse cleanly");
        let root = parsed.syntax();
        // Preorder puts an enclosing group before a nested one, so the last
        // match is the innermost.
        let group = root
            .descendants()
            .filter(|n| n.kind() == SyntaxKind::GROUP && n.text().to_string().contains(marker))
            .last()
            .expect("a group containing the marker");
        head_command_has_grouped_sibling_arg(&group)
    }

    #[test]
    fn grouped_sibling_walk_stops_at_the_statement_boundary() {
        // A grouped command in the *previous statement* must not suppress the
        // trailing-hang treatment for an unrelated `\bool_if:NF \l… {body}` —
        // free under the node-local read, since a node's children are one
        // statement by construction.
        let src = "\\ExplSyntaxOn\n\\tl_set:Nn \\l_x { v }\n\\bool_if:NF \\l_bool { body }\n\\ExplSyntaxOff\n";
        assert!(!grouped_sibling_walk(src, "body"));

        // Within one call the earlier grouped argument still counts: arity
        // attachment puts a recognized `\prop_get:NnNTF` call's `{#2}` and its
        // branches on one head node, so the multi-argument shape whose branch
        // list the hang path already lays out stably is read off the node.
        let src = "\\ExplSyntaxOn\n\\prop_get:NnNTF \\g_prop {#2} \\l_tl { branch } { f }\n\\ExplSyntaxOff\n";
        assert!(grouped_sibling_walk(src, "branch"));

        // An *aborted* call (the `F` branch is missing, so the five-slot spec
        // never resolves) keeps its groups on the small trailing command. An
        // unresolvable call is not the multi-argument shape this suppresses.
        let src =
            "\\ExplSyntaxOn\n\\prop_get:NnNTF \\g_prop {#2} \\l_tl { branch }\n\\ExplSyntaxOff\n";
        assert!(!grouped_sibling_walk(src, "branch"));
    }

    #[test]
    fn grouped_sibling_walk_matches_the_body_stream_segmentation() {
        // Inside a brace body, a preceding *statement's* grouped call must not
        // suppress the hang for the next statement's `\bool_if:NF … {body}` —
        // under the node-local read the earlier call's group belongs to that
        // call's own node, never to `{body}`'s owner.
        let src = "\\ExplSyntaxOn\n\\use:n { \\tl_set:Nn \\l_x { v } \\bool_if:NF \\l_b { body } }\n\\ExplSyntaxOff\n";
        assert!(!grouped_sibling_walk(src, "body"));
    }

    #[test]
    fn grouped_sibling_walk_ignores_out_of_region_prefix() {
        // The out-of-region `\emph{y}` sharing the authored line must not
        // read as an earlier grouped argument of the `\tl_put_right:Ne`
        // statement — its group belongs to `\emph`'s node, not the owner's.
        let src = "x \\emph{y} \\ExplSyntaxOn \\tl_put_right:Ne \\l_t { body } \\ExplSyntaxOff\n";
        assert!(!grouped_sibling_walk(src, "body"));
    }

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
    fn definee_provides_does_not_open_region() {
        // `\ProvidesExplPackage` as the definee of `\protected\def` is tokenized,
        // never executed, so it opens no formatter-owned region (issue #69).
        let input = "\\protected\\def\\ProvidesExplPackage{\\ProvidesPackage{demo}}\ntext";
        assert!(regions(input).is_empty());
    }

    #[test]
    fn definee_off_does_not_close_a_real_region() {
        // A gated-out `\ExplSyntaxOff` in definee position must not close the open
        // region — the gate skips it, so the region still runs to EOF.
        let input = "\\ExplSyntaxOn a \\let\\ExplSyntaxOff\\relax b";
        assert_eq!(regions(input), vec![(0, input.len())]);
    }

    #[test]
    fn stored_toggle_in_group_does_not_open_region() {
        // An `\ExplSyntaxOn` stored inside a definition body / attached group is
        // never executed at load, so it opens no region (issue #69).
        let input = "\\def\\store{\\ExplSyntaxOn \\foo:n {x}}\ntext";
        assert!(regions(input).is_empty());
    }

    #[test]
    fn top_level_provides_after_gated_definee_still_opens() {
        // The gate rejects only the false positives: a genuine top-level
        // `\ProvidesExplPackage` still opens a region even when a definee one
        // precedes it.
        let input = "\\def\\x{\\ExplSyntaxOn}\n\\ProvidesExplPackage\n\\cs_new:N \\foo:";
        let start = input.rfind("\\ProvidesExplPackage").unwrap();
        assert_eq!(regions(input), vec![(start, input.len())]);
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

    /// The expl3 regions of `input` parsed as a `.dtx`, as `(start, end)` pairs.
    fn regions_dtx(input: &str) -> Vec<(usize, usize)> {
        let config = crate::parser::LexConfig {
            flavor: LatexFlavor::Document,
            dtx: true,
        };
        let parsed = parse_with_flavor(input, config);
        assert!(parsed.errors.is_empty(), "test input should parse cleanly");
        expl3_regions(&parsed.syntax())
            .into_iter()
            .map(|r| (r.start().into(), r.end().into()))
            .collect()
    }

    #[test]
    fn dtx_region_owns_only_macrocode_bodies() {
        // The unmargined `␣%` line between the chunks is documentation, not code
        // (issue #58): the region intersects with the chunk bodies, so neither it
        // nor the margined doc line is formatter-owned.
        let input = "%    \\begin{macrocode}\n\
                     \\ExplSyntaxOn\n\
                     %    \\end{macrocode}\n\
                     \x20%\n\
                     % doc\n\
                     %    \\begin{macrocode}\n\
                     \\foo\n\
                     %    \\end{macrocode}\n";
        let first_frame = input.find("%    \\end{macrocode}").unwrap();
        // The `\begin` frame line's hole spans through its newline, so the second
        // region opens at the body's first code token.
        let second_body = input.find("\\foo").unwrap();
        let second_frame = input.rfind("%    \\end{macrocode}").unwrap();
        assert_eq!(
            regions_dtx(input),
            vec![
                (input.find("\\ExplSyntaxOn").unwrap(), first_frame),
                (second_body, second_frame),
            ]
        );
    }
}

#[cfg(test)]
mod gap_tests {
    use super::*;
    use crate::parser::parse;

    /// The [`Gap`] the boundary reads from the first collapsible-trivia run in
    /// `input`, taken through the same `consume_gap` every width-driven lowering
    /// uses. `descendants` is preorder, so the outermost node holding a trivia run
    /// answers first.
    fn first_gap(input: &str) -> Gap {
        let parsed = parse(input);
        assert!(parsed.errors.is_empty(), "test input should parse cleanly");
        for node in parsed.syntax().descendants() {
            let mut iter = node.children_with_tokens().peekable();
            while let Some(element) = iter.next() {
                let SyntaxElement::Token(token) = element else {
                    continue;
                };
                if is_collapsible_trivia(token.kind()) {
                    return consume_gap(&token, &mut iter);
                }
            }
        }
        panic!("no trivia run in {input:?}");
    }

    /// The property the whole normalization exists for: the two spellings the
    /// formatter converts between are one value at the boundary, so no rule taking
    /// a narrow [`Gap`] can tell them apart. This is the mechanical guard the
    /// K&R/Allman family (issues #71, #94, #96, #97) never had — each of those was
    /// one decision keying on exactly this difference.
    #[test]
    fn a_lone_newline_and_a_space_are_the_same_gap() {
        assert_eq!(first_gap("\\foo{a b}"), first_gap("\\foo{a\nb}"));
        assert_eq!(first_gap("\\foo{a\nb}"), Gap::space());
        // Indentation after the newline is the printer's to recreate, so it is
        // dropped rather than becoming part of the gap.
        assert_eq!(first_gap("\\foo{a\n    b}"), Gap::space());
        assert_eq!(first_gap("\\foo{a \n b}"), Gap::space());
    }

    /// Blank-line presence *is* preserved, so it keeps its own variant — and
    /// flattens to a single space, the only spelling a one-line rendering can
    /// write.
    #[test]
    fn a_blank_line_stays_visible() {
        assert_eq!(first_gap("\\foo{a\n\nb}"), Gap::Blank);
        assert_eq!(first_gap("\\foo{a\n\n\nb}"), Gap::Blank);
        assert_eq!(Gap::Blank.flat(), " ");
    }

    /// A run wider than one space rides verbatim wherever `flat` is read, so
    /// distinguishing it is not a read of the unsafe predicate — every reader
    /// preserves it. It is still not confusable with a break.
    #[test]
    fn a_wide_run_rides_verbatim() {
        assert_eq!(
            first_gap("\\foo{a    b}"),
            Gap::Space {
                flat: "    ".to_string()
            }
        );
        assert_ne!(first_gap("\\foo{a    b}"), first_gap("\\foo{a\nb}"));
    }

    /// The split-point rendering the folded-in `DividerGap`/`KeyBreak` prototypes
    /// agreed on: a glued junction renders as *nothing* flat, so a fitting line
    /// stays byte-identical to the source and only the broken form materializes a
    /// space token TeX would typeset.
    #[test]
    fn a_glued_junction_separates_without_a_flat_space() {
        assert!(matches!(Gap::Glued.separator(), Ir::SoftLine));
        assert_eq!(Gap::Glued.flat(), "");
        assert!(matches!(Gap::space().separator(), Ir::Line));
        assert!(matches!(Gap::Blank.separator(), Ir::Line));
        assert!(matches!(Gap::Comment.separator(), Ir::Line));
    }
}
