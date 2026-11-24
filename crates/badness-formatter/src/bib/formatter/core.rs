//! The BibTeX formatter entry points and the CST → [`Ir`] lowering.
//!
//! This module supplies BibTeX-specific lowering for the shared Wadler-style
//! layout engine.
//!
//! The BibTeX-specific style is fixed:
//! - **Case:** entry types and field names are lowercased; cite keys are preserved
//!   verbatim (case-sensitive identifiers); `@string` macro names are preserved
//!   (BibTeX references them case-insensitively, so changing case is cosmetic).
//! - **Layout:** one field per line, the field block indented one `indent_width`
//!   step, the opening delimiter trailing the header and the closing delimiter
//!   flush; normally one blank line between top-level blocks. A structured lint
//!   directive stays adjacent to the entry or region boundary it governs.
//! - **`=` alignment:** field names are padded to the longest field name in the entry.
//! - **Field order:** a shared title-first sequence, with unlisted fields alphabetized
//!   afterward. Entry-type validation does not determine display order.
//! - **Value delimiters:** in a regular field, a top-level `"…"` value piece is
//!   rewritten to `{…}` when safe (balanced inner braces). A bare `LITERAL` (a macro
//!   reference or a number) is **never** wrapped — that would change its meaning —
//!   and a `Verbatim`-category value (`url`, `doi`, `eprint`, `file`) is never
//!   reshaped. `@string` and `@preamble` values are also left as authored (their
//!   conventional quoting is kept). `#` concatenation structure is preserved,
//!   normalized to ` # `.
//! - **Value reflow:** a long single-piece value is wrapped to `line_width` with a
//!   hanging indent under the `=`, gated by field category (a *correctness* boundary,
//!   not a preference — `line_width` alone tunes it). `Literal` prose (`title`,
//!   `abstract`, …) reflows at any inter-word whitespace; a `Name` value
//!   (`author`/`editor`) reflows **only** at top-level ` and ` boundaries, breaking
//!   *after* "and" so the next name starts the continuation line. Verbatim, date,
//!   concatenated, bare literal, and unescaped-`%` values are not reflowed.
//! - **Trailing comma:** none after the last field.
//! - **`%` comments:** an own-line comment travels with the field below it, and an
//!   end-of-line comment travels with the field on its line. An own-line comment
//!   past the last field prints just above the closing delimiter. A
//!   `@string` / `@preamble` / field-less entry that carries a comment has nowhere
//!   to hang it, so the whole block is emitted verbatim rather than losing it.
//!
//! Protected regions are emitted byte-exact: `@comment` bodies and inter-entry
//! `JUNK` are spliced through untouched (only the blank-line spacing *around* them
//! is normalized), the same sanctioned exception as LaTeX verbatim/comments.
//!
//! Like the LaTeX formatter, this refuses any input the parser flagged — parsing is
//! the parser's job, and the formatter never reshapes around a parse error.

use std::collections::HashMap;

use rowan::{TextRange, TextSize};

use super::lint_directive_verb;
use crate::bib::ast::{self, FieldCommentPlacement};
use crate::bib::comment_directive;
use crate::bib::parse;
use crate::bib::semantic::{BibFieldDb, FieldCategory, builtin};
use crate::bib::syntax::{SyntaxKind, SyntaxNode};
use crate::directives::{Axis, Verb};
use crate::formatter::ir::Ir;
use crate::formatter::printer::Printer;
use crate::formatter::style::{FormatStyle, LineEnding, apply_line_ending, detect_line_ending};

/// Why a `.bib` document could not be formatted. Mirrors
/// [`crate::formatter::core::FormatError`] but over the bib [`SyntaxKind`]; the
/// formatter only operates on a clean parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// The input parsed with `count` syntax error(s); the formatter only supports
    /// input the parser accepts without diagnostics.
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

/// Format `input` under `style`. Returns [`FormatError::ParseErrors`] if the input
/// does not parse cleanly.
pub fn format_with_style(input: &str, style: FormatStyle) -> Result<String, FormatError> {
    let parsed = parse(input);
    if !parsed.errors.is_empty() {
        return Err(FormatError::ParseErrors {
            count: parsed.errors.len(),
        });
    }
    format_node(&parsed.syntax(), style)
}

/// Format an already-parsed CST `root` under `style`. The reparse-free entry
/// (mirrors [`crate::formatter::format_node`]): the caller owns the `ParseErrors`
/// guard; this only enforces the `ERROR`-token invariant.
pub fn format_node(root: &SyntaxNode, style: FormatStyle) -> Result<String, FormatError> {
    validate_supported_tokens(root)?;

    Ok(format_root(root, style))
}

