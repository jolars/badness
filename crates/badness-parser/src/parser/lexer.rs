//! A total, lossless lexer for LaTeX surface syntax.
//!
//! Every byte of the input ends up in exactly one token, so concatenating all
//! token texts reproduces the input verbatim — the losslessness invariant. The
//! lexer is mostly context-free, with a small set of statically recognizable modes:
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

/// Per-parse context carrying the facts the parser can only learn by first
/// scanning the file's own definitions ([`crate::semantic::define`]) — the
/// sanctioned second pass described in `parser::core`. Empty for the first pass;
/// populated for the second when the document defines any. Both the lexer and the
/// grammar read it, so the two can never disagree about what a name is.
///
/// It carries two families of fact, each read from static definition surface only
/// (no macro meaning, per `AGENTS.md` Core decision #1):
///
/// 1. *User-defined verbatim constructs* — those a document declares with catcode
///    manipulation (`\@makeother\$`, …). The lexer consults these (alongside the
///    built-in DB) to capture a verbatim *command*'s final argument as one `VERB`
///    token, and a verbatim *environment*'s body as one `VERBATIM_BODY` token.
/// 2. *Environment aliases* — a command whose definition body is exactly
///    `\begin{X}`/`\end{X}`, so `\bea … \eea` pairs as an `ENVIRONMENT` of `X`
///    (issue #109). The grammar consults these; the lexer does not.
///
/// A command entry maps a name (no leading `\`) to its *leading*, non-verbatim
/// argument shape, the verbatim argument itself being implicit — matching the built-in
/// convention. An environment entry maps a name to its full argument shape (an
/// environment's args are all leading; its body follows the `\begin{…}` arguments), so
/// presence in `environments` means the environment is verbatim.
///
/// `suppressed` names the inverse case: commands the current file *redefines* to an
/// ordinary (non-verbatim) macro whose name collides with a built-in raw-argument
/// command (`\code`, `\url`, `\href`, …). A local definition shadows the built-in, so
/// [`lex_verbatim_command`] must lex `\code{…}` as an ordinary group rather than capture
/// the built-in `VERB` (follow-up to issue #53). We read only static definition facts (a
/// visible `\newcommand`/`\def` with no catcode signal), never macro meaning.
/// `PartialEq` is load-bearing rather than incidental: `parser::core` decides
/// whether the second pass has anything to do by comparing the scanned context
/// against the declaration seed it started from, which stays correct as fields
/// are added in a way a hand-maintained "did anything change" flag would not.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParseCtx {
    commands: HashMap<SmolStr, Vec<ArgSpec>>,
    environments: HashMap<SmolStr, Vec<ArgSpec>>,
    suppressed: HashSet<SmolStr>,
    /// Environment-alias openers: command name (no leading `\`) → target
    /// environment. See [`crate::semantic::signature::SignatureDb::env_begin_alias`]
    /// for the admission rules that decide what lands here.
    begin_aliases: HashMap<SmolStr, SmolStr>,
    /// The closer mirror of [`begin_aliases`](Self::begin_aliases).
    end_aliases: HashMap<SmolStr, SmolStr>,
    /// Environment signatures a project *declared*
    /// ([`crate::declarations`]), whole rather than reduced to the one fact a
    /// map above records: body routing reads several flags (`math`,
    /// `verbatim_body`, `block`), and a declaration is authoritative for every
    /// one of them at once.
    declared_environments: HashMap<SmolStr, EnvironmentSig>,
}

/// The former name of [`ParseCtx`], kept so the published crate's API does not
/// break. It carries environment aliases as well as verbatim facts now.
pub type VerbCtx = ParseCtx;

