//! A total, lossless lexer for LaTeX surface syntax.
//!
//! Every byte of the input ends up in exactly one token, so concatenating all
//! token texts reproduces the input verbatim — the losslessness invariant. The
//! lexer understands only *surface* structure (control sequences, the active
//! characters, comments, runs of ordinary text); it never resolves macro
//! meaning or catcodes. See `AGENTS.md`, Core decision #1.

use smol_str::SmolStr;

use crate::syntax::SyntaxKind;

/// A single lexed token: its kind plus the exact source slice it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: SyntaxKind,
    pub text: SmolStr,
}

/// Lex `input` into a flat, lossless token stream.
pub fn lex(input: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < input.len() {
        let rest = &input[pos..];
        let (kind, len) = next_token(rest);
        debug_assert!(len > 0, "lexer made no progress at byte {pos}");
        out.push(Token {
            kind,
            text: SmolStr::new(&rest[..len]),
        });
        pos += len;
    }
    out
}

/// Classify the token at the start of `rest` and return its `(kind, byte_len)`.
fn next_token(rest: &str) -> (SyntaxKind, usize) {
    let c = rest.chars().next().expect("rest is non-empty");
    match c {
        '\\' => lex_control(rest),
        '%' => (
            SyntaxKind::COMMENT,
            run_len(rest, |c| c != '\n' && c != '\r'),
        ),
        '{' => (SyntaxKind::L_BRACE, 1),
        '}' => (SyntaxKind::R_BRACE, 1),
        '[' => (SyntaxKind::L_BRACKET, 1),
        ']' => (SyntaxKind::R_BRACKET, 1),
        '$' => (SyntaxKind::DOLLAR, 1),
        '&' => (SyntaxKind::AMPERSAND, 1),
        '#' => (SyntaxKind::HASH, 1),
        '^' => (SyntaxKind::CARET, 1),
        '_' => (SyntaxKind::UNDERSCORE, 1),
        '~' => (SyntaxKind::TILDE, 1),
        '\n' => (SyntaxKind::NEWLINE, 1),
        '\r' => {
            let len = if rest.as_bytes().get(1) == Some(&b'\n') {
                2
            } else {
                1
            };
            (SyntaxKind::NEWLINE, len)
        }
        ' ' | '\t' => (
            SyntaxKind::WHITESPACE,
            run_len(rest, |c| c == ' ' || c == '\t'),
        ),
        _ => (SyntaxKind::WORD, run_len(rest, is_word_char)),
    }
}

/// Lex a control sequence: `rest` is known to start with `\`.
fn lex_control(rest: &str) -> (SyntaxKind, usize) {
    match rest[1..].chars().next() {
        // Control word: backslash + one or more ASCII letters.
        Some(d) if d.is_ascii_alphabetic() => {
            let letters = run_len(&rest[1..], |c| c.is_ascii_alphabetic());
            (SyntaxKind::CONTROL_WORD, 1 + letters)
        }
        // Control symbol: backslash + exactly one other character.
        Some(d) => (SyntaxKind::CONTROL_SYMBOL, 1 + d.len_utf8()),
        // A lone trailing backslash at end of input.
        None => (SyntaxKind::CONTROL_SYMBOL, 1),
    }
}

/// Number of leading bytes of `s` whose chars all satisfy `pred`.
fn run_len(s: &str, pred: impl Fn(char) -> bool) -> usize {
    let mut len = 0;
    for c in s.chars() {
        if pred(c) {
            len += c.len_utf8();
        } else {
            break;
        }
    }
    len
}

/// Ordinary text: anything that is not whitespace, a line break, or one of the
/// characters the lexer treats specially.
fn is_word_char(c: char) -> bool {
    !matches!(
        c,
        '\\' | '%'
            | '{'
            | '}'
            | '['
            | ']'
            | '$'
            | '&'
            | '#'
            | '^'
            | '_'
            | '~'
            | ' '
            | '\t'
            | '\n'
            | '\r'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lexer is total and lossless: concatenated token text == input.
    fn assert_lossless(input: &str) {
        let joined: String = lex(input).iter().map(|t| t.text.as_str()).collect();
        assert_eq!(joined, input);
    }

    #[test]
    fn lossless_on_assorted_inputs() {
        for input in [
            "",
            "plain text",
            r"\section{Hi}[x]",
            "$a^2_b$",
            "a%c\n\nb",
            "café ∑ \\\\ \\{ \\,",
            "tab\tand  spaces",
            "trailing\\",
        ] {
            assert_lossless(input);
        }
    }

    #[test]
    fn control_word_stops_at_non_letter() {
        let toks = lex(r"\alpha2");
        assert_eq!(toks[0].kind, SyntaxKind::CONTROL_WORD);
        assert_eq!(toks[0].text, "\\alpha");
        assert_eq!(toks[1].kind, SyntaxKind::WORD);
        assert_eq!(toks[1].text, "2");
    }

    #[test]
    fn double_backslash_is_one_control_symbol() {
        let toks = lex(r"\\");
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].kind, SyntaxKind::CONTROL_SYMBOL);
        assert_eq!(toks[0].text, r"\\");
    }

    #[test]
    fn comment_stops_before_newline() {
        let toks = lex("% hi\nx");
        assert_eq!(toks[0].kind, SyntaxKind::COMMENT);
        assert_eq!(toks[0].text, "% hi");
        assert_eq!(toks[1].kind, SyntaxKind::NEWLINE);
    }

    #[test]
    fn crlf_is_a_single_newline() {
        let toks = lex("a\r\nb");
        assert_eq!(toks[1].kind, SyntaxKind::NEWLINE);
        assert_eq!(toks[1].text, "\r\n");
    }
}
