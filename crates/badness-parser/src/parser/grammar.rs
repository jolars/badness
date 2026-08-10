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

use crate::parser::conditional;
use crate::parser::core::SyntaxError;
use crate::parser::events::Event;
use crate::parser::lexer::{
    ExplToggle, Token, VerbCtx, expl_toggle, is_block_environment, is_math_environment,
};
use crate::syntax::SyntaxKind;

const BEGIN_CMD: &str = "\\begin";
const END_CMD: &str = "\\end";
const LEFT_CMD: &str = "\\left";
const RIGHT_CMD: &str = "\\right";

/// How [`Parser::attach_arguments`] treats a trailing `[…]` (issue #43).
/// `[`/`]` are not real grouping in TeX, so bracket attachment is a heuristic;
/// the policy is the caller's shape knowledge about the construct being
/// attached to. The in-math gates apply on top of it — see
/// [`Parser::attach_arguments`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum BracketPolicy {
    /// Attach across intervening trivia (decision #8's default).
    Greedy,
    /// Attach only a directly-abutting `[` (a curated math environment's
    /// `\begin`: its math body starts right after, so a detached `[` is
    /// content).
    Tight,
    /// Never attach one (the delimiter-size commands: their `[` is the
    /// delimiter being sized).
    Forbid,
}

/// The delimiter-size commands (`\big`…`\Bigg` and their `l`/`m`/`r` variants).
/// A closed, curated set of TeX/amsmath primitives whose sole "argument" is the
/// delimiter token that follows (`\Big[`, `\bigl(`, `\Bigg|`), so a `[…]` after
/// one is never an optional argument (issue #43). The static-fact posture
/// mirrors `\left`/`\right` (`AGENTS.md`, decision #1).
fn is_big_delimiter_command(text: &str) -> bool {
    let Some(name) = text.strip_prefix('\\') else {
        return false;
    };
    ["bigg", "Bigg", "big", "Big"].iter().any(|s| {
        name.strip_prefix(s)
            .is_some_and(|rest| matches!(rest, "" | "l" | "m" | "r"))
    })
}

/// The *definition-body* commands: commands whose trailing brace groups are
/// macro-code bodies, where TeX does not require `\begin`/`\end` to balance
/// within an individual group. Three families:
///
/// - The environment-definition commands (the LaTeX2e `\newenvironment` family
///   and the xparse `\NewDocumentEnvironment` family): the `\begin` lives in
///   the begin-code and its matching `\end` in the end-code by design
///   (`\newenvironment{wrap}{\begin{center}}{\end{center}}`, issue #45).
/// - The command-definition commands (the LaTeX2e `\newcommand` family and the
///   xparse `\NewDocumentCommand` family): a body may open or close an
///   environment for a matching hook to balance
///   (`\newcommand{\@@newpage}{\end{page}\begin{page}}`, issue #55).
/// - The LaTeX2e document/package hooks (`\AtBeginDocument` family): the code
///   argument runs at a different point in the document, so it balances
///   against that context, not within its own group
///   (`\AtBeginDocument{\begin{page}}` … `\AtEndDocument{\end{page}}`).
///
/// Inside those bodies `\begin`/`\end` parse as plain commands (see
/// [`Parser::in_def_body`]). A closed, curated set read as a static fact — the
/// bodies are never executed, mirroring [`is_big_delimiter_command`].
fn is_definition_body_command(text: &str) -> bool {
    matches!(
        text,
        "\\newenvironment"
            | "\\renewenvironment"
            | "\\provideenvironment"
            | "\\NewDocumentEnvironment"
            | "\\RenewDocumentEnvironment"
            | "\\ProvideDocumentEnvironment"
            | "\\DeclareDocumentEnvironment"
            | "\\newcommand"
            | "\\renewcommand"
            | "\\providecommand"
            | "\\DeclareRobustCommand"
            | "\\NewDocumentCommand"
            | "\\RenewDocumentCommand"
            | "\\ProvideDocumentCommand"
            | "\\DeclareDocumentCommand"
            | "\\AtBeginDocument"
            | "\\AtEndDocument"
            | "\\AtEndOfClass"
            | "\\AtEndOfPackage"
            | "\\AddToHook"
    )
}

/// The TeX `\def`-family primitives, whose next token is always the control
/// sequence being (re)defined. A control-*symbol* name would otherwise be
/// misparsed as live syntax — `\def\[{…}`/`\def\]{…}` (a document class
/// restyling display math, stacks-project issue #65) reads as a math opener,
/// `\def\\{…}` as a line break — so [`Parser::command`] consumes it as a plain
/// token inside the `\def`'s node. A control-*word* name already parses
/// benignly as a generic command and keeps its current shape. A closed,
/// curated set read as a static fact, mirroring [`is_definition_body_command`];
/// the definition is never executed.
///
/// Also read by the formatter's expl3 region gate (in the `badness-formatter` crate): a toggle
/// spelling immediately preceded by one of these is a *definee*, never an executed
/// catcode switch, so it must not open a formatter-owned region.
pub fn is_def_prefix_command(text: &str) -> bool {
    matches!(text, "\\def" | "\\gdef" | "\\edef" | "\\xdef")
}

/// Maximum number of consecutive cursor peeks with **no** token consumed before
/// the parser aborts as stuck. Modeled on rust-analyzer's `PARSER_STEP_LIMIT`
/// (`crates/parser/src/parser.rs`), a catch-all against a non-advancing loop that
/// holds *independent of grammar correctness*. The counter resets on every cursor
/// advance (see [`Parser::step`]), so in normal parsing only O(1) peeks accrue
/// between two consumed tokens; this ceiling is astronomically above any real
/// document and can only be reached by a genuine infinite loop. Unlike the
/// module's structural "`pos` only advances through `bump`" argument, this holds
/// even for malformed or adversarial input (fuzzing, a corrupt corpus file).
const PARSER_STEP_LIMIT: u32 = 15_000_000;

/// A content region that groups its children into `PARAGRAPH` nodes separated
/// by blank lines. Differs only in how the region terminates.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    /// The whole document; ends at EOF.
    Document,
    /// An environment body; ends at the next `\end` (any name — the caller
    /// checks the name and decides whether to consume it).
    Environment,
    /// A `.dtx` `macrocode` body: macro code, so a bare `\end` in the code is a
    /// plain command, and the block ends *positionally* at the pre-scanned frame
    /// terminator ([`Parser::macrocode_end`]), never at an arbitrary `\end`.
    Macrocode,
}

/// How the shared trivia scanner ([`Parser::scan_trivia`]) treats a `%` comment.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CommentMode {
    /// A comment is content that occupies its own line: it resets the newline
    /// run (without undoing a blank line already seen) and the scan continues
    /// past it. Used everywhere paragraph structure and leading-comment binds are
    /// decided.
    Skip,
    /// A comment stops the scan and is reported as the next meaningful token.
    /// Used by [`Parser::at_script`], where a comment ends the line so a `^`/`_`
    /// script never binds across it.
    Stop,
}

/// The result of scanning the contiguous trivia run at a position: everything the
/// blank-line and comment-bind rules (AGENTS.md #9) need to decide, computed once.
struct TriviaScan {
    /// Index of the next meaningful (non-skipped) token, or `tokens.len()` at EOF.
    next: usize,
    /// Kind of the token at [`Self::next`], or `None` at EOF.
    next_kind: Option<SyntaxKind>,
    /// The run before `next` contains a blank line (≥2 `NEWLINE`s: the `\par`
    /// boundary).
    saw_blank_line: bool,
    /// As [`Self::saw_blank_line`], but a `.dtx` docstrip guard line counts as
    /// content rather than blank space. Docstrip *deletes* a guard-only line
    /// when it strips the file, so `%<*dtx>` between two lines does not part
    /// them — read by the shape gates that only ask whether their construct's
    /// source ran out mid-shape (issue #71).
    saw_blank_line_outside_guards: bool,
    /// Start index of the leading own-line `%` comment run immediately preceding
    /// `next` — the maximal blank-line-free suffix, the start of a
    /// leading-comment bind. `None` if that suffix has no own-line comment.
    /// Only populated in [`CommentMode::Skip`].
    comment_start: Option<usize>,
}

/// Parse a token stream into parser events and a list of syntax errors.
pub(crate) fn parse(tokens: &[Token], ctx: &VerbCtx) -> (Vec<Event>, Vec<SyntaxError>) {
    let mut p = Parser::new(tokens, ctx);
    p.document();
    debug_assert_balanced(&p.events);
    (p.events, p.errors)
}

/// Debug-only structural tripwire: the event stream must be balanced — every
/// `Start` matched by a later `Finish`, no `Finish` before its `Start`, and the
/// document node closed exactly once. This is the cheap analog of
/// rust-analyzer's per-`Marker` `DropBomb`: a grammar edit that leaks an
/// [`Parser::open`] without a [`Parser::close`] (or a `precede`-style
/// `events.insert` that splices an unbalanced `Start`) is caught right here,
/// counting *all* start/finish events regardless of how they were emitted,
/// before [`super::tree_builder`] feeds rowan's `GreenNodeBuilder` and fails with
/// a far more opaque `finish_node` panic. Compiled out of release builds.
fn debug_assert_balanced(events: &[Event]) {
    if !cfg!(debug_assertions) {
        return;
    }
    let mut depth: i32 = 0;
    for ev in events {
        match ev {
            Event::Start(_) => depth += 1,
            Event::Finish => {
                depth -= 1;
                debug_assert!(depth >= 0, "parser emitted a Finish with no open node");
            }
            Event::Tok(_) | Event::SubTok { .. } => {}
        }
    }
    debug_assert_eq!(
        depth, 0,
        "parser left {depth} node(s) unclosed at end of parse"
    );
}

struct Parser<'t> {
    tokens: &'t [Token],
    /// User-defined verbatim constructs, consulted to route a verbatim environment to
    /// its raw-body branch (its body is already one `VERBATIM_BODY` token from the
    /// lexer; the grammar must not try to parse it structurally).
    ctx: &'t VerbCtx,
    /// `starts[i]` is the byte offset of token `i`; `starts[len]` is the total
    /// length. Used to give syntax errors byte ranges.
    starts: Vec<usize>,
    pos: usize,
    events: Vec<Event>,
    errors: Vec<SyntaxError>,
    /// Consecutive-peek budget for the stuck-loop guard ([`Self::step`]).
    /// `Cell` because the lookahead primitives that tick it are `&self`.
    steps: std::cell::Cell<u32>,
    /// The cursor position at the last [`Self::step`] tick; the budget resets
    /// whenever `pos` has advanced past it (i.e. real progress was made).
    last_step_pos: std::cell::Cell<usize>,
    /// Depth of lexically enclosing math bodies (`$…$`, `\[…\]`, `\(…\)`, math
    /// environments). Unlike the `math` routing flags threaded through the
    /// grammar, this *persists* into the text-mode body of an unknown
    /// environment nested inside math (`\[ … \begin{myaligned} … \]`): the
    /// grammar can't verify such a body is math, but the enclosing delimiters
    /// are a static lexical fact, and optional-argument attachment uses it to
    /// treat a spaced `[` as content (see [`Self::attach_arguments`]).
    math_depth: usize,
    /// Per-level flavor of the enclosing math bodies tracked by `math_depth`:
    /// `true` for a `$…$`/`$$…$$` (dollar-delimited) level, `false` for
    /// `\[…\]`/`\(…\)` and math environments. Pushed and popped in lockstep with
    /// `math_depth`, so its last entry is the *innermost* enclosing math's
    /// flavor. [`Self::bracket_closes_before_math_end`] reads it: inside dollar
    /// math a `$` is the closer (a boundary), whereas inside `\[…\]` a `$` opens
    /// a genuine nested inline region (`\inferrule*[right=$\Pi$-eq]`), so the two
    /// must be scanned differently.
    math_dollar: Vec<bool>,
    /// True while parsing the attached arguments of a definition-body command
    /// ([`is_definition_body_command`], issues #45/#55). Those groups are
    /// macro-code definition bodies that need not self-balance
    /// `\begin`/`\end`, so while set, `\begin`/`\end` parse as plain commands
    /// ([`Self::element`], [`Self::math_atom`]) and stop being bail anchors for
    /// an optional argument ([`Self::optional`]). Saved and restored around
    /// [`Self::attach_arguments`] in [`Self::command`], so it covers the whole
    /// definition subtree (nested groups included) and nothing after it.
    in_def_body: bool,
    /// Number of brace groups currently open around the cursor
    /// ([`Self::group`] / [`Self::math_group`]). A `\end` reached inside one
    /// has its `\begin` outside it, so it is macro code rather than a stray
    /// (`\StopEventually{\end{document}}`, issue #71) — the `\end`-side twin of
    /// [`Self::environment_escapes_group`].
    group_depth: usize,
    /// Environment names whose `\begin` the brace-group gate demoted to a plain
    /// command ([`Self::environment_escapes_group`]). Their `\end` is then an
    /// orphan by construction — the gate removed its partner, not the author — so
    /// [`Self::end_orphans_a_demoted_begin`] demotes it in the same way instead of
    /// letting it unwind (and falsely un-close) every enclosing environment.
    demoted_envs: std::collections::HashSet<String>,
    /// Names of the environments open around the cursor, outermost first. Read
    /// only by [`Self::end_orphans_a_demoted_begin`], to tell an `\end` that
    /// really does close something from one whose `\begin` was demoted.
    open_envs: Vec<String>,
    /// Token index of the `{` opening each currently-open brace group, innermost
    /// last (the positional twin of [`Self::group_depth`]). Read by
    /// [`Self::environment_escapes_group`] to tell a group the `.dtx`
    /// *documentation* layer opened itself from one stranded by the code layer.
    group_opens: Vec<usize>,
    /// Inside a `.dtx` `macrocode` body: the token index of the terminating
    /// frame `\end` (or `tokens.len()` when the frame is missing), pre-scanned
    /// by [`Self::macrocode_body`]. `None` outside a macrocode body. The frame
    /// is the *only* terminator of the chunk (docstrip is line-oriented), so
    /// [`Self::at_block_end`] and the bracket/optional guards read it to keep
    /// any construct from consuming past the frame.
    macrocode_end: Option<usize>,
    /// Brace tokens inside the current `macrocode` body with no match within
    /// the chunk. A `macrocode` chunk is macro code: a definition regularly
    /// opens a `{` in one chunk and closes it in a later one (`\def\foo#1{%` …
    /// frame … `bar}`), so an unmatched brace is an ordinary token — no
    /// `GROUP`, no unclosed/unmatched diagnostic. Matched pairs still parse as
    /// groups. Computed per chunk by [`Self::macrocode_body`].
    plain_braces: std::collections::HashSet<usize>,
    /// expl3 catcode-mode toggle tokens, ascending: `(token index, state after
    /// the toggle)`. The same fixed toggle set the lexer flips
    /// ([`expl_toggle`]), pre-scanned once so [`Self::in_expl_region`] is a
    /// binary search. An expl3 region is *code* — token lists pass
    /// `\begin`/`\end` around as data (`\tl_set:Nn { \begin{longtable} … }`,
    /// issue #60) — so inside one, `\begin`/`\end` parse as plain commands
    /// exactly as in a definition body ([`Self::plain_env`]). `.dtx` doc-margin
    /// lines are exempt: a region regularly spans macrocode chunks, and the
    /// doc-layer markup between them (`\begin{macro}`, the frames) must keep
    /// pairing.
    expl_toggles: Vec<(usize, bool)>,
    /// Token indices of the `CONTROL_WORD`s that are *live* conditional openers:
    /// `\if`-prefixed, not one of the brace-argument `if*` macros, and not
    /// sitting in an operand slot (`\newif\if@foo`, `\let\ifpdf\iftrue`) or an
    /// `\ifcsname` body. Pre-scanned once in [`Self::new`] because the
    /// operand-slot rule is a *running* state over the whole token stream, which
    /// the recursive-descent walk cannot carry, and because both
    /// [`Self::element`] and [`Self::conditional_pairs`] need the same verdict.
    ///
    /// Openers inside an expl3 region are excluded outright: in-region layout is
    /// the formatter's, owned through `semantic::expl3`'s statement segmentation
    /// (`AGENTS.md`, expl3 code formatting), and a `CONDITIONAL` node there would
    /// contend with it. The exclusion also keeps the `\else:`/`\or:`/`\fi:`
    /// spellings out of scope. Recognition itself is shared with the linter's
    /// `ConditionalIndex` ([`conditional::OpenerScan`]) so the two cannot drift.
    conditional_openers: std::collections::HashSet<usize>,
}

