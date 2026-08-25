//! Lossless lexer for LaTeX surface syntax.
//!
//! The lexer recognizes only bounded modes whose delimiters are visible in the
//! source, including verbatim regions and explicit catcode toggles.

use std::collections::{HashMap, HashSet};

use smol_str::SmolStr;

use crate::semantic::signature::{ArgKind, ArgSpec, EnvironmentSig, builtin};
use crate::syntax::SyntaxKind;

/// A single lexed token: its kind plus the exact source slice it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: SyntaxKind,
    pub text: SmolStr,
}

/// The LaTeX file flavor, fixing the lexer's *initial* catcode regime. A
/// document (`.tex`) starts in the ordinary regime; a package or class
/// (`.sty`/`.cls`) is loaded under an implicit `\makeatletter`, so `@` is a
/// letter from the first byte. A trailing explicit `\makeatother` still applies.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum LatexFlavor {
    /// A `.tex` document: ordinary catcodes at the start.
    #[default]
    Document,
    /// A `.sty`/`.cls` package or class: `@` is a letter from the start.
    Package,
}

impl LatexFlavor {
    /// Whether the lexer should begin with `@` already a letter (the implicit
    /// `\makeatletter` of a package/class load).
    fn letter_mode_start(self) -> bool {
        matches!(self, LatexFlavor::Package)
    }
}

/// The lexer's per-parse mode. [`flavor`](Self::flavor) fixes the *initial*
/// catcode regime (a `.sty`/`.cls` starts under an implicit `\makeatletter`),
/// while [`dtx`](Self::dtx) is an orthogonal axis: when set, the lexer runs the
/// bounded line-oriented docstrip mode for a `.dtx` file — line-leading `%`
/// margins become [`DOC_MARGIN`](SyntaxKind::DOC_MARGIN) trivia, line-leading
/// `%<…>` guards become [`GUARD`](SyntaxKind::GUARD) trivia, and `macrocode`
/// bodies lex as ordinary code (`AGENTS.md` decision #1). The two axes are
/// independent because a `.dtx`'s catcode regime varies *by layer* (its
/// documentation is `Document`-flavored, its `macrocode` `Package`-flavored), so
/// `dtx` cannot be folded into a [`LatexFlavor`] variant.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LexConfig {
    /// The initial catcode regime.
    pub flavor: LatexFlavor,
    /// Run the docstrip (`.dtx`) line-oriented lexer mode.
    pub dtx: bool,
}

impl From<LatexFlavor> for LexConfig {
    /// A plain (non-`.dtx`) config of the given flavor — the common case, so a
    /// bare [`LatexFlavor`] coerces into a [`LexConfig`] at call sites.
    fn from(flavor: LatexFlavor) -> Self {
        Self { flavor, dtx: false }
    }
}

/// Per-parse facts discovered from definitions or project declarations.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParseCtx {
    commands: HashMap<SmolStr, Vec<ArgSpec>>,
    environments: HashMap<SmolStr, Vec<ArgSpec>>,
    suppressed: HashSet<SmolStr>,
    begin_aliases: HashMap<SmolStr, SmolStr>,
    end_aliases: HashMap<SmolStr, SmolStr>,
    declared_environments: HashMap<SmolStr, EnvironmentSig>,
}

/// Backward-compatible alias for [`ParseCtx`].
pub type VerbCtx = ParseCtx;

impl ParseCtx {
    /// Returns whether the context contains no discovered or declared facts.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
            && self.environments.is_empty()
            && self.suppressed.is_empty()
            && self.begin_aliases.is_empty()
            && self.end_aliases.is_empty()
            && self.declared_environments.is_empty()
    }

    /// Applies project declarations over discovered facts.
    pub fn overlay_declarations(&mut self, declared: &crate::declarations::ResolvedDeclarations) {
        let db = declared.as_db();
        for name in db.environment_names() {
            if let Some(sig) = db.environment(name) {
                self.declared_environments
                    .insert(SmolStr::new(name), sig.clone());
            }
        }
        for (name, target) in db.env_begin_aliases() {
            self.insert_begin_alias(SmolStr::new(name), SmolStr::new(target));
        }
        for (name, target) in db.env_end_aliases() {
            self.insert_end_alias(SmolStr::new(name), SmolStr::new(target));
        }
    }

    pub(crate) fn insert(&mut self, name: SmolStr, leading: Vec<ArgSpec>) {
        self.commands.insert(name, leading);
    }

    pub(crate) fn suppress(&mut self, name: SmolStr) {
        self.suppressed.insert(name);
    }

    fn is_suppressed(&self, name: &str) -> bool {
        self.suppressed.contains(name)
    }

    pub(crate) fn insert_environment(&mut self, name: SmolStr, args: Vec<ArgSpec>) {
        self.environments.insert(name, args);
    }

    fn leading_args(&self, name: &str) -> Option<&[ArgSpec]> {
        self.commands.get(name).map(Vec::as_slice)
    }

    fn verbatim_environment_args(&self, name: &str) -> Option<&[ArgSpec]> {
        if let Some(sig) = self.declared_environment(name) {
            return sig.verbatim_body.then(|| &*sig.args);
        }
        self.environments.get(name).map(Vec::as_slice)
    }

    pub(crate) fn is_verbatim_environment(&self, name: &str) -> bool {
        match self.declared_environment(name) {
            Some(sig) => sig.verbatim_body,
            None => {
                self.environments.contains_key(name)
                    || builtin()
                        .environment(name)
                        .is_some_and(|env| env.verbatim_body)
            }
        }
    }

    pub(crate) fn insert_begin_alias(&mut self, name: SmolStr, target: SmolStr) {
        self.begin_aliases.insert(name, target);
    }

    pub(crate) fn insert_end_alias(&mut self, name: SmolStr, target: SmolStr) {
        self.end_aliases.insert(name, target);
    }

    pub(crate) fn begin_alias(&self, name: &str) -> Option<&str> {
        self.begin_aliases.get(name).map(SmolStr::as_str)
    }

    pub(crate) fn end_alias(&self, name: &str) -> Option<&str> {
        self.end_aliases.get(name).map(SmolStr::as_str)
    }

    pub(crate) fn begin_alias_targets(&self) -> impl Iterator<Item = &str> {
        self.begin_aliases.values().map(SmolStr::as_str)
    }

    fn declared_environment(&self, name: &str) -> Option<&EnvironmentSig> {
        self.declared_environments.get(name)
    }

    pub(crate) fn is_block_environment(&self, name: &str) -> bool {
        match self.declared_environment(name) {
            Some(sig) => sig.block(),
            None => builtin()
                .environment(name)
                .is_some_and(EnvironmentSig::block),
        }
    }

    pub(crate) fn is_math_environment(&self, name: &str) -> bool {
        match self.declared_environment(name) {
            Some(sig) => sig.math,
            None => builtin().environment(name).is_some_and(|env| env.math),
        }
    }

    pub(crate) fn is_statement_environment(&self, name: &str) -> bool {
        match self.declared_environment(name) {
            Some(sig) => sig.statement_body,
            None => builtin()
                .environment(name)
                .is_some_and(|env| env.statement_body),
        }
    }

    pub(crate) fn has_env_aliases(&self) -> bool {
        !self.begin_aliases.is_empty() || !self.end_aliases.is_empty()
    }
}