impl ParseCtx {
    /// Whether the context names nothing at all — no user verbatim constructs, no
    /// suppressions, and no environment aliases — so the second parse pass can be
    /// skipped entirely (the common case).
    ///
    /// Every map must be accounted for here: a file that defines an alias but no
    /// verbatim construct would otherwise never reach pass 2, and its aliases would
    /// silently do nothing.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
            && self.environments.is_empty()
            && self.suppressed.is_empty()
            && self.begin_aliases.is_empty()
            && self.end_aliases.is_empty()
            && self.declared_environments.is_empty()
    }

    /// Overlay a project's [declarations](crate::declarations) onto this
    /// context, taking precedence over anything already recorded.
    ///
    /// Declared beats scanned because a declaration is the user explicitly
    /// correcting an inference (`AGENTS.md` decision #12), which is why this is
    /// an overlay applied *after* the scan rather than a seed the scan writes
    /// over.
    ///
    /// Two families cross over: the declared environment signatures, which
    /// every body-routing predicate here then answers from, and the delimiter
    /// spellings. The alias entries skip the "is it called anywhere" filter
    /// `parser::core::parse_ctx` applies to scanned ones: that filter exists to
    /// avoid buying a *second* pass for an alias no call site uses, and a
    /// declaration is already in hand before the first.
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

    /// Record that `name` is a verbatim-argument command with the given `leading`
    /// (non-verbatim) argument shape.
    pub(crate) fn insert(&mut self, name: SmolStr, leading: Vec<ArgSpec>) {
        self.commands.insert(name, leading);
    }

    /// Record that `name` — a built-in raw-argument command — is redefined
    /// non-verbatim in this file, so its built-in verbatim capture is suppressed.
    pub(crate) fn suppress(&mut self, name: SmolStr) {
        self.suppressed.insert(name);
    }

    /// Whether `name`'s built-in verbatim capture is suppressed by a local redefinition.
    fn is_suppressed(&self, name: &str) -> bool {
        self.suppressed.contains(name)
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

    /// The argument shape of `name` if it is a user-defined or declared verbatim
    /// environment — what the lexer needs to find where the raw body begins.
    fn verbatim_environment_args(&self, name: &str) -> Option<&[ArgSpec]> {
        if let Some(sig) = self.declared_environment(name) {
            return sig.verbatim_body.then(|| &*sig.args);
        }
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

    /// Record that command `name` (no leading `\`) opens environment `target`.
    pub(crate) fn insert_begin_alias(&mut self, name: SmolStr, target: SmolStr) {
        self.begin_aliases.insert(name, target);
    }

    /// Record that command `name` closes environment `target`.
    pub(crate) fn insert_end_alias(&mut self, name: SmolStr, target: SmolStr) {
        self.end_aliases.insert(name, target);
    }

    /// The environment `name` opens, if it is a known alias opener.
    pub(crate) fn begin_alias(&self, name: &str) -> Option<&str> {
        self.begin_aliases.get(name).map(SmolStr::as_str)
    }

    /// The environment `name` closes, if it is a known alias closer.
    pub(crate) fn end_alias(&self, name: &str) -> Option<&str> {
        self.end_aliases.get(name).map(SmolStr::as_str)
    }

    /// Every environment some alias *opens*, so the pre-scan can recognize the
    /// literal `\end{X}` that closes it (issue #117). Names repeat when an
    /// environment has several opener spellings; callers collect into a set.
    pub(crate) fn begin_alias_targets(&self) -> impl Iterator<Item = &str> {
        self.begin_aliases.values().map(SmolStr::as_str)
    }

    /// The signature a project *declared* for environment `name`, if any.
    ///
    /// A declared entry is **authoritative** for its name: every predicate below
    /// answers from it alone rather than falling back to the built-in, because a
    /// declaration is the user correcting what badness would otherwise infer
    /// (`AGENTS.md` decision #12). Declaring `myenv` to be `like = "align"` when
    /// the file also `\newenvironment`s it verbatim means the declaration wins,
    /// not that the two answers are merged.
    fn declared_environment(&self, name: &str) -> Option<&EnvironmentSig> {
        self.declared_environments.get(name)
    }

    /// Is `name` a block/display environment — one whose lone occurrence the
    /// parser should leave unwrapped rather than nest in a redundant
    /// `PARAGRAPH`? A declared one, or a curated built-in.
    ///
    /// The parser runs before any per-file `\newenvironment` scan, so a *scanned*
    /// environment's block-ness is unknown at parse time and an unknown
    /// environment stays wrapped — the conservative, lossless-safe default. The
    /// bulk CWL tier is not consulted (it carries no `block` flag, and parser
    /// layout decisions stay on curated data).
    pub(crate) fn is_block_environment(&self, name: &str) -> bool {
        match self.declared_environment(name) {
            Some(sig) => sig.block(),
            None => builtin()
                .environment(name)
                .is_some_and(EnvironmentSig::block),
        }
    }

    /// Is `name` a math environment — one whose body the parser should parse in
    /// math mode, wrapping it in a `MATH` node exactly as `\[…\]` does (so
    /// scripts become `SCRIPTED`, operators split, and `\left…\right` pair)? A
    /// declared one, or a curated built-in.
    ///
    /// Never the bulk CWL tier, for the same reason as
    /// [`is_block_environment`](Self::is_block_environment) and
    /// [`is_verbatim_environment`](Self::is_verbatim_environment): routing a body
    /// into math mode is a structural (lossless-preserving but shape-changing)
    /// decision, so it rests solely on curated data — which a declaration is,
    /// since `like` copies a curated entry and resolves against nothing else.
    /// This stays a sanctioned static-fact mode (`AGENTS.md` decision #1): no
    /// macro meaning is resolved, only the `math` flag is read.
    pub(crate) fn is_math_environment(&self, name: &str) -> bool {
        match self.declared_environment(name) {
            Some(sig) => sig.math,
            None => builtin().environment(name).is_some_and(|env| env.math),
        }
    }

    /// Is `name` a statement-body environment — one whose body holds
    /// `;`-terminated statements (the TikZ/pgf picture family), so the parser
    /// wraps each run up to a top-level `;` in a `STATEMENT` node? A declared
    /// one, or a curated built-in.
    ///
    /// Never the bulk CWL tier or the definition scan, for the same reason as
    /// [`is_math_environment`](Self::is_math_environment): wrapping statements
    /// is a structural decision, so it rests solely on curated data — which a
    /// declaration is, since `like` copies a curated entry. The `;` terminator
    /// carries no special catcode; what makes this a sanctioned static-fact
    /// mode (`AGENTS.md` decision #1) is that recognition is retrospective pure
    /// shape (a top-level `;`-carrying WORD) and a run that never reaches one
    /// stays plain paragraph content.
    pub(crate) fn is_statement_environment(&self, name: &str) -> bool {
        match self.declared_environment(name) {
            Some(sig) => sig.statement_body,
            None => builtin()
                .environment(name)
                .is_some_and(|env| env.statement_body),
        }
    }

    /// Whether any environment alias is recorded — the cheap guard the grammar
    /// checks before building its per-token opener/closer index.
    ///
    /// Both maps are read, so this can never disagree with
    /// [`is_empty`](Self::is_empty) about whether the second pass has alias work
    /// to do. `parser::core::parse_ctx` additionally drops a closer whose target
    /// has no live opener, so in practice the maps are non-empty together.
    pub(crate) fn has_env_aliases(&self) -> bool {
        !self.begin_aliases.is_empty() || !self.end_aliases.is_empty()
    }
}

/// Whether `text` (a `CONTROL_WORD`, leading `\` included) is a command-definition
/// keyword whose immediately-following name must not be lexed as a verbatim call.
/// Covers the LaTeX2e and xparse families the definition scanner recognizes plus the
/// primitive `\def` family; `\let` is included since it too binds a following name.
/// Reads only the static keyword, no macro meaning.
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

/// How many immediately-following control words `text` (a `CONTROL_WORD`, leading
/// `\` included) claims as *names* rather than calls — `0` when it is not a
/// definition keyword at all.
///
/// `\let` claims **two**: the definee and the meaning it is given, so
/// `\let\oldbea\bea` mentions `\bea` without calling it. A bare "the next word is
/// a definee" boolean would let that source operand read as a live call, which for
/// the environment-alias index means a `\let` operand can pair with a later closer
/// and wrap the text between them in an environment nobody wrote. Mirrors the
/// `("let", 2)` entry in [`crate::parser::conditional`]'s operand table, which
/// subtracts the same slots for the same reason.
pub(crate) fn definition_name_slots(text: &str) -> u8 {
    match text {
        "\\let" => 2,
        _ if is_definition_keyword(text) => 1,
        _ => 0,
    }
}

/// Whether `text` (a `CONTROL_WORD`, leading `\` included) is a TeX primitive
/// that grabs the *next token* without expanding it, so a following character
/// keeps its literal shape. Only the short-verb capture reads this: an active
/// `|` after `\string` is the token being printed, not a `\verb`-style opener
/// (`\meta{first\texttt{\string|}last}`, lthooks.dtx). A closed curated set,
/// read from the static keyword alone — no macro meaning.
fn is_literal_token_command(text: &str) -> bool {
    matches!(
        text,
        "\\string" | "\\noexpand" | "\\meaning" | "\\expandafter" | "\\show"
    )
}

/// Whether `text` (a `CONTROL_WORD`, leading `\` included) is a TeX primitive
/// that opens a numeric context, where a following number is conventionally
/// written in backtick char-constant notation (`` \char`$ ``, `` \catcode`\%=12 ``,
/// `` \number`\[ ``): after it, a backtick makes the next character *data*, never
/// syntax. A closed curated set; reads only the static keyword, no macro meaning.
/// The number-*producing* primitives (`\number`/`\the`/`\romannumeral`) and the
/// numeric conditionals (`\ifnum`/`\ifodd`/`\ifdim`) are included alongside the
/// codetables because their operand is just as routinely a backtick constant.
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

/// An expl3 catcode-mode toggle recognized purely by its control-word spelling.
/// Shared by the lexer (which flips its `expl_syntax` flag) and the formatter's
/// region pre-pass (the `badness-formatter` crate recomputes in-region byte spans), so the
/// two read the *same* fixed toggle set and can never drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplToggle {
    /// `\ExplSyntaxOn`, or `\ProvidesExplPackage`/`Class`/`File` (which open expl3
    /// syntax for the rest of the file).
    On,
    /// `\ExplSyntaxOff`.
    Off,
}

/// Classify a control word's text as an expl3 catcode-mode toggle, if any. Only
/// meaningful on [`SyntaxKind::CONTROL_WORD`] text: a `\ExplSyntaxOn` inside a
/// `\verb`/comment lexes as a `VERB`/`COMMENT` token and so never reaches here.
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