impl<'t> Parser<'t> {
    fn new(tokens: &'t [Token], ctx: &'t VerbCtx) -> Self {
        let mut starts = Vec::with_capacity(tokens.len() + 1);
        let mut off = 0;
        let mut expl_toggles = Vec::new();
        let mut conditional_openers = std::collections::HashSet::new();
        let mut opener_scan = conditional::OpenerScan::new();
        // `expl_on` mirrors `in_expl_region` exactly: the state is the one in
        // force *before* this token, so a toggle sits outside its own region.
        let mut expl_on = false;
        for (i, t) in tokens.iter().enumerate() {
            starts.push(off);
            off += t.text.len();
            if t.kind != SyntaxKind::CONTROL_WORD {
                continue;
            }
            // `visit` is a *state machine* over the whole stream (the operand-slot
            // countdown, the `\ifcsname` body), so it must run for every control
            // word, whatever we then do with the verdict. Kept on its own statement
            // rather than folded into the condition below: buried mid-`&&` behind a
            // cheaper test, a later reordering would skip the call and silently
            // desync the scan from the linter's.
            let word = t
                .text
                .strip_prefix('\\')
                .map(|name| opener_scan.visit(name));
            if word == Some(conditional::Word::Opens) && !expl_on {
                conditional_openers.insert(i);
            }
            if let Some(toggle) = expl_toggle(&t.text) {
                expl_toggles.push((i, toggle == ExplToggle::On));
                expl_on = toggle == ExplToggle::On;
            }
        }
        starts.push(off);
        Self {
            tokens,
            ctx,
            starts,
            pos: 0,
            events: Vec::new(),
            steps: std::cell::Cell::new(0),
            last_step_pos: std::cell::Cell::new(0),
            errors: Vec::new(),
            math_depth: 0,
            math_dollar: Vec::new(),
            in_def_body: false,
            group_depth: 0,
            demoted_envs: std::collections::HashSet::new(),
            open_envs: Vec::new(),
            group_opens: Vec::new(),
            macrocode_end: None,
            plain_braces: std::collections::HashSet::new(),
            expl_toggles,
            conditional_openers,
        }
    }

    /// The conditional divider or closer at token `idx`, if any. Flow words are
    /// classified from the name alone — `\else`/`\or`/`\fi` are never anything
    /// else — but never inside an expl3 region, where the openers are excluded
    /// too (see [`Self::conditional_openers`]).
    ///
    /// Total in `idx`: `Parser::pos` is one past the last token at EOF, and
    /// [`Self::conditional`] asks about the cursor after its loop has run out of
    /// input, so an out-of-range index is "no flow word here", not a bug.
    fn conditional_flow_at(&self, idx: usize) -> Option<conditional::FlowWord> {
        let t = self.tokens.get(idx)?;
        if t.kind != SyntaxKind::CONTROL_WORD || self.in_expl_region(idx) {
            return None;
        }
        t.text.strip_prefix('\\').and_then(conditional::flow_word)
    }

    /// True when token `idx` sits inside an expl3 region (after an
    /// `\ExplSyntaxOn`/`\ProvidesExpl*` with no intervening `\ExplSyntaxOff`).
    /// The toggle token itself is outside its own region.
    fn in_expl_region(&self, idx: usize) -> bool {
        let n = self.expl_toggles.partition_point(|&(i, _)| i < idx);
        n > 0 && self.expl_toggles[n - 1].1
    }

    /// True when token `idx` lies on a `.dtx` doc-margin line (a `DOC_MARGIN`
    /// opens its physical line). Walks back to the preceding `NEWLINE`; doc
    /// lines are short, and the check runs only at `\begin`/`\end` tokens.
    fn on_doc_margin_line(&self, idx: usize) -> bool {
        self.tokens[..idx]
            .iter()
            .rev()
            .take_while(|t| t.kind != SyntaxKind::NEWLINE)
            .any(|t| t.kind == SyntaxKind::DOC_MARGIN)
    }

    /// Whether token `idx` is covered by the `.dtx` doc-margin exemption from the
    /// brace-group gates ([`Self::environment_escapes_group`] and its `\end`-side
    /// mirror): it sits on a documentation line *and* every group open around it
    /// was opened by the code layer.
    ///
    /// The exemption exists for braces the *code* layer stranded — a
    /// `\iffalse{\fi` editor-balance hack, a `` \char`{ `` constant, a
    /// catcode-swapped region — which hold `group_depth` above zero for the rest
    /// of the file and would otherwise unnest the whole doc layer behind them. A
    /// group the documentation layer opened itself is not stranded: it is right
    /// there on a doc line, so a `\begin`/`\end` inside it really is inside it
    /// and the gates apply as they do in code (theorem.dtx's
    /// `% \def\deflist#1{\begin{list}…}` / `% \def\enddeflist{\end{list}}`
    /// split definition, issue #71).
    fn doc_margin_exempt(&self, idx: usize) -> bool {
        self.on_doc_margin_line(idx)
            && !self
                .group_opens
                .last()
                .is_some_and(|&brace| self.on_doc_margin_line(brace))
    }

    /// Whether the `\end` at `idx` is the orphaned partner of a `\begin` the
    /// brace-group gate demoted: its name was gated somewhere earlier
    /// ([`Self::demoted_envs`]) and no environment of that name is open here.
    ///
    /// The gate turns a `\begin` into a plain command, and a lone `\end` then
    /// unwinds every enclosing environment on its way to the root — one gated
    /// `\begin` inside a `\lowercase{…}` group un-closes the whole `document`
    /// (amsldoc.tex, issue #71). Demoting the `\end` too keeps the gate's two
    /// halves consistent. A genuine typo (`\end{itemiz}`) is untouched: nothing
    /// demoted that name, so it stays a stray `\end`.
    fn end_orphans_a_demoted_begin(&self, idx: usize) -> bool {
        if self.demoted_envs.is_empty() {
            return false;
        }
        peek_end_name(self.tokens, idx).is_some_and(|name| {
            self.demoted_envs.contains(&name) && !self.open_envs.contains(&name)
        })
    }

    /// True when token `idx` sits in *macro code*: inside a definition body
    /// (issues #45/#55) or inside an expl3 region (issue #60; `.dtx` doc-margin
    /// lines exempt, see [`Self::expl_toggles`]). There `\begin`/`\end` are
    /// plain commands that need not pair, and an orphan `\]`/`\)` is data
    /// (`AGENTS.md` decision #1).
    fn in_macro_code(&self, idx: usize) -> bool {
        self.in_def_body || (self.in_expl_region(idx) && !self.on_doc_margin_line(idx))
    }

    /// True when the cursor sits lexically inside a math body — including inside
    /// a text-mode block (unknown environment, `\text{…}`-style group) nested in
    /// one. See the `math_depth` field.
    fn in_math(&self) -> bool {
        self.math_depth > 0
    }

    // --- cursor primitives -------------------------------------------------

    /// Tick the stuck-loop guard, called from every lookahead primitive. Resets
    /// the budget whenever the cursor has advanced since the last tick (real
    /// progress — via `bump` or the math-split fast path, both of which move
    /// `pos`), so the surviving count is the number of *consecutive* peeks with no
    /// token consumed. Exceeding [`PARSER_STEP_LIMIT`] means the parser is wedged
    /// in a non-advancing loop; abort loudly rather than hang. This can only fire
    /// on a grammar bug or pathological input, never on a real document, and the
    /// async callers (the language server's worker + read pool) already recover
    /// from a parse panic, degrading a wedged parse to a logged error.
    #[inline]
    fn step(&self) {
        if self.pos != self.last_step_pos.get() {
            self.last_step_pos.set(self.pos);
            self.steps.set(0);
        }
        let steps = self.steps.get();
        assert!(
            steps < PARSER_STEP_LIMIT,
            "parser exceeded {PARSER_STEP_LIMIT} peeks without consuming a token at position {} \
             — non-advancing loop",
            self.pos
        );
        self.steps.set(steps + 1);
    }

    fn kind(&self) -> Option<SyntaxKind> {
        self.step();
        self.tokens.get(self.pos).map(|t| t.kind)
    }