/// Returns whether a control word introduces a definition.
pub(crate) fn is_definition_keyword(text: &str) -> bool {
    matches!(
        text,
        "\\newcommand"
            | "\\renewcommand"
            | "\\providecommand"
            | "\\DeclareRobustCommand"
            | "\\NewDocumentCommand"
            | "\\RenewDocumentCommand"
            | "\\ProvideDocumentCommand"
            | "\\DeclareDocumentCommand"
            | "\\def"
            | "\\edef"
            | "\\gdef"
            | "\\xdef"
            | "\\let"
    )
}

/// Returns the number of following control words claimed as definition names.
pub(crate) fn definition_name_slots(text: &str) -> u8 {
    match text {
        "\\let" => 2,
        _ if is_definition_keyword(text) => 1,
        _ => 0,
    }
}

fn is_literal_token_command(text: &str) -> bool {
    matches!(
        text,
        "\\string" | "\\noexpand" | "\\meaning" | "\\expandafter" | "\\show"
    )
}

fn is_char_constant_command(text: &str) -> bool {
    matches!(
        text,
        "\\char"
            | "\\catcode"
            | "\\lccode"
            | "\\uccode"
            | "\\sfcode"
            | "\\mathcode"
            | "\\delcode"
            | "\\number"
            | "\\the"
            | "\\romannumeral"
            | "\\numexpr"
            | "\\dimexpr"
            | "\\ifnum"
            | "\\ifodd"
            | "\\ifdim"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// An expl3 lexer-mode transition.
pub enum ExplToggle {
    On,
    Off,
}

/// Returns the expl3 transition named by a control word.
pub fn expl_toggle(text: &str) -> Option<ExplToggle> {
    match text {
        "\\ExplSyntaxOn"
        | "\\ProvidesExplPackage"
        | "\\ProvidesExplClass"
        | "\\ProvidesExplFile" => Some(ExplToggle::On),
        "\\ExplSyntaxOff" => Some(ExplToggle::Off),
        _ => None,
    }
}

pub(crate) fn dtx_has_expl_signal(input: &str) -> bool {
    input.contains("\\ProvidesExpl")
        || input
            .lines()
            .any(|l| l.starts_with("%<@@=") && l[5..].contains('>'))
}

/// Lexes a document into a lossless token stream.
pub fn lex(input: &str) -> Vec<Token> {
    lex_with(input, &ParseCtx::default(), LexConfig::default())
}

/// Lexes with per-parse facts and an explicit file configuration.
pub fn lex_with(input: &str, ctx: &ParseCtx, config: LexConfig) -> Vec<Token> {
    Lexer::new(input, ctx, config, None).run()
}

pub(crate) fn lex_with_implicit_expl(
    input: &str,
    ctx: &ParseCtx,
    config: LexConfig,
    implicit_expl: bool,
) -> Vec<Token> {
    Lexer::new(input, ctx, config, Some(implicit_expl)).run()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Delim,
    Def,
    CharConstant,
    LiteralToken,
}

#[derive(Debug, Clone, Copy)]
struct MacrocodeSave {
    at_letter: bool,
    expl_syntax: bool,
}

struct Lexer<'a> {
    input: &'a str,
    ctx: &'a ParseCtx,
    config: LexConfig,
    implicit_expl: bool,
    out: Vec<Token>,
    pos: usize,
    at_letter: bool,
    expl_syntax: bool,
    at_line_start: bool,
    in_doc_line: bool,
    short_verbs: Vec<char>,
    macrocode: Option<MacrocodeSave>,
    pending: Option<Pending>,
    brace_depth: usize,
    brace_counted: usize,
}

impl<'a> Lexer<'a> {
    fn new(
        input: &'a str,
        ctx: &'a ParseCtx,
        config: LexConfig,
        implicit_expl_override: Option<bool>,
    ) -> Self {
        let implicit_expl = if config.dtx {
            implicit_expl_override.unwrap_or_else(|| dtx_has_expl_signal(input))
        } else {
            false
        };
        Self {
            input,
            ctx,
            config,
            implicit_expl,
            out: Vec::new(),
            pos: 0,
            at_letter: config.flavor.letter_mode_start(),
            expl_syntax: false,
            at_line_start: true,
            in_doc_line: false,
            short_verbs: if config.dtx { vec!['|'] } else { Vec::new() },
            macrocode: None,
            pending: None,
            brace_depth: 0,
            brace_counted: 0,
        }
    }

    fn run(mut self) -> Vec<Token> {
        while self.pos < self.input.len() {
            self.sync_brace_depth();
            if self.try_macrocode_frame()
                || self.try_guard()
                || self.try_doc_margin()
                || self.try_verbatim_environment()
                || self.try_verbatim_arg_environment()
            {
                continue;
            }
            let word_len = control_word_len(self.rest(), self.at_letter, self.expl_syntax);
            if self.try_verbatim_command(word_len)
                || self.try_short_verb()
                || self.try_char_constant()
                || self.try_doc_comment()
            {
                continue;
            }
            self.lex_token(word_len);
        }
        self.out
    }

    fn rest(&self) -> &'a str {
        &self.input[self.pos..]
    }

    fn push(&mut self, kind: SyntaxKind, text: &str) {
        self.out.push(Token {
            kind,
            text: SmolStr::new(text),
        });
    }

    fn consume(&mut self, len: usize) {
        self.pos += len;
        self.at_line_start = false;
        self.pending = None;
    }

    fn sync_brace_depth(&mut self) {
        while self.brace_counted < self.out.len() {
            match self.out[self.brace_counted].kind {
                SyntaxKind::L_BRACE => self.brace_depth += 1,
                SyntaxKind::R_BRACE => self.brace_depth = self.brace_depth.saturating_sub(1),
                _ => {}
            }
            self.brace_counted += 1;
        }
    }

    fn try_macrocode_frame(&mut self) -> bool {
        if !(self.config.dtx && self.at_line_start) {
            return false;
        }
        let rest = self.rest();
        let want_begin = self.macrocode.is_none();
        let Some(consumed) = lex_macrocode_frame(rest, want_begin, &mut self.out) else {
            return false;
        };
        match self.macrocode.take() {
            Some(saved) => {
                self.at_letter = saved.at_letter;
                self.expl_syntax = saved.expl_syntax;
            }
            None => {
                self.macrocode = Some(MacrocodeSave {
                    at_letter: self.at_letter,
                    expl_syntax: self.expl_syntax,
                });
                self.at_letter = true;
                if self.implicit_expl {
                    self.expl_syntax = true;
                }
            }
        }
        self.consume(consumed);
        true
    }

    fn try_guard(&mut self) -> bool {
        let rest = self.rest();
        if !(self.config.dtx && self.at_line_start && rest.starts_with("%<")) {
            return false;
        }
        let Some(rel) = rest[2..].find(['>', '\n', '\r']) else {
            return false;
        };
        if rest.as_bytes()[2 + rel] != b'>' {
            return false;
        }
        let len = 2 + rel + 1;
        self.push(SyntaxKind::GUARD, &rest[..len]);
        self.pos += len;
        self.at_line_start = false;
        true
    }

    fn try_doc_margin(&mut self) -> bool {
        let rest = self.rest();
        if !(self.config.dtx
            && self.at_line_start
            && self.macrocode.is_none()
            && rest.starts_with('%')
            && !rest.starts_with("%<"))
        {
            return false;
        }
        self.push(SyntaxKind::DOC_MARGIN, "%");
        self.pos += 1;
        self.at_line_start = false;
        self.in_doc_line = true;
        true
    }

    fn try_verbatim_environment(&mut self) -> bool {
        let (rest, ctx) = (self.rest(), self.ctx);
        let Some(consumed) = lex_verbatim_environment(rest, ctx, &mut self.out) else {
            return false;
        };
        self.consume(consumed);
        true
    }

    fn try_verbatim_arg_environment(&mut self) -> bool {
        if self.macrocode.is_some() {
            return false;
        }
        let rest = self.rest();
        let Some(consumed) = lex_verbatim_arg_environment(rest, &mut self.out) else {
            return false;
        };
        self.consume(consumed);
        true
    }

    fn try_verbatim_command(&mut self, word_len: Option<usize>) -> bool {
        if self.pending == Some(Pending::Def) {
            return false;
        }
        let (rest, ctx) = (self.rest(), self.ctx);
        let Some(consumed) = lex_verbatim_command(
            rest,
            word_len,
            ctx,
            self.config.dtx && self.in_doc_line,
            &mut self.out,
        ) else {
            return false;
        };
        self.consume(consumed);
        true
    }

    fn try_short_verb(&mut self) -> bool {
        if self.short_verbs.is_empty()
            || self.macrocode.is_some()
            || matches!(self.pending, Some(Pending::Delim | Pending::LiteralToken))
        {
            return false;
        }
        let rest = self.rest();
        if !rest
            .chars()
            .next()
            .is_some_and(|c| self.short_verbs.contains(&c))
        {
            return false;
        }
        let Some(len) = delimited_len(rest) else {
            return false;
        };
        self.push(SyntaxKind::VERB, &rest[..len]);
        self.consume(len);
        true
    }

    fn try_char_constant(&mut self) -> bool {
        let numeric_context = self.pending == Some(Pending::CharConstant);
        let rest = self.rest();
        let Some(after) = rest.strip_prefix('`') else {
            return false;
        };
        let Some(c) = after.chars().next() else {
            return false;
        };
        if matches!(c, '\n' | '\r') || (self.brace_depth > 0 && matches!(c, '{' | '}')) {
            return false;
        }
        let len = if c == '\\' {
            match after[1..]
                .chars()
                .next()
                .filter(|e| !matches!(e, '\n' | '\r'))
            {
                Some(e) => 2 + e.len_utf8(),
                None => return false,
            }
        } else {
            1 + c.len_utf8()
        };
        let alignment_cell = self.pending.is_none() && c == '\\' && rest[len..].starts_with('&');
        if !numeric_context && !alignment_cell {
            return false;
        }
        self.push(SyntaxKind::WORD, &rest[..len]);
        self.consume(len);
        true
    }

    fn try_doc_comment(&mut self) -> bool {
        let rest = self.rest();
        if !(self.in_doc_line && rest.starts_with("^^A")) {
            return false;
        }
        let len = run_len(rest, |c| c != '\n' && c != '\r');
        self.push(SyntaxKind::COMMENT, &rest[..len]);
        self.consume(len);
        true
    }

    fn lex_token(&mut self, word_len: Option<usize>) {
        let rest = self.rest();
        let (kind, mut len) = next_token(rest, word_len, self.expl_syntax);
        if self.pending == Some(Pending::Delim) && kind == SyntaxKind::WORD {
            len = rest.chars().next().expect("rest is non-empty").len_utf8();
        }
        if kind == SyntaxKind::WORD
            && !self.short_verbs.is_empty()
            && let Some((i, c)) = rest[..len]
                .char_indices()
                .find(|(_, c)| self.short_verbs.contains(c))
        {
            len = if i == 0 { c.len_utf8() } else { i };
        }
        debug_assert!(len > 0, "lexer made no progress at byte {}", self.pos);
        let text = &rest[..len];
        if kind == SyntaxKind::CONTROL_WORD {
            self.apply_toggles(text, &rest[len..]);
        }
        self.pending = next_pending(self.pending, kind, text);
        self.push(kind, text);
        self.at_line_start =
            kind == SyntaxKind::NEWLINE || text.ends_with('\n') || text.ends_with('\r');
        if self.at_line_start {
            self.in_doc_line = false;
        }
        self.pos += len;
    }

    fn apply_toggles(&mut self, text: &str, after: &str) {
        match text {
            "\\makeatletter" => self.at_letter = true,
            "\\makeatother" => self.at_letter = false,
            "\\MakeShortVerb" => {
                if let Some(c) = short_verb_char(after)
                    && !self.short_verbs.contains(&c)
                {
                    self.short_verbs.push(c);
                }
            }
            "\\DeleteShortVerb" => {
                if let Some(c) = short_verb_char(after) {
                    self.short_verbs.retain(|&x| x != c);
                }
            }
            "\\documentclass" | "\\LoadClass" => {
                if doc_class_enables_bar(after) && !self.short_verbs.contains(&'|') {
                    self.short_verbs.push('|');
                }
            }
            _ => {
                if let Some(toggle) = expl_toggle(text) {
                    self.expl_syntax = matches!(toggle, ExplToggle::On);
                }
            }
        }
    }
}

