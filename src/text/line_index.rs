//! Byte-offset → line/column conversion.
//!
//! Kept free of any LSP type dependency: it exposes a 1-indexed **code-point**
//! [`LineCol`] for CLI diagnostics and a 0-indexed `(line, character)` pair for
//! LSP positions, counted in the [`PositionEncoding`] the index was built with
//! (the encoding negotiated at `initialize`). (Marked an extraction candidate
//! in `AGENTS.md`.)
//!
//! The tables and the queries are *separate types*, because the language server
//! maintains its tables across an edit rather than rescanning:
//! [`LineTable`] is the value a [`TextBuffer`](super::TextBuffer) keeps and
//! [patches](LineTable::patch) per keystroke, and [`LineIndex`] is the
//! short-lived pairing of a text with a table that answers questions about it.
//! Building the view over a maintained table is free, so a handler that has one
//! should call it as often as it likes.
//!
//! A query reads the text, so the tables carry no per-character data: a line is
//! flagged for containing any non-ASCII byte and the one line concerned is
//! walked on demand. Precomputing every wide character instead — the shape this
//! module had — cost more to build than every conversion it ever answered, and
//! it is the shape that cannot be patched cheaply, since a hash table keyed by
//! line number has to be rekeyed wholesale when the line count moves.
//!
//! The one hazard the split introduces is [`LineIndex::with_table`], which is
//! the single point where a text and a table are paired: hand it a table built
//! for different bytes and it answers wrong positions rather than panicking.

use std::borrow::Cow;
use std::ops::Range;

/// A 1-indexed line/column, with the column counted in Unicode code points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    /// 1-indexed line number.
    pub line: usize,
    /// 1-indexed column in code points (not bytes, not UTF-16 units).
    pub column: usize,
}

/// How an LSP `Position.character` counts columns within a line — the position
/// encoding negotiated at `initialize` from the client's
/// `general.positionEncodings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PositionEncoding {
    /// `character` counts UTF-8 code units (bytes). Preferred when the client
    /// offers it: a column is then a plain byte distance, no per-line re-count.
    Utf8,
    /// `character` counts UTF-16 code units — the protocol-mandatory default
    /// every client supports.
    #[default]
    Utf16,
}

/// Whether the terminator byte at `at` — which the caller has established is
/// `\n` or `\r` — ends a line, making `at + 1` a line start.
///
/// A `\n` always does. A `\r` does *unless* it opens a `\r\n`, which is one
/// terminator, not two: that break is reported by the `\n`, at the offset the
/// `\r` would otherwise have claimed two bytes earlier. A bare `\r` — including
/// one at the end of the text, where there is no following byte — breaks on its
/// own.
///
/// Reading *two* bytes is what makes an edit's boundary positions
/// undetermined rather than merely shifted, which is the whole shape of
/// [`LineTable::patch`].
fn ends_line(bytes: &[u8], at: usize) -> bool {
    bytes[at] == b'\n' || bytes.get(at + 1) != Some(&b'\n')
}

/// The line structure of a text: where each line starts, and which lines hold
/// anything but ASCII.
///
/// A value of its own, separate from the text, so the language server's live
/// buffer can [patch](Self::patch) it across an edit rather than rescanning.
/// Scanning is linear in the document, which on a large file costs many times
/// the incremental reparse the edit goes on to trigger (`benches/keystroke.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineTable {
    /// Byte offset of the first character of each line (0-indexed). Always
    /// starts with `0`.
    line_starts: Vec<usize>,
    /// Per line, whether it contains any non-ASCII byte — parallel to
    /// `line_starts`. A clear flag means a column is a byte distance in every
    /// encoding, which is the O(1) path this array exists to preserve: a
    /// semantic-tokens response converts one position per token.
    wide_lines: Vec<bool>,
}

