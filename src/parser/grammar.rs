//! The Phase 1 recursive-descent grammar for LaTeX surface syntax.
//!
//! The parser walks the full token stream (trivia included) and emits a flat
//! list of [`Event`]s — `Start(kind)` / `Tok(idx)` / `Finish` — that
//! [`super::tree_builder`] replays into a green tree. Because every token is
//! emitted exactly once, in order, via [`Parser::bump`], losslessness holds by
//! construction: `pos` only ever advances through `bump`, and nothing else
//! touches it.
//!
//! It is **error-tolerant**: a malformed construct never aborts the parse. Each
//! recovery records a [`SyntaxError`] on the side channel and either closes the
//! current node gracefully or skips a single token, always making progress.
//! Recovery anchors are the LaTeX-natural ones: `\end`, `}`, `]`, `$`, blank
//! lines, and end of input.

use crate::parser::core::SyntaxError;
use crate::parser::events::Event;
use crate::parser::lexer::{Token, is_verbatim_environment};
use crate::syntax::SyntaxKind;

const BEGIN_CMD: &str = "\\begin";
const END_CMD: &str = "\\end";

/// A content region that groups its children into `PARAGRAPH` nodes separated
/// by blank lines. Differs only in how the region terminates.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    /// The whole document; ends at EOF.
    Document,
    /// An environment body; ends at the next `\end` (any name — the caller
    /// checks the name and decides whether to consume it).
    Environment,
}