/// Refuse any `ERROR` token. The bib lexer is total (never emits `ERROR`), so this
/// is a safety net identical in spirit to the LaTeX side.
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

fn format_root(root: &SyntaxNode, style: FormatStyle) -> String {
    let (protected, preserve_eof) = format_suppressions(root);
    if protected == [root.text_range()] {
        return root.to_string();
    }
    let source = (!protected.is_empty()).then(|| root.to_string());
    let protection = |start| {
        let index = protected.partition_point(|span| span.end() <= start);
        protected.get(index).filter(|span| span.contains(start))
    };
    let blocks = super::sort::sorted_blocks(root, |block| {
        protection(block.text_range().start()).is_some()
    });
    let ending = if style.line_ending == LineEnding::Auto {
        let text = root.text();
        // A leading protected span can supply the first output break before we
        // emit any separators. Detect from that span so the next pass agrees.
        let leading_protected = blocks
            .first()
            .and_then(|block| protection(block.text_range().start()))
            .map(|span| text.slice(*span - root.text_range().start()))
            .filter(|span| span.contains_char('\n'));
        detect_line_ending(leading_protected.as_ref().unwrap_or(&text))
    } else {
        style.line_ending.resolve(LineEnding::Lf)
    };
    let cx = Lower { db: builtin() };
    let printer = Printer::new(style);
    let mut output = String::new();
    let mut pending = Vec::new();
    let mut previous_verb = None;
    let mut verbatim_until = root.text_range().start();
    let mut previous_raw_junk = false;
    for (index, block) in blocks.iter().enumerate() {
        let start = block.text_range().start();
        let verb = lint_directive_verb(block);
        if let Some(span) = protection(start) {
            if start < verbatim_until {
                previous_verb = verb;
                continue;
            }
            append_formatted(&mut output, &mut pending, &printer, ending, false);
            if index > 0 && !previous_raw_junk {
                output.push_str(ending.as_str());
                if !attached(previous_verb, verb) {
                    output.push_str(ending.as_str());
                }
            }
            let relative = *span - root.text_range().start();
            let source = source
                .as_deref()
                .expect("protected spans carry source text");
            output.push_str(&source[usize::from(relative.start())..usize::from(relative.end())]);
            verbatim_until = span.end();
            // A skipped junk block already owns the gap up to the next entry.
            // A new separator would become part of its protected bytes on reformat.
            previous_raw_junk = block.kind() == SyntaxKind::JUNK;
        } else {
            if index > 0 && !previous_raw_junk {
                pending.push(if attached(previous_verb, verb) {
                    Ir::hard_line()
                } else {
                    Ir::empty_line()
                });
            }
            pending.push(lower_block(block, cx));
            previous_raw_junk = false;
        }
        previous_verb = verb;
    }
    if !pending.is_empty() {
        append_formatted(&mut output, &mut pending, &printer, ending, true);
    } else if !preserve_eof && !previous_raw_junk && !output.is_empty() {
        output.push_str(ending.as_str());
    }
    output
}

fn attached(previous: Option<Verb>, current: Option<Verb>) -> bool {
    matches!(previous, Some(Verb::Skip | Verb::Off)) || current == Some(Verb::On)
}

fn append_formatted(
    output: &mut String,
    pending: &mut Vec<Ir>,
    printer: &Printer,
    ending: LineEnding,
    eof: bool,
) {
    if pending.is_empty() {
        return;
    }
    let mut formatted = printer.print(&Ir::concat(std::mem::take(pending)));
    if eof {
        let trimmed_len = formatted.trim_end_matches([' ', '\t', '\n', '\r']).len();
        formatted.truncate(trimmed_len);
        if !formatted.is_empty() {
            formatted.push('\n');
        }
    }
    apply_line_ending(&mut formatted, ending);
    output.push_str(&formatted);
}

