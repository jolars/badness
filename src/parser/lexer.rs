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
//! - **`\left` / `\right` delimiters**: the single delimiter that follows is
//!   isolated as its own token, so a word-character delimiter (`(`, `)`, `|`,
//!   `/`, `.`, `<`, `>`) does not glue into the following word run and become
//!   un-splittable downstream (the same problem `\verb` has). Control-symbol /
//!   control-word / bracket delimiters already lex as single tokens.
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
    // True when the previous meaningful token was `\left`/`\right`, so the next
    // delimiter must be isolated as a single token (it carries across whitespace,
    // which TeX skips before the delimiter).
    let mut pending_delim = false;
    while pos < input.len() {
        let rest = &input[pos..];

        // Verbatim-like environment: emit `\begin{name}` then a raw body token.
        if let Some(consumed) = lex_verbatim_environment(rest, &mut out) {
            pos += consumed;
            pending_delim = false;
            continue;
        }

        // Verbatim-argument command (`\url{…}`, `\code{…}`, `\lstinline|…|`, …):
        // emit the control word and any leading args, then a raw argument token.
        // `\verb`/`\verb*` are handled separately in `lex_control` (delimiter
        // only), so they fall through here.
        if let Some(consumed) = lex_verbatim_command(rest, at_letter, &mut out) {
            pos += consumed;
            pending_delim = false;
            continue;
        }

        let (kind, mut len) = next_token(rest, at_letter);
        // A `\left`/`\right` delimiter that lexes as a word run: keep only its
        // first character so it does not glue into the following text.
        if pending_delim && kind == SyntaxKind::WORD {
            len = rest.chars().next().expect("rest is non-empty").len_utf8();
        }
        debug_assert!(len > 0, "lexer made no progress at byte {pos}");
        let text = &rest[..len];
        if kind == SyntaxKind::CONTROL_WORD {
            match text {
                "\\makeatletter" => at_letter = true,
                "\\makeatother" => at_letter = false,
                _ => {}
            }
        }
        pending_delim = match kind {
            // Trivia is skipped before the delimiter, so the mode persists.
            SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE => pending_delim,
            SyntaxKind::CONTROL_WORD if text == "\\left" || text == "\\right" => true,
            _ => false,
        };
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

/// Length in bytes of a `\verb` argument: an optional `*`, then a delimited run.
/// Returns `None` if malformed (no delimiter, or it spans a line break).
fn verb_len(after: &str) -> Option<usize> {
    match after.strip_prefix('*') {
        Some(rest) => Some(1 + delimited_len(rest)?),
        None => delimited_len(after),
    }
}

/// Length in bytes of a `\verb`-style delimited run: a delimiter character, then
/// everything up to and including its next occurrence. Returns `None` if the
/// delimiter is whitespace or the run spans a line break.
fn delimited_len(after: &str) -> Option<usize> {
    let mut chars = after.chars();
    let delim = chars.next()?;
    if delim.is_whitespace() {
        return None;
    }
    let mut consumed = delim.len_utf8();
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

/// If `rest` starts with a verbatim-argument command (`\url`, `\code`,
/// `\lstinline`, …), emit its control word, any leading non-verbatim arguments
/// (as ordinary tokens), and finally a single raw [`SyntaxKind::VERB`] token for
/// the verbatim argument; return the bytes consumed. Returns `None` when `rest`
/// is not such a command or no verbatim argument follows (so the caller lexes it
/// normally and losslessness is preserved either way).
///
/// The verbatim argument's form is decided by its first non-blank character,
/// matching how these commands actually parse: a brace introduces a balanced
/// `{…}` group (`\code{…}`, `\url{…}`); any other character is a `\verb`-style
/// delimiter run (`\lstinline|…|`). `\verb`/`\verb*` are deliberately excluded —
/// they are delimiter-only and handled in [`lex_control`]. Like the verbatim
/// environment path, this reads only static signature data (decision #1).
fn lex_verbatim_command(rest: &str, at_letter: bool, out: &mut Vec<Token>) -> Option<usize> {
    if !rest.starts_with('\\') {
        return None;
    }
    let letters = run_len(&rest[1..], |c| is_letter(c, at_letter));
    if letters == 0 {
        return None;
    }
    let word_len = 1 + letters;
    let name = &rest[1..word_len];
    // `\verb` keeps its dedicated delimiter-only path.
    if name == "verb" {
        return None;
    }
    let cmd = builtin().command(name).filter(|c| c.verbatim)?;

    // Leading arguments precede the verbatim one (e.g. `\mintinline{lang}{code}`).
    let after_word = &rest[word_len..];
    let args_len = scan_verbatim_args(after_word, &cmd.args);

    // Skip inline whitespace (never a line break — an argument never crosses a
    // newline) to reach the verbatim argument's opening delimiter.
    let region = &after_word[args_len..];
    let ws_len = region
        .bytes()
        .take_while(|&b| b == b' ' || b == b'\t')
        .count();
    let arg_region = &region[ws_len..];
    let arg_len = match arg_region.bytes().next() {
        Some(b'{') => balanced_group_len(arg_region, b'}')?,
        // A `\verb`-style delimiter run: the first character delimits, and the
        // argument may not span a line break.
        Some(_) => delimited_len(arg_region)?,
        None => return None,
    };

    out.push(Token {
        kind: SyntaxKind::CONTROL_WORD,
        text: SmolStr::new(&rest[..word_len]),
    });
    lex_into(&after_word[..args_len], out);
    if ws_len > 0 {
        out.push(Token {
            kind: SyntaxKind::WHITESPACE,
            text: SmolStr::new(&region[..ws_len]),
        });
    }
    out.push(Token {
        kind: SyntaxKind::VERB,
        text: SmolStr::new(&arg_region[..arg_len]),
    });
    Some(word_len + args_len + ws_len + arg_len)
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
            r"$\left(x+y\right)^2 \left.\frac{a}{b}\right|_0$",
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
    fn left_right_isolate_word_delimiter() {
        // `(` would normally glue into `(x+y` as one word; after `\left` it is
        // its own one-character token, and `\right)`'s `)` likewise.
        let toks = lex(r"\left(x+y\right)");
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert_eq!(
            seen,
            [
                (SyntaxKind::CONTROL_WORD, "\\left"),
                (SyntaxKind::WORD, "("),
                (SyntaxKind::WORD, "x+y"),
                (SyntaxKind::CONTROL_WORD, "\\right"),
                (SyntaxKind::WORD, ")"),
            ]
        );
    }

    #[test]
    fn left_delimiter_carries_across_whitespace() {
        // TeX skips spaces before the delimiter; the mode persists so `(` is
        // still isolated.
        let toks = lex(r"\left ( a");
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert_eq!(
            seen,
            [
                (SyntaxKind::CONTROL_WORD, "\\left"),
                (SyntaxKind::WHITESPACE, " "),
                (SyntaxKind::WORD, "("),
                (SyntaxKind::WHITESPACE, " "),
                (SyntaxKind::WORD, "a"),
            ]
        );
    }

    #[test]
    fn left_non_word_delimiters_are_untouched() {
        // A control-symbol (`\{`), control-word (`\langle`), or bracket delimiter
        // already lexes as a single token, so the mode changes nothing.
        for input in [r"\left\{", r"\left\langle", r"\left["] {
            assert_lossless(input);
        }
        let toks = lex(r"\left\langle x \right\rangle");
        assert!(toks.iter().any(|t| t.text == "\\langle"));
        assert!(toks.iter().any(|t| t.text == "\\rangle"));
    }

    #[test]
    fn leftarrow_is_not_left() {
        // The maximal letter run keeps `\leftarrow` one control word, so the
        // delimiter mode never triggers.
        let toks = lex(r"\leftarrow(x)");
        assert_eq!(toks[0].text, "\\leftarrow");
        // `(x)` glues normally — the mode did not fire.
        assert_eq!(toks[1].text, "(x)");
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