/// Parse a token stream into parser events and a list of syntax errors.
pub(crate) fn parse(tokens: &[Token]) -> (Vec<Event>, Vec<SyntaxError>) {
    let mut p = Parser::new(tokens);
    p.document();
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

    fn nth_kind(&self, n: usize) -> Option<SyntaxKind> {
        self.tokens.get(self.pos + n).map(|t| t.kind)
    }

    fn text(&self) -> &str {
        self.tokens
            .get(self.pos)
            .map(|t| t.text.as_str())
            .unwrap_or("")
    }

    fn at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    fn at_command(&self, name: &str) -> bool {
        self.kind() == Some(SyntaxKind::CONTROL_WORD) && self.text() == name
    }

    fn is_trivia(k: SyntaxKind) -> bool {
        matches!(
            k,
            SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
        )
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

    /// Peek the kind of the next non-trivia token and whether the intervening
    /// trivia contains a paragraph break (a blank line, i.e. ≥2 newlines).
    /// Does not consume.
    fn peek_meaningful(&self) -> (Option<SyntaxKind>, bool) {
        let mut i = self.pos;
        let mut newlines = 0;
        while let Some(t) = self.tokens.get(i) {
            match t.kind {
                SyntaxKind::NEWLINE => newlines += 1,
                SyntaxKind::WHITESPACE | SyntaxKind::COMMENT => {}
                k => return (Some(k), newlines >= 2),
            }
            i += 1;
        }
        (None, newlines >= 2)
    }

    /// True if a paragraph break (blank line) begins at the current position.
    fn at_paragraph_break(&self) -> bool {
        let mut i = self.pos;
        let mut newlines = 0;
        while let Some(t) = self.tokens.get(i) {
            match t.kind {
                SyntaxKind::NEWLINE => {
                    newlines += 1;
                    if newlines >= 2 {
                        return true;
                    }
                }
                SyntaxKind::WHITESPACE | SyntaxKind::COMMENT => {}
                _ => return false,
            }
            i += 1;
        }
        false
    }

    // --- grammar -----------------------------------------------------------

    fn document(&mut self) {
        self.parse_block(Block::Document);
    }

    /// Parse a content region, grouping runs of content into `PARAGRAPH` nodes
    /// delimited by blank lines (the TeX `\par` boundary). Blank-line trivia
    /// (and any trailing trivia) sits between paragraphs as direct children of
    /// the enclosing node, not inside a paragraph.
    fn parse_block(&mut self, block: Block) {
        loop {
            if self.at_block_end(block) {
                break;
            }
            // Separator trivia (blank lines / trailing whitespace) is emitted
            // directly, never wrapped in a paragraph.
            if self.kind().is_some_and(Self::is_trivia) && self.trivia_run_is_separator(block) {
                self.skip_trivia();
                continue;
            }
            // Otherwise we're at paragraph content (guaranteed ≥1 token, so no
            // empty paragraph and no infinite loop).
            self.open(SyntaxKind::PARAGRAPH);
            loop {
                if self.at_block_end(block) {
                    break;
                }
                if self.kind().is_some_and(Self::is_trivia) && self.trivia_run_is_separator(block) {
                    break;
                }
                self.element();
            }
            self.close();
        }
    }

    fn at_block_end(&self, block: Block) -> bool {
        self.at_end() || (block == Block::Environment && self.at_command(END_CMD))
    }

    /// True if the contiguous trivia run at the current position should separate
    /// paragraphs: it contains a blank line, or only trivia remains before the
    /// block terminator (the `\end`, or EOF).
    fn trivia_run_is_separator(&self, block: Block) -> bool {
        let mut i = self.pos;
        let mut newlines = 0;
        while let Some(t) = self.tokens.get(i) {
            match t.kind {
                SyntaxKind::NEWLINE => newlines += 1,
                SyntaxKind::WHITESPACE | SyntaxKind::COMMENT => {}
                SyntaxKind::CONTROL_WORD if block == Block::Environment && t.text == END_CMD => {
                    return true;
                }
                _ => return newlines >= 2,
            }
            i += 1;
        }
        true
    }

    /// One element in text mode. Always consumes at least one token.
    fn element(&mut self) {
        let Some(k) = self.kind() else { return };
        match k {
            SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT => self.bump(),
            SyntaxKind::CONTROL_WORD => {
                if self.at_command(BEGIN_CMD) {
                    self.environment();
                } else if self.at_command(END_CMD) {
                    self.stray_end();
                } else {
                    self.command();
                }
            }
            SyntaxKind::CONTROL_SYMBOL => {
                let sym = self.text().to_owned();
                match sym.as_str() {
                    "\\[" => self.delim_math(SyntaxKind::DISPLAY_MATH, "\\[", "\\]"),
                    "\\(" => self.delim_math(SyntaxKind::INLINE_MATH, "\\(", "\\)"),
                    "\\]" | "\\)" => {
                        self.error(format!("unmatched `{sym}`"));
                        self.bump();
                    }
                    // `\\` line break, with its tightly-bound `*` / `[len]`.
                    "\\\\" => self.line_break(),
                    // Any other bare control symbol (`\,`, `\%`, `\;`, …). Surface
                    // model: emit as a token; these take no arguments.
                    _ => self.bump(),
                }
            }
            SyntaxKind::L_BRACE => self.group(),
            SyntaxKind::R_BRACE => {
                self.error("unmatched `}`");
                self.bump();
            }
            SyntaxKind::DOLLAR => self.dollar_math(),
            // WORD, brackets, & # ^ _ ~, ERROR: ordinary tokens in text mode.
            _ => self.bump(),
        }
    }

    /// `\foo` followed by its greedily-attached argument groups.
    ///
    /// Arity is unknown without the semantic layer, so we attach every trailing
    /// `{…}` / `[…]` group, allowing intervening trivia but stopping at a
    /// paragraph break (see `AGENTS.md`, Core decision #8).
    fn command(&mut self) {
        self.open(SyntaxKind::COMMAND);
        self.bump(); // the control word
        self.attach_arguments();
        self.close();
    }

    /// The `\\` line break and its tightly-bound modifiers: an optional `*`
    /// (no-page-break variant) and an optional `[length]` (`\\`, `\\*`,
    /// `\\[2ex]`, `\\*[2ex]`). These bind to the `\\` only when they *directly*
    /// abut it — no intervening trivia is crossed — so a lone `\\` at end of line
    /// stays bare and the modifiers are never pulled across a break. Grouping
    /// them into one `LINE_BREAK` node (rather than leaving loose tokens) is what
    /// lets the formatter treat `\\[2ex]` as one unit instead of stranding the
    /// `[2ex]` on the next line.
    ///
    /// Unlike `command`, this attaches *no* `{…}` arguments (`\\` takes none) and
    /// does not skip trivia. The `*` is recognized only as its own `WORD` token
    /// (the lexer glues `*` into following letters, so `\\*foo` keeps the star on
    /// the word — a vanishingly rare form we deliberately leave alone).
    fn line_break(&mut self) {
        self.open(SyntaxKind::LINE_BREAK);
        self.bump(); // \\
        if self.kind() == Some(SyntaxKind::WORD) && self.text() == "*" {
            self.bump(); // *
        }
        if self.kind() == Some(SyntaxKind::L_BRACKET) {
            self.optional(); // [length]
        }
        self.close();
    }

    /// Greedily attach trailing `{…}` / `[…]` argument groups to the currently
    /// open node, allowing intervening trivia but stopping at a paragraph break.
    /// Shared by `\foo` commands and `\begin{env}` (see `AGENTS.md`, Core
    /// decision #8). Arity is unknown without the semantic layer.
    fn attach_arguments(&mut self) {
        loop {
            let (next, paragraph_break) = self.peek_meaningful();
            if paragraph_break {
                break;
            }
            match next {
                Some(SyntaxKind::L_BRACE) => {
                    self.skip_trivia();
                    self.group();
                }
                Some(SyntaxKind::L_BRACKET) => {
                    self.skip_trivia();
                    self.optional();
                }
                _ => break,
            }
        }
    }

    /// A brace group `{ … }`.
    fn group(&mut self) {
        debug_assert_eq!(self.kind(), Some(SyntaxKind::L_BRACE));
        self.open(SyntaxKind::GROUP);
        self.bump(); // {
        loop {
            match self.kind() {
                None => {
                    self.error("unclosed `{`");
                    break;
                }
                Some(SyntaxKind::R_BRACE) => {
                    self.bump();
                    break;
                }
                _ => self.element(),
            }
        }
        self.close();
    }

    /// An optional-argument group `[ … ]`.
    ///
    /// `[` and `]` are not real grouping in TeX, so this is heuristic: it ends
    /// at the first `]`, and bails defensively (rather than swallowing the
    /// document) on a `}`, a `\begin`/`\end`, a paragraph break, or EOF.
    fn optional(&mut self) {
        debug_assert_eq!(self.kind(), Some(SyntaxKind::L_BRACKET));
        self.open(SyntaxKind::OPTIONAL);
        self.bump(); // [
        loop {
            match self.kind() {
                None | Some(SyntaxKind::R_BRACE) => {
                    self.error("unclosed `[`");
                    break;
                }
                Some(SyntaxKind::R_BRACKET) => {
                    self.bump();
                    break;
                }
                Some(SyntaxKind::CONTROL_WORD)
                    if self.at_command(BEGIN_CMD) || self.at_command(END_CMD) =>
                {
                    self.error("unclosed `[`");
                    break;
                }
                _ => {
                    if self.at_paragraph_break() {
                        self.error("unclosed `[`");
                        break;
                    }
                    self.element();
                }
            }
        }
        self.close();
    }

    /// Inline `$ … $` or display `$$ … $$` math.
    fn dollar_math(&mut self) {
        let display = self.nth_kind(1) == Some(SyntaxKind::DOLLAR);
        let (kind, label) = if display {
            (SyntaxKind::DISPLAY_MATH, "$$")
        } else {
            (SyntaxKind::INLINE_MATH, "$")
        };
        self.open(kind);
        self.bump(); // $
        if display {
            self.bump(); // second $
        }
        loop {
            match self.kind() {
                None => {
                    self.error(format!("unclosed `{label}`"));
                    break;
                }
                // `}` and `\end` are recovery anchors: `$`-math cannot span a
                // group or environment boundary, so a `}` here closes the
                // enclosing group (a math subgroup would have entered via `{`)
                // and a `\end` belongs to an enclosing environment. Leave the
                // token for the caller and report the unclosed math.
                Some(SyntaxKind::R_BRACE) => {
                    self.error(format!("unclosed `{label}`"));
                    break;
                }
                Some(SyntaxKind::CONTROL_WORD) if self.at_command(END_CMD) => {
                    self.error(format!("unclosed `{label}`"));
                    break;
                }
                Some(SyntaxKind::DOLLAR) => {
                    if display && self.nth_kind(1) != Some(SyntaxKind::DOLLAR) {
                        // A lone `$` inside `$$`: malformed; emit and continue.
                        self.bump();
                        continue;
                    }
                    self.bump(); // closing $
                    if display {
                        self.bump(); // second closing $
                    }
                    break;
                }
                _ => {
                    if self.at_paragraph_break() {
                        self.error(format!("unclosed `{label}`"));
                        break;
                    }
                    self.element();
                }
            }
        }
        self.close();
    }

    /// Delimited math: `\[ … \]` (display) or `\( … \)` (inline).
    fn delim_math(&mut self, kind: SyntaxKind, opener: &str, closer: &str) {
        self.open(kind);
        self.bump(); // \[ or \(
        loop {
            match self.kind() {
                None => {
                    self.error(format!("unclosed `{opener}`"));
                    break;
                }
                Some(SyntaxKind::CONTROL_SYMBOL) if self.text() == closer => {
                    self.bump();
                    break;
                }
                // A `}` closes an enclosing group: it cannot belong to this
                // math (a subgroup would have entered via `{`). Leave it for
                // the caller and report the unclosed math.
                Some(SyntaxKind::R_BRACE) => {
                    self.error(format!("unclosed `{opener}`"));
                    break;
                }
                Some(SyntaxKind::CONTROL_WORD) if self.at_command(END_CMD) => {
                    self.error(format!("unclosed `{opener}`"));
                    break;
                }
                _ => {
                    if self.at_paragraph_break() {
                        self.error(format!("unclosed `{opener}`"));
                        break;
                    }
                    self.element();
                }
            }
        }
        self.close();
    }

    /// `\begin{name} … \end{name}`, with environment-mismatch recovery.
    fn environment(&mut self) {
        self.open(SyntaxKind::ENVIRONMENT);

        self.open(SyntaxKind::BEGIN);
        self.bump(); // \begin
        let name = self.name_group();
        self.attach_arguments(); // `\begin{tabular}{ll}`, `[options]`, etc.
        self.close(); // BEGIN

        if name.as_deref().is_some_and(is_verbatim_environment) {
            self.verbatim_body(name.as_deref().expect("verbatim name"));
        } else {
            self.parse_block(Block::Environment);
        }
        self.finish_environment(&name);
    }

    /// Consume the matching `\end`, or recover. `parse_block` / `verbatim_body`
    /// leave the cursor at a `\end` or at EOF.
    fn finish_environment(&mut self, name: &Option<String>) {
        match self.kind() {
            None => {
                self.error(format!(
                    "unclosed environment `{}`",
                    name.as_deref().unwrap_or("")
                ));
            }
            // The cursor is at a `\end` (the only non-EOF stop condition).
            Some(_) => {
                let end_name = peek_end_name(self.tokens, self.pos);
                if name.is_none() || *name == end_name {
                    // Matching \end: consume it as our END.
                    self.open(SyntaxKind::END);
                    self.bump(); // \end
                    self.name_group();
                    self.close();
                } else {
                    // Mismatched \end: it belongs to an enclosing environment.
                    // Close this one with a diagnostic and leave the \end for
                    // the caller (this unwinds the stack until some level
                    // matches, or it becomes a stray \end at the root).
                    self.error(format!(
                        "unclosed environment `{}` (found `\\end{{{}}}`)",
                        name.as_deref().unwrap_or(""),
                        end_name.as_deref().unwrap_or("")
                    ));
                }
            }
        }
        self.close(); // ENVIRONMENT
    }

    /// The raw body of a verbatim-like environment: consume tokens unstructured
    /// until the matching `\end{name}`. The lexer has already collapsed the body
    /// into a single `VERBATIM_BODY` token; this loop also serves as a fallback.
    fn verbatim_body(&mut self, name: &str) {
        loop {
            match self.kind() {
                None => break,
                Some(SyntaxKind::CONTROL_WORD)
                    if self.at_command(END_CMD)
                        && peek_end_name(self.tokens, self.pos).as_deref() == Some(name) =>
                {
                    break;
                }
                _ => self.bump(),
            }
        }
    }

    /// A `\end` with no matching open environment at this level.
    fn stray_end(&mut self) {
        self.error("`\\end` without matching `\\begin`");
        self.open(SyntaxKind::END);
        self.bump(); // \end
        self.name_group();
        self.close();
    }

    /// The `{name}` group following `\begin` / `\end`. Returns the trimmed name.
    fn name_group(&mut self) -> Option<String> {
        self.skip_trivia();
        if self.kind() != Some(SyntaxKind::L_BRACE) {
            self.error("expected `{` for environment name");
            return None;
        }
        self.open(SyntaxKind::NAME_GROUP);
        self.bump(); // {
        let mut name = String::new();
        loop {
            match self.kind() {
                None => {
                    self.error("unclosed environment name");
                    break;
                }
                Some(SyntaxKind::R_BRACE) => {
                    self.bump();
                    break;
                }
                _ => {
                    name.push_str(self.text());
                    self.bump();
                }
            }
        }
        self.close();
        Some(name.trim().to_owned())
    }
}

/// Read the environment name from a `\end{…}` at `end_pos` without consuming.
fn peek_end_name(tokens: &[Token], end_pos: usize) -> Option<String> {
    let mut i = end_pos + 1; // past the \end control word
    while tokens.get(i).is_some_and(|t| Parser::is_trivia(t.kind)) {
        i += 1;
    }
    if tokens.get(i).map(|t| t.kind) != Some(SyntaxKind::L_BRACE) {
        return None;
    }
    i += 1;
    let mut name = String::new();
    while let Some(t) = tokens.get(i) {
        if t.kind == SyntaxKind::R_BRACE {
            break;
        }
        name.push_str(&t.text);
        i += 1;
    }
    Some(name.trim().to_owned())
}
