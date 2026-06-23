//! Shared CST edits for bib lint autofixes.
//!
//! [`field_deletion_fix`] builds a single contiguous deletion that removes one
//! `FIELD` from its entry. Both `empty-field` (the field carries no data) and
//! `duplicate-field` (an identical repeat) delete a whole field, and the
//! byte-range arithmetic is the same, so it lives here once rather than in each
//! rule. The edit is judged on correctness (parses + lossless), not layout: it
//! never withholds to preserve `=` alignment and never runs the formatter,
//! leaving any re-padding to the formatter (tenet 1, fix-then-format).

use crate::bib::ast::{cite_key, fields};
use crate::bib::syntax::{SyntaxKind, SyntaxNode};
use crate::linter::Fix;

/// Build a deletion [`Fix`] that removes `field` from its entry, labeled
/// `description`, or `None` when the field's parent is not an `ENTRY` (e.g. a
/// `@string`'s `name = value` lives in `STRING_ENTRY`), or the field is not among
/// its siblings.
///
/// Deleting the strict-unique-longest field shifts the entry's `=` alignment, but
/// the fix does **not** withhold for that: layout is the formatter's job (tenet 1,
/// fix-then-format), and the linter never runs the formatter. The deletion is a
/// pure byte-range edit — correct (parses + lossless) by construction, even when
/// it leaves `=` columns the formatter will re-pad.
///
/// The deletion is computed from real CST byte ranges, so it works on messy input;
/// on already-formatted input that has no alignment to re-pad it also leaves
/// formatted output:
/// - **only field:** delete from the key's end to the closing delimiter, collapsing
///   to the fieldless form `@type{key}`;
/// - **last field:** delete from the previous field's end through this field
///   (dropping the previous field's now-trailing comma);
/// - **otherwise:** delete from this field's start to the next field's start
///   (dropping this field's comma and the line break before the next field).
pub fn field_deletion_fix(field: &SyntaxNode, description: String) -> Option<Fix> {
    let entry = field.parent()?;
    if entry.kind() != SyntaxKind::ENTRY {
        // `@string`'s `name = value` lives in STRING_ENTRY; not an entry field.
        return None;
    }
    let siblings: Vec<SyntaxNode> = fields(&entry).collect();
    let index = siblings.iter().position(|f| f == field)?;

    let (start, end) = if siblings.len() == 1 {
        // Only field: collapse to the fieldless single-line form `@type{key}`. Delete
        // from the key's end (dropping the post-key comma) to the closing delimiter.
        let (_, key_range) = cite_key(&entry)?;
        let close = entry
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| matches!(t.kind(), SyntaxKind::R_BRACE | SyntaxKind::R_PAREN))
            .last()?;
        (
            usize::from(key_range.end()),
            usize::from(close.text_range().start()),
        )
    } else if index + 1 == siblings.len() {
        // Last field: delete from the previous field's end through this field, which
        // drops the previous field's now-trailing comma. Stop at the field's last
        // non-trivia token: a last field's value absorbs the trailing newline before
        // the closer, which must survive so the `}` stays on its own line.
        let prev = &siblings[index - 1];
        (usize::from(prev.text_range().end()), content_end(field))
    } else {
        // Otherwise: delete from this field's start to the next field's start, which
        // drops this field's comma and the line break before the next field.
        let next = &siblings[index + 1];
        (
            usize::from(field.text_range().start()),
            usize::from(next.text_range().start()),
        )
    };
    Some(Fix::safe(start, end, "", description))
}

/// The byte offset just past the field's last non-trivia token, excluding any
/// trailing whitespace/newline the value absorbed.
fn content_end(field: &SyntaxNode) -> usize {
    field
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !matches!(t.kind(), SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE))
        .map(|t| usize::from(t.text_range().end()))
        .max()
        .unwrap_or_else(|| usize::from(field.text_range().end()))
}