impl LineTable {
    /// Scan `text` for its line structure.
    pub fn new(text: &str) -> Self {
        let bytes = text.as_bytes();
        // 40 bytes per line is a rough fit for hard-wrapped LaTeX; the scan
        // itself is `memchr`'s, which vectorizes where a byte loop cannot.
        let mut line_starts = Vec::with_capacity(text.len() / 40 + 1);
        line_starts.push(0);
        line_starts.extend(
            memchr::memchr2_iter(b'\n', b'\r', bytes)
                .filter(|&at| ends_line(bytes, at))
                .map(|at| at + 1),
        );
        let mut table = Self {
            wide_lines: vec![false; line_starts.len()],
            line_starts,
        };
        // One vectorized check for the whole document settles every line at once
        // when it passes, which for LaTeX source it almost always does. The
        // per-line loop is a call per line — ~10 ns on a document that fits in
        // L2 and ~22 ns on one that does not, which on the 730 KB thesis was the
        // entire cost of building the table (`benches/keystroke.rs`). Nothing is
        // lost on a document that *does* carry non-ASCII: the sweep below then
        // re-reads bytes this pass just read, and both are off the keystroke path
        // once [`patch`](Self::patch) is doing the work.
        if !bytes.is_ascii() {
            for line in 0..table.line_starts.len() {
                let wide = table.is_wide(text, line);
                table.wide_lines[line] = wide;
            }
        }
        table
    }

    /// The byte range of `line` *including* its terminator.
    fn line_span(&self, len: usize, line: usize) -> Range<usize> {
        let start = self.line_starts[line];
        let end = self.line_starts.get(line + 1).copied().unwrap_or(len);
        start..end
    }

    /// Whether `line` holds a non-ASCII byte, read from the text. The
    /// terminators are themselves ASCII, so scanning the span including them is
    /// the same answer as scanning the line's content.
    fn is_wide(&self, text: &str, line: usize) -> bool {
        !text.as_bytes()[self.line_span(text.len(), line)].is_ascii()
    }

    /// Patch the table for a replacement of `range` by `insert_len` bytes,
    /// leaving it exactly as a scan of the edited text would.
    ///
    /// `range` is a byte range in the *pre-edit* text; `new` is the text the
    /// edit produced. Line starts fall into three groups:
    ///
    /// 1. those at or before `range.start - 1` are untouched — both bytes their
    ///    verdict reads sit in the prefix the edit did not move;
    /// 2. those past `range.start + insert_len` keep their verdict and shift by
    ///    the edit's byte delta, one add per line rather than a scan per byte;
    /// 3. those in `range.start ..= range.start + insert_len` are *undetermined*
    ///    and re-derived from `new`.
    ///
    /// Group 3 is what makes this different from a table indexing `\n` alone,
    /// where the predicate reads one byte so a start *at* the edit cannot flip
    /// and the new breaks can be scanned out of the insert. Here it reads two
    /// (see [`ends_line`]), so an edit splits or joins a `\r\n` without touching
    /// either byte: inserting `x` into `"a\r\nb"` at 2 yields `"a\rx\nb"`, which
    /// has a line the pre-edit table had not. Both boundary positions are
    /// therefore re-read out of `new`, never carried across and never inferred
    /// from the insert. The `\n` lookahead reads the whole text rather than the
    /// window, so a `\r` at the window's last byte is still suppressed by a `\n`
    /// in the shifted tail.
    ///
    /// The wide-line flags splice alongside, but the lines the edit *created*
    /// have to be re-derived: the joined line's content comes from the surviving
    /// prefix, the insert, and the surviving suffix together, so the insert
    /// alone cannot answer for it.
    pub fn patch(&mut self, range: Range<usize>, insert_len: usize, new: &str) {
        let Range { start, end } = range;
        debug_assert!(start <= end, "reversed edit range {start}..{end}");
        let bytes = new.as_bytes();
        let delta = insert_len as isize - (end - start) as isize;

        // Offset 0 is a line start by definition rather than by the predicate,
        // so it is never re-derived and never spliced away — hence the `max(1)`.
        // `start <= end` makes `first <= last`, and `last` is at least 1 because
        // `line_starts[0]` is 0.
        let first = self.line_starts.partition_point(|&at| at < start).max(1);
        let last = self.line_starts.partition_point(|&at| at <= end);

        // Group 2. Shifted before the splice, while `last..` still indexes the
        // surviving tail.
        if delta != 0 {
            for at in &mut self.line_starts[last..] {
                *at = at.wrapping_add_signed(delta);
            }
        }

        // Group 3. A start at `p` is decided by the terminator at `p - 1`, so
        // the bytes to test run from `start - 1` up to the last inserted byte.
        // A pure deletion leaves exactly one: the byte before the seam.
        let lo = start.saturating_sub(1);
        let hi = start + insert_len;
        let derived: Vec<usize> = memchr::memchr2_iter(b'\n', b'\r', &bytes[lo..hi])
            .map(|at| lo + at)
            .filter(|&at| ends_line(bytes, at))
            .map(|at| at + 1)
            .collect();
        // Sorted by construction: the kept starts are `< start`, the derived are
        // in `start ..= start + insert_len`, and the shifted are past that.
        let derived_len = derived.len();
        self.line_starts.splice(first..last, derived);
        self.wide_lines
            .splice(first..last, std::iter::repeat_n(false, derived_len));

        // `first` is at least 1, so the line holding the edit's start is
        // `first - 1`, and the lines the edit created are `first` through
        // `first + derived_len - 1`. The first surviving tail line moved by the
        // same delta as the line after it, so its bytes are the bytes it already
        // had and its flag rides the splice untouched.
        for line in (first - 1)..(first + derived_len) {
            let wide = self.is_wide(new, line);
            self.wide_lines[line] = wide;
        }
    }
}