/// The markers and all bytes between them travel as one span. A standalone
/// skip pins only its target. Open regions track the two directive families
/// independently, so a layout-only closer cannot also close lint suppression.
fn format_suppressions(root: &SyntaxNode) -> (Vec<TextRange>, bool) {
    let mut spans = Vec::new();
    let mut open = [None, None];
    let mut skip = false;
    for block in root.children() {
        if block.kind() != SyntaxKind::COMMENT_ENTRY {
            if std::mem::take(&mut skip) {
                spans.push(block.text_range());
            }
            continue;
        }
        let Some(directive) = comment_directive(&block).filter(|d| d.axis.covers_format()) else {
            continue;
        };
        let region = &mut open[usize::from(directive.axis == Axis::Both)];
        match directive.verb {
            Verb::SkipFile => return (vec![root.text_range()], true),
            Verb::Skip => skip = true,
            Verb::Off => {
                region.get_or_insert(block.text_range().start());
            }
            Verb::On => {
                if let Some(start) = region.take() {
                    spans.push(TextRange::new(start, block.text_range().end()));
                }
            }
        }
    }
    let preserve_eof = open.iter().any(Option::is_some);
    for start in open.into_iter().flatten() {
        spans.push(TextRange::new(start, root.text_range().end()));
    }
    spans.sort_unstable_by_key(|span| span.start());
    let mut merged: Vec<TextRange> = Vec::new();
    for span in spans {
        match merged.last_mut() {
            Some(previous) if span.start() < previous.end() => *previous = previous.cover(span),
            _ => merged.push(span),
        }
    }
    (merged, preserve_eof)
}

/// State threaded through the lowering: the built-in field/entry DB (consulted for
/// field categories). `Copy`, like the LaTeX `LowerCtx`. There is no `wrap` field —
/// value reflow is driven entirely by the printer's `line_width` and the per-field
/// category (no per-value wrap mode), so the DB is all the lowering needs.
#[derive(Clone, Copy)]
struct Lower {
    db: &'static BibFieldDb,
}

fn lower_block(node: &SyntaxNode, cx: Lower) -> Ir {
    match node.kind() {
        SyntaxKind::ENTRY => lower_entry(node, cx),
        SyntaxKind::STRING_ENTRY => lower_string_entry(node),
        SyntaxKind::PREAMBLE_ENTRY => lower_preamble_entry(node),
        SyntaxKind::COMMENT_ENTRY => Ir::verbatim(node.to_string()),
        // Junk content is preserved, but its *surrounding* whitespace is owned by
        // `lower_root` (one blank line between blocks). Trimming the outer
        // whitespace is what keeps formatting idempotent: `junk()` greedily bumps
        // every token up to the next `@`, so the blank line we insert after junk
        // would otherwise be re-absorbed into the junk node on the next parse and
        // accumulate. Internal content (including internal newlines) is untouched.
        SyntaxKind::JUNK => Ir::verbatim(node.to_string().trim().to_string()),
        // ROOT holds only the block kinds above plus bare trivia (already
        // filtered out by `children()`); anything else is a malformed tree the
        // error guard has refused. Degrade losslessly.
        _ => Ir::verbatim(node.to_string()),
    }
}

/// The open/close delimiter the entry was authored with — `{ }` or `( )` — preserved
/// rather than normalized (surprise-free; a "normalize to braces" toggle is a clean
/// later add). Detected by the presence of an `L_PAREN` among the entry's tokens.
fn entry_delimiters(entry: &SyntaxNode) -> (&'static str, &'static str) {
    let is_paren = entry
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .any(|token| token.kind() == SyntaxKind::L_PAREN);
    if is_paren { ("(", ")") } else { ("{", "}") }
}

/// A regular entry `@type{key, field, …}`.
fn lower_entry(entry: &SyntaxNode, cx: Lower) -> Ir {
    let etype = ast::entry_type(entry).unwrap_or_default().to_lowercase();
    let key = ast::cite_key(entry)
        .map(|(text, _)| text)
        .unwrap_or_default();
    let (open, close) = entry_delimiters(entry);

    let fields: Vec<SyntaxNode> = super::sort::canonical_fields(entry);
    let names: Vec<String> = fields
        .iter()
        .map(|field| ast::field_name(field).unwrap_or_default().to_lowercase())
        .collect();
    let CommentPlacement {
        leading,
        same_line,
        trailing,
    } = place_comments(entry);

    // A keyless and/or fieldless entry stays on a single line: `@misc{key}` — unless
    // it carries a comment, which then has no field to bind to and no line to sit on.
    if fields.is_empty() {
        if trailing.is_empty() {
            return Ir::text(format!("@{etype}{open}{key}{close}"));
        }
        return Ir::verbatim(entry.to_string());
    }

    let width = names
        .iter()
        .map(|name| name.chars().count())
        .max()
        .unwrap_or(0);
    let header = Ir::text(format!("@{etype}{open}{key},"));

    let lines = fields.iter().enumerate().map(|(i, field)| {
        let last = i + 1 == fields.len();
        let at = field.text_range().start();
        // The field's bound comments travel with it through the canonical sort: the
        // same-line one after its comma, the rest stacked above its line.
        let mut line = vec![lower_field(field, &names[i], width, last, cx)];
        if let Some(text) = same_line.get(&at) {
            line.push(Ir::text(format!(" {text}")));
        }
        match leading.get(&at) {
            None => Ir::concat(line),
            Some(above) => Ir::concat(above.iter().map(|text| comment_line(text)).chain(line)),
        }
    });
    let body = Ir::concat([
        Ir::hard_line(),
        Ir::join(Ir::hard_line(), lines),
        // Comments past the last field print just above the closing delimiter.
        Ir::concat(
            trailing
                .iter()
                .flat_map(|text| [Ir::hard_line(), Ir::text(text.clone())]),
        ),
    ]);

    Ir::concat([
        header,
        Ir::indent(body),
        Ir::hard_line(),
        Ir::text(close.to_string()),
    ])
}

