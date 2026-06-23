//! Shared CST edits for bib lint autofixes.
//!
//! [`field_deletion_fix`] builds a single contiguous deletion that removes one
//! `FIELD` from its entry. Both `empty-field` (the field carries no data) and
//! `duplicate-field` (an identical repeat) delete a whole field, and the
//! byte-range arithmetic is the same, so it lives here once rather than in each
//! rule. The edit is judged on correctness (parses + lossless), not layout; it
//! happens to leave already-formatted input formatted, but that is incidental,
//! not required.

use crate::bib::ast::{cite_key, field_name, fields};
use crate::bib::syntax::{SyntaxKind, SyntaxNode};
use crate::linter::Fix;

/// Build a deletion [`Fix`] that removes `field` from its entry, labeled
/// `description`, or `None` when the edit must be **withheld**:
///
/// - the field's parent is not an `ENTRY` (e.g. a `@string`'s `name = value` lives
///   in `STRING_ENTRY`), or the field is not among its siblings;
/// - removing the field would change the entry's `=` alignment — its name is the
///   strict-unique longest, so the formatter would re-pad every sibling, which a
///   single contiguous edit cannot express. (A *duplicate* field never trips this:
///   the kept occurrence shares its name width, so the max is always tied.)
///
/// The deletion is computed from real CST byte ranges, so it works on messy input;
/// on already-formatted input it also leaves formatted output:
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

    // Withhold when removing this field would change the `=` alignment: if its name
    // is the strict-unique longest, every sibling's padding (and any wrapped value's
    // hanging indent) shifts, which a single contiguous edit cannot express.
    let this_len = field_name(field).unwrap_or_default().chars().count();
    let others_max = siblings
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != index)
        .filter_map(|(_, f)| field_name(f))
        .map(|n| n.to_lowercase().chars().count())
        .max();
    if let Some(max) = others_max
        && this_len > max
    {
        return None;
    }

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