pub(crate) fn reads_following_text(text: &str) -> bool {
    matches!(
        text,
        "\\MakeShortVerb" | "\\DeleteShortVerb" | "\\documentclass" | "\\LoadClass"
    ) || next_pending(None, SyntaxKind::CONTROL_WORD, text).is_some()
}

fn next_pending(pending: Option<Pending>, kind: SyntaxKind, text: &str) -> Option<Pending> {
    if kind == SyntaxKind::CONTROL_WORD {
        return if text == "\\left" || text == "\\right" {
            Some(Pending::Delim)
        } else if is_definition_keyword(text) {
            Some(Pending::Def)
        } else if is_char_constant_command(text) {
            Some(Pending::CharConstant)
        } else if is_literal_token_command(text) {
            Some(Pending::LiteralToken)
        } else {
            None
        };
    }
    match pending? {
        p @ (Pending::Delim | Pending::Def)
            if matches!(kind, SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE) =>
        {
            Some(p)
        }
        Pending::Def if kind == SyntaxKind::L_BRACE => Some(Pending::Def),
        p @ (Pending::CharConstant | Pending::LiteralToken) if kind == SyntaxKind::WHITESPACE => {
            Some(p)
        }
        _ => None,
    }
}

fn control_word_len(rest: &str, at_letter: bool, expl_syntax: bool) -> Option<usize> {
    let after = rest.strip_prefix('\\')?;
    let letters = run_len(after, |c| is_letter(c, at_letter, expl_syntax));
    (letters > 0).then_some(1 + letters)
}

