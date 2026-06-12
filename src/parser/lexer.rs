//! A total, lossless lexer for LaTeX surface syntax.
//!
//! Every byte of the input ends up in exactly one token, so concatenating all
//! token texts reproduces the input verbatim — the losslessness invariant. The
//! lexer is mostly context-free, with three bounded, statically-recognizable
//! modes sanctioned by `AGENTS.md` Core decision #1:
//!
//! - **`\verb` / `\verb*`** inline verbatim: the delimited argument is consumed
//!   as a single [`SyntaxKind::VERB`] token (otherwise the delimiters glue into
//!   ordinary `WORD` runs and become un-splittable downstream).
//! - **verbatim-like environments** (`verbatim`, `lstlisting`, `minted`, …): the
//!   body between `\begin{name}` and `\end{name}` is one
//!   [`SyntaxKind::VERBATIM_BODY`] token, so `%`, `$`, `\` inside are never
//!   (mis)lexed as comments / math. For argument-taking ones the `\begin`
//!   arguments are tokenized first (the built-in signature DB says where the raw
//!   body starts); see [`lex_verbatim_environment`].
//! - **`\makeatletter` / `\makeatother`**: toggles `@` into a letter so that
//!   `\foo@bar` lexes as one control word.
//!
//! None of these resolve macro meaning; they are surface lexing concerns (in
//! TeX, catcodes genuinely change in these regions).

use smol_str::SmolStr;

use crate::semantic::signature::{ArgKind, ArgSpec, builtin};
use crate::syntax::SyntaxKind;

/// A single lexed token: its kind plus the exact source slice it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: SyntaxKind,
    pub text: SmolStr,
}

/// Is `name` a verbatim-like environment — one whose body the lexer must capture
/// raw, per `AGENTS.md` Core decision #1? Resolved against the built-in signature
/// database ([`builtin`]), the single source of truth for which environments carry
/// a verbatim body and what arguments precede that body. Both the lexer (to find
/// where the raw body begins) and the structural parser (`grammar.rs`, to route the
/// environment to its raw-body branch) ask this question, so one lookup keeps them
/// in lockstep. We read only static argument-shape data; no macro meaning is
/// resolved, so this stays within decision #1's sanctioned lexer modes.
pub(crate) fn is_verbatim_environment(name: &str) -> bool {
    builtin()
        .environment(name)
        .is_some_and(|env| env.verbatim_body)
}

/// Lex `input` into a flat, lossless token stream.
pub fn lex(input: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut pos = 0;
    let mut at_letter = false; // `\makeatletter` state
    while pos < input.len() {
        let rest = &input[pos..];

        // Verbatim-like environment: emit `\begin{name}` then a raw body token.
        if let Some(consumed) = lex_verbatim_environment(rest, &mut out) {
            pos += consumed;
            continue;
        }

        let (kind, len) = next_token(rest, at_letter);
        debug_assert!(len > 0, "lexer made no progress at byte {pos}");
        let text = &rest[..len];
        if kind == SyntaxKind::CONTROL_WORD {
            match text {
                "\\makeatletter" => at_letter = true,
                "\\makeatother" => at_letter = false,
                _ => {}
            }
        }
        out.push(Token {
            kind,
            text: SmolStr::new(text),
        });
        pos += len;
    }
    out
}

