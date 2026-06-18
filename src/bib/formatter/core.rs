//! The BibTeX formatter entry points and the CST → [`Ir`] lowering.
//!
//! The bib analog of [`crate::formatter::core`]. It reuses the shared,
//! language-agnostic Wadler engine ([`crate::formatter::ir`] +
//! [`crate::formatter::printer`], both EXTRACTION CANDIDATEs) and supplies a
//! bib-specific lowering — the only genuinely different part, exactly as on the
//! LaTeX side.
//!
//! The style is deterministic (Tenet 1) and currently fixed (no bib-specific
//! options):
//! - **Case:** entry types and field names are lowercased; cite keys are preserved
//!   verbatim (case-sensitive identifiers); `@string` macro names are preserved
//!   (BibTeX references them case-insensitively, so changing case is cosmetic).
//! - **Layout:** one field per line, the field block indented one `indent_width`
//!   step, the opening delimiter trailing the header and the closing delimiter
//!   flush; exactly one blank line between top-level blocks.
//! - **`=` alignment:** field names are padded to the entry's longest field-name
//!   width so the `=` signs line up. The padding is a pure function of the entry's
//!   (lowercased) names, so it is recomputed identically on every run (idempotent).
//! - **Value delimiters:** in a regular field, a top-level `"…"` value piece is
//!   rewritten to `{…}` when safe (balanced inner braces). A bare `LITERAL` (a macro
//!   reference or a number) is **never** wrapped — that would change its meaning —
//!   and a `Verbatim`-category value (`url`, `doi`, `eprint`, `file`) is never
//!   reshaped. `@string` and `@preamble` values are also left as authored (their
//!   conventional quoting is kept). `#` concatenation structure is preserved,
//!   normalized to ` # `.
//! - **Trailing comma:** none after the last field.
//!
//! Protected regions are emitted byte-exact: `@comment` bodies and inter-entry
//! `JUNK` are spliced through untouched (only the blank-line spacing *around* them
//! is normalized), the same sanctioned exception as LaTeX verbatim/comments.
//!
//! Like the LaTeX formatter, this refuses any input the parser flagged — parsing is
//! the parser's job, and the formatter never reshapes around a parse error.

use crate::bib::ast;
use crate::bib::parse;
use crate::bib::semantic::{BibFieldDb, FieldCategory, builtin};
use crate::bib::syntax::{SyntaxKind, SyntaxNode};
use crate::formatter::ir::Ir;
use crate::formatter::printer::Printer;
use crate::formatter::style::FormatStyle;

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

    let mut formatted = format_root(root, style);
    // Normalize the document's trailing edge: drop trailing blank lines and
    // per-line trailing whitespace at EOF, then guarantee exactly one final
    // newline. Empty output stays empty. Copied verbatim from the LaTeX formatter
    // (`crate::formatter::core::format_node`) for an identical EOF guarantee.
    let trimmed_len = formatted.trim_end_matches([' ', '\t', '\n', '\r']).len();
    formatted.truncate(trimmed_len);
    if !formatted.is_empty() {
        formatted.push('\n');
    }
    Ok(formatted)
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
    let cx = Lower { db: builtin() };
    let ir = lower_root(root, cx);
    Printer::new(style).print(&ir)
}

/// State threaded through the lowering: the built-in field/entry DB (consulted for
/// field categories). `Copy`, like the LaTeX `LowerCtx`. There is no `wrap` field —
/// paragraph wrapping is a LaTeX concept; bib values are not reflowed in v1.
#[derive(Clone, Copy)]
struct Lower {
    db: &'static BibFieldDb,
}

/// Lower the `ROOT`: each top-level block (entry / `@string` / `@preamble` /
/// `@comment` / junk) on its own, separated by exactly one blank line. Bare ROOT
/// trivia tokens are discarded — blank-line separation is re-emitted deterministically.
fn lower_root(root: &SyntaxNode, cx: Lower) -> Ir {
    let blocks = root.children().map(|node| lower_block(&node, cx));
    Ir::join(Ir::empty_line(), blocks)
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

    let fields: Vec<SyntaxNode> = ast::fields(entry).collect();
    let names: Vec<String> = fields
        .iter()
        .map(|field| ast::field_name(field).unwrap_or_default().to_lowercase())
        .collect();

    // A keyless and/or fieldless entry stays on a single line: `@misc{key}`.
    if fields.is_empty() {
        return Ir::text(format!("@{etype}{open}{key}{close}"));
    }

    let width = names
        .iter()
        .map(|name| name.chars().count())
        .max()
        .unwrap_or(0);
    let header = Ir::text(format!("@{etype}{open}{key},"));

    let lines = fields.iter().enumerate().map(|(i, field)| {
        let last = i + 1 == fields.len();
        lower_field(field, &names[i], width, last, cx)
    });
    let body = Ir::concat([Ir::hard_line(), Ir::join(Ir::hard_line(), lines)]);

    Ir::concat([
        header,
        Ir::indent(body),
        Ir::hard_line(),
        Ir::text(close.to_string()),
    ])
}

/// One `name = value` field line. `name_lc` is the lowercased field name; `width` is
/// the entry's longest field-name width, used to pad so the `=` signs align. A
/// trailing comma is emitted on every field except the `last`.
fn lower_field(field: &SyntaxNode, name_lc: &str, width: usize, last: bool, cx: Lower) -> Ir {
    let pad = " ".repeat(width - name_lc.chars().count());
    let prefix = Ir::text(format!("{name_lc}{pad} = "));

    // Regular fields normalize quotes → braces, except `Verbatim`-category fields
    // (url/doi/…), whose value is never reshaped.
    let normalize = cx.db.category(name_lc) != FieldCategory::Verbatim;
    let value = match ast::field_value(field) {
        Some(value) => lower_value(&value, normalize),
        // Defensive: a clean field always has a value; a recovery one is refused.
        None => Ir::nil(),
    };

    let comma = if last { Ir::nil() } else { Ir::text(",") };
    Ir::concat([prefix, value, comma])
}

/// `@string{name = value}` on a single line. Both the macro name and the value are
/// preserved as authored — the value is *not* quote→brace normalized, since `@string`
/// definitions conventionally use quotes and rebracing them reads oddly (e.g.
/// `@string{jan = "January"}` stays quoted rather than becoming `{January}`).
fn lower_string_entry(entry: &SyntaxNode) -> Ir {
    let etype = ast::entry_type(entry).unwrap_or_default().to_lowercase();
    let (open, close) = entry_delimiters(entry);

    let Some(field) = ast::fields(entry).next() else {
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

    let Some(value) = entry.children().find(|n| n.kind() == SyntaxKind::VALUE) else {
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