/// True when a `.dtx` file carries a *static expl3 signal* even though it never
/// runs an in-file toggle: a line-leading `%<@@=…>` docstrip module-prefix guard,
/// or a `\ProvidesExpl{Package,Class,File}` declaration anywhere. Real expl3
/// package sources declare expl3 in the parent `.dtx`/build and set the module
/// prefix `@@` with a `%<@@=mod>` guard, so their `macrocode` bodies are expl3
/// code with no `\ExplSyntaxOn` to see (`ltx-talk-structure.dtx`, TODO.md).
///
/// Scans the raw text before lexing, so it cannot reuse [`expl_toggle`] (which
/// classifies already-lexed token text). Deliberately coarse and name-only, like
/// the lexer's other expl handling: it sees the whole file — prose and verbatim
/// examples included — so a `\ProvidesExpl*` mentioned as text also trips it. That
/// is acceptable (`AGENTS.md` decision #1): a false positive only *joins* `_`/`:`
/// into a control word (lossless), and reading the whole file keeps the signal
/// order-independent, so a body *above* the declaration is flagged too.
pub(crate) fn dtx_has_expl_signal(input: &str) -> bool {
    input.contains("\\ProvidesExpl")
        || input
            .lines()
            .any(|l| l.starts_with("%<@@=") && l[5..].contains('>'))
}

/// Lex `input` into a flat, lossless token stream, consulting only the built-in
/// signature DB for verbatim commands/environments. The entry used by the first
/// parse pass; [`lex_with`] adds user-defined verbatim commands. Uses the
/// [`Document`](LatexFlavor::Document) flavor (ordinary starting catcodes).
pub fn lex(input: &str) -> Vec<Token> {
    lex_with(input, &ParseCtx::default(), LexConfig::default())
}

/// Lex `input` like [`lex`], additionally treating the user-defined verbatim
/// commands in `ctx` as verbatim (their final argument captured as one `VERB`
/// token). Used by the second parse pass once definition scanning has discovered
/// catcode-othering commands. `config` fixes the initial catcode regime (a
/// [`Package`](LatexFlavor::Package) flavor starts with `@` already a letter) and
/// whether to run the `.dtx` docstrip mode.
pub fn lex_with(input: &str, ctx: &ParseCtx, config: LexConfig) -> Vec<Token> {
    Lexer::new(input, ctx, config, None).run()
}

/// Lex `input` like [`lex_with`], forcing the `.dtx` implicit-expl regime.
///
/// Only for incremental reparse tiers that relex a fragment under the base parse's
/// full-file lexer facts.
pub(crate) fn lex_with_implicit_expl(
    input: &str,
    ctx: &ParseCtx,
    config: LexConfig,
    implicit_expl: bool,
) -> Vec<Token> {
    Lexer::new(input, ctx, config, Some(implicit_expl)).run()
}

/// The lexer's one-shot lookahead mode: a state the token just lexed arms, which
/// changes how the *next* one reads. The four arming command sets are mutually
/// exclusive — `\left`/`\right`, the definition keywords, the char-constant
/// primitives, and the literal-token primitives are disjoint — so a single slot
/// holds them all faithfully, and a construct that consumes the awaited token
/// clears the slot wholesale rather than a hand-picked subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// After `\left`/`\right`: the delimiter that follows is isolated as a single
    /// token, so a word-character delimiter does not glue into the following run.
    Delim,
    /// After a definition keyword (`\newcommand\foo…`, `\NewDocumentCommand{\foo}…`,
    /// `\def\foo…`): the next control word is the *name being defined*, so it must
    /// not be lexed as a verbatim *call* — at a definition site the trailing `{…}`
    /// are the signature/body, not the command's argument. Without this, a command
    /// flagged verbatim in pass 1 would have its own definition's first group
    /// captured as a `VERB` in pass 2.
    Def,
    /// After a `\char`/`\catcode`-family primitive, where a backtick opens TeX's
    /// char-constant number notation: the character after the backtick is data
    /// (`` \char`$ ``, `` \char`} ``), never a math opener or group brace. The doc
    /// layer writes the notation in prose (issue #60), so without this the hidden
    /// `$`/`{` cascade into unclosed-math and unclosed-group diagnostics.
    CharConstant,
    /// After a primitive that consumes the *next token* unexpanded
    /// ([`is_literal_token_command`]), where a short-verb character is that token
    /// rather than a capture opener (`\string|`, lthooks.dtx, issue #71).
    LiteralToken,
}

/// The catcode state a `macrocode` chunk suspends. A body runs under
/// `\makeatletter` (and, in an implicit-expl3 `.dtx`, under `\ExplSyntaxOn`), and
/// its end frame restores what the documentation layer had. Held as one `Option`,
/// so "inside a body" and "what to restore" are the same fact and cannot disagree.
#[derive(Debug, Clone, Copy)]
struct MacrocodeSave {
    at_letter: bool,
    expl_syntax: bool,
}