/// Where each `%` comment in an entry goes. Both maps are keyed by a field's start
/// offset rather than by index, because the caller emits fields in *canonical* order.
/// See [`place_comments`].
#[derive(Default)]
struct CommentPlacement {
    /// Comment lines printed above a field's line, in source order.
    leading: HashMap<TextSize, Vec<String>>,
    /// The one comment that rides the end of a field's own line.
    same_line: HashMap<TextSize, String>,
    /// Comments with no field to bind to; printed above the closing delimiter.
    trailing: Vec<String>,
}

fn place_comments(entry: &SyntaxNode) -> CommentPlacement {
    let mut placement = CommentPlacement::default();

    for (comment, attachment) in ast::field_comments(entry) {
        // Trailing whitespace is dropped so a re-read yields the identical text.
        let text = comment.to_string().trim_end().to_string();

        match attachment {
            FieldCommentPlacement::SameLine(field) => {
                placement.same_line.insert(field.start(), text);
            }
            FieldCommentPlacement::Leading(field) => placement
                .leading
                .entry(field.start())
                .or_default()
                .push(text),
            FieldCommentPlacement::Trailing => placement.trailing.push(text),
        }
    }
    placement
}

/// One comment on its own line, above whatever follows it.
fn comment_line(text: &str) -> Ir {
    Ir::concat([Ir::text(text.to_string()), Ir::hard_line()])
}

/// Whether `node` contains a `%` comment anywhere. Used by the block lowerings that
/// have no per-line structure to hang one on, so they fall back to verbatim.
fn has_comment(node: &SyntaxNode) -> bool {
    node.descendants()
        .any(|child| child.kind() == SyntaxKind::COMMENT)
}

/// One `name = value` field line. `name_lc` is the lowercased field name; `width` is
/// the entry's longest field-name width, used to pad so the `=` signs align. A
/// trailing comma is emitted on every field except the `last`.
fn lower_field(field: &SyntaxNode, name_lc: &str, width: usize, last: bool, cx: Lower) -> Ir {
    let pad = " ".repeat(width - name_lc.chars().count());
    let prefix = Ir::text(format!("{name_lc}{pad} = "));
    // The value column = `name<pad> = ` width = `width + len(" = ")`. A pure function
    // of the entry's (lowercased) field-name set, so it is recomputed identically on
    // every run — the hanging indent stays stable under reflow (idempotent).
    let prefix_width = width + " = ".len();

    // Regular fields normalize quotes → braces, except `Verbatim`-category fields
    // (url/doi/…), whose value is never reshaped.
    let category = cx.db.category(name_lc);
    let normalize = category != FieldCategory::Verbatim;
    let value = match ast::field_value(field) {
        Some(value) => lower_value_reflowed(&value, normalize, category, prefix_width),
        // Defensive: a clean field always has a value; a recovery one is refused.
        None => Ir::nil(),
    };

    let comma = if last { Ir::nil() } else { Ir::text(",") };
    Ir::concat([prefix, value, comma])
}