/// A text paired with the table that indexes it, answering positions in one
/// [`PositionEncoding`].
///
/// Cheap to build over a maintained table ([`with_table`](Self::with_table)) and
/// linear in the text otherwise ([`new`](Self::new) /
/// [`with_encoding`](Self::with_encoding)) — prefer the former wherever a table
/// is at hand, which for the live buffer means always
/// ([`TextBuffer::line_index`](super::TextBuffer::line_index)).
#[derive(Debug, Clone)]
pub struct LineIndex<'a> {
    text: &'a str,
    table: Cow<'a, LineTable>,
    /// The column unit [`position`](Self::position)/[`offset_at`](Self::offset_at)
    /// count in. Irrelevant to [`line_col`](Self::line_col) (code points).
    encoding: PositionEncoding,
}

impl<'a> LineIndex<'a> {
    /// An index converting positions in the LSP-default **UTF-16** encoding,
    /// scanning `text` for its table. CLI diagnostics (which only use
    /// [`line_col`](Self::line_col)) use this too; LSP code should build with
    /// the *negotiated* encoding via [`with_encoding`](Self::with_encoding).
    pub fn new(text: &'a str) -> Self {
        Self::with_encoding(text, PositionEncoding::Utf16)
    }

    /// An index over `text`, scanning it for its table.
    pub fn with_encoding(text: &'a str, encoding: PositionEncoding) -> Self {
        Self {
            text,
            table: Cow::Owned(LineTable::new(text)),
            encoding,
        }
    }

    /// An index over `text` reusing an already-built table.
    ///
    /// `table` must be `text`'s own, as
    /// [`TextBuffer`](super::TextBuffer) keeps it: pairing it with different
    /// bytes yields wrong positions rather than a panic. This is the single
    /// place that pairing happens.
    pub fn with_table(text: &'a str, table: &'a LineTable, encoding: PositionEncoding) -> Self {
        Self {
            text,
            table: Cow::Borrowed(table),
            encoding,
        }
    }

    /// 0-indexed line containing `offset`.
    fn line_of(&self, offset: usize) -> usize {
        match self.table.line_starts.binary_search(&offset) {
            Ok(line) => line,
            Err(next) => next - 1,
        }
    }

    /// Byte offset of the start of the 0-indexed `line`. A line past the end
    /// clamps to the buffer end, so `line_start(n)..line_start(n + 1)` is always
    /// a valid slice range covering line `n` *including* its terminator — which
    /// is what distinguishes this from [`offset_at`](Self::offset_at), whose
    /// `character` clamps to the line's content and so stops short of the
    /// newline. The pretty diagnostic renderer slices a snippet window with it.
    pub fn line_start(&self, line: usize) -> usize {
        self.table
            .line_starts
            .get(line)
            .copied()
            .unwrap_or(self.text.len())
    }

    /// End of `line`'s *content*, before its `\n`/`\r\n`/EOF terminator.
    ///
    /// Derived rather than stored: the terminator's length is readable from the
    /// two bytes before the next line's start, so a parallel table of ends would
    /// be one more thing for [`LineTable::patch`] to keep in step for no
    /// information. The `end > start` guard is what keeps an *empty* line from
    /// reaching back past its own start — and, at offset 0, from underflowing.
    fn line_content_end(&self, line: usize) -> usize {
        let start = self.table.line_starts[line];
        let Some(&next) = self.table.line_starts.get(line + 1) else {
            // Only the last line lacks a terminator, and it always does.
            return self.text.len();
        };
        let bytes = self.text.as_bytes();
        let end = next - 1;
        if bytes[end] == b'\n' && end > start && bytes[end - 1] == b'\r' {
            // `\r\n` is one terminator, so it takes two bytes off the content.
            return end - 1;
        }
        end
    }

