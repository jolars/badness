//! The recursive-descent grammar for BibTeX/BibLaTeX surface syntax.
//!
//! The parser walks the full token stream (trivia included) and emits a flat list
//! of [`Event`]s — `Start(kind)` / `Tok(idx)` / `Finish` — that
//! [`super::tree_builder`] replays into a green tree. Because every token is
//! emitted exactly once, in order, via [`Parser::bump`], losslessness holds by
//! construction.
//!
//! It is **error-tolerant**: a malformed construct never aborts the parse. Each
//! recovery records a [`SyntaxError`] on the side channel and either closes the
//! current node gracefully or skips a single token, always making progress.
//! Recovery anchors are the BibTeX-natural ones: `@` (next entry), `}` / `)`
//! (entry close), `,` (next field), and end of input.
//!
//! The three reserved entry types (`@string`, `@preamble`, `@comment`,
//! case-insensitive) genuinely change the grammar, so the parser dispatches on
//! them by name. This is core BibTeX grammar — the analog of the LaTeX parser
//! special-casing `\begin` / `\end` — and reads only the literal type word, **not**
//! a signature database.

use crate::bib::core::SyntaxError;
use crate::bib::events::Event;
use crate::bib::lexer::Token;
use crate::bib::syntax::SyntaxKind;

/// Parse a token stream into parser events and a list of syntax errors.
pub(crate) fn parse(tokens: &[Token]) -> (Vec<Event>, Vec<SyntaxError>) {
    let mut p = Parser::new(tokens);
    p.file();
    (p.events, p.errors)
}

struct Parser<'t> {
    tokens: &'t [Token],
    /// `starts[i]` is the byte offset of token `i`; `starts[len]` is the total
    /// length. Used to give syntax errors byte ranges.
    starts: Vec<usize>,
    pos: usize,
    events: Vec<Event>,
    errors: Vec<SyntaxError>,
}

impl<'t> Parser<'t> {
    fn new(tokens: &'t [Token]) -> Self {
        let mut starts = Vec::with_capacity(tokens.len() + 1);
        let mut off = 0;
        for t in tokens {
            starts.push(off);
            off += t.text.len();
        }
        starts.push(off);
        Self {
            tokens,
            starts,
            pos: 0,
            events: Vec::new(),
            errors: Vec::new(),
        }
    }

    // --- cursor primitives -------------------------------------------------

    fn kind(&self) -> Option<SyntaxKind> {
        self.tokens.get(self.pos).map(|t| t.kind)
    }

    fn at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    fn is_trivia(k: SyntaxKind) -> bool {
        matches!(k, SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE)
    }

    fn at_name(&self) -> bool {
        matches!(self.kind(), Some(SyntaxKind::WORD | SyntaxKind::NUMBER))
    }

    /// The kind of the next non-trivia token at or after the cursor, without
    /// moving it. Used to probe for a `#` value continuation without pulling the
    /// intervening trivia into the current node.
    fn peek_past_trivia(&self) -> Option<SyntaxKind> {
        let mut i = self.pos;
        while self.tokens.get(i).is_some_and(|t| Self::is_trivia(t.kind)) {
            i += 1;
        }
        self.tokens.get(i).map(|t| t.kind)
    }

    // --- event emission ----------------------------------------------------

    fn bump(&mut self) {
        debug_assert!(!self.at_end(), "bump past end of input");
        self.events.push(Event::Tok(self.pos));
        self.pos += 1;
    }

    fn open(&mut self, kind: SyntaxKind) {
        self.events.push(Event::Start(kind));
    }

    fn close(&mut self) {
        self.events.push(Event::Finish);
    }

    fn error(&mut self, message: impl Into<String>) {
        let (start, end) = if self.at_end() {
            let end = *self.starts.last().expect("starts is non-empty");
            (end, end)
        } else {
            (self.starts[self.pos], self.starts[self.pos + 1])
        };
        self.errors.push(SyntaxError {
            message: message.into(),
            start,
            end,
        });
    }

    fn skip_trivia(&mut self) {
        while self.kind().is_some_and(Self::is_trivia) {
            self.bump();
        }
    }

    // --- grammar rules -----------------------------------------------------

    /// The whole file: a sequence of entries interleaved with free text. Trivia
    /// between entries floats as bare tokens under `ROOT`.
    fn file(&mut self) {
        loop {
            self.skip_trivia();
            match self.kind() {
                None => break,
                Some(SyntaxKind::AT) => self.entry(),
                Some(_) => self.junk(),
            }
        }
    }

