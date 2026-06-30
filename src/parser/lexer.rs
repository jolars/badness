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
//! - **`\ExplSyntaxOn` / `\ExplSyntaxOff`** (also opened by `\ProvidesExplPackage`
//!   / `\ProvidesExplClass` / `\ProvidesExplFile`): toggles `_` and `:` into
//!   letters so expl3 names (`\seq_new:N`, `\__module_internal:nn`) lex as one
//!   control word. Composes with `\makeatletter` for the `@@` module-prefix
//!   convention (`\g_@@_frame_title_tl`).
//! - **`\left` / `\right` delimiters**: the single delimiter that follows is
//!   isolated as its own token, so a word-character delimiter (`(`, `)`, `|`,
//!   `/`, `.`, `<`, `>`) does not glue into the following word run and become
//!   un-splittable downstream (the same problem `\verb` has). Control-symbol /
//!   control-word / bracket delimiters already lex as single tokens.
//!
//! None of these resolve macro meaning; they are surface lexing concerns (in
//! TeX, catcodes genuinely change in these regions).

use std::collections::HashMap;

use smol_str::SmolStr;

use crate::semantic::signature::{ArgKind, ArgSpec, builtin};
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
/// letter from the first byte (a static, extension-driven catcode fact —
/// sanctioned exactly like the explicit `\makeatletter` mode, `AGENTS.md`
/// decision #1). A trailing explicit `\makeatother` still applies.
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

/// Per-parse lexer context carrying *user-defined* verbatim constructs — those a
/// document declares with catcode manipulation (`\@makeother\$`, …), found by scanning
/// definition bodies ([`crate::semantic::define`]). The lexer consults it (alongside
/// the built-in DB) to capture a verbatim *command*'s final argument as one `VERB`
/// token, and a verbatim *environment*'s body as one `VERBATIM_BODY` token. Empty for
/// the first parse pass; populated for the second when the document defines any (see
/// `parser::core`).
///
/// A command entry maps a name (no leading `\`) to its *leading*, non-verbatim
/// argument shape, the verbatim argument itself being implicit — matching the built-in
/// convention. An environment entry maps a name to its full argument shape (an
/// environment's args are all leading; its body follows the `\begin{…}` arguments), so
/// presence in `environments` means the environment is verbatim.
#[derive(Debug, Default, Clone)]
pub struct VerbCtx {
    commands: HashMap<SmolStr, Vec<ArgSpec>>,
    environments: HashMap<SmolStr, Vec<ArgSpec>>,
}

impl VerbCtx {
    /// Whether the context names no user verbatim constructs (the common case — the
    /// second parse pass is skipped entirely).
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty() && self.environments.is_empty()
    }

    /// Record that `name` is a verbatim-argument command with the given `leading`
    /// (non-verbatim) argument shape.
    pub(crate) fn insert(&mut self, name: SmolStr, leading: Vec<ArgSpec>) {
        self.commands.insert(name, leading);
    }

    /// Record that environment `name` is verbatim, with the given argument shape (all
    /// leading; the raw body follows the arguments).
    pub(crate) fn insert_environment(&mut self, name: SmolStr, args: Vec<ArgSpec>) {
        self.environments.insert(name, args);
    }

    /// The leading argument shape of `name` if it is a known user verbatim command.
    fn leading_args(&self, name: &str) -> Option<&[ArgSpec]> {
        self.commands.get(name).map(Vec::as_slice)
    }

    /// The argument shape of `name` if it is a user-defined verbatim environment.
    fn verbatim_environment_args(&self, name: &str) -> Option<&[ArgSpec]> {
        self.environments.get(name).map(Vec::as_slice)
    }

    /// Is `name` a verbatim-like environment — one whose body the parser must route to
    /// its raw-body branch, per `AGENTS.md` Core decision #1? A user-defined one (from
    /// this context) or a built-in one ([`builtin`]). Both the lexer (to find where the
    /// raw body begins) and the structural parser (`grammar.rs`) ask this question, so
    /// one lookup keeps them in lockstep. We read only static argument-shape data; no
    /// macro meaning is resolved, so this stays within decision #1's sanctioned modes.
    ///
    /// Deliberately consults [`builtin`] only, never the bulk CWL tier
    /// ([`crate::semantic::signature::cwl`]): routing a body to the raw-verbatim
    /// branch is lossy if wrong, so this behavior decision rests solely on curated
    /// data (the CWL tier carries `verbatim_body == false` for every entry anyway).
    pub(crate) fn is_verbatim_environment(&self, name: &str) -> bool {
        self.environments.contains_key(name)
            || builtin()
                .environment(name)
                .is_some_and(|env| env.verbatim_body)
    }
}