    /// 1-indexed (line, column-in-code-points) for CLI diagnostics.
    ///
    /// An `offset` that is not on a char boundary counts the code point
    /// containing it, so the column is that code point's ordinal.
    pub fn line_col(&self, offset: usize) -> LineCol {
        let offset = offset.min(self.text.len());
        let line = self.line_of(offset);
        let start = self.table.line_starts[line];
        let points = if self.table.wide_lines[line] {
            self.chars_before(start, offset).count()
        } else {
            // One byte is one code point on an ASCII line.
            offset - start
        };
        LineCol {
            line: line + 1,
            column: points + 1,
        }
    }

    /// 0-indexed (line, character) for LSP positions, with `character` counted
    /// in the index's [`PositionEncoding`].
    pub fn position(&self, offset: usize) -> (u32, u32) {
        let offset = offset.min(self.text.len());
        let line = self.line_of(offset);
        let start = self.table.line_starts[line];
        let byte_col = offset - start;
        let character = match self.encoding {
            // A byte column *is* the answer, wide chars or not.
            PositionEncoding::Utf8 => byte_col,
            PositionEncoding::Utf16 if !self.table.wide_lines[line] => byte_col,
            PositionEncoding::Utf16 => self.chars_before(start, offset).map(char::len_utf16).sum(),
        };
        (line as u32, character as u32)
    }