    /// Free text outside any entry (BibTeX ignores it). Runs up to the next `@`
    /// or end of input.
    fn junk(&mut self) {
        self.open(SyntaxKind::JUNK);
        while !self.at_end() && self.kind() != Some(SyntaxKind::AT) {
            self.bump();
        }
        self.close();
    }

    /// An entry: `@type{ … }` or `@type( … )`. The node kind is chosen from the
    /// (case-insensitive) type word; the three reserved forms parse differently.
    fn entry(&mut self) {
        let node_kind = match self.peek_entry_type().to_ascii_lowercase().as_str() {
            "string" => SyntaxKind::STRING_ENTRY,
            "preamble" => SyntaxKind::PREAMBLE_ENTRY,
            "comment" => SyntaxKind::COMMENT_ENTRY,
            _ => SyntaxKind::ENTRY,
        };
        self.open(node_kind);
        self.bump(); // `@`
        self.skip_trivia();

        if self.kind() == Some(SyntaxKind::WORD) {
            self.open(SyntaxKind::ENTRY_TYPE);
            self.bump();
            self.close();
        } else {
            self.error("expected an entry type after `@`");
            self.close();
            return;
        }
        self.skip_trivia();

        let closer = match self.kind() {
            Some(SyntaxKind::L_BRACE) => {
                self.bump();
                SyntaxKind::R_BRACE
            }
            Some(SyntaxKind::L_PAREN) => {
                self.bump();
                SyntaxKind::R_PAREN
            }
            _ => {
                self.error("expected `{` or `(` after the entry type");
                self.close();
                return;
            }
        };

        match node_kind {
            SyntaxKind::STRING_ENTRY => self.string_body(closer),
            SyntaxKind::PREAMBLE_ENTRY => self.preamble_body(closer),
            SyntaxKind::COMMENT_ENTRY => self.comment_body(closer),
            _ => self.entry_body(closer),
        }
        self.close();
    }

    /// Peek the entry-type word that follows the `@` at the cursor (skipping
    /// trivia), without consuming. Empty if there is no word.
    fn peek_entry_type(&self) -> String {
        let mut i = self.pos + 1; // skip `@`
        while self.tokens.get(i).is_some_and(|t| Self::is_trivia(t.kind)) {
            i += 1;
        }
        match self.tokens.get(i) {
            Some(t) if t.kind == SyntaxKind::WORD => t.text.to_string(),
            _ => String::new(),
        }
    }

    /// The body of a regular entry: an optional cite key, then a comma-separated
    /// list of fields, then the closing delimiter.
    fn entry_body(&mut self, closer: SyntaxKind) {
        self.skip_trivia();
        // A cite key is present unless the first item is already a `name =` field
        // (a malformed key-less entry) or the body is empty.
        if self.at_name() && !self.looks_like_field() {
            self.open(SyntaxKind::KEY);
            while self.at_name() {
                self.bump();
            }
            self.close();
        }

        // Fields must be `,`-separated. After parsing one, a comma is required
        // before the next; a field that follows another with no comma between is
        // a syntax error (biber rejects it) that we recover from by parsing it as
        // a fresh field anyway.
        let mut need_comma = false;
        loop {
            self.skip_trivia();
            match self.kind() {
                None => {
                    self.error("unterminated entry");
                    break;
                }
                Some(k) if k == closer => {
                    self.bump();
                    break;
                }
                Some(SyntaxKind::AT) => {
                    // A new entry started before this one closed: leave the `@`
                    // for `file` to pick up.
                    self.error("unterminated entry");
                    break;
                }
                Some(SyntaxKind::COMMA) => {
                    self.bump(); // separator / trailing comma
                    need_comma = false;
                }
                Some(_) => {
                    if need_comma {
                        self.error("expected `,` between fields");
                    }
                    // Only a complete `name = value` field requires a separator
                    // before the next; a malformed one already carries its own
                    // diagnostic, so don't pile a spurious comma error on top.
                    need_comma = self.field(closer);
                }
            }
        }
    }

    /// True if the cursor sits on a `name … =` field opener rather than a key.
    fn looks_like_field(&self) -> bool {
        let mut i = self.pos;
        while self
            .tokens
            .get(i)
            .is_some_and(|t| matches!(t.kind, SyntaxKind::WORD | SyntaxKind::NUMBER))
        {
            i += 1;
        }
        while self.tokens.get(i).is_some_and(|t| Self::is_trivia(t.kind)) {
            i += 1;
        }
        self.tokens.get(i).map(|t| t.kind) == Some(SyntaxKind::EQ)
    }

