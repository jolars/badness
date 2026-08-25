//! `SyntaxKind` — the kinds of CST tokens and nodes — and the rowan `Language`
//! binding for badness's BibTeX/BibLaTeX surface CST.
//!
//! BibTeX is a distinct grammar from LaTeX, so it carries its own kind set and
//! [`Language`] marker rather than reusing [`crate::syntax`]. The structure
//! deliberately mirrors that module (`#[repr(u16)]`, tokens first, nodes after,
//! `ROOT` last + `COUNT`) so the two stay easy to compare.

/// Kinds of tokens (terminals, from the lexer) and nodes (composites, from the
/// parser) in the BibTeX CST.
///
/// Token kinds come first, node kinds after; `ROOT` is kept **last** so
/// [`rowan::Language::kind_from_raw`] can bounds-check the raw discriminant with
/// a single comparison. Keep the hidden end marker immediately after `ROOT`.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum SyntaxKind {
    // --- Tokens (terminals, produced by the lexer) ---
    AT,         // @
    L_BRACE,    // {
    R_BRACE,    // }
    L_PAREN,    // (
    R_PAREN,    // )
    COMMA,      // ,
    EQ,         // =
    HASH,       // #  (value concatenation)
    QUOTE,      // "
    PERCENT,    // %  (opens a comment only where the grammar says so)
    WORD,       // a run of identifier / value characters
    NUMBER,     // a pure-digit run (kept distinct from a macro name)
    WHITESPACE, // spaces / tabs
    NEWLINE,    // `\n`, `\r\n`, or `\r`
    ERROR,      // lexer fallback; the lexer is total, so this is unused today

    // --- Nodes (composites, produced by the parser) ---
    COMMENT,        // `%` … end of line, where a comment is legal (see grammar)
    JUNK,           // free text between entries (BibTeX ignores it)
    ENTRY,          // a regular bibliographic entry: @type{ key, fields }
    STRING_ENTRY,   // @string{ name = value }
    PREAMBLE_ENTRY, // @preamble{ value }
    COMMENT_ENTRY,  // @comment{ … }
    ENTRY_TYPE,     // the type word following `@`
    KEY,            // the cite key
    FIELD,          // name = value
    FIELD_NAME,     // the field / macro name on the left of `=`
    VALUE,          // the right-hand side; value pieces separated by `#`
    BRACE_GROUP,    // { … }  (recursive: may nest BRACE_GROUP)
    QUOTED,         // " … "  (may contain nested BRACE_GROUP)
    LITERAL,        // a bare word / number value piece (macro ref or number)
    ROOT,           // the file root  (keep LAST)
    #[doc(hidden)]
    __LAST,
}

impl SyntaxKind {
    /// The number of `SyntaxKind` variants. Sound because the enum is
    /// `#[repr(u16)]` with contiguous discriminants `0..=ROOT` and `ROOT` is kept
    /// last; used to size kind-indexed tables.
    pub const COUNT: usize = SyntaxKind::__LAST as usize;
}

/// The rowan language marker for badness's BibTeX CST.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BibLang {}

impl_rowan_lang!(BibLang, SyntaxKind, "BibTeX");

pub type SyntaxNode = rowan::SyntaxNode<BibLang>;
pub type SyntaxToken = rowan::SyntaxToken<BibLang>;
pub type SyntaxElement = rowan::SyntaxElement<BibLang>;