    /// The code points beginning at or after `start` and before `offset`.
    ///
    /// `start` is a line start and `offset` lies within that line, so this walks
    /// one line and no further — which is what the wide-line flag buys: the
    /// callers above take a byte distance whenever the flag is clear. An offset
    /// inside a code point yields that code point, since its own start is below
    /// the offset.
    fn chars_before(&self, start: usize, offset: usize) -> impl Iterator<Item = char> + '_ {
        self.text[start..]
            .char_indices()
            .take_while(move |&(at, _)| start + at < offset)
            .map(|(_, ch)| ch)
    }

    /// Byte offset of a 0-indexed LSP position (`character` in the index's
    /// [`PositionEncoding`]). The inverse of [`position`](Self::position), used
    /// to splice incremental `didChange` edits into a buffer.
    ///
    /// An out-of-range `line` clamps to the end of the text; a `character` past
    /// the line's content clamps to the line's end (the byte before its trailing
    /// newline, or the text end on the last line). A `character` landing inside a
    /// code point (a UTF-16 surrogate pair, or a UTF-8 multi-byte sequence) snaps
    /// to the end of that code point.
    pub fn offset_at(&self, line: u32, character: u32) -> usize {
        let line = line as usize;
        let Some(&start) = self.table.line_starts.get(line) else {
            return self.text.len();
        };
        // The line spans `[start, content_end)`, excluding the terminator so a
        // position never resolves past the line's own content.
        let content_end = self.line_content_end(line);
        let character = character as usize;
        if !self.table.wide_lines[line] {
            return content_end.min(start + character);
        }
        let mut units = 0usize;
        for (at, ch) in self.text[start..content_end].char_indices() {
            // Checked before advancing, so a target inside a code point falls
            // through to the *next* boundary rather than resolving inside it.
            if units >= character {
                return start + at;
            }
            units += match self.encoding {
                PositionEncoding::Utf8 => ch.len_utf8(),
                PositionEncoding::Utf16 => ch.len_utf16(),
            };
        }
        content_end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_basic() {
        let text = "ab\ncde\n";
        let idx = LineIndex::new(text);
        assert_eq!(idx.line_col(0), LineCol { line: 1, column: 1 });
        assert_eq!(idx.line_col(1), LineCol { line: 1, column: 2 });
        assert_eq!(idx.line_col(3), LineCol { line: 2, column: 1 });
        assert_eq!(idx.line_col(5), LineCol { line: 2, column: 3 });
    }

    #[test]
    fn utf16_counts_surrogates() {
        // "𝕏" (U+1D54F) is 4 UTF-8 bytes and 2 UTF-16 units.
        let text = "a𝕏b";
        let idx = LineIndex::new(text);
        let off = "a𝕏".len(); // byte offset just after the astral char
        assert_eq!(idx.position(off), (0, 3));
    }

    #[test]
    fn utf8_counts_bytes() {
        // The same buffer in UTF-8 encoding: characters are byte distances.
        let text = "a𝕏b";
        let idx = LineIndex::with_encoding(text, PositionEncoding::Utf8);
        let off = "a𝕏".len();
        assert_eq!(idx.position(off), (0, 5));
        assert_eq!(idx.offset_at(0, 5), off);
    }

    #[test]
    fn crlf_line_starts() {
        let text = "a\r\nb";
        let idx = LineIndex::new(text);
        assert_eq!(idx.line_col(3), LineCol { line: 2, column: 1 });
    }

    #[test]
    fn multiple_wide_chars_on_a_line() {
        // Mixed ASCII with a 2-byte (£, 1 UTF-16 unit), a 3-byte (€, 1 unit), and
        // a 4-byte astral (𝕏, 2 units) char — exercises the running sums and the
        // multi-wide-char `offset_at` walk, not just single-char lines.
        let text = "a£b€c𝕏d";
        let off = |s: &str| s.len();
        let after_a = off("a");
        let after_pound = off("a£");
        let after_b = off("a£b");
        let after_euro = off("a£b€");
        let after_c = off("a£b€c");
        let after_astral = off("a£b€c𝕏");
        let after_d = off("a£b€c𝕏d");

        // Code points: a=1, £=1, b=1, €=1, c=1, 𝕏=1, d=1 → columns 1..=7 boundaries.
        let cp = LineIndex::new(text);
        assert_eq!(cp.line_col(after_a).column, 2);
        assert_eq!(cp.line_col(after_pound).column, 3);
        assert_eq!(cp.line_col(after_b).column, 4);
        assert_eq!(cp.line_col(after_euro).column, 5);
        assert_eq!(cp.line_col(after_c).column, 6);
        assert_eq!(cp.line_col(after_astral).column, 7);
        assert_eq!(cp.line_col(after_d).column, 8);

        // UTF-16 units: a=1, £=1, b=1, €=1, c=1, 𝕏=2, d=1.
        let u16 = LineIndex::with_encoding(text, PositionEncoding::Utf16);
        assert_eq!(u16.position(after_a), (0, 1));
        assert_eq!(u16.position(after_pound), (0, 2));
        assert_eq!(u16.position(after_b), (0, 3));
        assert_eq!(u16.position(after_euro), (0, 4));
        assert_eq!(u16.position(after_c), (0, 5));
        assert_eq!(u16.position(after_astral), (0, 7));
        assert_eq!(u16.position(after_d), (0, 8));

        // UTF-8 units are byte distances.
        let u8 = LineIndex::with_encoding(text, PositionEncoding::Utf8);
        assert_eq!(u8.position(after_astral), (0, after_astral as u32));

        // Round-trip every char boundary in every encoding.
        for encoding in [PositionEncoding::Utf16, PositionEncoding::Utf8] {
            let idx = LineIndex::with_encoding(text, encoding);
            for offset in (0..=text.len()).filter(|&o| text.is_char_boundary(o)) {
                let (line, character) = idx.position(offset);
                assert_eq!(
                    idx.offset_at(line, character),
                    offset,
                    "offset {offset} ({encoding:?})"
                );
            }
        }
    }

    #[test]
    fn offset_at_round_trips_positions_in_both_encodings() {
        // Astral char on line 0, LF break, ASCII on line 1. Every char-boundary
        // offset's position must map back to that same offset. (CRLF is excluded
        // here because the byte *between* \r and \n is a terminator interior, not
        // an addressable position — see `offset_at_crlf_terminator` below.)
        let text = "a𝕏b\ncd";
        for encoding in [PositionEncoding::Utf16, PositionEncoding::Utf8] {
            let idx = LineIndex::with_encoding(text, encoding);
            for offset in (0..=text.len()).filter(|&o| text.is_char_boundary(o)) {
                let (line, character) = idx.position(offset);
                assert_eq!(
                    idx.offset_at(line, character),
                    offset,
                    "offset {offset} ({encoding:?})"
                );
            }
        }
    }

    #[test]
    fn offset_at_crlf_terminator() {
        // The line's content ends before \r\n; a column at the line's UTF-16
        // length resolves to just before the \r, never inside the terminator.
        let text = "ab\r\ncd";
        let idx = LineIndex::new(text);
        assert_eq!(idx.offset_at(0, 2), 2); // just after 'b', before '\r'
        assert_eq!(idx.offset_at(1, 0), 4); // start of "cd"
    }

    #[test]
    fn offset_at_clamps_out_of_range() {
        let text = "ab\ncde\n";
        let idx = LineIndex::new(text);
        // A character past the line's content clamps to the line end (before \n).
        assert_eq!(idx.offset_at(0, 99), 2);
        // The empty trailing line.
        assert_eq!(idx.offset_at(2, 0), 7);
        // A line past the end clamps to the text end.
        assert_eq!(idx.offset_at(99, 0), text.len());
    }

    #[test]
    fn offset_at_inside_surrogate_pair_snaps_to_code_point_end() {
        let text = "𝕏";
        let idx = LineIndex::new(text);
        // "𝕏" is 2 UTF-16 units; character 1 lands mid-pair → snaps to its end.
        assert_eq!(idx.offset_at(0, 1), text.len());
    }

    #[test]
    fn offset_at_inside_utf8_sequence_snaps_to_code_point_end() {
        let text = "𝕏";
        let idx = LineIndex::with_encoding(text, PositionEncoding::Utf8);
        // "𝕏" is 4 bytes; character 2 lands mid-sequence → snaps to its end.
        assert_eq!(idx.offset_at(0, 2), text.len());
        // A character past the line's content clamps to the line end.
        assert_eq!(idx.offset_at(0, 99), text.len());
    }

    /// An offset that is not on a char boundary reports the column of the code
    /// point *containing* it. Pinned because no caller can reach it — every
    /// diagnostic offset is a token boundary — and because the table this
    /// replaced answered a byte-ish column there instead (column 3 below), which
    /// was not a code-point column at all.
    #[test]
    fn line_col_inside_a_code_point_counts_that_code_point() {
        let idx = LineIndex::new("𝕏b");
        assert_eq!(idx.line_col(0).column, 1);
        assert_eq!(idx.line_col(2).column, 2);
        assert_eq!(idx.line_col(4).column, 2);
    }

    /// Texts whose line structure is awkward: no terminator, every terminator,
    /// terminators adjacent to each other in both orders, and wide characters
    /// beside them.
    const AWKWARD: &[&str] = &[
        "",
        "\n",
        "\r",
        "\n\n",
        "\r\n",
        "\n\r",
        "\r\r",
        "abc",
        "ab\ncd\nef\n",
        "a\r\nb\r\n",
        "a\rb\n",
        "a\r\r\nb",
        "\u{1F600}\nx\n",
        "café\r\nx",
        "ä",
        "ä\r\nö\rü\n",
    ];

    /// The `char_indices` scan this module used before `memchr`, kept as the
    /// reference the fast one is checked against. It produces the line starts
    /// *and* the per-line content ends the table used to store, so it pins the
    /// two-byte break predicate and the derived
    /// [`LineIndex::line_content_end`] together.
    fn reference_scan(text: &str) -> (Vec<usize>, Vec<usize>) {
        let len = text.len();
        let mut line_starts = vec![0];
        let mut line_ends = Vec::new();
        let bytes = text.as_bytes();
        // Set after a `\r` that begins a `\r\n`, so the following `\n` is not
        // counted as a second break.
        let mut skip_lf = false;
        for (i, ch) in text.char_indices() {
            match ch {
                '\n' if skip_lf => skip_lf = false,
                '\n' => {
                    line_ends.push(i);
                    line_starts.push(i + 1);
                }
                '\r' => {
                    line_ends.push(i);
                    if bytes.get(i + 1) == Some(&b'\n') {
                        line_starts.push(i + 2);
                        skip_lf = true;
                    } else {
                        line_starts.push(i + 1);
                    }
                }
                _ => {}
            }
        }
        line_ends.push(len);
        (line_starts, line_ends)
    }

    fn assert_matches_reference(text: &str, label: &str) {
        let (starts, ends) = reference_scan(text);
        let table = LineTable::new(text);
        assert_eq!(table.line_starts, starts, "line starts of {label}");
        let idx = LineIndex::new(text);
        let derived: Vec<usize> = (0..starts.len()).map(|l| idx.line_content_end(l)).collect();
        assert_eq!(derived, ends, "line content ends of {label}");
    }

    /// The `memchr2` scan and the derived content ends must be the same
    /// functions the stored tables were. There is no compile-time link between
    /// the two-byte predicate and the state machine it replaced, so this is the
    /// only thing that says they agree.
    #[test]
    fn the_scan_matches_the_char_by_char_reference() {
        for text in AWKWARD {
            assert_matches_reference(text, &format!("{text:?}"));
        }
        // A real document, and the same document as a Windows author would have
        // written it: `TODO.md`'s CRLF hazard is that a line-table bug shows up
        // only on the second one.
        let doc = include_str!("../../benches/documents/small.tex");
        assert_matches_reference(doc, "small.tex");
        assert_matches_reference(&doc.replace('\n', "\r\n"), "small.tex as CRLF");
    }

    /// Inserts whose own line structure is awkward, to pair with [`AWKWARD`]:
    /// nothing at all (a pure deletion), no terminator, each terminator, and the
    /// two-byte pair in both orders.
    const INSERTS: &[&str] = &["", "z", "\n", "\r", "\r\n", "\n\r", "\n\n", "x\ny\n", "é"];

    /// Every replacement of every char-boundary range of every awkward text must
    /// leave the table exactly as a rescan would. This is the whole correctness
    /// argument for patching rather than rebuilding, so it is checked
    /// exhaustively rather than by example, and the equality is over the whole
    /// table — the wide-line flags included.
    #[test]
    fn patching_matches_a_rescan() {
        for text in AWKWARD {
            for start in (0..=text.len()).filter(|&at| text.is_char_boundary(at)) {
                for end in (start..=text.len()).filter(|&at| text.is_char_boundary(at)) {
                    for insert in INSERTS {
                        let mut edited = text.to_string();
                        edited.replace_range(start..end, insert);

                        let mut patched = LineTable::new(text);
                        patched.patch(start..end, insert.len(), &edited);

                        assert_eq!(
                            patched,
                            LineTable::new(&edited),
                            "patching {text:?}[{start}..{end}] with {insert:?} \
                             diverged from a rescan of {edited:?}"
                        );
                    }
                }
            }
        }
    }

    /// The case a `\n`-only table cannot get wrong and this one can: neither byte
    /// of the `\r\n` is touched, and the line count still moves. Named so the
    /// failure reads as itself rather than as one case of a few thousand.
    #[test]
    fn an_edit_that_splits_a_crlf_makes_two_lines() {
        let mut table = LineTable::new("a\r\nb");
        assert_eq!(table.line_starts, vec![0, 3]);
        table.patch(2..2, 1, "a\rx\nb");
        assert_eq!(table.line_starts, vec![0, 2, 4]);
    }

    /// The mirror: an insert between a bare `\r` and the rest joins two lines
    /// into one, which a table carrying its boundary verdict across would keep.
    #[test]
    fn an_edit_that_joins_a_cr_and_an_lf_makes_one_line() {
        let mut table = LineTable::new("a\rb");
        assert_eq!(table.line_starts, vec![0, 2]);
        table.patch(2..3, 1, "a\r\n");
        assert_eq!(table.line_starts, vec![0, 3]);
    }

    /// The wide-line flag is an *optimization*: on a clear line the queries take
    /// a byte distance instead of walking. A rescan oracle cannot see a bug in
    /// that shortcut, because it compares tables rather than answers — so
    /// compare the answers against a walk that ignores the flag entirely.
    #[test]
    fn the_ascii_fast_path_and_the_wide_walk_agree() {
        for text in AWKWARD {
            for &encoding in &[PositionEncoding::Utf16, PositionEncoding::Utf8] {
                let idx = LineIndex::with_encoding(text, encoding);
                for offset in (0..=text.len()).filter(|&o| text.is_char_boundary(o)) {
                    let (line, character) = idx.position(offset);
                    let start = idx.line_start(line as usize);
                    let walked: usize = text[start..]
                        .char_indices()
                        .take_while(|&(at, _)| start + at < offset)
                        .map(|(_, ch)| match encoding {
                            PositionEncoding::Utf8 => ch.len_utf8(),
                            PositionEncoding::Utf16 => ch.len_utf16(),
                        })
                        .sum();
                    assert_eq!(
                        character as usize, walked,
                        "position({offset}) of {text:?} ({encoding:?})"
                    );
                    let points = text[start..]
                        .char_indices()
                        .take_while(|&(at, _)| start + at < offset)
                        .count();
                    assert_eq!(
                        idx.line_col(offset).column,
                        points + 1,
                        "line_col({offset}) of {text:?}"
                    );
                }
            }
        }
    }
}