    /// A `name = value` field. Always consumes at least one token. Returns whether
    /// it parsed a *complete* field (name, `=`, value); a malformed field carries
    /// its own diagnostic, so the caller does not also demand a separator after it.
    fn field(&mut self, closer: SyntaxKind) -> bool {
        self.open(SyntaxKind::FIELD);
        if !self.at_name() {
            // A stray token where a field name was expected: report and skip it
            // so the surrounding loop makes progress.
            self.error("expected a field name");
            self.bump();
            self.close();
            return false;
        }

        self.open(SyntaxKind::FIELD_NAME);
        while self.at_name() {
            self.bump();
        }
        self.close();
        self.skip_trivia();

        let complete = if self.kind() == Some(SyntaxKind::EQ) {
            self.bump();
            self.skip_trivia();
            self.value(closer);
            true
        } else {
            self.error("expected `=` after the field name");
            false
        };
        self.close();
        complete
    }

    /// A field value: one or more pieces joined by `#`.
    fn value(&mut self, closer: SyntaxKind) {
        self.open(SyntaxKind::VALUE);
        loop {
            self.value_piece();
            // Probe for a `#` continuation *past* trivia without consuming it: the
            // trivia is intra-value only when a `#` actually follows. Otherwise it
            // is trailing trivia that floats at the entry level (like inter-field
            // trivia), so we leave it for the caller and the VALUE ends at the
            // piece's last real token.
            if self.peek_past_trivia() != Some(SyntaxKind::HASH) {
                break;
            }
            self.skip_trivia();
            self.bump(); // `#`
            self.skip_trivia();
            // Defend against a `#` with nothing after it before the closer/EOF.
            if self.at_end() || self.kind() == Some(closer) {
                break;
            }
        }
        self.close();
    }

    /// A single value piece: a brace group, a quoted string, or a bare literal.
    fn value_piece(&mut self) {
        match self.kind() {
            Some(SyntaxKind::L_BRACE) => self.brace_group(),
            Some(SyntaxKind::QUOTE) => self.quoted(),
            Some(SyntaxKind::WORD | SyntaxKind::NUMBER) => {
                self.open(SyntaxKind::LITERAL);
                self.bump();
                self.close();
            }
            _ => self.error("expected a value"),
        }
    }

    /// A balanced `{ … }` group. Braces nest; everything else is raw content.
    fn brace_group(&mut self) {
        self.open(SyntaxKind::BRACE_GROUP);
        self.bump(); // `{`
        loop {
            match self.kind() {
                None => {
                    self.error("unterminated `{`");
                    break;
                }
                Some(SyntaxKind::R_BRACE) => {
                    self.bump();
                    break;
                }
                Some(SyntaxKind::L_BRACE) => self.brace_group(),
                Some(_) => self.bump(),
            }
        }
        self.close();
    }

    /// A `" … "` quoted string. Inner brace groups balance and protect a `"`, so
    /// only a `"` at brace-depth 0 closes the string.
    fn quoted(&mut self) {
        self.open(SyntaxKind::QUOTED);
        self.bump(); // opening `"`
        loop {
            match self.kind() {
                None => {
                    self.error("unterminated `\"`");
                    break;
                }
                Some(SyntaxKind::QUOTE) => {
                    self.bump();
                    break;
                }
                Some(SyntaxKind::L_BRACE) => self.brace_group(),
                Some(_) => self.bump(),
            }
        }
        self.close();
    }

    /// `@string{ name = value }`: a single field, then the closer.
    fn string_body(&mut self, closer: SyntaxKind) {
        self.skip_trivia();
        if self.at_name() {
            self.field(closer);
        } else if self.kind() != Some(closer) && !self.at_end() {
            self.error("expected `name = value` in @string");
        }
        self.expect_close(closer);
    }

    /// `@preamble{ value }`: a lone value, then the closer.
    fn preamble_body(&mut self, closer: SyntaxKind) {
        self.skip_trivia();
        if self.kind() != Some(closer) && !self.at_end() {
            self.value(closer);
        }
        self.expect_close(closer);
    }

    /// `@comment{ … }`: balanced raw content up to the matching closer.
    fn comment_body(&mut self, closer: SyntaxKind) {
        loop {
            match self.kind() {
                None => {
                    self.error("unterminated @comment");
                    break;
                }
                Some(k) if k == closer => {
                    self.bump();
                    break;
                }
                Some(SyntaxKind::L_BRACE) => self.brace_group(),
                Some(_) => self.bump(),
            }
        }
    }

    /// Consume the closing delimiter, reporting if it is missing.
    fn expect_close(&mut self, closer: SyntaxKind) {
        self.skip_trivia();
        match self.kind() {
            Some(k) if k == closer => self.bump(),
            None => self.error("unterminated entry"),
            _ => self.error("expected the closing delimiter"),
        }
    }
}