/// Lower a field value, reflowing it to `line_width` when the field's category and
/// shape make wrapping meaning-safe; otherwise fall back to the byte-exact
/// [`lower_value`]. The guards (in order):
///
/// 1. `Verbatim`/`Date` categories never reflow.
/// 2. A `#`-concatenated value (more than one piece) never reflows — the ` # `
///    structure is layout, not prose whitespace.
/// 3. A single bare `LITERAL` (macro reference or number) never reflows — wrapping
///    would change its meaning.
/// 4. A single `BRACE_GROUP` (or a `QUOTED` piece that safely rewrites to braces)
///    reflows: `Literal` as prose, `Name` at ` and ` boundaries only —
/// 5. …unless the content carries an unescaped `%`. A `%` is an ordinary character
///    to BibTeX but a *comment* to the LaTeX that finally typesets the value, so
///    where the line breaks fall decides what is typeset. Reflow moves them, so it
///    would silently change output.
fn lower_value_reflowed(
    value: &SyntaxNode,
    normalize: bool,
    category: FieldCategory,
    prefix_width: usize,
) -> Ir {
    // Guard 1: categories whose values are structural, not prose.
    if matches!(category, FieldCategory::Verbatim | FieldCategory::Date) {
        return lower_value(value, normalize);
    }
    // Guard 2: only a lone value piece is a reflow candidate (no `#` concatenation).
    let pieces: Vec<SyntaxNode> = value
        .children()
        .filter(|piece| {
            matches!(
                piece.kind(),
                SyntaxKind::LITERAL | SyntaxKind::QUOTED | SyntaxKind::BRACE_GROUP
            )
        })
        .collect();
    let [piece] = pieces.as_slice() else {
        return lower_value(value, normalize);
    };

    // Guard 3 + extract the inner text to reflow. A bare `LITERAL` and an unsafe
    // `QUOTED` both fall back; a `BRACE_GROUP` or a safely-rebraced `QUOTED` reflow.
    let inner = match piece.kind() {
        SyntaxKind::BRACE_GROUP => brace_inner(piece),
        SyntaxKind::QUOTED => match quoted_inner_if_safe(piece, normalize) {
            Some(inner) => inner,
            None => return lower_value(value, normalize),
        },
        _ => return lower_value(value, normalize),
    };

    // Guard 5: a LaTeX comment inside the value makes its line breaks load-bearing.
    if has_unescaped_percent(&inner) {
        return lower_value(value, normalize);
    }

    match category {
        FieldCategory::Name => reflow_name_value(&inner, prefix_width),
        // `Literal` prose (and, defensively, any other non-Verbatim/Date category).
        _ => reflow_prose_value(&inner, prefix_width),
    }
}

/// Reflow a `Literal` prose value (`inner` is the content between the outer braces).
/// Words split at brace-/math-depth-0 whitespace ([`split_brace_aware`]) and flow
/// through an [`Ir::fill`]; continuation lines hang under the value column via
/// [`Ir::align`]. The outer braces are re-emitted around the aligned fill. An empty
/// value degenerates to `{}`.
fn reflow_prose_value(inner: &str, prefix_width: usize) -> Ir {
    let words = split_brace_aware(inner);
    let fill = Ir::fill(words.into_iter().map(Ir::text));
    Ir::concat([
        Ir::text("{"),
        // `+ 1` for the `{`: align continuation lines under the first value char.
        Ir::align(prefix_width + 1, fill),
        Ir::text("}"),
    ])
}

/// Reflow a `Name` value (`author`/`editor`). Names split at top-level ` and `
/// boundaries only ([`split_top_level_and`]); each whole name is an unbreakable atom.
/// The separator carries the literal " and" followed by an [`Ir::Line`], so the fill
/// breaks *after* "and" (it stays at the line end; the next name starts the
/// continuation line). A single name (or none) emits intact — there is no structural
/// break point. Outer braces and the hanging indent match [`reflow_prose_value`].
fn reflow_name_value(inner: &str, prefix_width: usize) -> Ir {
    let names = split_top_level_and(inner);
    let body = if names.len() <= 1 {
        // One name (commas and intra-name spaces kept, normalized to single spaces)
        // or none. Nothing to break on, so a single unbreakable atom.
        Ir::text(names.into_iter().next().unwrap_or_default())
    } else {
        // Build the fill directly: `Ir::fill` only inserts a bare `Line`, but the
        // name separator must also print the word "and" before the break.
        let sep = Ir::concat([Ir::text(" and"), Ir::Line]);
        let mut parts = Vec::with_capacity(names.len() * 2 - 1);
        for (i, name) in names.into_iter().enumerate() {
            if i > 0 {
                parts.push(sep.clone());
            }
            parts.push(Ir::text(name));
        }
        Ir::Fill(parts.into())
    };
    Ir::concat([
        Ir::text("{"),
        Ir::align(prefix_width + 1, body),
        Ir::text("}"),
    ])
}

