//! Byte-offset → line/column conversion.
//!
//! Kept free of any LSP type dependency: it exposes a 1-indexed **code-point**
//! [`LineCol`] for CLI diagnostics and a 0-indexed `(line, character)` pair for
//! LSP positions, counted in the [`PositionEncoding`] the index was built with
//! (the encoding negotiated at `initialize`). (Marked an extraction candidate
//! in `AGENTS.md`.)

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

/// Precomputed line-start byte offsets for a text buffer.
#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Byte offset of the first character of each line (0-indexed). Always
    /// starts with `0`.
    line_starts: Vec<usize>,
    /// Total length of the indexed text, in bytes.
    len: usize,
    /// The column unit [`position`](Self::position)/[`offset_at`](Self::offset_at)
    /// count in. Irrelevant to [`line_col`](Self::line_col) (code points).
    encoding: PositionEncoding,
}

impl LineIndex {
    /// An index converting positions in the LSP-default **UTF-16** encoding.
    /// CLI diagnostics (which only use [`line_col`](Self::line_col)) use this
    /// too; LSP code should build with the *negotiated* encoding via
    /// [`with_encoding`](Self::with_encoding).
    pub fn new(text: &str) -> Self {
        Self::with_encoding(text, PositionEncoding::Utf16)
    }

    pub fn with_encoding(text: &str, encoding: PositionEncoding) -> Self {
        let mut line_starts = vec![0];
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => {
                    i += 1;
                    line_starts.push(i);
                }
                b'\r' => {
                    i += if bytes.get(i + 1) == Some(&b'\n') {
                        2
                    } else {
                        1
                    };
                    line_starts.push(i);
                }
                _ => i += 1,
            }
        }
        Self {
            line_starts,
            len: text.len(),
            encoding,
        }
    }

    /// 0-indexed line containing `offset`.
    fn line_of(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(line) => line,
            Err(next) => next - 1,
        }
    }

    /// 1-indexed (line, column-in-code-points) for CLI diagnostics.
    ///
    /// `text` must be the same buffer the index was built from.
    pub fn line_col(&self, text: &str, offset: usize) -> LineCol {
        let offset = offset.min(self.len);
        let line = self.line_of(offset);
        let start = self.line_starts[line];
        let column = text[start..offset].chars().count() + 1;
        LineCol {
            line: line + 1,
            column,
        }
    }

    /// 0-indexed (line, character) for LSP positions, with `character` counted
    /// in the index's [`PositionEncoding`].
    ///
    /// `text` must be the same buffer the index was built from.
    pub fn position(&self, text: &str, offset: usize) -> (u32, u32) {
        let offset = offset.min(self.len);
        let line = self.line_of(offset);
        let start = self.line_starts[line];
        let character: usize = match self.encoding {
            PositionEncoding::Utf8 => offset - start,
            PositionEncoding::Utf16 => text[start..offset].chars().map(char::len_utf16).sum(),
        };
        (line as u32, character as u32)
    }

    /// Byte offset of a 0-indexed LSP position (`character` in the index's
    /// [`PositionEncoding`]). The inverse of [`position`](Self::position), used
    /// to splice incremental `didChange` edits into a buffer.
    ///
    /// `text` must be the same buffer the index was built from. An out-of-range
    /// `line` clamps to the end of the text; a `character` past the line's content
    /// clamps to the line's end (the byte before its trailing newline, or the text
    /// end on the last line). A `character` landing inside a code point (a UTF-16
    /// surrogate pair, or a UTF-8 multi-byte sequence) snaps to the end of that
    /// code point.
    pub fn offset_at(&self, text: &str, line: u32, character: u32) -> usize {
        let line = line as usize;
        let Some(&start) = self.line_starts.get(line) else {
            return self.len;
        };
        // The line spans `[start, line_end)`, excluding the newline so a position
        // never resolves past the line's own content.
        let line_end = self
            .line_starts
            .get(line + 1)
            .map(|&next| line_end_excluding_newline(text, start, next))
            .unwrap_or(self.len);

        match self.encoding {
            PositionEncoding::Utf8 => {
                let mut offset = line_end.min(start + character as usize);
                while !text.is_char_boundary(offset) {
                    offset += 1;
                }
                offset
            }
            PositionEncoding::Utf16 => {
                let mut units = 0u32;
                for (i, ch) in text[start..line_end].char_indices() {
                    if units >= character {
                        return start + i;
                    }
                    units += ch.len_utf16() as u32;
                }
                line_end
            }
        }
    }
}