    fn nth_kind(&self, n: usize) -> Option<SyntaxKind> {
        self.step();
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

    /// True if the `\begin`/`\end` at token index `pos` reads as a LaTeX
    /// environment delimiter: a `{` follows across trivia, without crossing a
    /// blank line, and the name inside is name-shaped. Macro code uses the
    /// bare TeX primitive and delimiter patterns (`\let\end\@@end`,
    /// `\long\def\@gobble@nv#1\end#2{…}`, `\expandafter\end`, xparse's
    /// `\begin \end {#3}` argument data — issue #60) at least as often as
    /// prose omits the brace by mistake, so a brace-less `\begin`/`\end` is a
    /// plain command everywhere: no environment, no diagnostic, and no
    /// recovery anchor. Likewise a name group holding a parameter or control
    /// word (`\end{#2}`, `\edef…{\noexpand\end{\reserved@a}}`) is computed
    /// macro data — statically unpairable — so it too stays a plain command
    /// (the group attaches as an ordinary argument).
    fn env_name_follows(&self, pos: usize) -> bool {
        let s = self.scan_trivia(pos + 1, CommentMode::Skip);
        if s.saw_blank_line || s.next_kind != Some(SyntaxKind::L_BRACE) {
            return false;
        }
        // Scan the name up to the closing `}` on the same line: a parameter
        // (`#`), a control word/symbol, or a nested `{` before it is macro
        // data, not a name. An *unterminated* name (line end or EOF first) is
        // an in-progress edit — stay optimistic so `\begin{ali` still parses
        // as a `BEGIN` + `NAME_GROUP` and environment-name completion sees it.
        for t in &self.tokens[s.next + 1..] {
            match t.kind {
                SyntaxKind::R_BRACE | SyntaxKind::NEWLINE => return true,
                SyntaxKind::HASH
                | SyntaxKind::CONTROL_WORD
                | SyntaxKind::CONTROL_SYMBOL
                | SyntaxKind::L_BRACE => return false,
                _ => {}
            }
        }
        true
    }

    fn is_trivia(k: SyntaxKind) -> bool {
        matches!(
            k,
            SyntaxKind::WHITESPACE
                | SyntaxKind::NEWLINE
                | SyntaxKind::COMMENT
                | SyntaxKind::DOC_MARGIN
                | SyntaxKind::GUARD
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

    /// Report an error at an explicit byte range. Used for *unclosed*-delimiter
    /// errors, which are detected at the closing anchor (a recovery token or EOF)
    /// but belong on the *opener* (`{`, `$`, `\[`, `\left`, `\begin{…}`)—the
    /// token the reader must fix. Pointing them at the detection site would land
    /// every unclosed error on EOF (a zero-width span at end of file).
    fn error_at(&mut self, range: (usize, usize), message: impl Into<String>) {
        self.errors.push(SyntaxError {
            message: message.into(),
            start: range.0,
            end: range.1,
        });
    }

    /// Byte range of the token at `pos` (`[starts[pos], starts[pos + 1])`).
    /// Captured at a construct's opener before it is consumed, so an unclosed
    /// error can point back at it (see [`Self::error_at`]).
    fn token_span(&self, pos: usize) -> (usize, usize) {
        (self.starts[pos], self.starts[pos + 1])
    }

    fn skip_trivia(&mut self) {
        while self.kind().is_some_and(Self::is_trivia) {
            self.bump();
        }
    }

    /// Scan the contiguous trivia run starting at `from`, classifying each token
    /// so the blank-line and leading-comment-bind rules (AGENTS.md #9) can be
    /// decided from one walk instead of five near-identical ones. `WHITESPACE`,
    /// `DOC_MARGIN`, and `GUARD` float (a `.dtx` margin neither counts as a
    /// newline nor resets the run, so a margin-only line `%\n%\n` still reads as a
    /// blank line via its two `NEWLINE`s); `NEWLINE`s accumulate into the blank-line
    /// (`≥2`) test; a `COMMENT` is handled per [`CommentMode`]. A `GUARD` floats
    /// for `saw_blank_line` but breaks the run for
    /// [`TriviaScan::saw_blank_line_outside_guards`]. Does not consume.
    fn scan_trivia(&self, from: usize, comment_mode: CommentMode) -> TriviaScan {
        let mut i = from;
        let mut newlines = 0;
        let mut guard_newlines = 0;
        let mut saw_blank_line = false;
        let mut saw_blank_line_outside_guards = false;
        let mut comment_start = None;
        while let Some(t) = self.tokens.get(i) {
            match t.kind {
                SyntaxKind::NEWLINE => {
                    newlines += 1;
                    guard_newlines += 1;
                    if newlines >= 2 {
                        saw_blank_line = true;
                        // A blank line breaks a leading-comment bind: only a
                        // comment *after* it can still bind, so drop any comment
                        // seen before it.
                        comment_start = None;
                    }
                    if guard_newlines >= 2 {
                        saw_blank_line_outside_guards = true;
                    }
                }
                SyntaxKind::WHITESPACE | SyntaxKind::DOC_MARGIN => {}
                // A docstrip guard floats like a margin for the layout rules
                // (`saw_blank_line`), but it is *content* on its line — and a
                // line docstrip deletes outright when it strips the file, so a
                // guard-only line is not a blank line separating what surrounds
                // it. Constructs that only need to know whether their source
                // ran out mid-shape read `saw_blank_line_outside_guards`
                // instead (issue #71).
                SyntaxKind::GUARD => guard_newlines = 0,
                SyntaxKind::COMMENT if comment_mode == CommentMode::Stop => break,
                // A comment occupies its own line: it is content, not blank space,
                // so it resets the newline run (without undoing a blank line
                // already seen) and, if it starts its line, opens a
                // leading-comment bind.
                SyntaxKind::COMMENT => {
                    newlines = 0;
                    guard_newlines = 0;
                    if comment_start.is_none() && self.comment_starts_line(i) {
                        comment_start = Some(i);
                    }
                }
                _ => break,
            }
            i += 1;
        }
        TriviaScan {
            next: i,
            next_kind: self.tokens.get(i).map(|t| t.kind),
            saw_blank_line,
            saw_blank_line_outside_guards,
            comment_start,
        }
    }

    /// Peek the kind of the next non-trivia token and whether the intervening
    /// trivia contains a paragraph break (a blank line, i.e. ≥2 newlines).
    /// Does not consume.
    fn peek_meaningful(&self) -> (Option<SyntaxKind>, bool) {
        let s = self.scan_trivia(self.pos, CommentMode::Skip);
        (s.next_kind, s.saw_blank_line)
    }

    /// Text of the next non-trivia token at/after `self.pos`, if any. Does not
    /// consume. Used to distinguish a verbatim-argument `VERB` from a standalone
    /// `\verb…` token (see `attach_arguments`).
    fn peek_meaningful_text(&self) -> Option<&str> {
        let mut i = self.pos;
        while let Some(t) = self.tokens.get(i) {
            if !Self::is_trivia(t.kind) {
                return Some(t.text.as_str());
            }
            i += 1;
        }
        None
    }

    /// True if a paragraph break (blank line) begins at the current position.
    fn at_paragraph_break(&self) -> bool {
        self.scan_trivia(self.pos, CommentMode::Skip).saw_blank_line
    }

    /// [`Self::at_paragraph_break`], but blind to `.dtx` docstrip guard lines:
    /// a `%<*dtx>`/`%</dtx>` pair on its own lines is not the blank line it
    /// looks like, because docstrip deletes those lines outright. Used by the
    /// bail-out anchors of constructs that legitimately span a guarded block —
    /// `\ProvidesPackage{…}` and its `[…date…]` optional, split across
    /// `%<package>`/`%<*dtx>` variants (rotating.dtx, issue #71).
    fn at_paragraph_break_outside_guards(&self) -> bool {
        self.scan_trivia(self.pos, CommentMode::Skip)
            .saw_blank_line_outside_guards
    }

    /// True if the comment at `pos` starts its own line: scanning back over
    /// inline whitespace only, the preceding token is a `NEWLINE` or the start of
    /// input. A same-line trailing comment (`\foo % x`) returns `false` and never
    /// binds forward (see [`Self::binding_run`]).
    fn comment_starts_line(&self, pos: usize) -> bool {
        let mut i = pos;
        while i > 0 {
            i -= 1;
            match self.tokens[i].kind {
                // A `.dtx` margin or guard is skipped like whitespace when deciding
                // whether a comment owns its line (neither is itself the comment).
                SyntaxKind::WHITESPACE | SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD => {
                    continue;
                }
                SyntaxKind::NEWLINE => return true,
                _ => return false,
            }
        }
        true
    }

    /// If the trivia run at `from` ends in a `%` comment run that binds *leading*
    /// into a following documentable construct, return
    /// `(comment_start, construct_pos, construct_kind)`:
    /// - `comment_start` — index of the first own-line comment of the binding run
    ///   (the maximal blank-line-free suffix; trivia before it floats),
    /// - `construct_pos` — index of the construct's control word,
    /// - `construct_kind` — `ENVIRONMENT` for `\begin`, otherwise `COMMAND`.
    ///
    /// Returns `None` when the run has no own-line comment, a blank line separates
    /// the comment from the construct, or the next non-trivia token is not a
    /// documentable construct. Mirrors rust-analyzer's `n_attached_trivias`
    /// (AGENTS.md #9): comments bind forward to the item they annotate, a blank
    /// line breaks the bind, and a same-line trailing comment never binds.
    ///
    /// One deliberate divergence: RA peeks *past* a blank line and keeps attaching
    /// when the next comment is an outer doc comment (`///`/`//!`). LaTeX's single
    /// `%` carries no such intent marker, so we always stop at the blank line (only
    /// the maximal blank-line-free suffix binds). See AGENTS.md #9;
    /// `comment_after_blank_line_still_binds` (`tests/parser.rs`) pins the divergence.
    fn binding_run(&self, from: usize) -> Option<(usize, usize, SyntaxKind)> {
        let s = self.scan_trivia(from, CommentMode::Skip);
        let start = s.comment_start?;
        if s.next_kind != Some(SyntaxKind::CONTROL_WORD) {
            return None;
        }
        let kind = match self.tokens[s.next].text.as_str() {
            BEGIN_CMD => SyntaxKind::ENVIRONMENT,
            END_CMD => return None,
            _ => SyntaxKind::COMMAND,
        };
        Some((start, s.next, kind))
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
            // directly, never wrapped in a paragraph — except a trailing own-line
            // comment run that binds into the construct after it: stop before that
            // comment so the construct (next iteration) absorbs it as leading.
            if self.kind().is_some_and(Self::is_trivia) && self.trivia_run_is_separator(block) {
                let stop = self
                    .binding_run(self.pos)
                    .map_or(self.tokens.len(), |(comment_start, ..)| comment_start);
                while self.pos < stop && self.kind().is_some_and(Self::is_trivia) {
                    self.bump();
                }
                continue;
            }
            // Otherwise we're at paragraph content (guaranteed ≥1 token, so no
            // empty paragraph and no infinite loop). Parse the run first, then
            // splice in the `PARAGRAPH` wrapper afterwards (the `precede` idiom,
            // cf. `math_scripted`) — unless the run's only non-trivia element is a
            // lone block environment, which we leave bare. Block-ness is read from
            // the built-in signature DB (`is_block_environment`).
            let checkpoint = self.events.len();
            let mut nontrivia_count = 0usize;
            let mut lone_block_env = false;
            loop {
                if self.at_block_end(block) {
                    break;
                }
                if self.kind().is_some_and(Self::is_trivia) && self.trivia_run_is_separator(block) {
                    break;
                }
                // Leading comment-bind: an own-line `%` run immediately before a
                // documentable construct attaches *leading* into it. Float any
                // trivia before the comment run, then wrap the comments + construct
                // in the construct's node (the `precede` idiom: the construct
                // self-opens, then its `Start` is pulled back over the comments).
                // The bound run itself is grouped into a `DOC_COMMENT` node — the
                // named-trivia enrichment AGENTS.md #9 reserved — so downstream
                // (LSP/formatter) sees the doc comment as one unit rather than
                // bare leaves.
                if let Some((comment_start, construct_pos, _)) = self.binding_run(self.pos) {
                    while self.pos < comment_start {
                        self.bump();
                    }
                    let checkpoint = self.events.len();
                    self.open(SyntaxKind::DOC_COMMENT);
                    while self.pos < construct_pos {
                        self.bump();
                    }
                    self.close();
                    let starts_block_env = self.tokens[construct_pos].text == BEGIN_CMD
                        && peek_begin_name(self.tokens, construct_pos)
                            .as_deref()
                            .is_some_and(is_block_environment);
                    let construct_start = self.events.len();
                    self.element();
                    if let Event::Start(kind) = self.events[construct_start] {
                        self.events.remove(construct_start);
                        self.events.insert(checkpoint, Event::Start(kind));
                    }
                    nontrivia_count += 1;
                    lone_block_env = nontrivia_count == 1 && starts_block_env;
                    continue;
                }
                let is_nontrivia = !self.kind().is_some_and(Self::is_trivia);
                // Peek block-env status *before* consuming (the name is only
                // available while still on the `\begin`).
                let starts_block_env = self.at_command(BEGIN_CMD)
                    && peek_begin_name(self.tokens, self.pos)
                        .as_deref()
                        .is_some_and(is_block_environment);
                self.element();
                if is_nontrivia {
                    nontrivia_count += 1;
                    lone_block_env = nontrivia_count == 1 && starts_block_env;
                }
            }
            if !lone_block_env {
                self.events
                    .insert(checkpoint, Event::Start(SyntaxKind::PARAGRAPH));
                self.close(); // matching Finish for PARAGRAPH
            }
        }
    }

    fn at_block_end(&self, block: Block) -> bool {
        self.at_end()
            || match block {
                Block::Document => false,
                Block::Environment => {
                    self.at_command(END_CMD)
                        && self.env_name_follows(self.pos)
                        && !self.end_orphans_a_demoted_begin(self.pos)
                }
                // `>=` (not `==`): defensive against an element overshooting the
                // pre-scanned terminator, so the loop still stops.
                Block::Macrocode => self.macrocode_end.is_some_and(|end| self.pos >= end),
            }
    }

    /// True if the contiguous trivia run at the current position should separate
    /// paragraphs: it contains a blank line, or only trivia remains before the
    /// block terminator (the `\end`, or EOF).
    fn trivia_run_is_separator(&self, block: Block) -> bool {
        let s = self.scan_trivia(self.pos, CommentMode::Skip);
        if s.saw_blank_line {
            return true;
        }
        // A macrocode body ends positionally at the frame terminator; trivia
        // reaching it (the frame line's own margin and indent) is a separator.
        if block == Block::Macrocode {
            return s.next_kind.is_none() || self.macrocode_end.is_some_and(|end| s.next >= end);
        }
        match s.next_kind {
            // Only trivia remains before the block terminator (`\end`, or EOF).
            None => true,
            Some(SyntaxKind::CONTROL_WORD) => {
                block == Block::Environment
                    && self.tokens[s.next].text == END_CMD
                    && self.env_name_follows(s.next)
            }
            Some(_) => false,
        }
    }

    /// One element in text mode. Always consumes at least one token.
    fn element(&mut self) {
        let Some(k) = self.kind() else { return };
        match k {
            SyntaxKind::WHITESPACE
            | SyntaxKind::NEWLINE
            | SyntaxKind::COMMENT
            | SyntaxKind::DOC_MARGIN
            | SyntaxKind::GUARD => self.bump(),
            SyntaxKind::CONTROL_WORD => {
                // Inside a definition body or an expl3 region, `\begin`/`\end`
                // are plain commands: the two need not balance within one group
                // (issues #45/#60), so neither opens an environment nor is
                // stray. A brace-less `\begin`/`\end` is likewise a plain
                // command (`env_name_follows`).
                if !self.in_macro_code(self.pos)
                    && self.at_command(BEGIN_CMD)
                    && self.env_name_follows(self.pos)
                {
                    // Shape-gated like `\[`: an environment cannot outlive the
                    // brace group it opened in, so one whose `\end` is not
                    // reachable before that group closes is macro code — a
                    // plain command, no diagnostic (issue #71).
                    if self.environment_escapes_group(self.pos) {
                        if let Some(name) = peek_end_name(self.tokens, self.pos) {
                            self.demoted_envs.insert(name);
                        }
                        self.command();
                    } else {
                        self.environment();
                    }
                } else if !self.in_macro_code(self.pos)
                    && self.at_command(END_CMD)
                    && self.env_name_follows(self.pos)
                {
                    // The mirror case: reached inside a group, this `\end`'s
                    // `\begin` is outside it, so it is macro code rather than
                    // stray (`\StopEventually{\end{document}}`, issue #71).
                    if (self.group_depth > 0 && !self.doc_margin_exempt(self.pos))
                        || self.end_orphans_a_demoted_begin(self.pos)
                    {
                        self.command();
                    } else {
                        self.stray_end();
                    }
                } else if let Some(closer) = self
                    .conditional_openers
                    .contains(&self.pos)
                    .then(|| self.conditional_closer(self.pos))
                    .flatten()
                {
                    // Shape-gated like `\[` and `\begin`: an `\if` whose own
                    // `\fi` is not reachable is macro code — a plain command,
                    // no diagnostic ([`Self::conditional_closer`]).
                    self.conditional(closer);
                } else {
                    self.command();
                }
            }
            SyntaxKind::CONTROL_SYMBOL => {
                let sym = self.text().to_owned();
                match sym.as_str() {
                    // Shape-gated like `$` ([`Self::delim_math_closes`]): an
                    // opener with no reachable closer is macro-code data
                    // (`\expandafter\@tempa\[\@nil`, issue #65) — an ordinary
                    // token, no math, no diagnostic.
                    "\\[" => {
                        if self.delim_math_closes(self.pos, "\\]") {
                            self.delim_math(SyntaxKind::DISPLAY_MATH, "\\[", "\\]");
                        } else {
                            self.bump();
                        }
                    }
                    "\\(" => {
                        if self.delim_math_closes(self.pos, "\\)") {
                            self.delim_math(SyntaxKind::INLINE_MATH, "\\(", "\\)");
                        } else {
                            self.bump();
                        }
                    }
                    "\\]" | "\\)" => {
                        // In macro code (a definition body, macrocode chunk,
                        // or expl3 region) an orphan closer is data, not a
                        // stray delimiter (`\char_set_catcode_letter:N \)`,
                        // issue #60) — an ordinary token, no diagnostic. In
                        // prose it still diagnoses, catching a `\[…\]` typo'd
                        // across a paragraph break on its closer.
                        if !self.in_macro_code(self.pos) {
                            self.error(format!("unmatched `{sym}`"));
                        }
                        self.bump();
                    }
                    // `\\` line break, with its tightly-bound `*` / `[len]`.
                    "\\\\" => self.line_break(),
                    // Any other bare control symbol (`\,`, `\%`, `\;`, …). Surface
                    // model: emit as a token; these take no arguments.
                    _ => self.bump(),
                }
            }
            // A brace unmatched within a `macrocode` chunk is an ordinary macro-
            // code token (the definition it belongs to spans chunks): no `GROUP`,
            // no diagnostic.
            SyntaxKind::L_BRACE => {
                if self.plain_braces.contains(&self.pos) {
                    self.bump();
                } else {
                    self.group();
                }
            }
            SyntaxKind::R_BRACE => {
                if !self.plain_braces.contains(&self.pos) {
                    self.error("unmatched `}`");
                }
                self.bump();
            }
            SyntaxKind::DOLLAR => {
                let display = self.nth_kind(1) == Some(SyntaxKind::DOLLAR);
                if self.dollar_closes(self.pos, display) {
                    self.dollar_math();
                } else {
                    // No reachable closer: this dollar is macro-code data
                    // (`>{$}`, `{ $ }`), not a math delimiter — an ordinary
                    // token, no math, no diagnostic. Each `$` of an ungated
                    // `$$` re-enters here and is gated independently.
                    self.bump();
                }
            }
            // WORD, brackets, & # ^ _ ~, ERROR: ordinary tokens in text mode.
            _ => self.bump(),
        }
    }

    /// `\foo` followed by its greedily-attached argument groups.
    ///
    /// Arity is unknown without the semantic layer, so we attach every trailing
    /// `{…}` / `[…]` group (see `AGENTS.md`, Core decision #8, and
    /// [`Self::attach_arguments`] for the `[…]` shape gates). The one curated
    /// exception: a delimiter-size command (`\Big`, `\bigl`, …) never takes a
    /// `[…]` argument — its `[` is the delimiter it sizes (`\Big[ x \Big]`),
    /// mirroring the `\left`/`\right` special case.
    fn command(&mut self) {
        let bracket = if is_big_delimiter_command(self.text()) {
            BracketPolicy::Forbid
        } else {
            BracketPolicy::Greedy
        };
        // A definition-body command's attached groups are macro-code bodies
        // (issues #45/#55): flag them so `\begin`/`\end` inside parse as
        // plain commands. OR-ed with the saved flag so a definition nested in
        // another definition's body stays flagged; restored after the
        // arguments so following siblings are unaffected.
        let saved = self.in_def_body;
        self.in_def_body = saved || is_definition_body_command(self.text());
        let def_prefix = is_def_prefix_command(self.text());
        self.open(SyntaxKind::COMMAND);
        self.bump(); // the control word
        // A `\def`-family primitive's next token is the control sequence being
        // defined ([`is_def_prefix_command`]). A control-symbol name is
        // consumed here as a plain token so it is never misparsed as syntax
        // (`\def\[{…}` is not a math opener), and the attached body is then a
        // macro-code body: the stacks-project redefinition opens `trivlist` in
        // `\def\[`'s body and closes it in `\def\]`'s (issue #65), the same
        // no-balance fact as `is_definition_body_command`.
        if def_prefix {
            let scan = self.scan_trivia(self.pos, CommentMode::Skip);
            if scan.next_kind == Some(SyntaxKind::CONTROL_SYMBOL) && !scan.saw_blank_line {
                self.skip_trivia();
                self.bump(); // the defined name
                self.in_def_body = true;
            }
        }
        self.attach_arguments(bracket);
        self.in_def_body = saved;
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
    ///
    /// `[…]` attachment is additionally shape-gated (issue #43) — `[`/`]` are
    /// not real grouping in TeX, so a bracket is an argument only when it reads
    /// as one:
    /// - **Lexically inside math, only when it directly abuts.** Real math
    ///   optionals are written tight (`\sqrt[3]{x}`, `\\[2ex]`); a spaced `[`
    ///   is a delimiter or interval (`\bE [ x ]`). This uses [`Self::in_math`],
    ///   so it also covers text-mode bodies of unknown environments nested in
    ///   math (`\[ … \begin{myaligned} \Big [ … \]`).
    /// - **Inside math, only when [`Self::bracket_closes_before_math_end`]
    ///   finds its `]`**; otherwise it is left for the math loop as an ordinary
    ///   atom, so open-interval notation (`$]0;\num{0.5}[$`) does not swallow
    ///   the math closer as an optional-argument body.
    /// - **In text mode, only when [`Self::bracket_closes_in_text`] finds its
    ///   `]`** (issue #60): macro code tests for and re-emits lone brackets
    ///   (`\@ifnextchar [\@xmpar\@ympar`), so a `[` whose closer is not
    ///   reachable stays an ordinary token — no `OPTIONAL`, no diagnostic —
    ///   mirroring the `$` shape gate ([`Self::dollar_closes`]).
    /// - **Per the caller's [`BracketPolicy`]:** `Tight` (a curated math
    ///   environment's `\begin` — its math body starts right after, so a
    ///   detached `[` is content: `\begin{align}` + newline + `[a]_1`) demands
    ///   a directly-abutting `[` even outside math; `Forbid` (the
    ///   delimiter-size commands, [`Self::command`]) never attaches one.
    ///   `Greedy` — everything else — keeps decision #8's trivia-crossing
    ///   attachment, which the semantic layer legitimizes downstream (the
    ///   xparse-signature glue relies on a next-line `[Warning]` still
    ///   attaching to `\begin{note}`).
    fn attach_arguments(&mut self, bracket: BracketPolicy) {
        loop {
            let (next, paragraph_break) = self.peek_meaningful();
            if paragraph_break {
                break;
            }
            match next {
                Some(SyntaxKind::L_BRACE) => {
                    // A chunk-unmatched macrocode brace is a plain token, never
                    // an argument group (`\gdef\foo{%` … next chunk).
                    let scan = self.scan_trivia(self.pos, CommentMode::Skip);
                    if self.plain_braces.contains(&scan.next) {
                        break;
                    }
                    self.skip_trivia();
                    self.group();
                }
                Some(SyntaxKind::L_BRACKET) => {
                    if bracket == BracketPolicy::Forbid {
                        break;
                    }
                    let scan = self.scan_trivia(self.pos, CommentMode::Skip);
                    let tight_only = self.in_math() || bracket == BracketPolicy::Tight;
                    if tight_only && scan.next != self.pos {
                        break;
                    }
                    if self.in_math() && !self.bracket_closes_before_math_end(scan.next) {
                        break;
                    }
                    // In a macrocode body, a `[` is an argument only when its `]`
                    // closes inside the chunk: macro code uses bare brackets
                    // freely, and an optional must never consume the frame.
                    if self.macrocode_end.is_some()
                        && !self.bracket_closes_before_macrocode_end(scan.next)
                    {
                        break;
                    }
                    // In text mode, a `[` is an argument only when its `]` is
                    // reachable ([`Self::bracket_closes_in_text`]): macro code
                    // tests for and re-emits lone brackets at least as often as
                    // prose writes real optionals (`\@ifnextchar [\@xmpar\@ympar`,
                    // issue #60), so an unreachable closer means the bracket is
                    // data, not an argument.
                    if !self.in_math()
                        && self.macrocode_end.is_none()
                        && !self.bracket_closes_in_text(scan.next)
                    {
                        break;
                    }
                    self.skip_trivia();
                    self.optional();
                }
                // A verbatim-argument command's body (`\url{…}`, `\lstinline|…|`,
                // the final arg of `\mintinline{lang}{code}`) is lexed as a single
                // `VERB` token immediately following the command, so attach it as a
                // child like any other argument (decision #8) instead of leaving it
                // a sibling. A *standalone* `\verb…`/`\verb*…` token (its text starts
                // with `\`) is self-contained and belongs to no command — never
                // capture it. `lex_verbatim_command` emits its non-`\` `VERB`
                // *directly* after its own command tokens, so only a directly
                // abutting `VERB` attaches: a spaced one is a doc short-verb span
                // (`\emph{x} |y|`), a freestanding sibling that must keep its
                // interword space.
                Some(SyntaxKind::VERB)
                    if self.scan_trivia(self.pos, CommentMode::Skip).next == self.pos
                        && !self
                            .peek_meaningful_text()
                            .is_some_and(|t| t.starts_with('\\')) =>
                {
                    self.bump(); // the VERB argument
                }
                // A starred-variant marker `*` folds into the invocation so the
                // arguments that follow it still attach (`\section*{…}`,
                // `\inferrule*[…]`, `\\*[2pt]`).
                Some(SyntaxKind::WORD) if self.at_star_variant_marker() => {
                    self.bump(); // the `*`
                }
                _ => break,
            }
        }
    }

    /// Whether the next token is a *starred-variant marker* to fold into the
    /// command invocation: a lone `*` tight to the command, itself followed by
    /// an argument opener (`[`/`{`). LaTeX's `\@ifstar` commands carry the star
    /// before their arguments (`\section*{…}`, mathpartir's `\inferrule*[…]`,
    /// the `\\*[2pt]` line break), so folding it lets those arguments attach
    /// (decision #8) instead of the `*` breaking the run. Gating on a *following
    /// argument* keeps a math operator (`\pi*r`, `\Gamma * x`) — a `*` with no
    /// argument after it — from being mistaken for a marker. The `*` must be a
    /// lone token tight to the command: a spaced `\foo *` is not a marker, and
    /// `\foo*bar` lexes the star into a single `*bar` word (text ≠ `*`), so
    /// neither folds. Does not consume.
    fn at_star_variant_marker(&self) -> bool {
        if self.scan_trivia(self.pos, CommentMode::Skip).next != self.pos {
            return false; // the star must be tight to the command
        }
        if self.tokens.get(self.pos).map(|t| (t.kind, t.text.as_str()))
            != Some((SyntaxKind::WORD, "*"))
        {
            return false;
        }
        matches!(
            self.scan_trivia(self.pos + 1, CommentMode::Skip).next_kind,
            Some(SyntaxKind::L_BRACKET | SyntaxKind::L_BRACE)
        )
    }

    /// A brace group `{ … }`.
    fn group(&mut self) {
        debug_assert_eq!(self.kind(), Some(SyntaxKind::L_BRACE));
        let opener = self.token_span(self.pos);
        self.open(SyntaxKind::GROUP);
        self.bump(); // {
        self.group_depth += 1;
        self.group_opens.push(self.pos - 1);
        loop {
            match self.kind() {
                None => {
                    self.error_at(opener, "unclosed `{`");
                    break;
                }
                Some(SyntaxKind::R_BRACE) => {
                    self.bump();
                    break;
                }
                _ => self.element(),
            }
        }
        self.group_depth -= 1;
        self.group_opens.pop();
        self.close();
    }

    /// An optional-argument group `[ … ]`.
    ///
    /// `[` and `]` are not real grouping in TeX, so this is heuristic: it ends
    /// at the first `]`, and bails defensively (rather than swallowing the
    /// document) on a `}`, a `\begin`/`\end`, a paragraph break, or EOF.
    fn optional(&mut self) {
        debug_assert_eq!(self.kind(), Some(SyntaxKind::L_BRACKET));
        let opener = self.token_span(self.pos);
        self.open(SyntaxKind::OPTIONAL);
        self.bump(); // [
        loop {
            match self.kind() {
                None | Some(SyntaxKind::R_BRACE) => {
                    self.error_at(opener, "unclosed `[`");
                    break;
                }
                Some(SyntaxKind::R_BRACKET) => {
                    self.bump();
                    break;
                }
                // In a definition body or expl3 region `\begin`/`\end` are
                // plain commands (issues #45/#60), so they don't signal a
                // runaway `[` — nor does a brace-less one (issue #60).
                Some(SyntaxKind::CONTROL_WORD)
                    if !self.in_macro_code(self.pos)
                        && (self.at_command(BEGIN_CMD) || self.at_command(END_CMD))
                        && self.env_name_follows(self.pos) =>
                {
                    self.error_at(opener, "unclosed `[`");
                    break;
                }
                _ => {
                    // The macrocode frame terminator is absolute: an optional
                    // still open there is abandoned, never consumes the frame.
                    if self.at_paragraph_break_outside_guards()
                        || self.macrocode_end.is_some_and(|end| self.pos >= end)
                    {
                        self.error_at(opener, "unclosed `[`");
                        break;
                    }
                    self.element();
                }
            }
        }
        self.close();
    }

    /// True if the `[` at token index `open` is closed by a `]` before the
    /// current macrocode chunk's frame terminator. Depth-tracks only the braces
    /// that really form groups (chunk-matched ones — [`Self::plain_braces`] are
    /// plain tokens), and gives up at a *blank line* — the same paragraph-break
    /// bail as [`Self::optional`], so an optional the formatter has re-wrapped
    /// over several lines still attaches on the second pass. Keeps a code
    /// bracket (`\@tempcnta[` with no `]` in the chunk) an ordinary token
    /// instead of an optional that would swallow the frame.
    fn bracket_closes_before_macrocode_end(&self, open: usize) -> bool {
        let Some(end) = self.macrocode_end else {
            return true;
        };
        let mut depth = 0usize;
        let mut newline_run = 0;
        for (off, t) in self.tokens[open + 1..end.min(self.tokens.len())]
            .iter()
            .enumerate()
        {
            let idx = open + 1 + off;
            match t.kind {
                SyntaxKind::NEWLINE => {
                    newline_run += 1;
                    if newline_run >= 2 {
                        return false;
                    }
                    continue;
                }
                SyntaxKind::WHITESPACE => continue,
                SyntaxKind::L_BRACE if !self.plain_braces.contains(&idx) => depth += 1,
                SyntaxKind::R_BRACE if !self.plain_braces.contains(&idx) => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                SyntaxKind::R_BRACKET if depth == 0 => return true,
                _ => {}
            }
            newline_run = 0;
        }
        false
    }

    /// True if the `[` at token index `open` is closed by a `]` before a token
    /// that would end the enclosing math. Mirrors [`Self::optional`]'s bail
    /// anchors (an unbalanced `}`, `\begin`/`\end`, a paragraph break, EOF) and
    /// adds the delimited math closers (`\]`, `\)`), which `optional` cannot
    /// stop at in text mode (`\item[$x$]` is legit) but which inside math mean
    /// the `[` is not an argument at all — e.g. the open-interval notation
    /// `$]0;\num{0.5}[$`. A `]` counts only outside `{…}` nesting, matching how
    /// `optional` consumes whole groups via `element` — and only past the `]`s
    /// owed to intervening *command-abutting* `[`s: such a `[` is itself
    /// argument-shaped (or a `\left`/`\Big` delimiter) and will claim the next
    /// `]` when parsed, so that `]` cannot also satisfy the outer `[`
    /// (`\P[\gamma[0, \infty) \cap A = \emptyset]`, issue #55 — the lone `]`
    /// belongs to `\gamma[`, so `\P[` stays an ordinary atom). A `[` abutting
    /// anything else (`x[i]`, the interval `[0, \infty)`) parses as an ordinary
    /// atom and claims nothing, so it adds no nesting here either.
    ///
    /// How a `$` at brace depth 0 is read depends on the *innermost enclosing
    /// math's flavor* ([`Self::math_dollar`]):
    /// - **Enclosing `\[…\]`/`\(…\)` (or a math environment).** A `$` opens a
    ///   genuine nested inline region, so a balanced `$…$` pair inside the
    ///   bracket is *transparent*: the `$` toggles an inline region rather than
    ///   ending the search, and `]`/`[` inside it are math content, ignored
    ///   (`\[ \inferrule*[right=$\Pi$-eq]{A}{B} \]` — the `$\Pi$` label sits
    ///   inside the optional). An *unbalanced* `$` leaves the region open, no
    ///   `]` is ever accepted, and the scan falls through to `false`.
    /// - **Enclosing `$…$`/`$$…$$`.** TeX cannot nest a `$` inside dollar math,
    ///   so the first depth-0 `$` is this math's *closer*: a `]` beyond it lives
    ///   in a later math and cannot be this bracket's, so bail like `\]`/`\)`.
    ///   Without this a stray `[` in dollar math (`$\mathcal{N}[\mathcal{S}$`,
    ///   a missing `]`, stacks-project issue #99) would scan past the closing
    ///   `$` into following math and wrongly attach an optional that swallows
    ///   it. Does not consume.
    fn bracket_closes_before_math_end(&self, open: usize) -> bool {
        let enclosing_is_dollar = self.math_dollar.last().copied().unwrap_or(false);
        let mut depth = 0usize;
        let mut brackets = 0usize;
        let mut in_inline = false;
        let mut newlines = 0;
        let mut abuts_command = false;
        for (off, t) in self.tokens[open + 1..].iter().enumerate() {
            let idx = open + 1 + off;
            let prev_abuts_command = abuts_command;
            abuts_command = false;
            match t.kind {
                SyntaxKind::NEWLINE => {
                    newlines += 1;
                    if newlines >= 2 {
                        return false;
                    }
                    continue;
                }
                SyntaxKind::WHITESPACE | SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD => continue,
                SyntaxKind::L_BRACE => depth += 1,
                SyntaxKind::R_BRACE => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                SyntaxKind::DOLLAR if depth == 0 => {
                    if enclosing_is_dollar {
                        return false;
                    }
                    in_inline = !in_inline;
                }
                SyntaxKind::L_BRACKET if depth == 0 && !in_inline && prev_abuts_command => {
                    brackets += 1
                }
                SyntaxKind::R_BRACKET if depth == 0 && !in_inline => {
                    if brackets == 0 {
                        return true;
                    }
                    brackets -= 1;
                }
                SyntaxKind::CONTROL_SYMBOL if matches!(t.text.as_str(), "\\]" | "\\)") => {
                    return false;
                }
                SyntaxKind::CONTROL_WORD
                    if matches!(t.text.as_str(), BEGIN_CMD | END_CMD)
                        && self.env_name_follows(idx) =>
                {
                    return false;
                }
                SyntaxKind::CONTROL_WORD | SyntaxKind::CONTROL_SYMBOL => abuts_command = true,
                _ => {}
            }
            newlines = 0;
        }
        false
    }

    /// True if the `[` at token index `open` is closed by a `]` before a token
    /// that would make [`Self::optional`] bail in text mode. `[`/`]` are not
    /// real grouping in TeX, and macro code tests for and re-emits lone
    /// brackets (`\@ifnextchar [\@xmpar\@ympar`, `\def\@xfloat#1[#2]{…}`
    /// re-implementations — issue #60) at least as often as prose writes real
    /// optionals, so — like the `$` shape gate ([`Self::dollar_closes`]) — a
    /// bracket attaches only when it *reads* as an argument: its closer must be
    /// reachable. Mirrors `optional`'s bail anchors (an unbalanced `}`,
    /// `\begin`/`\end` outside a definition body, a paragraph break, EOF). A
    /// `]` counts only outside `{…}` nesting (matching how `optional` consumes
    /// whole groups via `element`) and only past the `]`s owed to intervening
    /// *command-abutting* `[`s, exactly as in
    /// [`Self::bracket_closes_before_math_end`] (issue #55). A gated bracket
    /// stays an ordinary token with **no diagnostic**: in code the shape is
    /// routine, so it is not statically an error. Does not consume.
    fn bracket_closes_in_text(&self, open: usize) -> bool {
        let mut depth = 0usize;
        let mut brackets = 0usize;
        let mut newlines = 0;
        let mut abuts_command = false;
        for (off, t) in self.tokens[open + 1..].iter().enumerate() {
            let idx = open + 1 + off;
            let prev_abuts_command = abuts_command;
            abuts_command = false;
            match t.kind {
                SyntaxKind::NEWLINE => {
                    newlines += 1;
                    if newlines >= 2 {
                        return false;
                    }
                    continue;
                }
                SyntaxKind::WHITESPACE | SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD => continue,
                SyntaxKind::L_BRACE if !self.plain_braces.contains(&idx) => depth += 1,
                SyntaxKind::R_BRACE if !self.plain_braces.contains(&idx) => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                SyntaxKind::L_BRACKET if depth == 0 && prev_abuts_command => brackets += 1,
                SyntaxKind::R_BRACKET if depth == 0 => {
                    if brackets == 0 {
                        return true;
                    }
                    brackets -= 1;
                }
                SyntaxKind::CONTROL_WORD
                    if !self.in_macro_code(idx)
                        && matches!(t.text.as_str(), BEGIN_CMD | END_CMD)
                        && self.env_name_follows(idx) =>
                {
                    return false;
                }
                SyntaxKind::CONTROL_WORD | SyntaxKind::CONTROL_SYMBOL => abuts_command = true,
                _ => {}
            }
            newlines = 0;
        }
        false
    }

    /// Whether a paragraph break seen by the math shape gates
    /// ([`Self::dollar_closes`], [`Self::delim_math_closes`]) ends the math.
    ///
    /// Only at the math body's own level. Both gates must mirror the parse
    /// they guard, and `dollar_math`/`delim_math` test `at_paragraph_break`
    /// only between top-level atoms: once the body descends into a `{…}`
    /// group ([`Self::math_group`]) or a nested environment
    /// ([`Self::environment`]), blank lines are ordinary body trivia and the
    /// math runs on. Scanning them as blockers made the gate stricter than
    /// the parse, so a display equation built out of `tikzpicture` cells
    /// (`\[ \begin{array}… \begin{tikzpicture}<blank line>… \]`, issue #70)
    /// lost its math node and reported its own `\]` as unmatched.
    fn paragraph_break_blocks(depth: usize, envs: usize) -> bool {
        depth == 0 && envs == 0
    }

    /// True if the `$` (or `$$`) opener at token index `open` is closed by a
    /// matching delimiter before a token that would end the math. `$`/`$$` are
    /// data in macro code at least as often as they are math delimiters (a
    /// tabular preamble's `>{$}`, an expl3 token list's `{ $ }`, catcode
    /// comparisons in `\def` bodies), so — like `[…]` attachment (issue #43) —
    /// a dollar opens math only when it *reads* as math: a closer must be
    /// reachable. Mirrors [`Self::dollar_math`]'s recovery anchors (an
    /// unbalanced `}`, an `\end` not owed to an intervening `\begin`, a
    /// paragraph break, EOF, the macrocode chunk end). A closing `$` counts
    /// only outside `{…}` nesting — [`Self::math_group`] consumes a nested
    /// dollar as an ordinary atom, never as the closer — and for `$$` a lone
    /// `$` is skipped exactly as `dollar_math` skips it (malformed but
    /// consumed). Likewise a paragraph break blocks only at the math body's
    /// own level ([`Self::paragraph_break_blocks`]). Inside a definition body
    /// `\begin`/`\end` are plain commands (issue #45), so neither anchors nor
    /// nests there. Does not consume.
    fn dollar_closes(&self, open: usize, display: bool) -> bool {
        let mut depth = 0usize;
        let mut envs = 0usize;
        let mut newlines = 0;
        let start = open + if display { 2 } else { 1 };
        let end = self
            .macrocode_end
            .unwrap_or(self.tokens.len())
            .min(self.tokens.len());
        let mut i = start;
        while i < end {
            let t = &self.tokens[i];
            match t.kind {
                SyntaxKind::NEWLINE => {
                    newlines += 1;
                    if newlines >= 2 && Self::paragraph_break_blocks(depth, envs) {
                        return false;
                    }
                    i += 1;
                    continue;
                }
                SyntaxKind::WHITESPACE | SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD => {
                    i += 1;
                    continue;
                }
                SyntaxKind::L_BRACE if !self.plain_braces.contains(&i) => depth += 1,
                SyntaxKind::R_BRACE if !self.plain_braces.contains(&i) => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                SyntaxKind::DOLLAR if depth == 0 => {
                    if !display
                        || self.tokens.get(i + 1).map(|t| t.kind) == Some(SyntaxKind::DOLLAR)
                    {
                        return true;
                    }
                }
                SyntaxKind::CONTROL_WORD if !self.in_macro_code(i) => {
                    if t.text.as_str() == BEGIN_CMD && self.env_name_follows(i) {
                        envs += 1;
                    } else if t.text.as_str() == END_CMD && self.env_name_follows(i) {
                        if envs == 0 {
                            return false;
                        }
                        envs -= 1;
                    }
                }
                _ => {}
            }
            newlines = 0;
            i += 1;
        }
        false
    }

    /// The delimited-math twin of [`Self::dollar_closes`]: `\[`/`\(` opens
    /// math only when its `\]`/`\)` is reachable. Macro code passes the
    /// delimiters around as data tokens — stacks-project feeds `\[` to a
    /// splitter (`\expandafter\@tempa\[\@nil`, issue #65) — so an opener with
    /// no reachable closer is an ordinary token, no math, **no diagnostic**
    /// (the shape is routine in code, so it is not statically an error; a
    /// likely-typo unclosed `\[` in prose is linter territory, exactly as for
    /// `$`). Same blockers as `dollar_closes`, mirroring
    /// [`Self::delim_math`]'s recovery anchors: an unbalanced `}`, an `\end`
    /// not owed to an intervening `\begin`, a paragraph break, the macrocode
    /// chunk end, EOF. The closer counts only outside `{…}` nesting, and a
    /// paragraph break blocks only at the math body's own level
    /// ([`Self::paragraph_break_blocks`]).
    fn delim_math_closes(&self, open: usize, closer: &str) -> bool {
        let mut depth = 0usize;
        let mut envs = 0usize;
        let mut newlines = 0;
        let end = self
            .macrocode_end
            .unwrap_or(self.tokens.len())
            .min(self.tokens.len());
        let mut i = open + 1;
        while i < end {
            let t = &self.tokens[i];
            match t.kind {
                SyntaxKind::NEWLINE => {
                    newlines += 1;
                    if newlines >= 2 && Self::paragraph_break_blocks(depth, envs) {
                        return false;
                    }
                    i += 1;
                    continue;
                }
                SyntaxKind::WHITESPACE | SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD => {
                    i += 1;
                    continue;
                }
                SyntaxKind::L_BRACE if !self.plain_braces.contains(&i) => depth += 1,
                SyntaxKind::R_BRACE if !self.plain_braces.contains(&i) => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                SyntaxKind::CONTROL_SYMBOL if depth == 0 && t.text.as_str() == closer => {
                    return true;
                }
                SyntaxKind::CONTROL_WORD if !self.in_macro_code(i) => {
                    if t.text.as_str() == BEGIN_CMD && self.env_name_follows(i) {
                        envs += 1;
                    } else if t.text.as_str() == END_CMD && self.env_name_follows(i) {
                        if envs == 0 {
                            return false;
                        }
                        envs -= 1;
                    }
                }
                _ => {}
            }
            newlines = 0;
            i += 1;
        }
        false
    }

    /// The `\left…\right` twin of [`Self::delim_math_closes`]: whether the
    /// `\left` at token index `open` has a matching `\right` reachable before a
    /// token that would end its body. `\left`/`\right` pair by *count* (nested
    /// pairs recurse in [`Self::left_right`]), so — unlike `$`/`\[` which are
    /// often data in code — an unclosed `\left` is genuinely malformed math, but
    /// it is still a *likely-typo* the linter should flag, never a parser error
    /// that blocks the whole file for the formatter (issue #77's
    /// `\left(1 …) …\left(…\right)` and `\left\bra …` with no `\right`). So it
    /// gets the same shape gate as `\[`: a `\left` whose `\right` is unreachable
    /// stays an ordinary command, **no diagnostic**. Mirrors [`Self::left_right`]'s
    /// recovery anchors — an unbalanced `}`, a closing `$`/`\]`/`\)`, an `\end`
    /// not owed to an intervening `\begin`, a paragraph break, EOF — with `\right`
    /// and the anchors counting only at the `\left`'s own brace/env/pair level.
    /// Does not consume.
    fn left_right_closes(&self, open: usize) -> bool {
        #[derive(PartialEq)]
        enum Ctx {
            Brace,
            Env,
            Left,
        }
        let mut stack: Vec<Ctx> = Vec::new();
        let mut newlines = 0;
        let end = self
            .macrocode_end
            .unwrap_or(self.tokens.len())
            .min(self.tokens.len());
        let mut i = open + 1;
        while i < end {
            let t = &self.tokens[i];
            // A brace group parses as [`Self::math_group`]: only its own braces
            // steer nesting; every other token inside it is ordinary content
            // (a stray `\right`/`\end` there is not our closer).
            if stack.last() == Some(&Ctx::Brace) {
                match t.kind {
                    SyntaxKind::L_BRACE if !self.plain_braces.contains(&i) => {
                        stack.push(Ctx::Brace)
                    }
                    SyntaxKind::R_BRACE if !self.plain_braces.contains(&i) => {
                        stack.pop();
                    }
                    _ => {}
                }
                i += 1;
                continue;
            }
            match t.kind {
                SyntaxKind::NEWLINE => {
                    newlines += 1;
                    // A paragraph break ends the body only at its own level
                    // (mirrors [`Self::left_right`]); inside a nested env/pair it
                    // is ordinary trivia.
                    if newlines >= 2 && stack.is_empty() {
                        return false;
                    }
                    i += 1;
                    continue;
                }
                SyntaxKind::WHITESPACE | SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD => {
                    i += 1;
                    continue;
                }
                SyntaxKind::L_BRACE if !self.plain_braces.contains(&i) => stack.push(Ctx::Brace),
                SyntaxKind::R_BRACE if !self.plain_braces.contains(&i) => return false,
                SyntaxKind::DOLLAR => return false,
                SyntaxKind::CONTROL_SYMBOL if matches!(t.text.as_str(), "\\]" | "\\)") => {
                    return false;
                }
                // `\left`/`\right` are catcode-neutral math structure that pair by
                // *count* no matter what — [`Self::left_right`] consumes them
                // unconditionally — so they must be counted even inside macro code.
                // A `.dtx` `macrocode` chunk (which sets `in_def_body`) or a `\def`
                // body is exactly where package math like `$\left#2\right#4$`
                // (delarray.dtx) or `$\left(…\right)$` (ltmath.dtx's `\bordermatrix`)
                // lives; gating these on `!in_macro_code` left the `\right`
                // invisible to the scan, so the pair never opened and the closer
                // reported a spurious "`\right` without matching `\left`" that
                // blocked the whole file for the formatter (issue #95).
                SyntaxKind::CONTROL_WORD if t.text.as_str() == LEFT_CMD => stack.push(Ctx::Left),
                SyntaxKind::CONTROL_WORD if t.text.as_str() == RIGHT_CMD => match stack.last() {
                    None => return true,
                    Some(Ctx::Left) => {
                        stack.pop();
                    }
                    _ => return false,
                },
                // `\begin`/`\end` are plain, non-pairing commands in macro code, so
                // they stay gated (`AGENTS.md` decision #1).
                SyntaxKind::CONTROL_WORD if !self.in_macro_code(i) => match t.text.as_str() {
                    BEGIN_CMD if self.env_name_follows(i) => stack.push(Ctx::Env),
                    END_CMD if self.env_name_follows(i) => match stack.last() {
                        Some(Ctx::Env) => {
                            stack.pop();
                        }
                        _ => return false,
                    },
                    _ => {}
                },
                _ => {}
            }
            newlines = 0;
            i += 1;
        }
        false
    }

    /// Inline `$ … $` or display `$$ … $$` math. The body's atoms are wrapped in
    /// a `MATH` node (the delimiters stay direct children of the math node); the
    /// atoms themselves are parsed in math mode (see [`Self::math_element`]).
    /// Entry is gated by [`Self::dollar_closes`]: the caller has already
    /// verified a closer is reachable, so the unclosed-math recovery paths
    /// below fire only for shapes the gate scan cannot see (they remain as
    /// belt-and-braces recovery, never the expected path).
    fn dollar_math(&mut self) {
        let display = self.nth_kind(1) == Some(SyntaxKind::DOLLAR);
        let (kind, label) = if display {
            (SyntaxKind::DISPLAY_MATH, "$$")
        } else {
            (SyntaxKind::INLINE_MATH, "$")
        };
        let opener = (
            self.starts[self.pos],
            self.starts[self.pos + if display { 2 } else { 1 }],
        );
        self.open(kind);
        self.bump(); // $
        if display {
            self.bump(); // second $
        }
        self.open(SyntaxKind::MATH);
        self.math_depth += 1;
        self.math_dollar.push(true);
        loop {
            match self.kind() {
                None => {
                    self.error_at(opener, format!("unclosed `{label}`"));
                    break;
                }
                // `}` and `\end` are recovery anchors: `$`-math cannot span a
                // group or environment boundary, so a `}` here closes the
                // enclosing group (a math subgroup would have entered via `{`)
                // and a `\end` belongs to an enclosing environment. Leave the
                // token for the caller and report the unclosed math.
                Some(SyntaxKind::R_BRACE) => {
                    self.error_at(opener, format!("unclosed `{label}`"));
                    break;
                }
                Some(SyntaxKind::CONTROL_WORD)
                    if self.at_command(END_CMD) && self.env_name_follows(self.pos) =>
                {
                    self.error_at(opener, format!("unclosed `{label}`"));
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
                        // Faithful to TeX: a blank line is a `\par`, and `\par`
                        // in math mode is "Missing $ inserted" — even inside an
                        // alignment cell (#35). Name the cause so the opener
                        // span isn't read as a bogus report.
                        self.error_at(
                            opener,
                            format!("unclosed `{label}` (a blank line ends math)"),
                        );
                        break;
                    }
                    self.math_element();
                }
            }
        }
        self.math_depth -= 1;
        self.math_dollar.pop();
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
        let opener_span = self.token_span(self.pos);
        self.open(kind);
        self.bump(); // \[ or \(
        self.open(SyntaxKind::MATH);
        self.math_depth += 1;
        self.math_dollar.push(false);
        loop {
            match self.kind() {
                None => {
                    self.error_at(opener_span, format!("unclosed `{opener}`"));
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
                    self.error_at(opener_span, format!("unclosed `{opener}`"));
                    break;
                }
                Some(SyntaxKind::CONTROL_WORD)
                    if self.at_command(END_CMD) && self.env_name_follows(self.pos) =>
                {
                    self.error_at(opener_span, format!("unclosed `{opener}`"));
                    break;
                }
                _ => {
                    if self.at_paragraph_break() {
                        // Same rationale as in `dollar_math`: `\par` ends math.
                        self.error_at(
                            opener_span,
                            format!("unclosed `{opener}` (a blank line ends math)"),
                        );
                        break;
                    }
                    self.math_element();
                }
            }
        }
        self.math_depth -= 1;
        self.math_dollar.pop();
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
            Some(
                SyntaxKind::WHITESPACE
                | SyntaxKind::NEWLINE
                | SyntaxKind::COMMENT
                | SyntaxKind::DOC_MARGIN
                | SyntaxKind::GUARD,
            ) => self.bump(),
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
        // A math `WORD` glued around operators (`a+2*1`) splits into separate
        // operand/operator atoms (`AGENTS.md`, decision #3). Only the trailing
        // piece is the scriptable base, so `a+2*1^5` binds `^5` to `1` (matching
        // TeX); the leading pieces are flat sibling atoms of the math body. This
        // is a byte-range split of the WORD's text, not a re-lex — see
        // [`split_math_word`].
        if self.kind() == Some(SyntaxKind::WORD)
            && let Some(pieces) = split_math_word(self.text())
        {
            let idx = self.pos;
            let (last, lead) = pieces.split_last().expect("split yields >= 2 pieces");
            for &(start, end) in lead {
                self.events.push(Event::SubTok { idx, start, end });
            }
            let checkpoint = self.events.len();
            self.events.push(Event::SubTok {
                idx,
                start: last.0,
                end: last.1,
            });
            self.pos += 1; // the whole WORD is consumed by its pieces
            self.math_scripts(checkpoint);
            return;
        }
        let checkpoint = self.events.len();
        self.math_atom();
        self.math_scripts(checkpoint);
    }

    /// Attach any `^`/`_` scripts that follow the base atom emitted since
    /// `checkpoint`, retro-splicing a `SCRIPTED` wrapper in front of it (the
    /// event-stream analog of rust-analyzer's `precede`, done locally without
    /// touching the event layer). No script → the base stays a bare atom.
    fn math_scripts(&mut self, checkpoint: usize) {
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
        // `CommentMode::Stop`: a comment ends the line, so it stops the scan (and
        // is reported as the next meaningful token, which is not a script), rather
        // than being skipped as it is elsewhere. A blank line ends the math.
        let s = self.scan_trivia(self.pos, CommentMode::Stop);
        !s.saw_blank_line
            && matches!(
                s.next_kind,
                Some(SyntaxKind::CARET | SyntaxKind::UNDERSCORE)
            )
    }

    /// A single base atom: a `{…}` group (parsed in math mode), a command with
    /// its greedily-attached arguments, an environment, a `\\` line break, or one
    /// ordinary token. Always consumes ≥1 token when the cursor is at content.
    fn math_atom(&mut self) {
        match self.kind() {
            Some(SyntaxKind::L_BRACE) => self.math_group(),
            Some(SyntaxKind::CONTROL_WORD) => {
                // Same definition-body/expl3-region and brace-less gates as
                // [`Self::element`] (issues #45/#60).
                if !self.in_macro_code(self.pos)
                    && self.at_command(BEGIN_CMD)
                    && self.env_name_follows(self.pos)
                {
                    self.environment();
                } else if !self.in_macro_code(self.pos)
                    && self.at_command(END_CMD)
                    && self.env_name_follows(self.pos)
                {
                    self.stray_end();
                } else if self.at_command(LEFT_CMD) && self.left_right_closes(self.pos) {
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
            Some(SyntaxKind::CONTROL_WORD) => {
                self.at_command(END_CMD) && self.env_name_follows(self.pos)
            }
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
        let opener = self.token_span(self.pos);
        self.open(SyntaxKind::GROUP);
        self.bump(); // {
        self.group_depth += 1;
        self.group_opens.push(self.pos - 1);
        loop {
            match self.kind() {
                None => {
                    self.error_at(opener, "unclosed `{`");
                    break;
                }
                Some(SyntaxKind::R_BRACE) => {
                    self.bump();
                    break;
                }
                _ => self.math_element(),
            }
        }
        self.group_depth -= 1;
        self.group_opens.pop();
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
        let opener = self.token_span(self.pos);
        self.open(SyntaxKind::LEFT_RIGHT);
        self.bump(); // \left
        self.math_delim(LEFT_CMD);
        self.open(SyntaxKind::MATH);
        loop {
            match self.kind() {
                None => {
                    self.error_at(opener, "unclosed `\\left`");
                    break;
                }
                Some(SyntaxKind::CONTROL_WORD) if self.at_command(RIGHT_CMD) => break,
                // Enclosing-scope closers: `\left … \right` cannot span a group,
                // math, or environment boundary, so hand the token back.
                Some(SyntaxKind::R_BRACE | SyntaxKind::DOLLAR) => {
                    self.error_at(opener, "unclosed `\\left`");
                    break;
                }
                Some(SyntaxKind::CONTROL_SYMBOL) if matches!(self.text(), "\\]" | "\\)") => {
                    self.error_at(opener, "unclosed `\\left`");
                    break;
                }
                Some(SyntaxKind::CONTROL_WORD)
                    if self.at_command(END_CMD) && self.env_name_follows(self.pos) =>
                {
                    self.error_at(opener, "unclosed `\\left`");
                    break;
                }
                _ => {
                    if self.at_paragraph_break() {
                        self.error_at(opener, "unclosed `\\left`");
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
                (self.at_command(END_CMD) && self.env_name_follows(self.pos))
                    || self.at_command(LEFT_CMD)
                    || self.at_command(RIGHT_CMD)
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

    /// The environment twin of [`Self::delim_math_closes`]: whether the
    /// `\begin` at `open` is cut short by the closing brace of a group it sits
    /// *inside*, with no `\end` of its own reachable first.
    ///
    /// Brace groups are catcode-level structure while `\begin`/`\end` are only
    /// macros, so a `}` closing a group opened before the `\begin` always wins —
    /// the environment cannot span it. Package code leans on this constantly:
    /// the two halves sit in sibling groups
    /// (`\newcolumntype{w}[2]{>{\begin{lrbox}…}c<{\end{lrbox}…}}`, array.sty),
    /// in sibling macros (`\newcommand\BeginExample{…\begin{VerbatimOut}…}`
    /// paired with `\EndExample`, rotex.tex), or the `\begin` is prose in a
    /// message argument that never runs as structure
    /// (`\PackageError{amstex}{\string\begin{split} is not allowed…}`,
    /// amstex.sty — all issue #71). In each the `\begin` is an ordinary token:
    /// it opens no `ENVIRONMENT` and draws **no diagnostic**, the same shape
    /// gate `\[` already gets from [`Self::delim_math_closes`]. Without it the
    /// environment swallows the `}` and cascades into unmatched-brace noise
    /// that fails the whole file for the formatter.
    ///
    /// Only the *group boundary* suppresses the environment. A `\begin` that
    /// merely runs out of file still opens one, so the unclosed-environment
    /// diagnostic keeps firing on a genuinely forgotten `\end`. A `\end` of
    /// another name terminates the scan too, leaving the existing mismatch
    /// recovery in [`Self::finish_environment`] untouched. Does not consume.
    fn environment_escapes_group(&self, open: usize) -> bool {
        // Only a group the `\begin` is *actually* inside can cut it short. At
        // the outer level there is no such brace, and a later unbalanced `}`
        // is somebody else's business — notably a `.dtx` doc-line
        // `\begin{macro}`, whose intervening `macrocode` chunks split
        // definitions across braces on purpose ([`Self::plain_braces`], only
        // populated once that chunk is entered). Without this guard the scan
        // reads those as its own boundary and unnests the whole doc layer.
        if self.group_depth == 0 {
            return false;
        }
        // `.dtx` doc-margin lines are exempt, exactly as they are from the
        // expl3 carve-out ([`Self::expl_toggles`]): `\begin{macro}` and friends
        // are the *documentation* layer and must keep pairing across the
        // macrocode chunks between them. Those bodies routinely span code that
        // leaves a brace open on purpose — a `\iffalse}\fi` editor-balance
        // hack, a `` \char`} `` constant, a catcode-swapped region — which
        // strands `group_depth` above zero for the rest of the file and would
        // otherwise unnest the whole doc layer behind it. (A paragraph-break
        // bound cannot stand in here: a blank `.dtx` doc line is still a `%`
        // margin, so it never reads as a `\par`.)
        //
        // The exemption is about *stranded* braces, so it lifts when the
        // enclosing group opened on a doc-margin line too: that `{` is the
        // documentation layer's own, locally visible, and the `\begin` really is
        // inside it. `% \def\deflist#1{\begin{list}…}` paired with
        // `% \def\enddeflist{\end{list}}` (theorem.dtx, issue #71) is the split
        // environment definition the gate exists for, merely written as doc
        // prose.
        if self.doc_margin_exempt(open) {
            return false;
        }
        let mut depth = 0usize;
        let mut envs = 0usize;
        let end = self
            .macrocode_end
            .unwrap_or(self.tokens.len())
            .min(self.tokens.len());
        let mut i = open + 1;
        while i < end {
            let t = &self.tokens[i];
            match t.kind {
                // The `{name}` group of this very `\begin` nests and unnests
                // here, so the scan resumes at the environment's own level.
                SyntaxKind::L_BRACE if !self.plain_braces.contains(&i) => depth += 1,
                SyntaxKind::R_BRACE if !self.plain_braces.contains(&i) => {
                    if depth == 0 {
                        return true;
                    }
                    depth -= 1;
                }
                SyntaxKind::CONTROL_WORD if depth == 0 && !self.in_macro_code(i) => {
                    if t.text.as_str() == BEGIN_CMD && self.env_name_follows(i) {
                        envs += 1;
                    } else if t.text.as_str() == END_CMD && self.env_name_follows(i) {
                        if envs == 0 {
                            return false;
                        }
                        envs -= 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        false
    }

    /// The conditional twin of [`Self::delim_math_closes`]: whether the live
    /// opener at token `open` ([`Self::conditional_openers`]) has its own `\fi`
    /// reachable before a token that would end it.
    ///
    /// `\if…\else…\or…\fi` is not a construct the surface syntax guarantees. A
    /// `\fi` is routinely assembled elsewhere — `\def\stopit{\fi}`,
    /// `\expandafter\fi`, an `\iffalse…\fi` used to comment a region out — so
    /// after subtracting the `\newif` and `\ifthenelse` families 268 of 6205
    /// corpus files still have unbalanced opener/`\fi` counts. An opener that
    /// does not pair is therefore ordinary macro code: it stays a plain
    /// `COMMAND` with **no diagnostic**, exactly as a gated `$`/`\[`/`\begin`
    /// does (`AGENTS.md` decision #1). Does not consume.
    ///
    /// The anchors mirror the math gates — an unbalanced `}`, an `\end` not owed
    /// to an intervening `\begin`, a paragraph break, the macrocode chunk end,
    /// EOF — with two deliberate differences from
    /// [`Self::environment_escapes_group`]:
    ///
    /// - **EOF does not pair.** The environment gate keeps a run-out-of-file
    ///   `\begin` so `finish_environment` can still report an unclosed
    ///   environment. A conditional has no diagnostic to preserve, and an
    ///   unpaired `\if` is routine, so running out of file just demotes.
    /// - **No `.dtx` doc-margin exemption.** That exemption exists so the
    ///   documentation layer keeps pairing `\begin{macro}` across the macrocode
    ///   chunks between them. A conditional has no such split-across-chunks
    ///   story, and bounding the scan at `macrocode_end` is precisely what makes
    ///   the `\iffalse}\fi` editor-balance hack demote instead of swallowing the
    ///   chunk.
    ///
    /// A paragraph break anchors at the construct's own level only
    /// ([`Self::paragraph_break_blocks`]), so the ~11% of corpus conditionals
    /// that span a blank line demote and keep their pre-node layout. That keeps
    /// `CONDITIONAL` a within-paragraph construct: it can never straddle a
    /// `PARAGRAPH` boundary, so no paragraph nests inside one.
    ///
    /// The closer must be reachable at the opener's **own level of every nesting
    /// the parse itself recognizes** — braces, environments, and math alike — not
    /// just braces. A token scan that counts a `\fi` the parse will consume inside
    /// some other construct promises a pairing the walk cannot honor, and
    /// [`Self::conditional`] then runs past it looking for a closer that is gone:
    /// `ltboxes.dtx`'s `\else\@pboxswtrue $\vcenter \fi\fi\fi … \if@pboxsw
    /// \m@th$\fi` puts all three `\fi`s inside a `$…$`, and the construct ran over
    /// 160 lines and every `macrocode` chunk in between. Hence the `envs == 0`
    /// requirement on the closer and the math anchor.
    ///
    /// The guarantee this buys is **one-directional, and that is the direction
    /// that matters**: the walk never runs *past* the index returned here (it is
    /// bounded by it outright). The walk may still stop *earlier*, because this
    /// scan counts nested openers by name while the walk re-gates each one and may
    /// demote it — and a demoted opener's `\fi` is then a closer the walk reaches
    /// first. `\ifA \begin{center} \ifB \end{center} \fi \fi` is the shape: the
    /// scan counts `\ifB` as nested and picks the second `\fi`, while the walk
    /// demotes `\ifB` (whose own scan meets an unowed `\end`) and closes at the
    /// first, leaving the second a plain `COMMAND`. Lossless, and the node is still
    /// well formed — but it is why [`crate::ast::Conditional::closer`] is fallible
    /// and why nothing downstream may assume the two indices agree
    /// (`conditional_walk_may_close_before_the_located_fi`, `tests/parser.rs`).
    ///
    /// **Cost.** One forward scan per live opener, so O(n·openers) worst case. Every
    /// ordinary anchor cuts it short, which is why real conditional-heavy packages
    /// show no measurable change (`biblatex.sty`, `latexrelease.sty`, `memoir.cls`
    /// all within noise of the pre-node parser). The shape that does not cut short is
    /// thousands of *top-level* openers with no blank line, no unbalanced brace, and
    /// no reachable `\fi`: 8000 such lines cost ~2s against ~0.07s. No real corpus
    /// file hits it; see `TODO.md` for the linear rewrite if one ever does.
    fn conditional_closer(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut envs = 0usize;
        let mut nested = 0usize;
        let mut newlines = 0;
        let end = self
            .macrocode_end
            .unwrap_or(self.tokens.len())
            .min(self.tokens.len());
        let mut i = open + 1;
        while i < end {
            let t = &self.tokens[i];
            match t.kind {
                SyntaxKind::NEWLINE => {
                    newlines += 1;
                    if newlines >= 2 && Self::paragraph_break_blocks(depth, envs) {
                        return None;
                    }
                    i += 1;
                    continue;
                }
                SyntaxKind::WHITESPACE | SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD => {
                    i += 1;
                    continue;
                }
                // Math swallows whatever it spans, and this scan does not model
                // the `$`/`\[`/`\(` shape gates that decide whether a delimiter
                // opens any. Rather than re-derive them, refuse: a conditional
                // whose closer sits behind a math delimiter stays a plain command.
                // A conservative false negative, per the parser's standing
                // preference for them.
                SyntaxKind::DOLLAR if depth == 0 => return None,
                SyntaxKind::CONTROL_SYMBOL
                    if depth == 0 && matches!(t.text.as_str(), "\\[" | "\\(") =>
                {
                    return None;
                }
                SyntaxKind::L_BRACE if !self.plain_braces.contains(&i) => depth += 1,
                SyntaxKind::R_BRACE if !self.plain_braces.contains(&i) => {
                    if depth == 0 {
                        // A `}` closing a group opened before the opener always
                        // wins: braces are catcode structure while `\if`/`\fi`
                        // are only macros. At the outer level there is no such
                        // group, so a stray `}` is somebody else's business and
                        // the scan carries on (the `\begin` gate's `group_depth`
                        // guard, transcribed).
                        if self.group_depth > 0 {
                            return None;
                        }
                    } else {
                        depth -= 1;
                    }
                }
                SyntaxKind::CONTROL_WORD if depth == 0 => {
                    if self.conditional_openers.contains(&i) {
                        nested += 1;
                    } else if self.conditional_flow_at(i) == Some(conditional::FlowWord::Fi) {
                        // `envs == 0` for the same reason as `depth == 0`: a `\fi`
                        // inside an environment the conditional opened is consumed
                        // by that environment's body, so it is not a closer the
                        // walk can reach.
                        if nested == 0 {
                            return (envs == 0).then_some(i);
                        }
                        nested -= 1;
                    } else if !self.in_macro_code(i) {
                        // In a definition body or an expl3 region `\begin`/`\end`
                        // are plain commands that need not pair, so neither
                        // anchors nor nests there (issues #45/#60).
                        if t.text.as_str() == BEGIN_CMD && self.env_name_follows(i) {
                            // A `macrocode` chunk is a hard boundary in both
                            // directions: docstrip is line-oriented, so the code
                            // layer and the documentation layer around it are
                            // different files as far as TeX is concerned. Nothing
                            // is gained by pairing across one, and a `.dtx` doc
                            // layer that does — `%<latexrelease>` guarded
                            // `\if#1b\vbox \else…` blocks in `ltboxes.dtx` — runs
                            // the construct over every chunk in between, stranding
                            // the cursor past `macrocode_end` for every
                            // chunk-bounded scan downstream. (The other direction
                            // is already bounded: a conditional *inside* a chunk
                            // scans only to `macrocode_end`.)
                            if peek_begin_name(self.tokens, i)
                                .is_some_and(|n| matches!(n.as_str(), "macrocode" | "macrocode*"))
                            {
                                return None;
                            }
                            envs += 1;
                        } else if t.text.as_str() == END_CMD && self.env_name_follows(i) {
                            if envs == 0 {
                                return None;
                            }
                            envs -= 1;
                        }
                    }
                }
                _ => {}
            }
            newlines = 0;
            i += 1;
        }
        None
    }

    /// `\if… … \else … \or … \fi`, for the closer [`Self::conditional_closer`]
    /// located at token index `closer`.
    ///
    /// The shape is a run of `CONDITIONAL_BRANCH` nodes closed by the `\fi` as
    /// the last child, mirroring `ENVIRONMENT > BEGIN … END`. The opener and its
    /// *test* ride the first branch rather than a head node of their own: the
    /// test's extent is not statically resolvable — `\ifnum\radius>5` scans
    /// ⟨number⟩⟨rel⟩⟨number⟩ by TeX's own scanner, `\ifx` takes two tokens, a
    /// `\newif`-defined `\if@foo` takes none — and inventing a boundary there
    /// would be the macro expansion the parser does not do.
    ///
    /// Every later branch *starts with* its divider, so a consumer finds the
    /// boundaries positionally and never by matching the name `\else`.
    fn conditional(&mut self, closer: usize) {
        self.open(SyntaxKind::CONDITIONAL);
        self.open(SyntaxKind::CONDITIONAL_BRANCH);
        self.command(); // the opener, with its usual greedy attachment
        loop {
            // The walk is bounded by the closer the gate located, so a nested
            // construct that consumes more than the token scan predicted can
            // never carry the conditional past it. Without the bound an
            // overrunning construct strands the cursor past `macrocode_end`, and
            // every chunk-bounded scan downstream then slices backwards.
            if self.pos >= closer || self.at_block_end(Block::Macrocode) {
                break;
            }
            match self.conditional_flow_at(self.pos) {
                Some(conditional::FlowWord::Fi) => break,
                Some(conditional::FlowWord::Else | conditional::FlowWord::Or) => {
                    self.close(); // CONDITIONAL_BRANCH
                    self.open(SyntaxKind::CONDITIONAL_BRANCH);
                    self.flow_command();
                    continue;
                }
                None => {}
            }
            // Leading comment-bind, as in [`Self::parse_block`]: an own-line `%`
            // run immediately before a documentable construct attaches *leading*
            // into it. A divider is not documentable, so a comment run before one
            // floats (the trivia falls through to `element` a token at a time and
            // the loop reaches the divider above).
            if let Some((comment_start, construct_pos, _)) = self.binding_run(self.pos)
                && self.conditional_flow_at(construct_pos).is_none()
            {
                while self.pos < comment_start {
                    self.bump();
                }
                let checkpoint = self.events.len();
                self.open(SyntaxKind::DOC_COMMENT);
                while self.pos < construct_pos {
                    self.bump();
                }
                self.close();
                let construct_start = self.events.len();
                self.element();
                if let Event::Start(kind) = self.events[construct_start] {
                    self.events.remove(construct_start);
                    self.events.insert(checkpoint, Event::Start(kind));
                }
                continue;
            }
            self.element();
        }
        self.close(); // CONDITIONAL_BRANCH
        if self.conditional_flow_at(self.pos) == Some(conditional::FlowWord::Fi) {
            self.flow_command();
        }
        self.close(); // CONDITIONAL
    }

    /// A conditional divider or closer as a bare `COMMAND`, with **no** argument
    /// attachment.
    ///
    /// Inside a `CONDITIONAL` an `\else`/`\or`/`\fi` is a structural delimiter,
    /// parsed like `\end`, so a following group is the next branch's first
    /// element rather than the divider's argument. Greedy attachment is the
    /// text-pure default precisely because the text carries no arity protocol
    /// (`AGENTS.md` decision #8); here position in the construct *is* that
    /// protocol, and it is a static fact, so this is a sanctioned deviation on
    /// the same footing as the starred-variant fold.
    fn flow_command(&mut self) {
        self.open(SyntaxKind::COMMAND);
        self.bump();
        self.close();
    }

    /// `\begin{name} … \end{name}`, with environment-mismatch recovery.
    fn environment(&mut self) {
        self.open(SyntaxKind::ENVIRONMENT);

        let begin_pos = self.pos;
        let begin_start = self.starts[self.pos];
        self.open(SyntaxKind::BEGIN);
        self.bump(); // \begin
        let name = self.name_group();
        // Span of the opener `\begin{name}` (before any trailing arguments), so
        // an unclosed environment points back at the `\begin`, not at EOF.
        let opener = (begin_start, self.starts[self.pos]);
        // A frame-lexed `.dtx` macrocode `\begin` (it rides a `DOC_MARGIN`, so
        // this never fires on a stray `\begin{macrocode}` in a plain document).
        // The frame line holds nothing but the name (`lex_macrocode_frame`), so
        // it takes *no* arguments — the next line's `{` is body macro code, not
        // an attachment — and the body routes to `macrocode_body` below.
        let macrocode_frame = name
            .as_deref()
            .is_some_and(|n| matches!(n, "macrocode" | "macrocode*"))
            && self.frame_margin_before(begin_pos);
        // `\begin{tabular}{ll}`, `[options]`, etc. A curated math environment's
        // body starts right after its `\begin`, so only a directly-abutting
        // `[t]`-style optional attaches; a detached bracket is body content
        // (`\begin{align}` + newline + `[\partial_\mu V]_1`, issue #43).
        let bracket = if name.as_deref().is_some_and(is_math_environment) {
            BracketPolicy::Tight
        } else {
            BracketPolicy::Greedy
        };
        if !macrocode_frame {
            self.attach_arguments(bracket);
        }
        self.close(); // BEGIN

        if let Some(open) = name.as_deref() {
            self.open_envs.push(open.to_owned());
        }
        if name
            .as_deref()
            .is_some_and(|n| self.ctx.is_verbatim_environment(n))
        {
            self.verbatim_body(name.as_deref().expect("verbatim name"));
        } else if name.as_deref().is_some_and(is_math_environment) {
            self.math_environment_body();
        } else if macrocode_frame {
            // A frame-lexed macrocode body is macro code, not document
            // structure (see `macrocode_frame` above).
            self.macrocode_body(name.as_deref().expect("macrocode name"));
        } else {
            self.parse_block(Block::Environment);
        }
        if name.is_some() {
            self.open_envs.pop();
        }
        self.finish_environment(&name, opener);
    }

    /// True if the token at `pos` sits on a `.dtx` frame line: walking back over
    /// inline whitespace, the preceding token is a `DOC_MARGIN`. Margins never
    /// occur *inside* a macrocode body (code lines own their `%`), so this
    /// fingerprint distinguishes the frame `\begin`/`\end{macrocode}` from any
    /// look-alike in the code. Pinned by
    /// `macrocode_frame_margins_sit_where_the_formatter_expects` (`tests/dtx.rs`).
    fn frame_margin_before(&self, pos: usize) -> bool {
        let mut i = pos;
        while i > 0 {
            i -= 1;
            match self.tokens[i].kind {
                SyntaxKind::WHITESPACE => continue,
                SyntaxKind::DOC_MARGIN => return true,
                _ => return false,
            }
        }
        false
    }

    /// The body of a `.dtx` `macrocode`/`macrocode*` environment: macro code
    /// whose one true terminator is the frame line (`%    \end{macrocode}`),
    /// a line-oriented docstrip fact. TeX places no balance requirements on the
    /// chunk — a definition regularly opens a brace in one chunk and closes it
    /// several chunks later, and kernel code uses the `\end` primitive — so,
    /// like the definition bodies of decision #1 (issues #45/#55):
    /// - `\begin`/`\end` inside parse as plain commands ([`Self::in_def_body`]),
    /// - chunk-unmatched braces are plain tokens with no diagnostics
    ///   ([`Self::plain_braces`]; matched pairs still parse as `GROUP`s),
    /// - a `[` attaches as an optional only when it closes inside the chunk.
    ///
    /// The terminator is pre-scanned here (the first `\end` on a margin whose
    /// name matches — [`Self::frame_margin_before`]) and parsing stops
    /// positionally at it ([`Block::Macrocode`]); [`Self::finish_environment`]
    /// then consumes and name-checks it as usual. Nesting is impossible (the
    /// lexer never opens a frame inside a body), but state is saved/restored
    /// anyway so a malformed tree cannot leak it.
    fn macrocode_body(&mut self, name: &str) {
        let mut end = self.tokens.len();
        for i in self.pos..self.tokens.len() {
            if self.tokens[i].kind == SyntaxKind::CONTROL_WORD
                && self.tokens[i].text == END_CMD
                && self.frame_margin_before(i)
                && peek_end_name(self.tokens, i).as_deref() == Some(name)
            {
                end = i;
                break;
            }
        }

        let saved_plain = std::mem::take(&mut self.plain_braces);
        let saved_end = self.macrocode_end;
        let saved_def = self.in_def_body;

        let mut open_stack = Vec::new();
        for i in self.pos..end {
            match self.tokens[i].kind {
                SyntaxKind::L_BRACE => open_stack.push(i),
                SyntaxKind::R_BRACE if open_stack.pop().is_none() => {
                    self.plain_braces.insert(i);
                }
                _ => {}
            }
        }
        self.plain_braces.extend(open_stack);
        self.macrocode_end = Some(end);
        self.in_def_body = true;

        self.parse_block(Block::Macrocode);

        self.plain_braces = saved_plain;
        self.macrocode_end = saved_end;
        self.in_def_body = saved_def;
    }

    /// Consume the matching `\end`, or recover. `parse_block` / `verbatim_body`
    /// leave the cursor at a `\end` or at EOF.
    fn finish_environment(&mut self, name: &Option<String>, opener: (usize, usize)) {
        match self.kind() {
            None => {
                self.error_at(
                    opener,
                    format!("unclosed environment `{}`", name.as_deref().unwrap_or("")),
                );
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
                    self.error_at(
                        opener,
                        format!(
                            "unclosed environment `{}` (found `\\end{{{}}}`)",
                            name.as_deref().unwrap_or(""),
                            end_name.as_deref().unwrap_or("")
                        ),
                    );
                }
            }
        }
        self.close(); // ENVIRONMENT
    }

    /// The body of a named math environment (`equation`, `align`, `gather`, …): its
    /// atoms wrapped in a `MATH` node and parsed in math mode, exactly as `\[…\]`
    /// (see [`Self::delim_math`]) — so `^`/`_` build `SCRIPTED` nodes, the operator
    /// split fires, and `\left…\right` pair. Routed here for environments the
    /// built-in signature DB flags `math` ([`is_math_environment`]).
    ///
    /// The terminator is the matching `\end` (or EOF), read via [`Self::at_block_end`]
    /// just like [`Self::parse_block`]; [`Self::finish_environment`] then consumes and
    /// name-checks it. Unlike `$`-math (where a `\end` is an *unclosed*-recovery
    /// anchor), `\end` is the normal, expected terminator here. A blank line inside the
    /// body stays trivia within the `MATH` node — no paragraph split — so losslessness
    /// holds. Progress is guaranteed: [`Self::math_element`] bumps trivia or descends
    /// into [`Self::math_scripted`], whose atom parser always consumes a token.
    fn math_environment_body(&mut self) {
        self.open(SyntaxKind::MATH);
        self.math_depth += 1;
        self.math_dollar.push(false);
        while !self.at_block_end(Block::Environment) {
            self.math_element();
        }
        self.math_depth -= 1;
        self.math_dollar.pop();
        self.close(); // MATH
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

/// Split a math `WORD`'s text at operator boundaries into `[start, end)` byte
/// ranges covering the whole text, or `None` when it holds no operator (a single
/// operand run needs no split). Operators are catcode-12 "other" characters that
/// glue into `WORD` (`a+2*1`); isolating them lets the math-aware parser and
/// formatter treat them as atoms (spacing, line breaks) without a catcode-carrying
/// lexer. The rule:
///
/// - `+ - * /`: each is its own single-char piece (so `2*-1` → `2`,`*`,`-`,`1`,
///   letting the formatter read a leading `-`/`+` as unary).
/// - `= < >`: a maximal run coalesces into one piece (`<=`, `>=`, `==` stay
///   together), but never merges with an adjacent sign (`=-` → `=`,`-`).
/// - anything else: a maximal operand run.
///
/// The pieces concatenate back to the input, preserving losslessness.
fn split_math_word(text: &str) -> Option<Vec<(usize, usize)>> {
    #[derive(PartialEq, Clone, Copy)]
    enum Cls {
        Operand,
        /// `+ - * /`: always its own single-char piece.
        Sign,
        /// `= < >`: coalescing relation run.
        Rel,
    }
    let classify = |c: char| match c {
        '+' | '-' | '*' | '/' => Cls::Sign,
        '=' | '<' | '>' => Cls::Rel,
        _ => Cls::Operand,
    };
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut prev: Option<Cls> = None;
    for (i, c) in text.char_indices() {
        let cls = classify(c);
        // Break before this char when the class changed, or when either side is a
        // sign (each sign stands alone). A same-class run (operand/operand or
        // rel/rel) coalesces.
        let boundary = prev.is_some_and(|p| p != cls || cls == Cls::Sign);
        if boundary {
            pieces.push((start, i));
            start = i;
        }
        prev = Some(cls);
    }
    pieces.push((start, text.len()));
    (pieces.len() >= 2).then_some(pieces)
}

/// Read the environment name from a `\begin{…}` at `begin_pos` without consuming.
/// Identical in shape to [`peek_end_name`] (skip the control word and trivia, then
/// read the `{name}` group); named separately for call-site clarity.
fn peek_begin_name(tokens: &[Token], begin_pos: usize) -> Option<String> {
    peek_end_name(tokens, begin_pos)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::lexer::lex;

    /// The stuck-loop guard aborts once `PARSER_STEP_LIMIT` peeks accrue with no
    /// cursor advance, turning a hypothetical non-advancing loop into a loud
    /// panic instead of a hang.
    #[test]
    fn step_guard_trips_when_wedged() {
        let tokens = lex("x");
        let ctx = VerbCtx::default();
        let p = Parser::new(&tokens, &ctx);
        // Park one tick short of the ceiling with the cursor pinned, so no reset
        // fires on the next peeks.
        p.last_step_pos.set(p.pos);
        p.steps.set(PARSER_STEP_LIMIT - 1);
        p.step(); // reaches the ceiling exactly — still allowed
        let wedged = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| p.step()));
        assert!(wedged.is_err(), "the guard must abort a non-advancing loop");
    }

    /// A real cursor advance resets the budget, so an arbitrarily long *advancing*
    /// parse never trips the guard.
    #[test]
    fn step_budget_resets_on_cursor_progress() {
        let tokens = lex("xx");
        let ctx = VerbCtx::default();
        let mut p = Parser::new(&tokens, &ctx);
        p.last_step_pos.set(p.pos);
        p.steps.set(PARSER_STEP_LIMIT - 1);
        // Advance the cursor as a real consume would; the next peek must reset.
        p.pos += 1;
        p.step();
        assert_eq!(p.steps.get(), 1, "progress should reset the peek budget");
    }
}