/// The text between a `BRACE_GROUP`'s outer `{ }`. The piece parsed cleanly, so the
/// braces are present and balanced; the fallback (`unwrap_or`) is defensive.
fn brace_inner(piece: &SyntaxNode) -> String {
    let raw = piece.to_string();
    raw.strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or(&raw)
        .to_string()
}

/// The inner text of a `QUOTED` piece when it is safe to reflow as a braced value:
/// `normalize` is set and the content's braces are balanced (the same SAFE condition
/// as [`lower_quoted`]). Returns `None` otherwise, so the caller leaves the value
/// byte-exact.
fn quoted_inner_if_safe(piece: &SyntaxNode, normalize: bool) -> Option<String> {
    let raw = piece.to_string();
    let inner = raw
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))?;
    (normalize && braces_balanced(inner)).then(|| inner.to_string())
}

/// Split prose into reflowable words at **brace-/math-depth-0 whitespace runs**. A
/// token that spans a `{…}` group or a `$…$` math span stays one unbreakable atom, so
/// inner braces never straddle a wrap and math is never broken mid-formula. Every
/// whitespace run (space, newline, or newline+indent) collapses to a single break, so
/// a value the formatter already wrapped re-splits into the identical word list
/// (idempotence). `\{`, `\}`, and `\$` are escaped and do not change depth.
fn split_brace_aware(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut brace_depth: i32 = 0;
    let mut in_math = false;
    let mut escaped = false;
    for ch in s.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => {
                cur.push(ch);
                escaped = true;
            }
            c if c.is_whitespace() && brace_depth == 0 && !in_math => {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            }
            '{' => {
                brace_depth += 1;
                cur.push(ch);
            }
            '}' => {
                brace_depth -= 1;
                cur.push(ch);
            }
            '$' => {
                in_math = !in_math;
                cur.push(ch);
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Split a `Name` value into whole names at top-level ` and ` boundaries. Tokenizes
/// with [`split_brace_aware`] (so an "and" inside braces — `{Barnes and Noble}` — is
/// part of one atom and never a separator), then groups the words into names,
/// closing the current name at each standalone `and` token. Each name's words rejoin
/// with single spaces, normalizing source hard-wraps inside the list. An empty
/// segment (leading/trailing/duplicate ` and `) is dropped.
fn split_top_level_and(s: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for word in split_brace_aware(s) {
        if word == "and" {
            if !cur.is_empty() {
                names.push(cur.join(" "));
                cur.clear();
            }
        } else {
            cur.push(word);
        }
    }
    if !cur.is_empty() {
        names.push(cur.join(" "));
    }
    names
}

/// `@string{name = value}` on a single line. Both the macro name and the value are
/// preserved as authored — the value is *not* quote→brace normalized, since `@string`
/// definitions conventionally use quotes and rebracing them reads oddly (e.g.
/// `@string{jan = "January"}` stays quoted rather than becoming `{January}`).
fn lower_string_entry(entry: &SyntaxNode) -> Ir {
    let etype = ast::entry_type(entry).unwrap_or_default().to_lowercase();
    let (open, close) = entry_delimiters(entry);

    // A comment has no line of its own in the single-line `@string` layout, so an
    // entry carrying one is preserved as authored rather than losing it.
    let Some(field) = ast::fields(entry).next().filter(|_| !has_comment(entry)) else {
        return Ir::verbatim(entry.to_string());
    };
    let name = ast::field_name(&field).unwrap_or_default();
    let value = match ast::field_value(&field) {
        Some(value) => lower_value(&value, false),
        None => Ir::nil(),
    };

    Ir::concat([
        Ir::text(format!("@{etype}{open}{name} = ")),
        value,
        Ir::text(close.to_string()),
    ])
}

/// `@preamble{value}` on a single line. The value is preserved as authored (not
/// quote→brace normalized): a quoted preamble such as `"\newcommand{\x}{}"` stays
/// quoted rather than becoming the visually odd double-braced `{{\newcommand{\x}{}}}`.
fn lower_preamble_entry(entry: &SyntaxNode) -> Ir {
    let etype = ast::entry_type(entry).unwrap_or_default().to_lowercase();
    let (open, close) = entry_delimiters(entry);

    // As in `lower_string_entry`: a comment would have nowhere to go on one line.
    let Some(value) = entry
        .children()
        .find(|n| n.kind() == SyntaxKind::VALUE)
        .filter(|_| !has_comment(entry))
    else {
        return Ir::verbatim(entry.to_string());
    };
    let value = lower_value(&value, false);

    Ir::concat([
        Ir::text(format!("@{etype}{open}")),
        value,
        Ir::text(close.to_string()),
    ])
}

/// A field value: its `LITERAL` / `QUOTED` / `BRACE_GROUP` pieces joined by ` # `
/// (concatenation structure preserved, spacing normalized). `normalize` enables the
/// quote→brace rewrite of `QUOTED` pieces; callers pass `false` to leave a value's
/// delimiters exactly as authored (`@string` / `@preamble` values).
fn lower_value(value: &SyntaxNode, normalize: bool) -> Ir {
    let pieces = value
        .children()
        .filter(|piece| {
            matches!(
                piece.kind(),
                SyntaxKind::LITERAL | SyntaxKind::QUOTED | SyntaxKind::BRACE_GROUP
            )
        })
        .map(|piece| lower_value_piece(&piece, normalize));
    Ir::join(Ir::text(" # "), pieces)
}

fn lower_value_piece(piece: &SyntaxNode, normalize: bool) -> Ir {
    match piece.kind() {
        // A bare macro reference or number — never wrapped (wrapping would change
        // its meaning). It is a single token, so it carries no newline.
        SyntaxKind::LITERAL => Ir::text(piece.to_string()),
        // An already-braced group: emitted byte-exact (may span lines).
        SyntaxKind::BRACE_GROUP => Ir::verbatim(piece.to_string()),
        // A quoted string: normalized to braces when safe, else left quoted.
        SyntaxKind::QUOTED => lower_quoted(piece, normalize),
        _ => Ir::verbatim(piece.to_string()),
    }
}

/// Lower a `QUOTED` value piece, rewriting `"…"` → `{…}` when `normalize` is set and
/// the conversion is safe. SAFE requires the quoted content's braces to be balanced,
/// so the result is a well-formed group with identical meaning (`"` is an ordinary
/// character inside braces, so any inner `"` survives). Otherwise — and whenever
/// `normalize` is `false` (verbatim fields, `@string` / `@preamble`) — the piece is
/// emitted verbatim.
fn lower_quoted(piece: &SyntaxNode, normalize: bool) -> Ir {
    let raw = piece.to_string();
    if normalize
        && let Some(inner) = raw
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
        && braces_balanced(inner)
    {
        return Ir::verbatim(format!("{{{inner}}}"));
    }
    Ir::verbatim(raw)
}

/// Whether `s` contains a `%` that LaTeX would read as starting a comment — i.e. one
/// not escaped as `\%`. A preceding `\\` is itself an escaped backslash, so the `%` in
/// `\\%` is live.
fn has_unescaped_percent(s: &str) -> bool {
    let mut escaped = false;
    for ch in s.chars() {
        match ch {
            '%' if !escaped => return true,
            '\\' => escaped = !escaped,
            _ => escaped = false,
        }
    }
    false
}

/// Whether every `{` in `s` is matched by a later `}` and vice versa. Mirrors the
/// parser's structural brace counting, so a piece that parsed cleanly inside a
/// balanced quote is recognized as balanced here too.
fn braces_balanced(s: &str) -> bool {
    let mut depth: i32 = 0;
    for ch in s.chars() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

#[cfg(test)]
mod tests {
    use super::{split_brace_aware, split_top_level_and};

    #[test]
    fn splits_prose_at_depth_zero_whitespace() {
        assert_eq!(split_brace_aware("A title here"), ["A", "title", "here"]);
    }

    #[test]
    fn collapses_every_whitespace_run() {
        // Spaces, a newline, and a newline+indent all collapse to one break, so a
        // re-read of an already-wrapped value yields the identical word list.
        assert_eq!(
            split_brace_aware("A  title\nthat\n  wraps"),
            ["A", "title", "that", "wraps"]
        );
    }

    #[test]
    fn glues_braced_and_math_spans() {
        // A `{…}` group and a `$…$` span never split, so inner braces and math stay
        // intact across a wrap.
        assert_eq!(
            split_brace_aware("a {Protected group} b"),
            ["a", "{Protected group}", "b"]
        );
        assert_eq!(split_brace_aware("x $a + b$ y"), ["x", "$a + b$", "y"]);
    }

    #[test]
    fn escapes_do_not_change_depth() {
        // `\{` / `\}` / `\$` are literal, not delimiters: the spaces around them still
        // split.
        assert_eq!(split_brace_aware(r"a \{ b"), ["a", r"\{", "b"]);
        assert_eq!(split_brace_aware(r"a \$ b"), ["a", r"\$", "b"]);
    }

    #[test]
    fn splits_names_at_top_level_and() {
        assert_eq!(
            split_top_level_and("John Doe and Jane Smith"),
            ["John Doe", "Jane Smith"]
        );
    }

    #[test]
    fn protects_braced_and() {
        // An "and" inside braces is part of one corporate name, not a separator.
        assert_eq!(
            split_top_level_and("{Barnes and Noble} and Jane Public"),
            ["{Barnes and Noble}", "Jane Public"]
        );
    }

    #[test]
    fn drops_empty_name_segments_and_normalizes_spacing() {
        // A trailing ` and ` leaves an empty segment (dropped); intra-name source
        // wraps collapse to single spaces.
        assert_eq!(
            split_top_level_and("Knuth, Donald\n  E. and Lamport and "),
            ["Knuth, Donald E.", "Lamport"]
        );
    }

    #[test]
    fn single_name_stays_whole() {
        assert_eq!(
            split_top_level_and("Knuth, Donald E."),
            ["Knuth, Donald E."]
        );
    }
}

#[cfg(test)]
mod suppression_tests {
    use super::*;

    fn assert_format(source: &str, expected: &str) {
        let style = FormatStyle {
            line_ending: LineEnding::Lf,
            ..FormatStyle::default()
        };
        assert_eq!(format_with_style(source, style).unwrap(), expected);
        assert_eq!(format_with_style(expected, style).unwrap(), expected);
    }

    #[test]
    fn skip_file_preserves_all_bytes_on_both_axes() {
        for family in ["badness-format", "badness"] {
            let source = format!("\r\n@MISC{{z}}\r\n@comment{{{family} skip-file}}  ");
            assert_format(&source, &source);
        }
    }

    #[test]
    fn skip_file_reads_only_the_structured_comment_body() {
        let source = "@comment % (note)\n{badness-format skip-file}\n@MISC{z}";
        assert_format(source, source);
        assert_format(
            "@comment % {badness-format skip-file: example}\n(ordinary)\n@MISC{z}",
            "@comment % {badness-format skip-file: example}\n(ordinary)\n\n@misc{z}\n",
        );
    }

    #[test]
    fn auto_uses_the_first_break_in_a_leading_protected_region() {
        let protected = "@comment{badness-format off}\r\n@MISC{z}\r\n@comment{badness-format on}";
        let source = format!("\n{protected}\n@MISC{{a}}");
        let expected = format!("{protected}\r\n\r\n@misc{{a}}\r\n");
        let style = FormatStyle {
            line_ending: LineEnding::Auto,
            ..FormatStyle::default()
        };
        let formatted = format_with_style(&source, style).unwrap();
        assert_eq!(formatted, expected);
        assert_eq!(format_with_style(&formatted, style).unwrap(), formatted);
    }

    #[test]
    fn skip_pins_the_next_entry_through_stacked_comments() {
        assert_format(
            "@comment{badness-format skip}\n@comment{ordinary}\n@MISC{z}\n@MISC{b}\n@MISC{a}",
            "@comment{badness-format skip}\n\n@comment{ordinary}\n\n@MISC{z}\n\n@misc{a}\n\n@misc{b}\n",
        );
    }

    #[test]
    fn regions_preserve_trivia_and_track_each_family() {
        let protected = concat!(
            "@comment{badness-format off}\r\n",
            "@MISC{z}  \r\n\t",
            "@comment{badness off}\r\n",
            "@MISC{b} @comment{badness-format on}\r\n",
            "@MISC{a}\r\n",
            "@comment{badness on}",
        );
        assert_format(
            &format!("@MISC{{before}}\n{protected}\n@MISC{{after}}"),
            &format!("@misc{{before}}\n\n{protected}\n\n@misc{{after}}\n"),
        );
    }

    #[test]
    fn open_region_preserves_the_end_of_the_file() {
        let protected = "@comment{badness-format off}\r\n@MISC{z}  ";
        assert_format(
            &format!("@MISC{{before}}\n{protected}"),
            &format!("@misc{{before}}\n\n{protected}"),
        );
    }

    #[test]
    fn skip_preserves_junk_without_absorbing_new_separators() {
        assert_format(
            "@comment{badness-format skip}\nforeign text \r\n @MISC{z}",
            "@comment{badness-format skip}\n\nforeign text \r\n @misc{z}\n",
        );
    }
}