/// The lexer's state machine over one input. [`run`](Lexer::run) is a short loop
/// over the `try_*` probes, each of which either consumes a whole construct
/// (returning `true`) or declines; whatever no probe claims is lexed as one
/// ordinary token by [`lex_token`](Lexer::lex_token).
struct Lexer<'a> {
    input: &'a str,
    ctx: &'a ParseCtx,
    config: LexConfig,
    /// Implicit expl3: a toggle-less `.dtx` whose static signal (a `%<@@=mod>`
    /// module guard or a `\ProvidesExpl*` anywhere) marks its `macrocode` bodies
    /// as expl3 code. When set, `expl_syntax` is forced on inside every macrocode
    /// body and restored on exit, alongside the `at_letter` save. Only `.dtx`
    /// files have macrocode bodies, so this is gated on `config.dtx`.
    implicit_expl: bool,
    out: Vec<Token>,
    pos: usize,
    /// `\makeatletter` state: while true, `@` is a catcode-11 letter.
    at_letter: bool,
    /// `\ExplSyntaxOn` state: while true, `_` and `:` are catcode-11 letters, so
    /// expl3 names (`\seq_new:N`, `\__module_internal:nn`) lex as single control
    /// words. Toggled by `\ExplSyntaxOn`/`\ExplSyntaxOff` and turned on by the
    /// `\ProvidesExpl*` package/class/file declarations (a sanctioned static lexer
    /// mode, `AGENTS.md` decision #1). Independent of `at_letter`; the two compose.
    expl_syntax: bool,
    /// True at the start of a physical line (start of input or just after a
    /// `NEWLINE`), so a line-leading `%` can be recognized as a `.dtx`
    /// documentation margin. Any token — including whitespace — clears it,
    /// matching docstrip's rule that only a `%` in *column 0* is a margin.
    at_line_start: bool,
    /// True while lexing the remainder of a `.dtx` documentation line (a line whose
    /// column-0 `%` was emitted as a `DOC_MARGIN`). On such lines the ltxdoc/l3doc
    /// `` \catcode`\^^A=14 `` convention applies, so a literal `^^A` reads as a
    /// comment to end of line. Cleared at every physical line boundary.
    in_doc_line: bool,
    /// doc-package short-verb characters (`\MakeShortVerb{\|}`): while a char is
    /// enabled, `<c>…<c>` on one line captures as a single opaque `VERB` token,
    /// exactly like `\verb<c>…<c>`. A sanctioned static lexer mode (`AGENTS.md`
    /// decision #1): the toggles are the explicit `\MakeShortVerb`/
    /// `\DeleteShortVerb` calls (left-to-right, like `\makeatletter`), plus the
    /// curated doc classes that enable `|` themselves ([`doc_class_enables_bar`]).
    /// The `.dtx` documentation layer gets `|` from the start — dtx files are
    /// typeset under `ltxdoc`, and the driver holding the `\documentclass` may
    /// live in a separate file.
    short_verbs: Vec<char>,
    /// `Some` while inside a `macrocode`/`macrocode*` environment body (between its
    /// frame lines), holding the catcode state to restore on exit. There, code
    /// lines carry no margin, a line-leading `%` is an ordinary code comment (not a
    /// margin), and `@` is a letter (`macrocode` runs under `\makeatletter`).
    macrocode: Option<MacrocodeSave>,
    /// The one-shot mode the previous token armed, if any.
    pending: Option<Pending>,
    /// Number of brace groups open at the cursor, counted over every token emitted
    /// so far (the `try_*` probes push braces of their own, so `out` is the one
    /// place that sees them all). Read by the char-constant probe: inside a group
    /// TeX has already claimed a `{`/`}` as balanced-text structure, so a backtick
    /// there cannot hide it. Saturating, so an unbalanced file never underflows.
    brace_depth: usize,
    /// How far into `out` [`sync_brace_depth`](Lexer::sync_brace_depth) has folded.
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

    /// Lex the whole input. The probe order is load-bearing — an earlier probe
    /// wins the bytes outright — so it mirrors the layering the modes assume: the
    /// `.dtx` line-oriented trivia first (only they may claim column 0), then the
    /// constructs that swallow a span whole (verbatim environments before verbatim
    /// commands, since `\begin` is itself a command), then the one-shot modes.
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
            // The control word's letter run is scanned once here and handed to both
            // the verbatim-command probe and the ordinary classification below,
            // which otherwise ask the same question about the same bytes twice.
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

    /// The unlexed remainder of the input.
    fn rest(&self) -> &'a str {
        &self.input[self.pos..]
    }

    fn push(&mut self, kind: SyntaxKind, text: &str) {
        self.out.push(Token {
            kind,
            text: SmolStr::new(text),
        });
    }

    /// Consume `len` bytes of a construct a `try_*` probe claimed whole: the cursor
    /// lands mid-line, and any armed one-shot mode is spent — the construct either
    /// *was* the token the mode was waiting for or is an ordinary token that ends
    /// the wait, and no mode outlives a construct it did not fire on.
    fn consume(&mut self, len: usize) {
        self.pos += len;
        self.at_line_start = false;
        self.pending = None;
    }

    /// Fold every token pushed since the last call into `brace_depth`.
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

    /// `.dtx` `macrocode` frame line. A `%␣*\begin{macrocode}` line opens a code
    /// region; its `%␣*\end{macrocode}` terminator closes it. Both lex as a
    /// margin + indent + `\begin`/`\end{macrocode}` so the ordinary environment
    /// grammar pairs them, but the *body* in between lexes as real code, under the
    /// package regime (`@` a letter) with no margin stripping. We look for a begin
    /// frame outside the body and the end frame inside it; anything else on a `%`
    /// line inside the body is an ordinary code comment.
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

    /// `.dtx` docstrip guard: a line-leading `%<…>` is a docstrip guard expression
    /// (`%<*tag>`/`%</tag>` block delimiters or an inline `%<tag>` prefix), not a
    /// comment. Emit the `%<…>` (through the closing `>`) as a single `GUARD`
    /// trivia leaf; code after an inline guard's `>` lexes normally. Guards nest on
    /// the docstrip axis, orthogonal to LaTeX nesting, so this is a flat floating
    /// leaf (no block node), like a margin. Recognized at line start only (column-0
    /// rule) but in *any* layer — guards punctuate `macrocode` bodies too — so it
    /// is not gated on being outside one. A `%<` with no closing `>` before the
    /// line ends is not a guard; it falls through to an ordinary comment. Trivia,
    /// so the [`Pending`] mode carries across.
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

    /// `.dtx` documentation margin: a line-leading `%` (but not a `%<…>` guard,
    /// which lexes as a `GUARD` above) is a documentation line's comment *margin*,
    /// not a comment. Emit it as a `DOC_MARGIN` trivia token — one byte, never the
    /// following space — so the rest of the line lexes (and parses) as ordinary
    /// LaTeX and the margin floats like whitespace. Only the line-leading `%` is a
    /// margin; a later `%` on the same line stays a `COMMENT`. Inside a `macrocode`
    /// body there is no margin (code lines own their `%`). The margin is trivia, so
    /// it carries the [`Pending`] mode across unchanged (like whitespace).
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

    /// Verbatim-like environment: emit `\begin{name}` then a raw body token.
    fn try_verbatim_environment(&mut self) -> bool {
        let (rest, ctx) = (self.rest(), self.ctx);
        let Some(consumed) = lex_verbatim_environment(rest, ctx, &mut self.out) else {
            return false;
        };
        self.consume(consumed);
        true
    }

    /// l3doc `v`-type name argument in delimited form (`\begin{macro}+…+`): capture
    /// the span as one opaque `VERB` token so its unbalanced braces stay data.
    /// Gated off inside a `macrocode` body, where a `\begin` is plain macro code,
    /// not an l3doc environment.
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

    /// Verbatim-argument command (`\url{…}`, `\code{…}`, `\lstinline|…|`, …): emit
    /// the control word and any leading args, then a raw argument token.
    /// `\verb`/`\verb*` are handled separately in [`lex_control`] (delimiter only),
    /// so they fall through here. Suppressed at a definition site ([`Pending::Def`]),
    /// where the following groups are the signature/body.
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

    /// Short-verb span (`|…|` under doc's `\MakeShortVerb{\|}`): capture the
    /// delimited run as one opaque `VERB` token, same-line only (like `\verb`).
    /// Gated off inside a `macrocode` body (a code layer, where `|` is an ordinary
    /// catcode-12 character) and after `\left`/`\right` (whose next character is a
    /// delimiter, `\left|x\right|`). With no closing delimiter on the line, decline:
    /// the word-run truncation in [`lex_token`](Lexer::lex_token) still emits the
    /// lone character as its own token. Also gated off after a primitive that takes
    /// the next token unexpanded ([`Pending::LiteralToken`]): `\string|` prints the
    /// bar, it does not open a capture that would run to the next `|` and swallow
    /// the intervening braces (lthooks.dtx's
    /// `\meta{first\texttt{\string|}last}\verb|):|`, issue #71).
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

    /// TeX char-constant backtick notation: after a `\char`/`\catcode`-family
    /// primitive ([`Pending::CharConstant`]), a backtick makes the next character
    /// data (`` \char`$ ``, `` \char`} ``), so emit the backtick and that character
    /// as one plain `WORD` token — a `$`/`{` there must not open math or a group.
    /// The escaped single-character form (`` \number`\[ ``) is captured the same
    /// way, backtick plus the whole control symbol: a `\[`/`\]` there is the
    /// *character* `[`/`]`, not a math delimiter (encguide.tex's char-code table,
    /// issue #71). The same reading is statically certain when the escaped form
    /// occupies a whole alignment cell (`` `\X& ``): the alignment template can
    /// supply that cell to `\char#`, as in TeX by Topic's character-code tables
    /// (issue #144). Requiring the immediate `&` keeps this local shape from
    /// claiming an ordinary backtick before live `\[…\]` math.
    ///
    /// A *bare* `{`/`}` is the exception, and only at brace depth 0. Inside a group
    /// the brace has already been claimed as structure by whichever balanced-text
    /// scan opened it — a `\def` body or a macro argument, both of which count brace
    /// *tokens* long before `\char` ever runs — so the `}` in `` \def\v{\char`} ``
    /// (longtable.dtx) and the `` \ifnum`}=0\fi `` brace-balance idiom
    /// (longtable/amsmath) closes its group and is not data. At depth 0 there is no
    /// such scan and the constant reading stands (`a close-group character is
    /// written \char`} in running text`). The *escaped* form `` `\} `` is
    /// unaffected: a control symbol is never a group delimiter, so it stays data at
    /// any depth (issue #71).
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
            // `` `\X ``: backtick, backslash, and one escaped character; a bare
            // `` `\ `` at line end has no character and falls through.
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

    /// `.dtx` `^^A` comment: ltxdoc/l3doc set `` \catcode`\^^A=14 ``, and the doc
    /// layer leans on it for editor-balance hacks in prose (`^^A{` paired with a
    /// verb `|}|`, a commented-out `^^A\end{function}`), so on a doc-margin line the
    /// literal `^^A` sequence is a comment to end of line — a bounded static fact
    /// like the on-by-default `|` short verb (`AGENTS.md` decision #1). Scoped to
    /// doc lines only: inside a `macrocode` body `^^A` is live code
    /// (``\char_set_catcode:nn { `\^^A }`` must not swallow its line), and
    /// unmargined driver lines keep ordinary lexing.
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

    /// Lex the one ordinary token at the cursor: classify it, apply the truncations
    /// an armed mode or an enabled short-verb character imposes, run whatever
    /// catcode toggle its text carries, and advance. `word_len` is the pre-scanned
    /// control-word length at the cursor ([`control_word_len`]).
    fn lex_token(&mut self, word_len: Option<usize>) {
        let rest = self.rest();
        let (kind, mut len) = next_token(rest, word_len, self.expl_syntax);
        // A `\left`/`\right` delimiter that lexes as a word run: keep only its
        // first character so it does not glue into the following text.
        if self.pending == Some(Pending::Delim) && kind == SyntaxKind::WORD {
            len = rest.chars().next().expect("rest is non-empty").len_utf8();
        }
        // An enabled short-verb char never joins a word run: split it off so a
        // mid-word `x|y|` still opens a capture on the next iteration, and an
        // unclosed `|` stands alone rather than gluing into the following text.
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
        // A new physical line begins right after a `NEWLINE` — or after any token
        // that swallows its trailing line break, like the `\<newline>` control
        // symbol (`… \LaTeX\` at end of line): the next byte is column 0 either
        // way, so a `.dtx` margin there must still be recognized. Any other token
        // (whitespace included) leaves the cursor mid-line.
        self.at_line_start =
            kind == SyntaxKind::NEWLINE || text.ends_with('\n') || text.ends_with('\r');
        if self.at_line_start {
            self.in_doc_line = false;
        }
        self.pos += len;
    }

    /// Apply the catcode / short-verb toggle a control word carries, if any.
    /// `after` is the text following it, from which the toggles that take a
    /// character or class argument read it.
    fn apply_toggles(&mut self, text: &str, after: &str) {
        match text {
            "\\makeatletter" => self.at_letter = true,
            "\\makeatother" => self.at_letter = false,
            // doc's short-verb toggles: `\MakeShortVerb{\|}` (or the `*` and
            // unbraced forms) enables the char, `\DeleteShortVerb{\|}` disables it.
            // Read as static facts left-to-right; a definition site
            // (`\def\MakeShortVerb{…`) never matches the `\c` argument shape, so it
            // does not toggle.
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
            // The curated doc classes make `|` a short verb themselves
            // ([`BAR_SHORT_VERB_CLASSES`]), so loading one enables `|`.
            "\\documentclass" | "\\LoadClass" => {
                if doc_class_enables_bar(after) && !self.short_verbs.contains(&'|') {
                    self.short_verbs.push('|');
                }
            }
            // `\ExplSyntaxOn`/`Off`, and the `\ProvidesExpl*` declarations which
            // open expl3 syntax for the rest of the file (they appear at the top of
            // an expl3 package/class) so left-to-right they act as an On.
            _ => {
                if let Some(toggle) = expl_toggle(text) {
                    self.expl_syntax = matches!(toggle, ExplToggle::On);
                }
            }
        }
    }
}

