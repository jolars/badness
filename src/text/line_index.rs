//! Byte-offset → line/column conversion.
//!
//! Kept free of any LSP type dependency: it exposes a 1-indexed **code-point**
//! [`LineCol`] for CLI diagnostics and a 0-indexed `(line, character)` pair for
//! LSP positions, counted in the [`PositionEncoding`] the index was built with
//! (the encoding negotiated at `initialize`). (Marked an extraction candidate
//! in `AGENTS.md`.)
//!
//! The index owns its conversion tables and touches no text after construction:
//! at build time it records, per line, the *wide characters* (any char wider than
//! one byte), so every query answers from that table in O(wide-chars-on-line)
//! without re-walking — and without the caller handing the original buffer back
//! in (there is no stale-buffer misuse hazard).

use std::collections::HashMap;

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

/// A char wider than one byte, recorded once at construction. `start`/`end` are
/// absolute byte offsets; `utf16_len` is 1 (BMP) or 2 (astral surrogate pair).
/// The UTF-8 length is `end - start`, and a wide char always spans exactly one
/// code point.
#[derive(Debug, Clone, Copy)]
struct WideChar {
    start: usize,
    end: usize,
    utf16_len: u8,
}

impl WideChar {
    fn utf8_len(&self) -> usize {
        self.end - self.start
    }
}

/// Precomputed line and wide-char tables for a text buffer.
#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Byte offset of the first character of each line (0-indexed). Always
    /// starts with `0`.
    line_starts: Vec<usize>,
    /// End of each line's *content* (before its `\n`/`\r\n`/EOF terminator),
    /// parallel to `line_starts`.
    line_ends: Vec<usize>,
    /// Wide chars per line, keyed by 0-indexed line. An absent key means an
    /// ASCII-only line (the common case, so no empty `Vec` is allocated). The
    /// per-line `Vec`s are start-sorted.
    line_wide_chars: HashMap<usize, Vec<WideChar>>,
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
        let len = text.len();
        let mut line_starts = vec![0];
        let mut line_ends = Vec::new();
        let mut line_wide_chars: HashMap<usize, Vec<WideChar>> = HashMap::new();
        let mut line = 0usize;

        let bytes = text.as_bytes();
        // Set after a `\r` that begins a `\r\n`, so the following `\n` (which
        // `char_indices` still yields) is not counted as a second break.
        let mut skip_lf = false;
        for (i, ch) in text.char_indices() {
            match ch {
                '\n' if skip_lf => {
                    // The `\n` half of a `\r\n` already recorded by the `\r` arm.
                    skip_lf = false;
                }
                '\n' => {
                    // The line's content ends just before this `\n`.
                    line_ends.push(i);
                    line_starts.push(i + 1);
                    line += 1;
                }
                '\r' => {
                    // `\r\n` is a single break; a bare `\r` breaks on its own.
                    line_ends.push(i);
                    if bytes.get(i + 1) == Some(&b'\n') {
                        line_starts.push(i + 2);
                        skip_lf = true;
                    } else {
                        line_starts.push(i + 1);
                    }
                    line += 1;
                }
                _ if ch.len_utf8() > 1 => {
                    line_wide_chars.entry(line).or_default().push(WideChar {
                        start: i,
                        end: i + ch.len_utf8(),
                        utf16_len: ch.len_utf16() as u8,
                    });
                }
                _ => {}
            }
        }
        // The final line's content runs to the end of the text.
        line_ends.push(len);

        Self {
            line_starts,
            line_ends,
            line_wide_chars,
            len,
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

    /// Wide chars on `line`, or an empty slice for an ASCII-only line.
    fn wide_chars(&self, line: usize) -> &[WideChar] {
        self.line_wide_chars
            .get(&line)
            .map_or(&[][..], Vec::as_slice)
    }

    /// 1-indexed (line, column-in-code-points) for CLI diagnostics.
    pub fn line_col(&self, offset: usize) -> LineCol {
        let offset = offset.min(self.len);
        let line = self.line_of(offset);
        let start = self.line_starts[line];
        // Each wide char spans `utf8_len` bytes but one code point; ASCII chars
        // are one byte each. So the code-point count is the byte distance less
        // the extra bytes every wide char before `offset` contributes.
        let extra: usize = self
            .wide_chars(line)
            .iter()
            .take_while(|w| w.end <= offset)
            .map(|w| w.utf8_len() - 1)
            .sum();
        let column = (offset - start) - extra + 1;
        LineCol {
            line: line + 1,
            column,
        }
    }

    /// 0-indexed (line, character) for LSP positions, with `character` counted
    /// in the index's [`PositionEncoding`].
    pub fn position(&self, offset: usize) -> (u32, u32) {
        let offset = offset.min(self.len);
        let line = self.line_of(offset);
        let start = self.line_starts[line];
        let byte_col = offset - start;
        let character = match self.encoding {
            PositionEncoding::Utf8 => byte_col,
            // Each wide char contributes `utf8_len` bytes but `utf16_len` units;
            // ASCII contributes 1 to both. Subtract the per-wide-char surplus.
            PositionEncoding::Utf16 => {
                let surplus: usize = self
                    .wide_chars(line)
                    .iter()
                    .take_while(|w| w.end <= offset)
                    .map(|w| w.utf8_len() - w.utf16_len as usize)
                    .sum();
                byte_col - surplus
            }
        };
        (line as u32, character as u32)
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
        let Some(&start) = self.line_starts.get(line) else {
            return self.len;
        };
        // The line spans `[start, line_end)`, excluding the newline so a position
        // never resolves past the line's own content.
        let line_end = self.line_ends[line];
        let character = character as usize;
        let wides = self.wide_chars(line);

        match self.encoding {
            PositionEncoding::Utf8 => {
                let mut offset = line_end.min(start + character);
                // A byte column can only land inside a code point at a wide char;
                // snap forward to that char's end (the next char boundary).
                if let Some(w) = wides.iter().find(|w| w.start < offset && offset < w.end) {
                    offset = w.end;
                }
                offset
            }
            PositionEncoding::Utf16 => {
                // Walk the line's wide chars, tracking the running byte offset and
                // UTF-16 unit count. ASCII gaps between them advance both 1:1.
                let mut byte = start;
                let mut units = 0usize;
                for w in wides {
                    let gap = w.start - byte; // ASCII units before this wide char
                    // `<=` so a target *at* the wide char's start (unit `units +
                    // gap`) resolves to that boundary rather than snapping inside.
                    if character <= units + gap {
                        return (byte + (character - units)).min(line_end);
                    }
                    units += gap;
                    let w_units = w.utf16_len as usize;
                    if character < units + w_units {
                        // Target lands within the surrogate pair → snap to its end.
                        return w.end.min(line_end);
                    }
                    byte = w.end;
                    units += w_units;
                }
                // Remaining ASCII tail up to the line end.
                (byte + character.saturating_sub(units)).min(line_end)
            }
        }
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
}
