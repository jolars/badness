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
const LEFT_CMD: &str = "\\left";
const RIGHT_CMD: &str = "\\right";

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

    /// Inline `$ … $` or display `$$ … $$` math. The body's atoms are wrapped in
    /// a `MATH` node (the delimiters stay direct children of the math node); the
    /// atoms themselves are parsed in math mode (see [`Self::math_element`]).
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
        self.open(SyntaxKind::MATH);
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
                    // The closing delimiter belongs to the math node, not its
                    // body: break and bump it after closing `MATH`.
                    break;
                }
                _ => {
                    if self.at_paragraph_break() {
                        self.error(format!("unclosed `{label}`"));
                        break;
                    }
                    self.math_element();
                }
            }
        }
        self.close(); // MATH
        if self.kind() == Some(SyntaxKind::DOLLAR) {
            self.bump(); // closing $
            if display {
                self.bump(); // second closing $
            }
        }
        self.close(); // INLINE_MATH / DISPLAY_MATH
    }

    /// Delimited math: `\[ … \]` (display) or `\( … \)` (inline). As with
    /// [`Self::dollar_math`], the body's atoms are wrapped in a `MATH` node and
    /// parsed in math mode.
    fn delim_math(&mut self, kind: SyntaxKind, opener: &str, closer: &str) {
        self.open(kind);
        self.bump(); // \[ or \(
        self.open(SyntaxKind::MATH);
        loop {
            match self.kind() {
                None => {
                    self.error(format!("unclosed `{opener}`"));
                    break;
                }
                Some(SyntaxKind::CONTROL_SYMBOL) if self.text() == closer => {
                    // The closer belongs to the math node, not its body.
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
                    self.math_element();
                }
            }
        }
        self.close(); // MATH
        if self.kind() == Some(SyntaxKind::CONTROL_SYMBOL) && self.text() == closer {
            self.bump(); // \] or \)
        }
        self.close(); // INLINE_MATH / DISPLAY_MATH
    }

    /// One element inside a math body. Trivia is emitted inline (for
    /// losslessness); everything else is an atom, possibly carrying `^`/`_`
    /// scripts (see [`Self::math_scripted`]). Callers guard the math closers and
    /// recovery anchors before invoking this, so the cursor is at body content.
    fn math_element(&mut self) {
        match self.kind() {
            Some(SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT) => self.bump(),
            _ => self.math_scripted(),
        }
    }

    /// A base atom with any tightly-bound `^`/`_` scripts — the one sanctioned
    /// Pratt site (`AGENTS.md`, decision #3). Sub/superscripts are postfix with a
    /// single-atom right operand, so this is a base atom followed by a postfix
    /// loop, not full precedence climbing.
    ///
    /// We only wrap the base in a `SCRIPTED` node when a script actually
    /// attaches, so an unscripted atom stays a bare token/node (matching the
    /// `LINE_BREAK`-only-when-modifiers idiom). Because the base atom's extent is
    /// not known until parsed (a command greedily attaches its args), we parse it
    /// first and, if a script follows, retroactively splice a `SCRIPTED` start
    /// event in front of it — the event-stream analog of rust-analyzer's
    /// `precede`, done locally without touching the event layer.
    fn math_scripted(&mut self) {
        let checkpoint = self.events.len();
        self.math_atom();
        if !self.at_script() {
            return; // bare atom, no wrapper
        }
        self.events
            .insert(checkpoint, Event::Start(SyntaxKind::SCRIPTED));
        while self.at_script() {
            self.skip_trivia(); // trivia between base/scripts rides inside SCRIPTED
            let sub = self.kind() == Some(SyntaxKind::UNDERSCORE);
            self.open(if sub {
                SyntaxKind::SUBSCRIPT
            } else {
                SyntaxKind::SUPERSCRIPT
            });
            self.bump(); // `_` or `^`
            self.math_script_arg();
            self.close();
        }
        self.close(); // SCRIPTED
    }

    /// True if a `^`/`_` script operator directly follows, skipping only
    /// `WHITESPACE`/`NEWLINE` (not a comment, which must end its line — so a
    /// script never binds across a comment) and not a blank line (a paragraph
    /// break ends the math).
    fn at_script(&self) -> bool {
        let mut i = self.pos;
        let mut newlines = 0;
        while let Some(t) = self.tokens.get(i) {
            match t.kind {
                SyntaxKind::NEWLINE => {
                    newlines += 1;
                    if newlines >= 2 {
                        return false;
                    }
                    i += 1;
                }
                SyntaxKind::WHITESPACE => i += 1,
                SyntaxKind::CARET | SyntaxKind::UNDERSCORE => return true,
                _ => return false,
            }
        }
        false
    }

    /// A single base atom: a `{…}` group (parsed in math mode), a command with
    /// its greedily-attached arguments, an environment, a `\\` line break, or one
    /// ordinary token. Always consumes ≥1 token when the cursor is at content.
    fn math_atom(&mut self) {
        match self.kind() {
            Some(SyntaxKind::L_BRACE) => self.math_group(),
            Some(SyntaxKind::CONTROL_WORD) => {
                if self.at_command(BEGIN_CMD) {
                    self.environment();
                } else if self.at_command(END_CMD) {
                    self.stray_end();
                } else if self.at_command(LEFT_CMD) {
                    self.left_right();
                } else if self.at_command(RIGHT_CMD) {
                    self.stray_right();
                } else {
                    self.command();
                }
            }
            // `\\` line break (with its tightly-bound `*`/`[len]`) vs. a bare
            // control symbol (`\,`, `\;`, `\!`, spacing) — emit the latter as a
            // single token.
            Some(SyntaxKind::CONTROL_SYMBOL) if self.text() == "\\\\" => self.line_break(),
            // Any other single token (WORD, digit, `&`, `~`, `#`, brackets, a
            // bare control symbol, or a `^`/`_` with no base): one token, so the
            // loop always makes progress.
            Some(_) => self.bump(),
            None => {}
        }
    }

    /// One script argument: a single atom (a `{…}` group, a command with its
    /// args, or one token). A missing argument (the next meaningful token is a
    /// closer, `\end`, a paragraph break, or EOF) is reported, not consumed —
    /// the closer must stay for the enclosing math loop.
    fn math_script_arg(&mut self) {
        if self.at_paragraph_break() {
            self.error("missing argument after `^`/`_`");
            return;
        }
        self.skip_trivia();
        let missing = match self.kind() {
            None | Some(SyntaxKind::R_BRACE | SyntaxKind::DOLLAR) => true,
            Some(SyntaxKind::CONTROL_SYMBOL) => matches!(self.text(), "\\]" | "\\)"),
            Some(SyntaxKind::CONTROL_WORD) => self.at_command(END_CMD),
            _ => false,
        };
        if missing {
            self.error("missing argument after `^`/`_`");
            return;
        }
        self.math_atom();
    }

    /// A brace group `{ … }` whose body is parsed in math mode (so `x^{a_b}`
    /// nests). Recovery mirrors [`Self::group`].
    fn math_group(&mut self) {
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
                _ => self.math_element(),
            }
        }
        self.close();
    }

    /// A `\left<delim> … \right<delim>` matched delimiter pair (`AGENTS.md`,
    /// decision #3: the one precedence-climbing site — here just balanced
    /// matching by *count*, which is exactly how TeX pairs them, so a mismatched
    /// `\left( … \right]` still nests correctly). The `\left`/`\right` control
    /// words and their delimiter tokens are direct children (mirroring how `$` /
    /// `\[` delimiters stay direct children of the math node); the enclosed atoms
    /// are wrapped in a `MATH` body. Nested pairs recurse via [`Self::math_atom`].
    ///
    /// An unclosed `\left` recovers at the enclosing math/group/environment
    /// closer (the same anchors the surrounding math loop uses), leaving that
    /// token for the caller.
    fn left_right(&mut self) {
        debug_assert!(self.at_command(LEFT_CMD));
        self.open(SyntaxKind::LEFT_RIGHT);
        self.bump(); // \left
        self.math_delim(LEFT_CMD);
        self.open(SyntaxKind::MATH);
        loop {
            match self.kind() {
                None => {
                    self.error("unclosed `\\left`");
                    break;
                }
                Some(SyntaxKind::CONTROL_WORD) if self.at_command(RIGHT_CMD) => break,
                // Enclosing-scope closers: `\left … \right` cannot span a group,
                // math, or environment boundary, so hand the token back.
                Some(SyntaxKind::R_BRACE | SyntaxKind::DOLLAR) => {
                    self.error("unclosed `\\left`");
                    break;
                }
                Some(SyntaxKind::CONTROL_SYMBOL) if matches!(self.text(), "\\]" | "\\)") => {
                    self.error("unclosed `\\left`");
                    break;
                }
                Some(SyntaxKind::CONTROL_WORD) if self.at_command(END_CMD) => {
                    self.error("unclosed `\\left`");
                    break;
                }
                _ => {
                    if self.at_paragraph_break() {
                        self.error("unclosed `\\left`");
                        break;
                    }
                    self.math_element();
                }
            }
        }
        self.close(); // MATH
        if self.at_command(RIGHT_CMD) {
            self.bump(); // \right
            self.math_delim(RIGHT_CMD);
        }
        self.close(); // LEFT_RIGHT
    }

    /// Consume the single delimiter token following `\left`/`\right`: skip inline
    /// trivia (it rides as a direct child of the pair for losslessness; the
    /// formatter drops it), then take one token. The lexer has already isolated a
    /// word-character delimiter (`(`, `|`, `.`, …) into its own token, so a single
    /// `bump` suffices. A missing delimiter — the next meaningful token is a
    /// closer, another `\left`/`\right`, `\end`, a paragraph break, or EOF — is
    /// reported, not consumed.
    fn math_delim(&mut self, after: &str) {
        self.skip_trivia();
        let missing = match self.kind() {
            None | Some(SyntaxKind::R_BRACE | SyntaxKind::DOLLAR) => true,
            Some(SyntaxKind::CONTROL_SYMBOL) => matches!(self.text(), "\\]" | "\\)"),
            Some(SyntaxKind::CONTROL_WORD) => {
                self.at_command(END_CMD) || self.at_command(LEFT_CMD) || self.at_command(RIGHT_CMD)
            }
            _ => false,
        };
        if missing {
            self.error(format!("missing delimiter after `{after}`"));
            return;
        }
        self.bump();
    }

    /// A `\right` with no open `\left` (the math loop only reaches one here when
    /// it is unmatched). Report it and consume it with its delimiter so the parse
    /// stays lossless and makes progress.
    fn stray_right(&mut self) {
        debug_assert!(self.at_command(RIGHT_CMD));
        self.error("`\\right` without matching `\\left`");
        self.bump(); // \right
        self.math_delim(RIGHT_CMD);
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