/// Whether a control word makes the lexer read the *raw text that follows it*,
/// beyond the ordinary token scan — so a later token's own text can decide how the
/// rest of the file lexes.
///
/// Two families, and between them this is the whole set. [`apply_toggles`] reads a
/// following argument for the short-verb and document-class toggles (the
/// `\makeatletter` and expl3 toggles read only the control word itself, so they are
/// not here). And [`next_pending`] arms the one-shot lookahead, which changes how
/// the *next* token lexes; asking it rather than restating its four sets is what
/// keeps this from drifting when a fifth is added.
///
/// Exists for [`crate::parser::reparse`]'s token tier, which may not splice a leaf
/// whose text one of these reads: the tier's soundness rests on the token *kind*
/// vector being unchanged, and these are the lexer's way of making one token's text
/// change another token's kind.
pub(crate) fn reads_following_text(text: &str) -> bool {
    matches!(
        text,
        "\\MakeShortVerb" | "\\DeleteShortVerb" | "\\documentclass" | "\\LoadClass"
    ) || next_pending(None, SyntaxKind::CONTROL_WORD, text).is_some()
}

/// The one-shot mode in force after lexing a token of `kind`/`text`: newly armed
/// by a command that takes one, carried across the trivia the awaited token may
/// sit behind, and otherwise spent.
///
/// Each variant carries across exactly the trivia TeX skips before *its* token.
/// Spaces always; a line break additionally for [`Pending::Delim`] (TeX scans for
/// the delimiter across lines) and for [`Pending::Def`], whose braced form
/// `\newcommand{\foo}` also interposes the `{`. A char constant and a
/// literal-token grab conventionally stay on their line.
fn next_pending(pending: Option<Pending>, kind: SyntaxKind, text: &str) -> Option<Pending> {
    if kind == SyntaxKind::CONTROL_WORD {
        // The four arming sets are disjoint, so the order of these tests is
        // immaterial; any other control word — the defined name itself included —
        // spends whatever was armed.
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

/// Byte length of the control word at the start of `rest` — the backslash plus its
/// maximal letter run — or `None` when `rest` does not start one (no backslash, or
/// no letter behind it). Scanned once per cursor position and threaded to every
/// consumer, since the letter run is the same bytes under the same catcode regime.
fn control_word_len(rest: &str, at_letter: bool, expl_syntax: bool) -> Option<usize> {
    let after = rest.strip_prefix('\\')?;
    let letters = run_len(after, |c| is_letter(c, at_letter, expl_syntax));
    (letters > 0).then_some(1 + letters)
}

/// Classify the token at the start of `rest` and return its `(kind, byte_len)`.
/// `word_len` is the pre-scanned [`control_word_len`] at `rest`.
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

/// Lex a control sequence: `rest` is known to start with `\`, and `word_len` is
/// its pre-scanned [`control_word_len`] — `Some` for a control word (backslash
/// plus one or more letters, `@` too under `\makeatletter`, `_`/`:` too under
/// `\ExplSyntaxOn`), `None` for a control symbol.
fn lex_control(rest: &str, word_len: Option<usize>) -> (SyntaxKind, usize) {
    match word_len {
        Some(word_len) => {
            // `\verb` / `\verb*`: swallow the delimited argument as one token.
            if &rest[..word_len] == "\\verb"
                && let Some(arg_len) = verb_len(&rest[word_len..])
            {
                return (SyntaxKind::VERB, word_len + arg_len);
            }
            (SyntaxKind::CONTROL_WORD, word_len)
        }
        // Control symbol: backslash + exactly one other character — or a lone
        // trailing backslash at end of input. CRLF is one physical line ending,
        // so consume it atomically just as the ordinary newline lexer does.
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

/// The character argument of `\MakeShortVerb`/`\DeleteShortVerb`, read from the
/// text following the control word: an optional `*`, inline whitespace, then
/// `{\c}` or a bare `\c`. Returns `None` when the shape does not match (e.g. at
/// the command's own definition site, `\def\MakeShortVerb{…`), so a non-call
/// never toggles. Same-line only — the argument conventionally abuts the call.
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

/// The documentation classes that make `|` a short verb themselves, so loading one
/// enables the short-verb capture with no `\MakeShortVerb` in the file. `ltxdoc`
/// and `l3doc` call `\MakeShortVerb` on `\|`; `ltxguide`, `ltnews`, and `amsldoc`
/// define the equivalent active `|` (`\gdef|{\protect\activevert{}}`, amsldoc.cls).
/// Curated and closed — a class outside it leaves `|` alone (issue #71).
const BAR_SHORT_VERB_CLASSES: [&str; 5] = ["ltxdoc", "ltxguide", "ltnews", "l3doc", "amsldoc"];

/// Whether the `{name}` argument following `\documentclass`/`\LoadClass` names one
/// of [`BAR_SHORT_VERB_CLASSES`]. A leading `[options]` group is skipped; a
/// trailing `[date]` is ignored.
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
fn lex_verbatim_environment(rest: &str, ctx: &ParseCtx, out: &mut Vec<Token>) -> Option<usize> {
    let (name, prefix_len) = begin_name(rest)?;
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

    push_env_delimiter(out, "\\begin", name);

    // Locate the argument span, then tokenize it normally. It holds no nested
    // verbatim-begin, so the ordinary token loop is safe and lets the parser build
    // the usual OPTIONAL/GROUP argument nodes.
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

/// Byte offset within `body` of the `\end{name}` that terminates it, or `body`'s
/// full length when the environment is never closed (the raw body then runs to end
/// of input, which keeps the lex lossless either way).
///
/// Matched by scanning for the fixed `\end{` lead and comparing the name in place,
/// rather than searching for a per-environment `\end{name}` string — the latter
/// allocates once per verbatim environment in the file for a comparison the borrow
/// already supports.
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

/// If `rest` starts with `\begin{name}` for an environment whose name argument is
/// xparse `v`-type (`verbatim_arg` in the curated DB: l3doc's `macro`/`function`/
/// `variable`, declared `{ O{} +v }`), emit the `\begin{name}` tokens, a leading
/// `[…]` optional as ordinary tokens, and the name argument as one opaque `VERB`
/// token, returning the bytes consumed. Both argument forms capture:
/// - The *delimited* form (`\begin{macro}+\@@_compile_{:+`) captures the whole
///   delimited span as the `VERB`. Upstream chooses this form precisely when the
///   name holds unbalanced braces (`\@@_compile_}:`), which would otherwise
///   corrupt group pairing for the rest of the file. The delimiter must directly
///   abut and be punctuation that cannot open another argument shape (never `\`,
///   a brace or bracket, `%`, `*`, or `$`), so an ordinary `\begin{macro}`
///   followed by prose or code never captures.
/// - The *braced* form (`\begin{macro}{\]}`) keeps its `{`/`}` as ordinary brace
///   tokens (the parser still builds the usual name `GROUP`) with the balanced
///   content between them as the `VERB`: the content is raw data, so a `\]`,
///   `\(`, or `$` in a name never opens math or draws an orphan-closer
///   diagnostic (issue #60). Balance tracking skips escaped braces (`\{`, `\}`
///   are part of a name, not group delimiters).
///
/// Same-line only, like `\verb`, in both forms. The parser attaches the abutting
/// `VERB` or name group into the `BEGIN` node like any verbatim command argument
/// (`attach_arguments`).
fn lex_verbatim_arg_environment(rest: &str, out: &mut Vec<Token>) -> Option<usize> {
    let (name, prefix_len) = begin_name(rest)?;
    builtin().environment(name).filter(|e| e.verbatim_arg)?;

    // A leading `[…]` optional (the `O{}` slot, `\begin{macro}[EXP]+…+`) is
    // structured, not verbatim; it lexes normally below. Same-line, unnested.
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
        // Braced form: `{` VERB(content) `}` — the braces stay real tokens so
        // the parser builds the ordinary name `GROUP`.
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

/// Length of the brace-balanced content of a braced `v`-type name argument,
/// starting just past the opening `{`. Same-line only; escaped braces (`\{`,
/// `\}`) are name characters, not delimiters. `None` when the closing `}` is
/// not on the line (falls back to normal lexing) or the content is empty
/// (nothing to capture; a bare `{}` lexes normally).
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
/// `\begin{macrocode}{x}` is not mistaken for a frame. The *end* frame also
/// tolerates a trailing `%` comment (`%    \end{macrocode}%`, a guard against a
/// stray trailing space): doc.sty's terminator is a delimited match on the
/// `%    \end{macrocode}` string, so anything after it on the line is doc-layer
/// material. A begin frame stays strict — same-line text there is captured into
/// the body by `\xmacro@code`, not doc prose.
///
/// A *begin* frame additionally tolerates indentation before the `%`. In the
/// documentation layer `\DocInput` runs under `\MakePercentIgnore`
/// (`` \catcode`\%=9 ``, doc.dtx), so a `%` there is an *ignored* character at any
/// column and `␣*%␣*\begin{macrocode}` opens a chunk exactly like the column-0
/// spelling (multicol.dtx, latex-lab-block.dtx — issue #71). The indent rides as a
/// `WHITESPACE` token before the margin, so the line stays lossless and the
/// formatter re-pins the frame at column 0. The *end* frame stays column-0 strict:
/// inside the body `%` is a comment again, and doc.sty terminates on a delimited
/// match against the literal `%    \end{macrocode}` line.
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
    // The frame line carries nothing but trailing whitespace after `}` — plus,
    // on an end frame, an optional `%` comment tail (lexed as an ordinary
    // `COMMENT` by the main loop).
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

/// If `rest` starts with a verbatim-argument command (`\url`, `\href`,
/// `\lstinline`, …), emit its control word, any leading ordinary arguments, and
/// one raw [`SyntaxKind::VERB`] token; return the bytes consumed. A whole-command
/// capture treats the raw argument as an implicit final slot, while a positional
/// capture stops at its marked slot and leaves later arguments for the ordinary
/// lexer. Returns `None` when no complete raw argument follows.
///
/// The verbatim argument's form is decided by its first non-blank character,
/// matching how these commands actually parse: a brace introduces a balanced
/// `{…}` group (`\code{…}`, `\url{…}`); any other character is a `\verb`-style
/// delimiter run (`\lstinline|…|`), but only for built-ins whose signature
/// grants the delimiter form (`verbatim_delimited`). For braced-only commands —
/// `\code`, `\path`, and every scanner-discovered user command — a non-brace
/// follower means this occurrence is not a verbatim argument (the name may be an
/// unrelated user macro: `\code` as a math operator, TikZ's `\path (0,0)`), so
/// we return `None` and lex normally; a missed capture is benign where a wrong
/// delimiter capture swallows text across the line. `\verb`/`\verb*` are
/// deliberately excluded — they are delimiter-only and handled in
/// [`lex_control`]. Like the verbatim environment path, this reads only static
/// signature data (decision #1).
///
/// `word_len` is the pre-scanned [`control_word_len`] at `rest`, so the command's
/// letter run is not re-scanned here and again when the caller falls through to
/// ordinary lexing.
fn lex_verbatim_command(
    rest: &str,
    word_len: Option<usize>,
    ctx: &ParseCtx,
    on_dtx_doc_line: bool,
    out: &mut Vec<Token>,
) -> Option<usize> {
    let word_len = word_len?;
    let name = &rest[1..word_len];
    // `\verb` keeps its dedicated delimiter-only path.
    if name == "verb" {
        return None;
    }
    // A user-defined catcode-verbatim command (from `ctx`) wins over the built-in DB.
    // Otherwise only the curated tier may establish either the legacy implicit-final
    // capture or a positional raw slot. Discovered commands are `\newcommand`-style
    // braced definitions, so they never get the delimiter form.
    let (leading, delimited): (&[ArgSpec], bool) = match ctx.leading_args(name) {
        Some(args) => (args, false),
        None => {
            // A visible non-verbatim redefinition in this file shadows the built-in, so
            // don't capture — lex the braced argument as an ordinary group (issue #53).
            if ctx.is_suppressed(name) {
                return None;
            }
            let sig = builtin().command(name)?;
            if sig.verbatim {
                (&sig.args, sig.verbatim_delimited)
            } else {
                let raw = sig.args.iter().position(|arg| arg.verbatim)?;
                // Positional raw arguments are brace-delimited. Delimiter runs remain
                // the legacy implicit-final command facet because they have no GROUP
                // slot in the CST or signature model.
                if sig.args[raw].kind != ArgKind::Brace {
                    return None;
                }
                (&sig.args[..raw], false)
            }
        }
    };

    // Leading arguments precede the verbatim one (e.g. `\mintinline{lang}{code}`).
    let after_word = &rest[word_len..];
    let args_len = scan_verbatim_args(after_word, leading);

    // A braced-only argument is an ordinary TeX argument and may begin on the
    // next line. Delimiter-style verbatim remains same-line: its closing delimiter
    // cannot cross a line break.
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
        // A `\verb`-style delimiter run: the first character delimits, and the
        // argument may not span a line break.
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
        let probe = pos + inline_ws_len(&region[pos..]);
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

/// The environment name of a `\begin{name}` at the start of `rest`, together with
/// the byte length of the whole `\begin{name}` prefix. `None` when `rest` does not
/// open one, or when the name group never closes.
fn begin_name(rest: &str) -> Option<(&str, usize)> {
    let after = rest.strip_prefix("\\begin{")?;
    let close = after.find('}')?;
    Some((&after[..close], "\\begin{".len() + close + 1))
}

/// Emit the four tokens of an environment delimiter — `\begin`/`\end`, `{`, the
/// name, `}` — so the ordinary environment grammar sees the shape it expects even
/// where the lexer claimed the surrounding line itself (a verbatim `\begin`, a
/// `.dtx` `macrocode` frame).
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

/// Number of leading bytes of `s` that are inline whitespace — spaces and tabs,
/// never a line break. An argument never crosses a newline, and a `.dtx` frame
/// line's indent is likewise same-line, so every scan in this module that steps
/// over blanks means exactly this.
fn inline_ws_len(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b' ' || b == b'\t').count()
}

/// Number of leading ASCII whitespace bytes TeX may skip before a braced argument.
fn tex_whitespace_len(s: &str) -> usize {
    s.bytes()
        .take_while(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
        .count()
}

/// A verbatim command on a `.dtx` documentation line may take its braced argument
/// on the next margined line. Return the gap through that line's indentation.
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

/// `s` past its leading [`inline_ws_len`].
fn skip_inline_ws(s: &str) -> &str {
    &s[inline_ws_len(s)..]
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

/// Could `name` (without its leading `\`) lex as a single [control
/// word](control_word_len) in *some* catcode regime?
///
/// The most permissive regime is the bar on purpose: a name is checked here
/// against `\makeatletter` *and* `\ExplSyntaxOn` letters at once, because a
/// declaration does not say which file it will be read in. Shares
/// [`is_letter`] with the lexer rather than restating the letter set, for the
/// same reason the expl3 toggle names are one set: a name the lexer would split
/// into two tokens can never match a declaration, so accepting one would be a
/// silent no-op (see [`crate::declarations`]).
pub fn is_control_word_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| is_letter(c, true, true))
}

/// Ordinary text: anything that is not whitespace, a line break, or one of the
/// characters the lexer treats specially.
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

    /// The lexer is total and lossless: concatenated token text == input.
    fn assert_lossless(input: &str) {
        let joined: String = lex(input).iter().map(|t| t.text.as_str()).collect();
        assert_eq!(joined, input);
    }

    #[test]
    fn the_pending_arming_sets_are_disjoint() {
        // `Pending` is one slot, which is faithful only because no control word
        // arms two modes. `next_pending` tests the four in a fixed order, so an
        // overlap would silently make one set shadow the other rather than fail
        // to compile. Every name currently in any set is checked here; a name
        // added to a *second* set trips this.
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
        // Every construct a `try_*` probe claims whole clears the one-shot slot,
        // so an armed mode never survives an unrelated capture. Directly after
        // `\char` a backtick still opens the char constant…
        let direct = lex("\\char `\\%");
        assert!(
            direct
                .iter()
                .any(|t| t.kind == SyntaxKind::WORD && t.text == "`\\%")
        );
        // …but with a short-verb capture in between, the mode is spent and the
        // backtick is ordinary text. (Before the four flags collapsed into one
        // slot this branch cleared a hand-picked subset that left the
        // char-constant mode armed indefinitely.)
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

    /// Lex `input` under the docstrip (`.dtx`) config, the regime in which
    /// implicit expl3 applies.
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
        // A toggle-less `.dtx` with only a `%<@@=mod>` module guard: its macrocode
        // body is expl3 code, so `\seq_new:N` lexes as one control word.
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
        // The same shape without a signal: `.dtx` macrocode is plain code, so
        // `\seq_new:N` stops at the first `_` (the feature is opt-in).
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
        // `\ProvidesExplPackage` is a whole-file signal, so a macrocode body
        // *above* the declaration is expl3 too — the property left-to-right
        // toggling misses.
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
        // Implicit expl3 is forced inside the macrocode body and restored on exit,
        // so the doc-margin line between/around bodies is ordinary LaTeX: `a_b`
        // joins in the body but splits on the doc line.
        let toks = lex_dtx(
            "%<@@=mod>\n\
             % a_b\n\
             %    \\begin{macrocode}\n\
             c_d\n\
             %    \\end{macrocode}\n",
        );
        let seen: Vec<_> = toks.iter().map(|t| (t.kind, t.text.as_str())).collect();
        // Body: `_` is a letter, one word.
        assert!(seen.contains(&(SyntaxKind::WORD, "c_d")));
        // Doc layer: `_` stays a subscript, so `a_b` splits.
        assert!(seen.iter().any(|(k, _)| *k == SyntaxKind::UNDERSCORE));
    }

    #[test]
    fn implicit_expl_explicit_off_wins_then_next_body_re_enters() {
        // An explicit `\ExplSyntaxOff` inside an implicit body turns expl off for
        // the rest of that body; the next body still re-enters expl (the
        // save/restore restores the pre-body state, not the toggled-off one).
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
        // After the explicit off, `a_b` splits.
        assert!(seen.iter().any(|(k, _)| *k == SyntaxKind::UNDERSCORE));
        // The second body re-enters expl despite the earlier off.
        assert!(seen.contains(&(SyntaxKind::CONTROL_WORD, "\\tl_set:Nn")));
    }

    #[test]
    fn implicit_expl_gated_off_outside_dtx() {
        // The signal only fires under `.dtx` mode: a `.sty` with the same bytes
        // must not enable implicit expl (there are no macrocode bodies anyway).
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
        // A `.sty`/`.cls` is loaded under an implicit `\makeatletter`, so `@` is a
        // letter from the first byte — `\foo@bar` is one control word with no
        // explicit `\makeatletter`.
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
        // Letter-mode starts on, but an explicit `\makeatother` still turns it off.
        let toks = lex_with(
            r"\foo@bar\makeatother\foo@bar",
            &ParseCtx::default(),
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
        let toks = lex_with("% \\foo\nbar % tail\n", &ParseCtx::default(), dtx);
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
        let block = lex_with("%<*driver>\n%</driver>\n", &ParseCtx::default(), dtx);
        assert_eq!(block[0].kind, SyntaxKind::GUARD);
        assert_eq!(block[0].text, "%<*driver>");
        assert!(
            block
                .iter()
                .any(|t| t.kind == SyntaxKind::GUARD && t.text == "%</driver>")
        );
        // An inline `%<tag>` is a `GUARD` prefix; the rest of the line lexes as code.
        let inline = lex_with("%<plain>\\RequirePackage{x}\n", &ParseCtx::default(), dtx);
        assert_eq!(inline[0].kind, SyntaxKind::GUARD);
        assert_eq!(inline[0].text, "%<plain>");
        assert!(
            inline
                .iter()
                .any(|t| t.kind == SyntaxKind::CONTROL_WORD && t.text == "\\RequirePackage")
        );
        // A boolean tag expression stays one token (through the closing `>`).
        let expr = lex_with("%<*package|driver>\n", &ParseCtx::default(), dtx);
        assert_eq!(expr[0].kind, SyntaxKind::GUARD);
        assert_eq!(expr[0].text, "%<*package|driver>");
        // A guard recognized only at column 0: a mid-line `%<…>` stays a comment.
        let midline = lex_with("a %<x>\n", &ParseCtx::default(), dtx);
        assert!(
            midline
                .iter()
                .any(|t| t.kind == SyntaxKind::COMMENT && t.text == "%<x>")
        );
        assert!(!midline.iter().any(|t| t.kind == SyntaxKind::GUARD));
        // A `%<` with no closing `>` before the line ends is not a guard.
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

    #[test]
    fn make_short_verb_toggles_pipe_capture() {
        // Before the toggle a `|…|` is ordinary text; after `\MakeShortVerb{\|}`
        // it captures as one opaque `VERB`; `\DeleteShortVerb{\|}` turns it off.
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
        // The curated doc classes (`ltxdoc`, `ltxguide`, `ltnews`, `l3doc`,
        // `amsldoc`) make `|` a short verb themselves, so loading one enables the
        // capture — options and trailing release dates included. `amsldoc` does it
        // with an active `|` (`\\gdef|{\\protect\\activevert{}}`, amsldoc.cls),
        // like `ltxguide`/`ltnews`; without it amsldoc.tex's `|\\begin{alignat}|`
        // prose read as real structure (issue #71).
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
        // An unrelated class leaves `|` alone.
        let toks = lex("\\documentclass{article}\n|x| done");
        assert!(!toks.iter().any(|t| t.kind == SyntaxKind::VERB));
    }

    #[test]
    fn short_verb_never_captures_a_left_right_delimiter() {
        // `\left|x\right|` in math: the bars are delimiters, not a verb span.
        let toks = lex("\\MakeShortVerb{\\|} $\\left|x\\right|$");
        assert!(!toks.iter().any(|t| t.kind == SyntaxKind::VERB));
        assert_lossless("\\MakeShortVerb{\\|} $\\left|x\\right|$");
    }

    #[test]
    fn unclosed_short_verb_char_stands_alone() {
        // With no closing partner on the line, the enabled char is a lone
        // one-character word (never gluing into the following text).
        let toks = lex("\\MakeShortVerb{\\|} a|b\nc");
        assert!(!toks.iter().any(|t| t.kind == SyntaxKind::VERB));
        assert!(
            toks.iter()
                .any(|t| t.kind == SyntaxKind::WORD && t.text == "|")
        );
        assert_lossless("\\MakeShortVerb{\\|} a|b\nc");
    }

    /// A raw capture's *content* changes nothing about how the rest of the file
    /// lexes.
    ///
    /// This is a lexer property stated as one, but the reason it is pinned lives in
    /// `parser::reparse::protected`: that tier splices a new body into an existing
    /// tree without re-lexing anything after it, which is sound only because the
    /// lexer leaves a raw capture in the state it entered. Structurally it holds
    /// because `lex_verbatim_environment` / `lex_verbatim_command` /
    /// [`Lexer::try_short_verb`] push straight to `out`, so the captured bytes never
    /// reach [`Lexer::apply_toggles`], [`next_pending`], or
    /// [`Lexer::sync_brace_depth`] — but that is an argument about code, and this is
    /// the test that would notice it stop being true.
    ///
    /// The suffix is chosen to be sensitive to every state variable the lexer
    /// carries: `@` in a control word (`at_letter`), `_`/`:` (`expl_syntax`), a `|`
    /// (`short_verbs`), a `` ` `` after a `\char` (`brace_depth`), and a `\left`
    /// delimiter (`pending`).
    #[test]
    fn raw_capture_content_does_not_change_later_lexing() {
        /// Bodies that stay captured. Each would toggle a lexer mode or open a
        /// group if it were read as code rather than swallowed as data.
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
        /// The same, restricted to what every inline form can hold: no newline, no
        /// `+` (the delimiter), and braces balanced (`\url`'s scan needs them).
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

                // The premise: the region really did capture. A body that *breaks*
                // its capture is a different case — see the test below.
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

    /// The other half, and the reason the reparse tier re-lexes a whole fragment
    /// rather than trusting the body alone: a body that *breaks* its capture does
    /// change how the rest of the file lexes.
    ///
    /// `\url{{}` leaves `braced_verb_content_len` unbalanced, so no `VERB` forms and
    /// the braces are ordinary structure — which ratchets `brace_depth` and flips
    /// the char-constant reading of a later `` \char`{ ``. Nothing about the body's
    /// own bytes says that; only re-lexing the construct does.
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