/// Is `name` a block/display environment — one whose lone occurrence the parser
/// should leave unwrapped rather than nest in a redundant `PARAGRAPH`? Resolved
/// against the built-in signature database ([`builtin`]) only: the parser runs
/// before any per-file `\newenvironment` scan, so (as with verbatim) user-defined
/// block-ness is unknown at parse time and a user/unknown environment stays
/// wrapped — the conservative, lossless-safe default. The bulk CWL tier is not
/// consulted here (it carries no `block` flag, and parser layout decisions stay on
/// curated data).
pub(crate) fn is_block_environment(name: &str) -> bool {
    builtin().environment(name).is_some_and(|env| env.block)
}

/// Whether `text` (a `CONTROL_WORD`, leading `\` included) is a command-definition
/// keyword whose immediately-following name must not be lexed as a verbatim call.
/// Covers the LaTeX2e and xparse families the definition scanner recognizes plus the
/// primitive `\def` family; `\let` is included since it too binds a following name.
/// Reads only the static keyword, no macro meaning.
fn is_definition_keyword(text: &str) -> bool {
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

/// An expl3 catcode-mode toggle recognized purely by its control-word spelling.
/// Shared by the lexer (which flips its `expl_syntax` flag) and the formatter's
/// region pre-pass ([`crate::formatter`] recomputes in-region byte spans), so the
/// two read the *same* fixed toggle set and can never drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExplToggle {
    /// `\ExplSyntaxOn`, or `\ProvidesExplPackage`/`Class`/`File` (which open expl3
    /// syntax for the rest of the file).
    On,
    /// `\ExplSyntaxOff`.
    Off,
}

/// Classify a control word's text as an expl3 catcode-mode toggle, if any. Only
/// meaningful on [`SyntaxKind::CONTROL_WORD`] text: a `\ExplSyntaxOn` inside a
/// `\verb`/comment lexes as a `VERB`/`COMMENT` token and so never reaches here.
pub(crate) fn expl_toggle(text: &str) -> Option<ExplToggle> {
    match text {
        "\\ExplSyntaxOn"
        | "\\ProvidesExplPackage"
        | "\\ProvidesExplClass"
        | "\\ProvidesExplFile" => Some(ExplToggle::On),
        "\\ExplSyntaxOff" => Some(ExplToggle::Off),
        _ => None,
    }
}

/// Lex `input` into a flat, lossless token stream, consulting only the built-in
/// signature DB for verbatim commands/environments. The entry used by the first
/// parse pass; [`lex_with`] adds user-defined verbatim commands. Uses the
/// [`Document`](LatexFlavor::Document) flavor (ordinary starting catcodes).
pub fn lex(input: &str) -> Vec<Token> {
    lex_with(input, &VerbCtx::default(), LexConfig::default())
}