fn next_token(rest: &str, word_len: Option<usize>, expl_syntax: bool) -> (SyntaxKind, usize) {
    let c = rest.chars().next().expect("rest is non-empty");
    match c {
        '\\' => lex_control(rest, word_len),
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
        '_' if !expl_syntax => (SyntaxKind::UNDERSCORE, 1),
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
        _ => (
            SyntaxKind::WORD,
            run_len(rest, |c| is_word_char(c) || (expl_syntax && c == '_')),
        ),
    }
}

fn lex_control(rest: &str, word_len: Option<usize>) -> (SyntaxKind, usize) {
    match word_len {
        Some(word_len) => {
            if &rest[..word_len] == "\\verb"
                && let Some(arg_len) = verb_len(&rest[word_len..])
            {
                return (SyntaxKind::VERB, word_len + arg_len);
            }
            (SyntaxKind::CONTROL_WORD, word_len)
        }
        None => {
            let after = &rest[1..];
            let symbol_len = if after.starts_with("\r\n") {
                2
            } else {
                after.chars().next().map_or(0, char::len_utf8)
            };
            (SyntaxKind::CONTROL_SYMBOL, 1 + symbol_len)
        }
    }
}

fn verb_len(after: &str) -> Option<usize> {
    match after.strip_prefix('*') {
        Some(rest) => Some(1 + delimited_len(rest)?),
        None => delimited_len(after),
    }
}

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

fn short_verb_char(after: &str) -> Option<char> {
    let s = skip_inline_ws(after.strip_prefix('*').unwrap_or(after));
    let (body, braced) = match s.strip_prefix('{') {
        Some(inner) => (skip_inline_ws(inner), true),
        None => (s, false),
    };
    let arg = body.strip_prefix('\\')?;
    let c = arg.chars().next()?;
    if c == '\n' || c == '\r' {
        return None;
    }
    if braced && !skip_inline_ws(&arg[c.len_utf8()..]).starts_with('}') {
        return None;
    }
    Some(c)
}

const BAR_SHORT_VERB_CLASSES: [&str; 5] = ["ltxdoc", "ltxguide", "ltnews", "l3doc", "amsldoc"];

fn doc_class_enables_bar(after: &str) -> bool {
    let mut s = skip_inline_ws(after);
    if let Some(rest) = s.strip_prefix('[') {
        match rest.find(']') {
            Some(i) => s = rest[i + 1..].trim_start_matches([' ', '\t', '\n', '\r']),
            None => return false,
        }
    }
    let Some(rest) = s.strip_prefix('{') else {
        return false;
    };
    let Some(close) = rest.find('}') else {
        return false;
    };
    BAR_SHORT_VERB_CLASSES.contains(&rest[..close].trim())
}

fn lex_verbatim_environment(rest: &str, ctx: &ParseCtx, out: &mut Vec<Token>) -> Option<usize> {
    let (name, prefix_len) = begin_name(rest)?;
    let args: &[ArgSpec] = match ctx.verbatim_environment_args(name) {
        Some(args) => args,
        None => {
            &builtin()
                .environment(name)
                .filter(|e| e.verbatim_body)?
                .args
        }
    };

    push_env_delimiter(out, "\\begin", name);

    let args_region = &rest[prefix_len..];
    let args_len = scan_verbatim_args(args_region, args);
    lex_into(&args_region[..args_len], out);

    let body_region = &args_region[args_len..];
    let body_len = verbatim_body_len(body_region, name);
    if body_len > 0 {
        out.push(Token {
            kind: SyntaxKind::VERBATIM_BODY,
            text: SmolStr::new(&body_region[..body_len]),
        });
    }
    Some(prefix_len + args_len + body_len)
}

fn verbatim_body_len(body: &str, name: &str) -> usize {
    const LEAD: &str = "\\end{";
    let mut from = 0;
    while let Some(rel) = body[from..].find(LEAD) {
        let at = from + rel;
        let after = &body[at + LEAD.len()..];
        if let Some(tail) = after.strip_prefix(name)
            && tail.starts_with('}')
        {
            return at;
        }
        from = at + LEAD.len();
    }
    body.len()
}

fn lex_verbatim_arg_environment(rest: &str, out: &mut Vec<Token>) -> Option<usize> {
    let (name, prefix_len) = begin_name(rest)?;
    builtin().environment(name).filter(|e| e.verbatim_arg)?;

    let region = &rest[prefix_len..];
    let mut args_len = 0;
    if let Some(after) = region.strip_prefix('[') {
        let i = after.find([']', '\n', '\r'])?;
        if after.as_bytes()[i] != b']' {
            return None;
        }
        args_len = 1 + i + 1;
    }
    let arg_region = &region[args_len..];
    let delim = arg_region.chars().next()?;
    let braced_content_len = if delim == '{' {
        Some(braced_verb_content_len(&arg_region[1..])?)
    } else {
        if !delim.is_ascii_punctuation()
            || matches!(delim, '\\' | '}' | '[' | ']' | '%' | '*' | '$')
        {
            return None;
        }
        None
    };

    push_env_delimiter(out, "\\begin", name);
    lex_into(&region[..args_len], out);
    let verb_len = match braced_content_len {
        Some(content_len) => {
            out.push(Token {
                kind: SyntaxKind::L_BRACE,
                text: SmolStr::new("{"),
            });
            out.push(Token {
                kind: SyntaxKind::VERB,
                text: SmolStr::new(&arg_region[1..1 + content_len]),
            });
            out.push(Token {
                kind: SyntaxKind::R_BRACE,
                text: SmolStr::new("}"),
            });
            1 + content_len + 1
        }
        None => {
            let verb_len = delimited_len(arg_region)?;
            out.push(Token {
                kind: SyntaxKind::VERB,
                text: SmolStr::new(&arg_region[..verb_len]),
            });
            verb_len
        }
    };
    Some(prefix_len + args_len + verb_len)
}

