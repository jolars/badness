//! `SyntaxKind` — the kinds of CST tokens and nodes — and the rowan `Language`
//! binding for badness's LaTeX surface CST.

use rowan::Language;

/// Kinds of tokens (terminals, from the lexer) and nodes (composites, from the
/// parser) in the CST.
///
/// Token kinds come first, node kinds after; `ROOT` is kept **last** so
/// [`BadnessLang::kind_from_raw`] can bounds-check the raw discriminant with a
/// single comparison. Do not add variants after `ROOT`.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum SyntaxKind {
    // --- Tokens (terminals, produced by the lexer) ---
    CONTROL_WORD,   // `\foo`  (backslash + ASCII letters)
    CONTROL_SYMBOL, // `\\`, `\{`, `\%`, `\,` … (backslash + one non-letter)
    L_BRACE,        // {
    R_BRACE,        // }
    L_BRACKET,      // [
    R_BRACKET,      // ]
    DOLLAR,         // $
    AMPERSAND,      // &
    HASH,           // #
    CARET,          // ^
    UNDERSCORE,     // _
    TILDE,          // ~
    COMMENT,        // `% …` up to (not including) the line break
    WHITESPACE,     // spaces / tabs
    NEWLINE,        // `\n`, `\r\n`, or `\r`
    WORD,           // a run of ordinary text characters
    VERB,           // `\verb|…|` / `\verb*|…|` inline verbatim (a single token)
    VERBATIM_BODY,  // the raw body of a verbatim-like environment (a single token)
    DOC_MARGIN,     // a `.dtx` documentation line's leading `%` margin (trivia)
    GUARD,          // a `.dtx` docstrip guard `%<…>` (`%<*t>`/`%</t>`/inline) (trivia)
    ERROR,          // lexer fallback; the lexer is total, so this is unused today

    // --- Nodes (composites, produced by the Phase 1 parser) ---
    GROUP,       // { … }
    OPTIONAL,    // [ … ] optional argument
    ARGUMENT,    // an argument attached to a command
    COMMAND,     // a control sequence with its arguments
    ENVIRONMENT, // \begin{…} … \end{…}
    BEGIN,       // \begin{name}
    END,         // \end{name}
    NAME_GROUP,  // {name} following \begin / \end
    // `\if…\else…\or…\fi`, when the shape gate pairs it. The construct is a run
    // of `CONDITIONAL_BRANCH` nodes followed by the closing `\fi` as the last
    // child, mirroring `ENVIRONMENT > BEGIN … END`. The `\if` test's extent is
    // not statically resolvable (`\ifnum\radius>5` scans ⟨number⟩⟨rel⟩⟨number⟩
    // by TeX's own scanner), so the opener and its test ride the *first* branch
    // rather than a head node of their own.
    CONDITIONAL,
    // One branch of a `CONDITIONAL`. The first holds the opener, its test, and
    // the then-body; every later one *starts with* its `\else`/`\or` divider, so
    // a consumer finds the boundaries positionally and never by command name.
    CONDITIONAL_BRANCH,
    INLINE_MATH,  // $ … $   or   \( … \)
    DISPLAY_MATH, // $$ … $$  or   \[ … \]
    MATH,         // a math body (the atoms between the delimiters)
    SCRIPTED,     // a base atom with attached scripts: base (SUBSCRIPT | SUPERSCRIPT)+
    SUBSCRIPT,    // `_` and its tightly-bound script argument
    SUPERSCRIPT,  // `^` and its tightly-bound script argument
    LEFT_RIGHT,   // `\left( … \right)` — a matched delimiter pair wrapping a MATH body
    PARAGRAPH,    // text delimited by blank lines
    DOC_COMMENT,  // a bound leading-`%` comment run, grouped before its construct
    TEXT,         // a run of text and trivia
    LINE_BREAK,   // `\\`, with a tightly-bound `*` and/or `[len]` (`\\*[2ex]`)
    ROOT,         // the document root  (keep LAST)
}

impl SyntaxKind {
    /// The number of `SyntaxKind` variants. Sound because the enum is
    /// `#[repr(u16)]` with contiguous discriminants `0..=ROOT` and `ROOT` is kept
    /// last; used to size kind-indexed tables (e.g. the linter's dispatch table).
    pub const COUNT: usize = SyntaxKind::ROOT as usize + 1;
}

impl From<SyntaxKind> for rowan::SyntaxKind {
    fn from(kind: SyntaxKind) -> Self {
        Self(kind as u16)
    }
}

/// The rowan language marker for badness's CST.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BadnessLang {}

impl Language for BadnessLang {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        assert!(
            raw.0 <= SyntaxKind::ROOT as u16,
            "invalid SyntaxKind discriminant: {}",
            raw.0
        );
        // SAFETY: `SyntaxKind` is `#[repr(u16)]` with contiguous discriminants
        // `0..=ROOT`, and the assert above bounds `raw.0` into that range.
        unsafe { std::mem::transmute::<u16, SyntaxKind>(raw.0) }
    }

    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        kind.into()
    }
}

pub type SyntaxNode = rowan::SyntaxNode<BadnessLang>;
pub type SyntaxToken = rowan::SyntaxToken<BadnessLang>;
pub type SyntaxElement = rowan::SyntaxElement<BadnessLang>;

/// Whitespace and newlines are the only trivia the formatter rewrites; comments
/// are preserved verbatim and so are *not* collapsible. A shared shape
/// predicate: the formatter's gap normalization and the semantic layer's expl3
/// statement segmentation both skip exactly this class.
pub fn is_collapsible_trivia(kind: SyntaxKind) -> bool {
    matches!(kind, SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE)
}

/// Whether a `WORD` token is a single TeX parameter digit (`1`..=`9`) — the
/// shape that follows `#` in a parameter reference. Reads only the token text.
pub fn is_param_digit(t: &SyntaxToken) -> bool {
    matches!(t.text().as_bytes(), [b'1'..=b'9'])
}