/// Lex `input` like [`lex`], additionally treating the user-defined verbatim
/// commands in `ctx` as verbatim (their final argument captured as one `VERB`
/// token). Used by the second parse pass once definition scanning has discovered
/// catcode-othering commands. `config` fixes the initial catcode regime (a
/// [`Package`](LatexFlavor::Package) flavor starts with `@` already a letter) and
/// whether to run the `.dtx` docstrip mode.
pub fn lex_with(input: &str, ctx: &VerbCtx, config: LexConfig) -> Vec<Token> {
    let mut out = Vec::new();
    let mut pos = 0;
    let mut at_letter = config.flavor.letter_mode_start(); // `\makeatletter` state
    // `\ExplSyntaxOn` state: while true, `_` and `:` are catcode-11 letters, so
    // expl3 names (`\seq_new:N`, `\__module_internal:nn`) lex as single control
    // words. Toggled by `\ExplSyntaxOn`/`\ExplSyntaxOff` and turned on by the
    // `\ProvidesExpl*` package/class/file declarations (a sanctioned static lexer
    // mode, `AGENTS.md` decision #1). Independent of `at_letter`; the two compose.
    let mut expl_syntax = false;
    // `.dtx` docstrip mode: true at the start of a physical line (start of input
    // or just after a `NEWLINE`), so a line-leading `%` can be recognized as a
    // documentation margin. Any token — including whitespace — clears it, matching
    // docstrip's rule that only a `%` in *column 0* is a margin.
    let mut at_line_start = true;
    // True while inside a `macrocode`/`macrocode*` environment body (between its
    // frame lines). There, code lines carry no margin, a line-leading `%` is an
    // ordinary code comment (not a margin), and `@` is a letter (`macrocode` runs
    // under `\makeatletter`). The pre-macrocode `at_letter` is saved here and
    // restored on exit.
    let mut in_macrocode = false;
    let mut saved_at_letter = at_letter;
    // True when the previous meaningful token was `\left`/`\right`, so the next
    // delimiter must be isolated as a single token (it carries across whitespace,
    // which TeX skips before the delimiter).
    let mut pending_delim = false;
    // True while the next control word is the *name being defined* by a definition
    // keyword (`\newcommand\foo…`, `\NewDocumentCommand{\foo}…`, `\def\foo…`), so it
    // must not be lexed as a verbatim *call*: at a definition site the trailing
    // `{…}` are the signature/body, not the command's argument. Persists across the
    // intervening `{`/whitespace of the braced form and clears once the name is
    // consumed. Without this, a command flagged verbatim in pass 1 would have its own
    // definition's first group captured as a `VERB` in pass 2.
    let mut pending_def = false;
    while pos < input.len() {
        let rest = &input[pos..];

        // `.dtx` `macrocode` frame line. A `%␣*\begin{macrocode}` line opens a code
        // region; its `%␣*\end{macrocode}` terminator closes it. Both lex as a
        // margin + indent + `\begin`/`\end{macrocode}` so the ordinary environment
        // grammar pairs them, but the *body* in between lexes as real code, under
        // the package regime (`@` a letter) with no margin stripping. We look for a
        // begin frame outside the body and the end frame inside it; anything else on
        // a `%` line inside the body is an ordinary code comment.
        if config.dtx
            && at_line_start
            && let Some(consumed) = lex_macrocode_frame(rest, !in_macrocode, &mut out)
        {
            if in_macrocode {
                in_macrocode = false;
                at_letter = saved_at_letter;
            } else {
                in_macrocode = true;
                saved_at_letter = at_letter;
                at_letter = true;
            }
            pos += consumed;
            at_line_start = false;
            pending_delim = false;
            pending_def = false;
            continue;
        }

        // `.dtx` docstrip guard: a line-leading `%<…>` is a docstrip guard
        // expression (`%<*tag>`/`%</tag>` block delimiters or an inline `%<tag>`
        // prefix), not a comment. Emit the `%<…>` (through the closing `>`) as a
        // single `GUARD` trivia leaf; code after an inline guard's `>` lexes
        // normally. Guards nest on the docstrip axis, orthogonal to LaTeX nesting,
        // so this is a flat floating leaf (no block node), like a margin. Recognized
        // at line start only (column-0 rule) but in *any* layer — guards punctuate
        // `macrocode` bodies too — so it is not gated on `in_macrocode`. A `%<` with
        // no closing `>` before the line ends is not a guard; it falls through to an
        // ordinary comment. Trivia, so `pending_delim`/`pending_def` carry across.
        if config.dtx
            && at_line_start
            && rest.starts_with("%<")
            && let Some(rel) = rest[2..].find(['>', '\n', '\r'])
            && rest.as_bytes()[2 + rel] == b'>'
        {
            let len = 2 + rel + 1;
            out.push(Token {
                kind: SyntaxKind::GUARD,
                text: SmolStr::new(&rest[..len]),
            });
            pos += len;
            at_line_start = false;
            continue;
        }

        // `.dtx` documentation margin: a line-leading `%` (but not a `%<…>` guard,
        // which lexes as a `GUARD` above) is a documentation line's
        // comment *margin*, not a comment. Emit it as a `DOC_MARGIN` trivia token —
        // one byte, never the following space — so the rest of the line lexes (and
        // parses) as ordinary LaTeX and the margin floats like whitespace. Only the
        // line-leading `%` is a margin; a later `%` on the same line stays a
        // `COMMENT`. Inside a `macrocode` body there is no margin (code lines own
        // their `%`), so this is gated on `!in_macrocode`. The margin is trivia, so
        // it carries `pending_delim`/`pending_def` across unchanged (like whitespace).
        if config.dtx
            && at_line_start
            && !in_macrocode
            && rest.starts_with('%')
            && !rest.starts_with("%<")
        {
            out.push(Token {
                kind: SyntaxKind::DOC_MARGIN,
                text: SmolStr::new("%"),
            });
            pos += 1;
            at_line_start = false;
            continue;
        }

        // Verbatim-like environment: emit `\begin{name}` then a raw body token.
        if let Some(consumed) = lex_verbatim_environment(rest, ctx, &mut out) {
            pos += consumed;
            pending_delim = false;
            pending_def = false;
            at_line_start = false;
            continue;
        }

        // Verbatim-argument command (`\url{…}`, `\code{…}`, `\lstinline|…|`, …):
        // emit the control word and any leading args, then a raw argument token.
        // `\verb`/`\verb*` are handled separately in `lex_control` (delimiter
        // only), so they fall through here. Suppressed at a definition site
        // (`pending_def`), where the following groups are the signature/body.
        if !pending_def
            && let Some(consumed) =
                lex_verbatim_command(rest, at_letter, expl_syntax, ctx, &mut out)
        {
            pos += consumed;
            pending_delim = false;
            at_line_start = false;
            continue;
        }

        let (kind, mut len) = next_token(rest, at_letter, expl_syntax);
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
                // `\ExplSyntaxOn`/`Off`, and the `\ProvidesExpl*` declarations which
                // open expl3 syntax for the rest of the file (they appear at the top
                // of an expl3 package/class) so left-to-right they act as an On.
                _ => {
                    if let Some(toggle) = expl_toggle(text) {
                        expl_syntax = matches!(toggle, ExplToggle::On);
                    }
                }
            }
        }
        pending_delim = match kind {
            // Trivia is skipped before the delimiter, so the mode persists.
            SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE => pending_delim,
            SyntaxKind::CONTROL_WORD if text == "\\left" || text == "\\right" => true,
            _ => false,
        };
        pending_def = match kind {
            // A definition keyword arms the suppression for the name that follows.
            SyntaxKind::CONTROL_WORD if is_definition_keyword(text) => true,
            // The braced name form (`\newcommand{\foo}`) interposes a `{` and
            // whitespace before the name; keep the suppression armed across them.
            SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::L_BRACE => pending_def,
            // Any other token — in particular the defined name's own control word —
            // consumes the suppression.
            _ => false,
        };
        out.push(Token {
            kind,
            text: SmolStr::new(text),
        });
        // A new physical line begins right after a `NEWLINE`; any other token
        // (whitespace included) leaves the cursor mid-line.
        at_line_start = kind == SyntaxKind::NEWLINE;
        pos += len;
    }
    out
}

