//! A total, lossless lexer for BibTeX/BibLaTeX surface syntax.
//!
//! Every byte of the input ends up in exactly one token, so concatenating all
//! token texts reproduces the input verbatim — the losslessness invariant.
//!
//! The lexer is context-free: brace/quote *structure* is the parser's job (it
//! tracks brace depth to build `BRACE_GROUP` / `QUOTED` value nodes), exactly as
//! the LaTeX lexer leaves `{`/`}` grouping to its grammar. BibTeX needs no
//! verbatim or catcode modes, so this lexer is simpler than the LaTeX one.
//!
//! The specials `@ { } ( ) , = # "` each lex as a single-character token; runs of
//! whitespace, line breaks, and "word" characters (anything else) coalesce. A
//! word run made up solely of ASCII digits is classified [`SyntaxKind::NUMBER`],
//! else [`SyntaxKind::WORD`] — so a later formatter / linter can tell an unquoted
//! number from a macro name.

use smol_str::SmolStr;

use crate::bib::syntax::SyntaxKind;

/// A single lexed token: its kind plus the exact source slice it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: SyntaxKind,
    pub text: SmolStr,
}

/// Is `c` one of the single-character special tokens?
fn special_kind(c: u8) -> Option<SyntaxKind> {
    Some(match c {
        b'@' => SyntaxKind::AT,
        b'{' => SyntaxKind::L_BRACE,
        b'}' => SyntaxKind::R_BRACE,
        b'(' => SyntaxKind::L_PAREN,
        b')' => SyntaxKind::R_PAREN,
        b',' => SyntaxKind::COMMA,
        b'=' => SyntaxKind::EQ,
        b'#' => SyntaxKind::HASH,
        b'"' => SyntaxKind::QUOTE,
        _ => return None,
    })
}

/// Does `c` end a word run? (whitespace, a line break, or a special)
fn is_word_boundary(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r') || special_kind(c).is_some()
}

/// Lex `input` into a flat, lossless token stream.
pub fn lex(input: &str) -> Vec<Token> {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < bytes.len() {
        let c = bytes[pos];
        let (kind, len) = if let Some(kind) = special_kind(c) {
            (kind, 1)
        } else if c == b'\n' {
            (SyntaxKind::NEWLINE, 1)
        } else if c == b'\r' {
            // `\r\n` is one line break; a lone `\r` is its own.
            let len = if bytes.get(pos + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
            (SyntaxKind::NEWLINE, len)
        } else if c == b' ' || c == b'\t' {
            let len = run_len(bytes, pos, |b| b == b' ' || b == b'\t');
            (SyntaxKind::WHITESPACE, len)
        } else {
            let len = run_len(bytes, pos, |b| !is_word_boundary(b));
            let kind = if bytes[pos..pos + len].iter().all(u8::is_ascii_digit) {
                SyntaxKind::NUMBER
            } else {
                SyntaxKind::WORD
            };
            (kind, len)
        };
        out.push(Token {
            kind,
            text: SmolStr::new(&input[pos..pos + len]),
        });
        pos += len;
    }
    out
}

/// Length of the maximal run of bytes from `start` satisfying `pred`. The byte at
/// `start` is assumed to satisfy `pred`, so the run is always at least one byte.
fn run_len(bytes: &[u8], start: usize, pred: impl Fn(u8) -> bool) -> usize {
    let mut i = start + 1;
    while i < bytes.len() && pred(bytes[i]) {
        i += 1;
    }
    i - start
}