fn braced_verb_content_len(content: &str) -> Option<usize> {
    let mut depth = 1usize;
    let mut chars = content.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next()?;
            }
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return (i > 0).then_some(i);
                }
            }
            '\n' | '\r' => return None,
            _ => {}
        }
    }
    None
}

fn lex_macrocode_frame(rest: &str, want_begin: bool, out: &mut Vec<Token>) -> Option<usize> {
    let indent = if want_begin { inline_ws_len(rest) } else { 0 };
    let after_pct = rest[indent..].strip_prefix('%')?;
    let ws_len = inline_ws_len(after_pct);
    let body = &after_pct[ws_len..];
    let (control, open) = if want_begin {
        ("\\begin", "\\begin{")
    } else {
        ("\\end", "\\end{")
    };
    let after_open = body.strip_prefix(open)?;
    let close = after_open.find('}')?;
    let name = &after_open[..close];
    if name != "macrocode" && name != "macrocode*" {
        return None;
    }
    let after_close = &after_open[close + 1..];
    let tail = skip_inline_ws(after_close);
    let comment_tail = !want_begin && tail.starts_with('%');
    if !(tail.is_empty() || tail.starts_with('\n') || tail.starts_with('\r') || comment_tail) {
        return None;
    }

    if indent > 0 {
        out.push(Token {
            kind: SyntaxKind::WHITESPACE,
            text: SmolStr::new(&rest[..indent]),
        });
    }
    out.push(Token {
        kind: SyntaxKind::DOC_MARGIN,
        text: SmolStr::new("%"),
    });
    if ws_len > 0 {
        out.push(Token {
            kind: SyntaxKind::WHITESPACE,
            text: SmolStr::new(&after_pct[..ws_len]),
        });
    }
    push_env_delimiter(out, control, name);
    Some(indent + 1 + ws_len + control.len() + 1 + name.len() + 1)
}

fn lex_verbatim_command(
    rest: &str,
    word_len: Option<usize>,
    ctx: &ParseCtx,
    on_dtx_doc_line: bool,
    out: &mut Vec<Token>,
) -> Option<usize> {
    let word_len = word_len?;
    let name = &rest[1..word_len];
    if name == "verb" {
        return None;
    }
    let (leading, delimited): (&[ArgSpec], bool) = match ctx.leading_args(name) {
        Some(args) => (args, false),
        None => {
            if ctx.is_suppressed(name) {
                return None;
            }
            let sig = builtin().command(name)?;
            if sig.verbatim {
                (&sig.args, sig.verbatim_delimited)
            } else {
                let raw = sig.args.iter().position(|arg| arg.verbatim)?;
                if sig.args[raw].kind != ArgKind::Brace {
                    return None;
                }
                (&sig.args[..raw], false)
            }
        }
    };

    let after_word = &rest[word_len..];
    let args_len = scan_verbatim_args(after_word, leading);

    let region = &after_word[args_len..];
    let dtx_gap = (!delimited && on_dtx_doc_line)
        .then(|| dtx_doc_argument_gap_len(region))
        .flatten();
    let ws_len = if let Some(len) = dtx_gap {
        len
    } else if delimited {
        inline_ws_len(region)
    } else {
        tex_whitespace_len(region)
    };
    let arg_region = &region[ws_len..];
    let arg_len = match arg_region.bytes().next() {
        Some(b'{') => balanced_group_len(arg_region, b'}')?,
        Some(_) if delimited => delimited_len(arg_region)?,
        _ => return None,
    };

    out.push(Token {
        kind: SyntaxKind::CONTROL_WORD,
        text: SmolStr::new(&rest[..word_len]),
    });
    lex_into(&after_word[..args_len], out);
    if ws_len > 0 {
        if dtx_gap.is_some() {
            lex_dtx_doc_argument_gap(&region[..ws_len], out);
        } else {
            out.push(Token {
                kind: SyntaxKind::WHITESPACE,
                text: SmolStr::new(&region[..ws_len]),
            });
        }
    }
    out.push(Token {
        kind: SyntaxKind::VERB,
        text: SmolStr::new(&arg_region[..arg_len]),
    });
    Some(word_len + args_len + ws_len + arg_len)
}

fn scan_verbatim_args(region: &str, args: &[ArgSpec]) -> usize {
    let bytes = region.as_bytes();
    let mut pos = 0;
    for arg in args {
        let probe = pos + inline_ws_len(&region[pos..]);
        let (open, close) = match arg.kind {
            ArgKind::Bracket => (b'[', b']'),
            ArgKind::Brace => (b'{', b'}'),
        };
        if bytes.get(probe) != Some(&open) {
            continue;
        }
        match balanced_group_len(&region[probe..], close) {
            Some(len) => pos = probe + len,
            None => break, // unbalanced: treat the remainder as body
        }
    }
    pos
}