/// Classify the token at the start of `rest` and return its `(kind, byte_len)`.
fn next_token(rest: &str, at_letter: bool, expl_syntax: bool) -> (SyntaxKind, usize) {
    let c = rest.chars().next().expect("rest is non-empty");
    match c {
        '\\' => lex_control(rest, at_letter, expl_syntax),
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
        // Under `\ExplSyntaxOn`, `_` is a catcode-11 letter, not a subscript: a
        // bare `_` joins the surrounding word run (handled by the default arm).
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

/// Lex a control sequence: `rest` is known to start with `\`.
fn lex_control(rest: &str, at_letter: bool, expl_syntax: bool) -> (SyntaxKind, usize) {
    match rest[1..].chars().next() {
        // Control word: backslash + one or more letters (`@` too under
        // `\makeatletter`; `_`/`:` too under `\ExplSyntaxOn`).
        Some(d) if is_letter(d, at_letter, expl_syntax) => {
            let letters = run_len(&rest[1..], |c| is_letter(c, at_letter, expl_syntax));
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
fn lex_verbatim_environment(rest: &str, ctx: &VerbCtx, out: &mut Vec<Token>) -> Option<usize> {
    let after_begin = rest.strip_prefix("\\begin{")?;
    let close = after_begin.find('}')?;
    let name = &after_begin[..close];
    // A user-defined catcode-verbatim environment (from `ctx`) wins over the built-in
    // DB; either way we read only the static leading-argument shape, never macro
    // meaning. The verbatim args are all leading — the raw body follows them.
    let args: &[ArgSpec] = match ctx.verbatim_environment_args(name) {
        Some(args) => args,
        None => {
            &builtin()
                .environment(name)
                .filter(|e| e.verbatim_body)?
                .args
        }
    };

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
    let args_len = scan_verbatim_args(args_region, args);
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

/// A `.dtx` `macrocode` frame line, at a line start: `%␣*\begin{macrocode}` (when
/// `want_begin`) or `%␣*\end{macrocode}` (otherwise), with the `*` variant
/// accepted. On a match, emit the frame tokens — the `%` margin, the indent
/// whitespace, the `\begin`/`\end` control word, and the `{macrocode}` name group —
/// and return the bytes consumed (through the closing `}`; the trailing newline
/// lexes normally). Returns `None` when `rest` is not the requested frame.
///
/// Unlike a verbatim environment, the body is *not* captured here: it lexes as
/// ordinary code in the main loop (under the package regime). The frame line must
/// hold nothing but trailing whitespace after the name group, so a stray
/// `\begin{macrocode}{x}` is not mistaken for a frame.
fn lex_macrocode_frame(rest: &str, want_begin: bool, out: &mut Vec<Token>) -> Option<usize> {
    let after_pct = rest.strip_prefix('%')?;
    let ws_len = after_pct
        .bytes()
        .take_while(|&b| b == b' ' || b == b'\t')
        .count();
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
    // The frame line carries nothing but trailing whitespace after `}`.
    let after_close = &after_open[close + 1..];
    let trailing = after_close
        .bytes()
        .take_while(|&b| b == b' ' || b == b'\t')
        .count();
    let tail = &after_close[trailing..];
    if !(tail.is_empty() || tail.starts_with('\n') || tail.starts_with('\r')) {
        return None;
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
    Some(1 + ws_len + control.len() + 1 + name.len() + 1)
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
fn lex_verbatim_command(
    rest: &str,
    at_letter: bool,
    expl_syntax: bool,
    ctx: &VerbCtx,
    out: &mut Vec<Token>,
) -> Option<usize> {
    if !rest.starts_with('\\') {
        return None;
    }
    let letters = run_len(&rest[1..], |c| is_letter(c, at_letter, expl_syntax));
    if letters == 0 {
        return None;
    }
    let word_len = 1 + letters;
    let name = &rest[1..word_len];
    // `\verb` keeps its dedicated delimiter-only path.
    if name == "verb" {
        return None;
    }
    // A user-defined catcode-verbatim command (from `ctx`) wins over the built-in DB;
    // either way we read only the static leading-argument shape, never macro meaning.
    let leading = match ctx.leading_args(name) {
        Some(args) => args,
        None => &builtin().command(name).filter(|c| c.verbatim)?.args,
    };

    // Leading arguments precede the verbatim one (e.g. `\mintinline{lang}{code}`).
    let after_word = &rest[word_len..];
    let args_len = scan_verbatim_args(after_word, leading);

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
            c @ (b'}' | b']') if stack.last() == Some(&c) => {
                stack.pop();
                if stack.is_empty() {
                    return Some(i + 1);
                }
            }
            // A non-matching closer is literal text; ignore it.
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
        let (kind, len) = next_token(&region[pos..], false, false);
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

/// A control-word continuation character: a letter, `@` under `\makeatletter`,
/// or `_`/`:` under `\ExplSyntaxOn` (where they are catcode-11 letters).
fn is_letter(c: char, at_letter: bool, expl_syntax: bool) -> bool {
    c.is_ascii_alphabetic() || (at_letter && c == '@') || (expl_syntax && (c == '_' || c == ':'))
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
    fn block_environment_classification() {
        assert!(is_block_environment("figure"));
        assert!(is_block_environment("itemize")); // derived via `list`
        assert!(!is_block_environment("myenv")); // unknown
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
    fn expl_syntax_makes_underscore_and_colon_letters() {
        let toks = lex(r"\ExplSyntaxOn\seq_new:N\ExplSyntaxOff\seq_new:N");
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        // Under \ExplSyntaxOn, `\seq_new:N` is one control word…
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\seq_new:N")));
        // …after \ExplSyntaxOff it stops at the first `_`.
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
        // The `\ProvidesExplPackage` declaration opens expl3 syntax, so the later
        // `\tl_set:Nn` lexes as one control word.
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\tl_set:Nn")));
    }

    #[test]
    fn expl_syntax_composes_with_makeatletter() {
        // The `@@` module-prefix convention needs both `@` and `_`/`:` as letters.
        let toks = lex(r"\makeatletter\ExplSyntaxOn\g_@@_frame_title_tl");
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\g_@@_frame_title_tl")));
    }

    #[test]
    fn expl_syntax_makes_bare_underscore_a_word_not_subscript() {
        let toks = lex(r"\ExplSyntaxOn a_b");
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        // Under expl3, `_` is a catcode-11 letter: `a_b` is one word, no UNDERSCORE.
        assert!(seen.contains(&(SyntaxKind::WORD, "a_b")));
        assert!(!seen.iter().any(|(k, _)| *k == SyntaxKind::UNDERSCORE));
    }

    #[test]
    fn package_flavor_starts_in_letter_mode() {
        // A `.sty`/`.cls` is loaded under an implicit `\makeatletter`, so `@` is a
        // letter from the first byte — `\foo@bar` is one control word with no
        // explicit `\makeatletter`.
        let toks = lex_with(
            r"\foo@bar",
            &VerbCtx::default(),
            LatexFlavor::Package.into(),
        );
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert_eq!(seen, vec![(SyntaxKind::CONTROL_WORD, "\\foo@bar")]);
    }

    #[test]
    fn package_flavor_respects_trailing_makeatother() {
        // Letter-mode starts on, but an explicit `\makeatother` still turns it off.
        let toks = lex_with(
            r"\foo@bar\makeatother\foo@bar",
            &VerbCtx::default(),
            LatexFlavor::Package.into(),
        );
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\foo@bar")));
        // After \makeatother the second occurrence splits into `\foo` + `@bar`.
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\foo")));
    }

    #[test]
    fn document_flavor_keeps_at_non_letter() {
        // The default `.tex` flavor does not start in letter-mode.
        let toks = lex(r"\foo@bar");
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\foo")));
        assert!(!seen.contains(&(SyntaxKind::CONTROL_WORD, "\\foo@bar")));
    }

    #[test]
    fn dtx_mode_lexes_line_leading_percent_as_a_margin() {
        // A line-leading `%` is a one-byte `DOC_MARGIN`; the rest of the doc line
        // lexes as ordinary LaTeX. A `%` not in column 0 stays a `COMMENT`.
        let dtx = LexConfig {
            flavor: LatexFlavor::Document,
            dtx: true,
        };
        let toks = lex_with("% \\foo\nbar % tail\n", &VerbCtx::default(), dtx);
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        assert_eq!(seen[0], (SyntaxKind::DOC_MARGIN, "%"));
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\foo")));
        assert!(seen.contains(&(SyntaxKind::COMMENT, "% tail")));
        // Exactly one margin (column 0 of the first line only).
        assert_eq!(
            seen.iter()
                .filter(|(k, _)| *k == SyntaxKind::DOC_MARGIN)
                .count(),
            1
        );
    }

    #[test]
    fn dtx_mode_is_off_by_default_for_margins_and_guards() {
        // Without the docstrip flag a `%` line stays a comment (plain `.tex`); a
        // `%<…>` guard likewise stays a single comment.
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
        // `%<*tag>` / `%</tag>` block delimiters are single `GUARD` tokens.
        let block = lex_with("%<*driver>\n%</driver>\n", &VerbCtx::default(), dtx);
        assert_eq!(block[0].kind, SyntaxKind::GUARD);
        assert_eq!(block[0].text, "%<*driver>");
        assert!(
            block
                .iter()
                .any(|t| t.kind == SyntaxKind::GUARD && t.text == "%</driver>")
        );
        // An inline `%<tag>` is a `GUARD` prefix; the rest of the line lexes as code.
        let inline = lex_with("%<plain>\\RequirePackage{x}\n", &VerbCtx::default(), dtx);
        assert_eq!(inline[0].kind, SyntaxKind::GUARD);
        assert_eq!(inline[0].text, "%<plain>");
        assert!(
            inline
                .iter()
                .any(|t| t.kind == SyntaxKind::CONTROL_WORD && t.text == "\\RequirePackage")
        );
        // A boolean tag expression stays one token (through the closing `>`).
        let expr = lex_with("%<*package|driver>\n", &VerbCtx::default(), dtx);
        assert_eq!(expr[0].kind, SyntaxKind::GUARD);
        assert_eq!(expr[0].text, "%<*package|driver>");
        // A guard recognized only at column 0: a mid-line `%<…>` stays a comment.
        let midline = lex_with("a %<x>\n", &VerbCtx::default(), dtx);
        assert!(
            midline
                .iter()
                .any(|t| t.kind == SyntaxKind::COMMENT && t.text == "%<x>")
        );
        assert!(!midline.iter().any(|t| t.kind == SyntaxKind::GUARD));
        // A `%<` with no closing `>` before the line ends is not a guard.
        let malformed = lex_with("%<unterminated\n", &VerbCtx::default(), dtx);
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
