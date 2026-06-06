//! Byte-offset → line/column conversion.
//!
//! Lifted in spirit from ravel's `text/line_index.rs`, but kept free of any LSP
//! type dependency for now: it exposes a 1-indexed **code-point** [`LineCol`]
//! for CLI diagnostics and a 0-indexed **UTF-16** `(line, character)` pair for
//! LSP positions, which Phase 6 will map onto `lsp_types::Position`. (Marked an
//! extraction candidate in `AGENTS.md`.)

/// A 1-indexed line/column, with the column counted in Unicode code points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    /// 1-indexed line number.
    pub line: usize,
    /// 1-indexed column in code points (not bytes, not UTF-16 units).
    pub column: usize,
}

/// Precomputed line-start byte offsets for a text buffer.
#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Byte offset of the first character of each line (0-indexed). Always
    /// starts with `0`.
    line_starts: Vec<usize>,
    /// Total length of the indexed text, in bytes.
    len: usize,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
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

    /// 0-indexed (line, character-in-UTF-16-units) for LSP positions.
    ///
    /// `text` must be the same buffer the index was built from.
    pub fn utf16_position(&self, text: &str, offset: usize) -> (u32, u32) {
        let offset = offset.min(self.len);
        let line = self.line_of(offset);
        let start = self.line_starts[line];
        let character: usize = text[start..offset].chars().map(char::len_utf16).sum();
        (line as u32, character as u32)
    }
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
        assert_eq!(idx.utf16_position(text, off), (0, 3));
    }

    #[test]
    fn crlf_line_starts() {
        let text = "a\r\nb";
        let idx = LineIndex::new(text);
        assert_eq!(idx.line_col(text, 3), LineCol { line: 2, column: 1 });
    }
}