fn balanced_group_len(s: &str, close: u8) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut stack = vec![close];
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'{' => stack.push(b'}'),
            b'[' => stack.push(b']'),
            c @ (b'}' | b']') if stack.last() == Some(&c) => {
                stack.pop();
                if stack.is_empty() {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn lex_into(region: &str, out: &mut Vec<Token>) {
    let mut pos = 0;
    while pos < region.len() {
        let rest = &region[pos..];
        let (kind, len) = next_token(rest, control_word_len(rest, false, false), false);
        debug_assert!(len > 0, "lexer made no progress in verbatim args");
        out.push(Token {
            kind,
            text: SmolStr::new(&region[pos..pos + len]),
        });
        pos += len;
    }
}

fn begin_name(rest: &str) -> Option<(&str, usize)> {
    let after = rest.strip_prefix("\\begin{")?;
    let close = after.find('}')?;
    Some((&after[..close], "\\begin{".len() + close + 1))
}

fn push_env_delimiter(out: &mut Vec<Token>, control: &str, name: &str) {
    out.push(Token {
        kind: SyntaxKind::CONTROL_WORD,
        text: SmolStr::new(control),
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
}

fn inline_ws_len(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b' ' || b == b'\t').count()
}

fn tex_whitespace_len(s: &str) -> usize {
    s.bytes()
        .take_while(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
        .count()
}

fn dtx_doc_argument_gap_len(s: &str) -> Option<usize> {
    let inline = inline_ws_len(s);
    let rest = &s[inline..];
    let newline = if rest.starts_with("\r\n") {
        2
    } else if rest.starts_with(['\n', '\r']) {
        1
    } else {
        return None;
    };
    let after_newline = &rest[newline..];
    let after_margin = after_newline.strip_prefix('%')?;
    Some(inline + newline + 1 + inline_ws_len(after_margin))
}

fn lex_dtx_doc_argument_gap(gap: &str, out: &mut Vec<Token>) {
    let inline = inline_ws_len(gap);
    if inline > 0 {
        out.push(Token {
            kind: SyntaxKind::WHITESPACE,
            text: SmolStr::new(&gap[..inline]),
        });
    }
    let rest = &gap[inline..];
    let newline = if rest.starts_with("\r\n") { 2 } else { 1 };
    out.push(Token {
        kind: SyntaxKind::NEWLINE,
        text: SmolStr::new(&rest[..newline]),
    });
    out.push(Token {
        kind: SyntaxKind::DOC_MARGIN,
        text: SmolStr::new("%"),
    });
    let trailing = &rest[newline + 1..];
    if !trailing.is_empty() {
        out.push(Token {
            kind: SyntaxKind::WHITESPACE,
            text: SmolStr::new(trailing),
        });
    }
}

fn skip_inline_ws(s: &str) -> &str {
    &s[inline_ws_len(s)..]
}

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

fn is_letter(c: char, at_letter: bool, expl_syntax: bool) -> bool {
    c.is_ascii_alphabetic() || (at_letter && c == '@') || (expl_syntax && (c == '_' || c == ':'))
}

/// Returns whether `name` can be a control word in any supported mode.
pub fn is_control_word_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| is_letter(c, true, true))
}

/// Returns whether `c` belongs to an ordinary text token.
pub fn is_word_char(c: char) -> bool {
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

    fn assert_lossless(input: &str) {
        let joined: String = lex(input).iter().map(|t| t.text.as_str()).collect();
        assert_eq!(joined, input);
    }

    #[test]
    fn the_pending_arming_sets_are_disjoint() {
        for name in [
            "\\left",
            "\\right",
            "\\newcommand",
            "\\renewcommand",
            "\\providecommand",
            "\\DeclareRobustCommand",
            "\\NewDocumentCommand",
            "\\RenewDocumentCommand",
            "\\ProvideDocumentCommand",
            "\\DeclareDocumentCommand",
            "\\def",
            "\\edef",
            "\\gdef",
            "\\xdef",
            "\\let",
            "\\char",
            "\\catcode",
            "\\lccode",
            "\\uccode",
            "\\sfcode",
            "\\mathcode",
            "\\delcode",
            "\\number",
            "\\the",
            "\\romannumeral",
            "\\numexpr",
            "\\dimexpr",
            "\\ifnum",
            "\\ifodd",
            "\\ifdim",
            "\\string",
            "\\noexpand",
            "\\meaning",
            "\\expandafter",
            "\\show",
        ] {
            let armed = [
                name == "\\left" || name == "\\right",
                is_definition_keyword(name),
                is_char_constant_command(name),
                is_literal_token_command(name),
            ];
            assert_eq!(
                armed.iter().filter(|&&x| x).count(),
                1,
                "{name} arms {armed:?} — the `Pending` slot needs disjoint sets"
            );
        }
    }

    #[test]
    fn a_claimed_construct_spends_the_armed_char_constant_mode() {
        let direct = lex("\\char `\\%");
        assert!(
            direct
                .iter()
                .any(|t| t.kind == SyntaxKind::WORD && t.text == "`\\%")
        );
        let intervened = lex("\\MakeShortVerb{\\|} \\char |a| `\\%");
        assert!(
            intervened
                .iter()
                .any(|t| t.kind == SyntaxKind::VERB && t.text == "|a|")
        );
        assert!(
            !intervened
                .iter()
                .any(|t| t.kind == SyntaxKind::WORD && t.text == "`\\%")
        );
        assert_lossless("\\MakeShortVerb{\\|} \\char |a| `\\%");
    }

    #[test]
    fn block_environment_classification() {
        let ctx = ParseCtx::default();
        assert!(ctx.is_block_environment("figure"));
        assert!(ctx.is_block_environment("itemize")); // derived via `list`
        assert!(!ctx.is_block_environment("myenv")); // unknown
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
            r"\ExplSyntaxOn\seq_new:N \g_@@_x_tl a_b\ExplSyntaxOff\seq_new:N",
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
    fn control_symbol_swallows_the_whole_line_ending() {
        for ending in ["\n", "\r", "\r\n"] {
            let input = format!("\\{ending}");
            let toks = lex(&input);
            assert_eq!(toks.len(), 1, "split line ending {ending:?}");
            assert_eq!(toks[0].kind, SyntaxKind::CONTROL_SYMBOL);
            assert_eq!(toks[0].text, input);
        }
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
        for input in [r"\left\{", r"\left\langle", r"\left["] {
            assert_lossless(input);
        }
        let toks = lex(r"\left\langle x \right\rangle");
        assert!(toks.iter().any(|t| t.text == "\\langle"));
        assert!(toks.iter().any(|t| t.text == "\\rangle"));
    }

    #[test]
    fn leftarrow_is_not_left() {
        let toks = lex(r"\leftarrow(x)");
        assert_eq!(toks[0].text, "\\leftarrow");
        assert_eq!(toks[1].text, "(x)");
    }

    #[test]
    fn makeatletter_makes_at_a_letter() {
        let toks = lex(r"\makeatletter\foo@bar\makeatother\foo@bar");
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\foo@bar")));
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\foo")));
    }

    #[test]
    fn expl_syntax_makes_underscore_and_colon_letters() {
        let toks = lex(r"\ExplSyntaxOn\seq_new:N\ExplSyntaxOff\seq_new:N");
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\seq_new:N")));
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\seq")));
    }

    #[test]
    fn expl_syntax_lexes_internal_double_underscore_name() {
        let toks = lex(r"\ExplSyntaxOn\__module_internal:nn");
        assert_eq!(toks[1].kind, SyntaxKind::CONTROL_WORD);
        assert_eq!(toks[1].text, "\\__module_internal:nn");
    }

    #[test]
    fn provides_expl_package_turns_on_expl_syntax() {
        let toks = lex(r"\ProvidesExplPackage{p}{2026/01/01}{1.0}{d}\tl_set:Nn");
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\tl_set:Nn")));
    }

    #[test]
    fn expl_syntax_composes_with_makeatletter() {
        let toks = lex(r"\makeatletter\ExplSyntaxOn\g_@@_frame_title_tl");
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\g_@@_frame_title_tl")));
    }

    #[test]
    fn expl_syntax_makes_bare_underscore_a_word_not_subscript() {
        let toks = lex(r"\ExplSyntaxOn a_b");
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert!(seen.contains(&(SyntaxKind::WORD, "a_b")));
        assert!(!seen.iter().any(|(k, _)| *k == SyntaxKind::UNDERSCORE));
    }

    fn lex_dtx(input: &str) -> Vec<Token> {
        lex_with(
            input,
            &ParseCtx::default(),
            LexConfig {
                flavor: LatexFlavor::Document,
                dtx: true,
            },
        )
    }

    #[test]
    fn implicit_expl_module_guard_makes_macrocode_body_expl3() {
        let toks = lex_dtx(
            "%<@@=mod>\n\
             %    \\begin{macrocode}\n\
             \\seq_new:N\n\
             %    \\end{macrocode}\n",
        );
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\seq_new:N")));
    }

    #[test]
    fn no_expl_signal_leaves_macrocode_body_plain() {
        let toks = lex_dtx(
            "%    \\begin{macrocode}\n\
             \\seq_new:N\n\
             %    \\end{macrocode}\n",
        );
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\seq")));
        assert!(!seen.contains(&(SyntaxKind::CONTROL_WORD, "\\seq_new:N")));
    }

    #[test]
    fn implicit_expl_provides_expl_flags_every_body_regardless_of_order() {
        let toks = lex_dtx(
            "%    \\begin{macrocode}\n\
             \\seq_new:N\n\
             %    \\end{macrocode}\n\
             % \\ProvidesExplPackage{p}{2026/01/01}{1.0}{d}\n\
             %    \\begin{macrocode}\n\
             \\tl_set:Nn\n\
             %    \\end{macrocode}\n",
        );
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\seq_new:N")));
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\tl_set:Nn")));
    }

    #[test]
    fn implicit_expl_is_body_only_doc_layer_stays_plain() {
        let toks = lex_dtx(
            "%<@@=mod>\n\
             % a_b\n\
             %    \\begin{macrocode}\n\
             c_d\n\
             %    \\end{macrocode}\n",
        );
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert!(seen.contains(&(SyntaxKind::WORD, "c_d")));
        assert!(seen.iter().any(|(k, _)| *k == SyntaxKind::UNDERSCORE));
    }

    #[test]
    fn implicit_expl_explicit_off_wins_then_next_body_re_enters() {
        let toks = lex_dtx(
            "%<@@=mod>\n\
             %    \\begin{macrocode}\n\
             \\seq_new:N\n\
             \\ExplSyntaxOff\n\
             a_b\n\
             %    \\end{macrocode}\n\
             %    \\begin{macrocode}\n\
             \\tl_set:Nn\n\
             %    \\end{macrocode}\n",
        );
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\seq_new:N")));
        assert!(seen.iter().any(|(k, _)| *k == SyntaxKind::UNDERSCORE));
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\tl_set:Nn")));
    }

    #[test]
    fn implicit_expl_gated_off_outside_dtx() {
        let toks = lex_with(
            "%<@@=mod>\n\\seq_new:N",
            &ParseCtx::default(),
            LexConfig {
                flavor: LatexFlavor::Package,
                dtx: false,
            },
        );
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\seq")));
        assert!(!seen.contains(&(SyntaxKind::CONTROL_WORD, "\\seq_new:N")));
    }

    #[test]
    fn package_flavor_starts_in_letter_mode() {
        let toks = lex_with(
            r"\foo@bar",
            &ParseCtx::default(),
            LatexFlavor::Package.into(),
        );
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert_eq!(seen, vec![(SyntaxKind::CONTROL_WORD, "\\foo@bar")]);
    }

    #[test]
    fn package_flavor_respects_trailing_makeatother() {
        let toks = lex_with(
            r"\foo@bar\makeatother\foo@bar",
            &ParseCtx::default(),
            LatexFlavor::Package.into(),
        );
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\foo@bar")));
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\foo")));
    }

    #[test]
    fn document_flavor_keeps_at_non_letter() {
        let toks = lex(r"\foo@bar");
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\foo")));
        assert!(!seen.contains(&(SyntaxKind::CONTROL_WORD, "\\foo@bar")));
    }

    #[test]
    fn dtx_mode_lexes_line_leading_percent_as_a_margin() {
        let dtx = LexConfig {
            flavor: LatexFlavor::Document,
            dtx: true,
        };
        let toks = lex_with("% \\foo\nbar % tail\n", &ParseCtx::default(), dtx);
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert_eq!(seen[0], (SyntaxKind::DOC_MARGIN, "%"));
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\foo")));
        assert!(seen.contains(&(SyntaxKind::COMMENT, "% tail")));
        assert_eq!(
            seen.iter()
                .filter(|(k, _)| *k == SyntaxKind::DOC_MARGIN)
                .count(),
            1
        );
    }

    #[test]
    fn dtx_mode_is_off_by_default_for_margins_and_guards() {
        let plain = lex("% \\foo\n");
        assert_eq!(plain[0].kind, SyntaxKind::COMMENT);
        let plain_guard = lex("%<*driver>\n");
        assert_eq!(plain_guard[0].kind, SyntaxKind::COMMENT);
        assert_eq!(plain_guard[0].text, "%<*driver>");
    }

    #[test]
    fn dtx_mode_lexes_line_leading_guards() {
        let dtx = LexConfig {
            flavor: LatexFlavor::Document,
            dtx: true,
        };
        let block = lex_with("%<*driver>\n%</driver>\n", &ParseCtx::default(), dtx);
        assert_eq!(block[0].kind, SyntaxKind::GUARD);
        assert_eq!(block[0].text, "%<*driver>");
        assert!(
            block
                .iter()
                .any(|t| t.kind == SyntaxKind::GUARD && t.text == "%</driver>")
        );
        let inline = lex_with("%<plain>\\RequirePackage{x}\n", &ParseCtx::default(), dtx);
        assert_eq!(inline[0].kind, SyntaxKind::GUARD);
        assert_eq!(inline[0].text, "%<plain>");
        assert!(
            inline
                .iter()
                .any(|t| t.kind == SyntaxKind::CONTROL_WORD && t.text == "\\RequirePackage")
        );
        let expr = lex_with("%<*package|driver>\n", &ParseCtx::default(), dtx);
        assert_eq!(expr[0].kind, SyntaxKind::GUARD);
        assert_eq!(expr[0].text, "%<*package|driver>");
        let midline = lex_with("a %<x>\n", &ParseCtx::default(), dtx);
        assert!(
            midline
                .iter()
                .any(|t| t.kind == SyntaxKind::COMMENT && t.text == "%<x>")
        );
        assert!(!midline.iter().any(|t| t.kind == SyntaxKind::GUARD));
        let malformed = lex_with("%<unterminated\n", &ParseCtx::default(), dtx);
        assert_eq!(malformed[0].kind, SyntaxKind::COMMENT);
        assert_eq!(malformed[0].text, "%<unterminated");
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
        assert!(!toks.iter().any(|t| t.kind == SyntaxKind::DOLLAR));
        assert!(!toks.iter().any(|t| t.kind == SyntaxKind::COMMENT));
    }

    #[test]
    fn argument_taking_verbatim_separates_args_from_body() {
        let toks = lex("\\begin{minted}[frame=single]{python}\nprint(\"$x$\")\n\\end{minted}");
        let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
        assert!(kinds.contains(&SyntaxKind::L_BRACKET));
        assert!(kinds.contains(&SyntaxKind::R_BRACKET));
        assert!(kinds.contains(&SyntaxKind::L_BRACE));
        assert!(
            toks.iter()
                .any(|t| t.kind == SyntaxKind::VERBATIM_BODY && t.text.contains("print(\"$x$\")"))
        );
        assert!(!toks.iter().any(|t| t.kind == SyntaxKind::DOLLAR));
    }

    #[test]
    fn verbatim_body_starting_with_bracket_is_not_an_argument() {
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

    #[test]
    fn make_short_verb_toggles_pipe_capture() {
        let toks = lex("|a| \\MakeShortVerb{\\|} |$| \\DeleteShortVerb{\\|} |b|");
        let verbs: Vec<_> = toks
            .iter()
            .filter(|t| t.kind == SyntaxKind::VERB)
            .map(|t| t.text.as_str())
            .collect();
        assert_eq!(verbs, ["|$|"]);
        assert_lossless("|a| \\MakeShortVerb{\\|} |$| \\DeleteShortVerb{\\|} |b|");
    }

    #[test]
    fn documentclass_ltxguide_enables_the_pipe_short_verb() {
        for preamble in [
            "\\documentclass{ltxguide}",
            "\\documentclass[a4paper]{ltxdoc}",
            "\\documentclass{ltxguide}[1994/11/20]",
            "\\documentclass{l3doc}",
            "\\documentclass[leqno,titlepage]{amsldoc}[1999/12/13]",
        ] {
            let input = format!("{preamble}\n|}}| done");
            let toks = lex(&input);
            assert!(
                toks.iter()
                    .any(|t| t.kind == SyntaxKind::VERB && t.text == "|}|"),
                "no VERB captured after {preamble}"
            );
        }
        let toks = lex("\\documentclass{article}\n|x| done");
        assert!(!toks.iter().any(|t| t.kind == SyntaxKind::VERB));
    }

    #[test]
    fn short_verb_never_captures_a_left_right_delimiter() {
        let toks = lex("\\MakeShortVerb{\\|} $\\left|x\\right|$");
        assert!(!toks.iter().any(|t| t.kind == SyntaxKind::VERB));
        assert_lossless("\\MakeShortVerb{\\|} $\\left|x\\right|$");
    }

    #[test]
    fn unclosed_short_verb_char_stands_alone() {
        let toks = lex("\\MakeShortVerb{\\|} a|b\nc");
        assert!(!toks.iter().any(|t| t.kind == SyntaxKind::VERB));
        assert!(
            toks.iter()
                .any(|t| t.kind == SyntaxKind::WORD && t.text == "|")
        );
        assert_lossless("\\MakeShortVerb{\\|} a|b\nc");
    }

    #[test]
    fn raw_capture_content_does_not_change_later_lexing() {
        const ENV_BODIES: &[&str] = &[
            "",
            "plain text",
            "\\makeatletter",
            "\\ExplSyntaxOn",
            "\\MakeShortVerb{\\|}",
            "{{{",
            "}}}",
            "% not a comment",
            "$ & # ^ _ ~",
            "\\end{verbatimx}",
            "\\begin{verbatim}",
            "\\char`{",
            "\\left(",
        ];
        const INLINE_BODIES: &[&str] = &[
            "",
            "x",
            "\\makeatletter",
            "\\ExplSyntaxOn",
            "{}",
            "$ & # ^ _ ~",
            "% not a comment",
            "\\char`",
        ];
        const SUFFIX: &str = "after \\my@cmd \\l_tmpa_tl |bar| \\char`{ \\left( x\n";

        for (prefix, open, close, bodies) in [
            (
                "before x\n",
                "\\begin{verbatim}\n",
                "\n\\end{verbatim}\n",
                ENV_BODIES,
            ),
            (
                "before x\n",
                "\\begin{lstlisting}[a=b]\n",
                "\n\\end{lstlisting}\n",
                ENV_BODIES,
            ),
            ("before x ", "\\verb+", "+ ", INLINE_BODIES),
            ("before x ", "\\url{", "} ", INLINE_BODIES),
            ("before x ", "\\href{", "}{visible} ", INLINE_BODIES),
            ("before x ", "\\lstinline+", "+ ", INLINE_BODIES),
        ] {
            let mut expected: Option<Vec<(SyntaxKind, String)>> = None;
            for body in bodies {
                let region = format!("{open}{body}{close}");
                let doc = format!("{prefix}{region}{SUFFIX}");
                assert_lossless(&doc);

                let toks = lex(&doc);
                assert!(
                    toks.iter()
                        .any(|t| matches!(t.kind, SyntaxKind::VERB | SyntaxKind::VERBATIM_BODY))
                        || body.is_empty(),
                    "no raw capture formed, so this case proves nothing\n  \
                     region: {region:?}",
                );

                let from = prefix.len() + region.len();
                let mut off = 0usize;
                let got: Vec<(SyntaxKind, String)> = toks
                    .into_iter()
                    .filter(|t| {
                        let start = off;
                        off += t.text.len();
                        start >= from
                    })
                    .map(|t| (t.kind, t.text.to_string()))
                    .collect();

                match &expected {
                    None => expected = Some(got),
                    Some(want) => assert_eq!(
                        &got, want,
                        "a raw body changed how the text after it lexes\n  \
                         region: {region:?}",
                    ),
                }
            }
        }
    }

    #[test]
    fn a_body_that_breaks_its_capture_changes_later_lexing() {
        let captured = lex("\\url{x} \\char`{");
        assert!(captured.iter().any(|t| t.kind == SyntaxKind::VERB));
        assert!(
            captured
                .iter()
                .any(|t| t.kind == SyntaxKind::WORD && t.text == "`{")
        );

        let broken = lex("\\url{{} \\char`{");
        assert!(!broken.iter().any(|t| t.kind == SyntaxKind::VERB));
        assert!(
            broken
                .iter()
                .any(|t| t.kind == SyntaxKind::WORD && t.text == "`")
        );
    }
}