/// The byte offset of the line break that ends the line starting at `start`,
/// given the next line begins at `next`. Strips a trailing `\n` (and a preceding
/// `\r`), so a column never lands on the newline itself.
fn line_end_excluding_newline(text: &str, start: usize, next: usize) -> usize {
    let bytes = text.as_bytes();
    let mut end = next;
    if end > start && bytes[end - 1] == b'\n' {
        end -= 1;
        if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_basic() {
        let text = "ab\ncde\n";
        let idx = LineIndex::new(text);
        assert_eq!(idx.line_col(text, 0), LineCol { line: 1, column: 1 });
        assert_eq!(idx.line_col(text, 1), LineCol { line: 1, column: 2 });
        assert_eq!(idx.line_col(text, 3), LineCol { line: 2, column: 1 });
        assert_eq!(idx.line_col(text, 5), LineCol { line: 2, column: 3 });
    }

    #[test]
    fn utf16_counts_surrogates() {
        // "𝕏" (U+1D54F) is 4 UTF-8 bytes and 2 UTF-16 units.
        let text = "a𝕏b";
        let idx = LineIndex::new(text);
        let off = "a𝕏".len(); // byte offset just after the astral char
        assert_eq!(idx.position(text, off), (0, 3));
    }

    #[test]
    fn utf8_counts_bytes() {
        // The same buffer in UTF-8 encoding: characters are byte distances.
        let text = "a𝕏b";
        let idx = LineIndex::with_encoding(text, PositionEncoding::Utf8);
        let off = "a𝕏".len();
        assert_eq!(idx.position(text, off), (0, 5));
        assert_eq!(idx.offset_at(text, 0, 5), off);
    }

    #[test]
    fn crlf_line_starts() {
        let text = "a\r\nb";
        let idx = LineIndex::new(text);
        assert_eq!(idx.line_col(text, 3), LineCol { line: 2, column: 1 });
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
                let (line, character) = idx.position(text, offset);
                assert_eq!(
                    idx.offset_at(text, line, character),
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
        assert_eq!(idx.offset_at(text, 0, 2), 2); // just after 'b', before '\r'
        assert_eq!(idx.offset_at(text, 1, 0), 4); // start of "cd"
    }

    #[test]
    fn offset_at_clamps_out_of_range() {
        let text = "ab\ncde\n";
        let idx = LineIndex::new(text);
        // A character past the line's content clamps to the line end (before \n).
        assert_eq!(idx.offset_at(text, 0, 99), 2);
        // The empty trailing line.
        assert_eq!(idx.offset_at(text, 2, 0), 7);
        // A line past the end clamps to the text end.
        assert_eq!(idx.offset_at(text, 99, 0), text.len());
    }

    #[test]
    fn offset_at_inside_surrogate_pair_snaps_to_code_point_end() {
        let text = "𝕏";
        let idx = LineIndex::new(text);
        // "𝕏" is 2 UTF-16 units; character 1 lands mid-pair → snaps to its end.
        assert_eq!(idx.offset_at(text, 0, 1), text.len());
    }

    #[test]
    fn offset_at_inside_utf8_sequence_snaps_to_code_point_end() {
        let text = "𝕏";
        let idx = LineIndex::with_encoding(text, PositionEncoding::Utf8);
        // "𝕏" is 4 bytes; character 2 lands mid-sequence → snaps to its end.
        assert_eq!(idx.offset_at(text, 0, 2), text.len());
        // A character past the line's content clamps to the line end.
        assert_eq!(idx.offset_at(text, 0, 99), text.len());
    }
}
