//! `SyntaxKind` — the kinds of CST tokens and nodes — and the rowan `Language`
//! binding for knuth's LaTeX surface CST.

use rowan::Language;

/// Kinds of tokens (terminals, from the lexer) and nodes (composites, from the
/// parser) in the CST.
///
/// Token kinds come first, node kinds after; `ROOT` is kept **last** so
/// [`KnuthLang::kind_from_raw`] can bounds-check the raw discriminant with a
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
    ERROR,          // lexer fallback; the lexer is total, so this is unused today

    // --- Nodes (composites, produced by the Phase 1 parser) ---
    GROUP,        // { … }
    OPTIONAL,     // [ … ] optional argument
    ARGUMENT,     // an argument attached to a command
    COMMAND,      // a control sequence with its arguments
    ENVIRONMENT,  // \begin{…} … \end{…}
    BEGIN,        // \begin{name}
    END,          // \end{name}
    NAME_GROUP,   // {name} following \begin / \end
    INLINE_MATH,  // $ … $   or   \( … \)
    DISPLAY_MATH, // $$ … $$  or   \[ … \]
    MATH,         // a math body
    PARAGRAPH,    // text delimited by blank lines
    TEXT,         // a run of text and trivia
    ROOT,         // the document root  (keep LAST)
}

impl From<SyntaxKind> for rowan::SyntaxKind {
    fn from(kind: SyntaxKind) -> Self {
        Self(kind as u16)
    }
}

/// The rowan language marker for knuth's CST.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KnuthLang {}

impl Language for KnuthLang {
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

pub type SyntaxNode = rowan::SyntaxNode<KnuthLang>;
pub type SyntaxToken = rowan::SyntaxToken<KnuthLang>;
pub type SyntaxElement = rowan::SyntaxElement<KnuthLang>;