/// Classify the token at the start of `rest` and return its `(kind, byte_len)`.
fn next_token(rest: &str, at_letter: bool) -> (SyntaxKind, usize) {
    let c = rest.chars().next().expect("rest is non-empty");
    match c {
        '\\' => lex_control(rest, at_letter),
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
fn lex_control(rest: &str, at_letter: bool) -> (SyntaxKind, usize) {
    match rest[1..].chars().next() {
        // Control word: backslash + one or more letters (`@` too under
        // `\makeatletter`).
        Some(d) if is_letter(d, at_letter) => {
            let letters = run_len(&rest[1..], |c| is_letter(c, at_letter));
            let word_len = 1 + letters;
            // `\verb` / `\verb*`: swallow the delimited argument as one token.
            if &rest[..word_len] == "\\verb"
                && let Some(arg_len) = verb_len(&rest[word_len..])
            {
                return (SyntaxKind::VERB, word_len + arg_len);
            }
            (SyntaxKind::CONTROL_WORD, word_len)
        }
        // Control symbol: backslash + exactly one other character.
        Some(d) => (SyntaxKind::CONTROL_SYMBOL, 1 + d.len_utf8()),
        // A lone trailing backslash at end of input.
        None => (SyntaxKind::CONTROL_SYMBOL, 1),
    }
}

/// Length in bytes of a `\verb` argument: an optional `*`, a delimiter
/// character, then everything up to and including the matching delimiter.
/// Returns `None` if malformed (no delimiter, or it spans a line break).
fn verb_len(after: &str) -> Option<usize> {
    let mut chars = after.chars();
    let mut consumed = 0;
    let mut delim = chars.next()?;
    if delim == '*' {
        consumed += 1;
        delim = chars.next()?;
    }
    if delim.is_whitespace() {
        return None;
    }
    consumed += delim.len_utf8();
    for c in chars {
        if c == '\n' || c == '\r' {
            return None;
        }
        consumed += c.len_utf8();
        if c == delim {
            return Some(consumed);
        }
    }
    None
}

/// If `rest` starts with `\begin{name}` for a verbatim-like `name`, emit the
/// `\begin{name}` tokens, then any environment arguments as ordinary tokens, and
/// finally a single raw body token, returning the bytes consumed (through the body,
/// up to the closing `\end{name}`).
///
/// Arguments are lexed *before* the body because the raw body begins only after
/// them: in `\begin{minted}{python}`, `{python}` is a structured argument, not body
/// text. The built-in signature ([`builtin`]) bounds how many leading groups count
/// as arguments, so a body that legitimately starts with `[` (an option-free
/// `lstlisting` whose first code line is `[1,2,3]`) is not mistaken for one.
fn lex_verbatim_environment(rest: &str, out: &mut Vec<Token>) -> Option<usize> {
    let after_begin = rest.strip_prefix("\\begin{")?;
    let close = after_begin.find('}')?;
    let name = &after_begin[..close];
    let env = builtin().environment(name).filter(|e| e.verbatim_body)?;

    let prefix_len = "\\begin{".len() + name.len() + "}".len();
    out.push(Token {
        kind: SyntaxKind::CONTROL_WORD,
        text: SmolStr::new("\\begin"),
    });
    out.push(Token {
        kind: SyntaxKind::L_BRACE,
        text: SmolStr::new("{"),
    });
    out.push(Token {
        kind: SyntaxKind::WORD,
        text: SmolStr::new(name),
    });
    out.push(Token {
        kind: SyntaxKind::R_BRACE,
        text: SmolStr::new("}"),
    });

    // Locate the argument span, then tokenize it normally. It holds no nested
    // verbatim-begin, so the ordinary token loop is safe and lets the parser build
    // the usual OPTIONAL/GROUP argument nodes.
    let args_region = &rest[prefix_len..];
    let args_len = scan_verbatim_args(args_region, &env.args);
    lex_into(&args_region[..args_len], out);

    let body_region = &args_region[args_len..];
    let end_marker = format!("\\end{{{name}}}");
    let body_len = body_region.find(&end_marker).unwrap_or(body_region.len());
    if body_len > 0 {
        out.push(Token {
            kind: SyntaxKind::VERBATIM_BODY,
            text: SmolStr::new(&body_region[..body_len]),
        });
    }
    Some(prefix_len + args_len + body_len)
}

/// Byte length of the argument span that precedes a verbatim body, given the
/// environment's declared `args`. For each argument in order, consume any inline
/// whitespace (spaces/tabs, never a line break — an argument never crosses a
/// newline, so a bracket on the next line is body text) followed by the balanced
/// group of the expected delimiter when present. A missing optional or required
/// argument is skipped; a malformed (unbalanced) group is left to the body, so the
/// scan never runs past the input and losslessness is preserved.
fn scan_verbatim_args(region: &str, args: &[ArgSpec]) -> usize {
    let bytes = region.as_bytes();
    let mut pos = 0;
    for arg in args {
        let mut probe = pos;
        while matches!(bytes.get(probe), Some(b' ' | b'\t')) {
            probe += 1;
        }
        let (open, close) = match arg.kind {
            ArgKind::Bracket => (b'[', b']'),
            ArgKind::Brace => (b'{', b'}'),
        };
        if bytes.get(probe) != Some(&open) {
            // Argument absent; the skipped whitespace belongs to the body.
            continue;
        }
        match balanced_group_len(&region[probe..], close) {
            Some(len) => pos = probe + len,
            None => break, // unbalanced: treat the remainder as body
        }
    }
    pos
}

/// Length in bytes of the balanced group starting at `s[0]` (an `[` or `{`), up to
/// and including its matching closer. Brace and bracket nesting is tracked with a
/// delimiter stack, so a `]` inside `{…}` (or vice versa) is treated as literal; a
/// `\`-escaped delimiter is skipped. Returns `None` if the group never closes.
fn balanced_group_len(s: &str, close: u8) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut stack = vec![close];
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                // Skip the escaped byte; a delimiter loses its meaning.
                i += 2;
                continue;
            }
            b'{' => stack.push(b'}'),
            b'[' => stack.push(b']'),
            c @ (b'}' | b']') => {
                if stack.last() == Some(&c) {
                    stack.pop();
                    if stack.is_empty() {
                        return Some(i + 1);
                    }
                }
                // A non-matching closer is literal text; ignore it.
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Tokenize `region` with the ordinary, context-free token loop, appending to
/// `out`. Used for the argument span of a verbatim-like environment, which carries
/// no `\makeatletter` or nested verbatim-begin context.
fn lex_into(region: &str, out: &mut Vec<Token>) {
    let mut pos = 0;
    while pos < region.len() {
        let (kind, len) = next_token(&region[pos..], false);
        debug_assert!(len > 0, "lexer made no progress in verbatim args");
        out.push(Token {
            kind,
            text: SmolStr::new(&region[pos..pos + len]),
        });
        pos += len;
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

/// A control-word continuation character: a letter, or `@` under
/// `\makeatletter`.
fn is_letter(c: char, at_letter: bool) -> bool {
    c.is_ascii_alphabetic() || (at_letter && c == '@')
}

/// Ordinary text: anything that is not whitespace, a line break, or one of the
/// characters the lexer treats specially.
pub(crate) fn is_word_char(c: char) -> bool {
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
            r"\verb|$x$|",
            "\\begin{verbatim}\n$x$ %not a comment\n\\end{verbatim}",
            "\\begin{lstlisting}[language=C]\nint a[3];  % raw\n\\end{lstlisting}",
            "\\begin{minted}[frame=single]{python}\nprint(\"$x$\")\n\\end{minted}",
            "\\begin{lstlisting}\n[1,2,3]\n\\end{lstlisting}",
            r"\makeatletter\a@b\makeatother\a@b",
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

    #[test]
    fn verb_inline_is_one_token() {
        let toks = lex(r"\verb|$x$|");
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].kind, SyntaxKind::VERB);
        assert_eq!(toks[0].text, r"\verb|$x$|");
    }

    #[test]
    fn verb_star_with_plus_delimiter() {
        let toks = lex(r"a\verb*+b+c");
        assert_eq!(toks[1].kind, SyntaxKind::VERB);
        assert_eq!(toks[1].text, r"\verb*+b+");
        assert_eq!(toks[2].text, "c");
    }

    #[test]
    fn verb_without_closing_delimiter_is_a_plain_control_word() {
        let toks = lex(r"\verb|x");
        assert_eq!(toks[0].kind, SyntaxKind::CONTROL_WORD);
        assert_eq!(toks[0].text, r"\verb");
    }

    #[test]
    fn makeatletter_makes_at_a_letter() {
        let toks = lex(r"\makeatletter\foo@bar\makeatother\foo@bar");
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        // Under \makeatletter, `\foo@bar` is one control word…
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\foo@bar")));
        // …after \makeatother it splits into `\foo` + `@bar`.
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\foo")));
    }

    #[test]
    fn verbatim_environment_body_is_one_raw_token() {
        let toks = lex("\\begin{verbatim}\n$not$ %literal\n\\end{verbatim}");
        assert_eq!(toks[0].text, "\\begin");
        assert_eq!(toks[2].text, "verbatim");
        assert!(
            toks.iter()
                .any(|t| t.kind == SyntaxKind::VERBATIM_BODY && t.text.contains("$not$ %literal"))
        );
        // Nothing inside the body was lexed as math or a comment.
        assert!(!toks.iter().any(|t| t.kind == SyntaxKind::DOLLAR));
        assert!(!toks.iter().any(|t| t.kind == SyntaxKind::COMMENT));
    }

    #[test]
    fn argument_taking_verbatim_separates_args_from_body() {
        // `minted` declares `[opt]{req}`: both groups are tokenized normally, then
        // the rest is one raw body token.
        let toks = lex("\\begin{minted}[frame=single]{python}\nprint(\"$x$\")\n\\end{minted}");
        let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
        // The optional and required argument delimiters survive as ordinary tokens…
        assert!(kinds.contains(&SyntaxKind::L_BRACKET));
        assert!(kinds.contains(&SyntaxKind::R_BRACKET));
        assert!(kinds.contains(&SyntaxKind::L_BRACE));
        // …and the body (with its `$`) is a single opaque token, not math.
        assert!(
            toks.iter()
                .any(|t| t.kind == SyntaxKind::VERBATIM_BODY && t.text.contains("print(\"$x$\")"))
        );
        assert!(!toks.iter().any(|t| t.kind == SyntaxKind::DOLLAR));
    }

    #[test]
    fn verbatim_body_starting_with_bracket_is_not_an_argument() {
        // `lstlisting`'s lone optional argument is absent (a newline separates the
        // `\begin` from the `[`), so `[1,2,3]` stays inside the raw body.
        let toks = lex("\\begin{lstlisting}\n[1,2,3]\n\\end{lstlisting}");
        assert!(
            !toks
                .iter()
                .take_while(|t| t.kind != SyntaxKind::VERBATIM_BODY)
                .any(|t| t.kind == SyntaxKind::L_BRACKET),
            "the bracket on the body's first line must not be lexed as an argument"
        );
        assert!(
            toks.iter()
                .any(|t| t.kind == SyntaxKind::VERBATIM_BODY && t.text.contains("[1,2,3]"))
        );
    }
}
