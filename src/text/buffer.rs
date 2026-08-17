//! The language server's live document buffer.
//!
//! A keystroke moves a document's text through several hands: the buffer the
//! `didChange` splices, the worker job that writes it into salsa, and every
//! read job the editor fires off the same edit (diagnostics, symbols, folding,
//! completion, hover…). Each of those only ever *reads* the text, and each of
//! the positional ones needs the same byte-offset ↔ position table over it.
//!
//! [`TextBuffer`] is what they share: the text is an immutable [`Arc<str>`], so
//! handing it to a job or to the salsa layer is a refcount bump, and the
//! [`LineIndex`] is built at most once per document version, on whichever
//! thread asks first.

use std::ops::{Deref, Range};
use std::sync::{Arc, OnceLock};

use super::line_index::{LineIndex, PositionEncoding};

/// An immutable snapshot of a document's text, plus the position index over it.
///
/// Immutable on purpose: an edit yields a *new* buffer
/// ([`with_replacement`](Self::with_replacement)) rather than mutating this one,
/// so a job that captured the previous version keeps reading a consistent text
/// and index without a lock. The text has to be rebuilt around a splice anyway —
/// an `Arc<str>` cannot be grown in place — so the immutability costs nothing
/// the edit was not already paying.
///
/// Derefs to `str`, so everything that just wants the text — the parser, the
/// formatter, the linter — takes it unchanged.
#[derive(Debug)]
pub struct TextBuffer {
    text: Arc<str>,
    /// The unit an LSP `Position.character` counts in, negotiated once at
    /// `initialize` and therefore fixed for every buffer in a session. Held
    /// here so [`line_index`](Self::line_index) can hand out *the* index for
    /// this document rather than one per caller's idea of the encoding.
    encoding: PositionEncoding,
    /// Built on first use and shared from there on. A document nobody asks a
    /// positional question about — the common case for a `.tex` file being
    /// typed into faster than the editor re-queries — never pays for one.
    index: OnceLock<LineIndex>,
}

impl TextBuffer {
    /// A buffer over `text`, answering positions in `encoding`.
    pub fn new(text: impl Into<Arc<str>>, encoding: PositionEncoding) -> Self {
        Self {
            text: text.into(),
            encoding,
            index: OnceLock::new(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The text as a shared handle: an O(1) clone, for the salsa boundary and
    /// anything else that stores the document rather than borrowing it.
    ///
    /// Handing salsa *this* handle is what lets
    /// [`text_is_current`](crate::incremental::Analysis::text_is_current)
    /// settle a read job's staleness check by pointer.
    pub fn text_arc(&self) -> Arc<str> {
        Arc::clone(&self.text)
    }

    pub fn encoding(&self) -> PositionEncoding {
        self.encoding
    }

    /// The position index over this buffer, built once and shared. Call it
    /// freely: unlike [`LineIndex::with_encoding`] it does not rescan.
    pub fn line_index(&self) -> &LineIndex {
        self.index
            .get_or_init(|| LineIndex::with_encoding(&self.text, self.encoding))
    }

    /// The buffer that results from replacing the bytes in `range` with
    /// `insert` — the `didChange` splice.
    ///
    /// Panics on a reversed range, one that is out of bounds, or one off a char
    /// boundary, as [`String::replace_range`] does.
    pub fn with_replacement(&self, range: Range<usize>, insert: &str) -> Self {
        // Slicing the removed region up front is what reproduces
        // `String::replace_range`'s panics: the arithmetic below cannot stand in
        // for it, since a reversed range measures zero and would silently
        // duplicate `end..start` while an out-of-bounds one underflows.
        let removed = self.text[range.clone()].len();
        let mut new = String::with_capacity(self.text.len() - removed + insert.len());
        new.push_str(&self.text[..range.start]);
        new.push_str(insert);
        new.push_str(&self.text[range.end..]);
        Self::new(new, self.encoding)
    }
}

impl Deref for TextBuffer {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str) -> TextBuffer {
        TextBuffer::new(text, PositionEncoding::Utf16)
    }

    #[test]
    fn the_index_is_built_once_and_shared() {
        let buf = buffer("ab\ncd\nef");
        let first = buf.line_index() as *const LineIndex;
        assert_eq!(buf.line_index().line_start(1), 3);
        assert!(std::ptr::eq(buf.line_index(), first));
    }

    #[test]
    fn the_index_answers_in_the_buffers_encoding() {
        // "𝕏" is 4 UTF-8 bytes and 2 UTF-16 units, so the encodings disagree
        // about the column just past it.
        let utf16 = TextBuffer::new("a𝕏b", PositionEncoding::Utf16);
        let utf8 = TextBuffer::new("a𝕏b", PositionEncoding::Utf8);
        let off = "a𝕏".len();
        assert_eq!(utf16.line_index().position(off), (0, 3));
        assert_eq!(utf8.line_index().position(off), (0, 5));
    }

    /// The point of the `Arc<str>` representation: handing the text out shares
    /// one allocation, and an edit yields a new buffer without disturbing
    /// handles taken before it — which is what lets the salsa layer and every
    /// in-flight read job hold the text without copying it.
    #[test]
    fn an_edit_leaves_earlier_handles_alone() {
        let before = buffer("ab\ncd");
        let handle = before.text_arc();
        assert!(Arc::ptr_eq(&handle, &before.text_arc()));

        let after = before.with_replacement(2..2, "\nxy");
        assert_eq!(&*handle, "ab\ncd");
        assert_eq!(after.text(), "ab\nxy\ncd");
        assert!(!Arc::ptr_eq(&handle, &after.text_arc()));
        assert_eq!(after.line_index().line_start(1), 3);
    }

    /// A malformed range must panic where [`String::replace_range`] would.
    /// Rebuilding the text around the splice can no longer rely on the string
    /// machinery to reject one, and the arithmetic that replaced it accepts
    /// both shapes below: a reversed range measures zero and would duplicate
    /// the region it names, an out-of-bounds one underflows.
    #[test]
    #[should_panic(expected = "byte range starts at 4 but ends at 2")]
    #[expect(
        clippy::reversed_empty_ranges,
        reason = "the malformed range is the subject of the test"
    )]
    fn a_reversed_edit_range_panics() {
        buffer("abcdefgh").with_replacement(4..2, "Z");
    }

    #[test]
    #[should_panic(expected = "out of bounds")]
    fn an_out_of_bounds_edit_range_panics() {
        buffer("abcdefgh").with_replacement(0..99, "Z");
    }

    #[test]
    #[should_panic(expected = "not a char boundary")]
    fn an_edit_range_off_a_char_boundary_panics() {
        buffer("\u{1F600}x").with_replacement(1..2, "Z");
    }
}
