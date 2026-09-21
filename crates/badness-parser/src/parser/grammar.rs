//! Recursive-descent grammar for LaTeX surface syntax.
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

mod expl3;
mod facts;
mod gates;
mod math;
mod prescan;
mod trivia;

use std::borrow::Cow;

use crate::parser::conditional;
use crate::parser::core::SyntaxError;
use crate::parser::events::{Event, Marker, extend_back};
use crate::parser::lexer::{ParseCtx, Token};
use crate::semantic::signature::{
    ArgKind, ArgSpec, ArgumentDomain, builtin, match_arg_slot, match_verbatim_arg_slot,
};
use crate::syntax::SyntaxKind;
use facts::{
    BracketPolicy, is_big_delimiter_command, is_command_definition_command,
    is_definition_body_command,
};
use gates::GateBatch;
use prescan::PreScan;
use smol_str::SmolStr;
use trivia::CommentMode;

/// Kept at this path for parser and formatter callers.
pub use facts::is_def_prefix_command;

/// Re-exported for [`crate::parser::reparse`]'s token tier, whose guard has to
/// name the same predicate the walk does rather than drift into a copy of it.
pub(crate) use facts::is_definition_body_command as reads_definition_body;

pub(crate) const BEGIN_CMD: &str = "\\begin";
pub(crate) const END_CMD: &str = "\\end";
const LEFT_CMD: &str = "\\left";
const RIGHT_CMD: &str = "\\right";

/// Maximum number of cursor peeks without consuming a token. This catches
/// non-advancing loops even on malformed input and resets after every advance.
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

/// Parse a token stream into parser events and a list of syntax errors.
pub(crate) fn parse(tokens: &[Token], ctx: &ParseCtx) -> (Vec<Event>, Vec<SyntaxError>) {
    let mut p = Parser::new(tokens, ctx);
    p.document();
    debug_assert_balanced(&p.events);
    (p.events, p.errors)
}

/// Check the entire event stream as well as individual marker completions.
/// Retroactive wrappers and comment binding move starts, so the final nesting
/// must still balance before rowan receives the events.
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

fn builtin_command_args(head: &str) -> Option<&'static [ArgSpec]> {
    head.strip_prefix('\\')
        .and_then(|name| builtin().command(name))
        .map(|sig| sig.args.as_ref())
}

struct Parser<'t> {
    tokens: &'t [Token],
    /// User-defined verbatim constructs, consulted to route a verbatim environment to
    /// its raw-body branch (its body is already one `VERBATIM_BODY` token from the
    /// lexer; the grammar must not try to parse it structurally).
    ctx: &'t ParseCtx,
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
    /// One entry per lexically enclosing math body (`$…$`, `\[…\]`, `\(…\)`,
    /// math environments), innermost last, holding that level's *flavor*:
    /// `true` for a `$…$`/`$$…$$` (dollar-delimited) level, `false` for
    /// `\[…\]`/`\(…\)` and math environments.
    ///
    /// Its **depth** ([`Self::in_math`]) is read where the `math` routing flags
    /// threaded through the grammar are not enough: it *persists* into the
    /// text-mode body of an unknown environment nested inside math
    /// (`\[ … \begin{myaligned} … \]`), where the grammar can't verify the body
    /// is math but the enclosing delimiters are a static lexical fact, and
    /// optional-argument attachment uses it to treat a spaced `[` as content
    /// (see [`Self::attach_arguments`]).
    ///
    /// Its **last entry** ([`Self::enclosing_math_is_dollar`]) is read by
    /// [`Self::bracket_closes_before_math_end`]: inside dollar math a `$` is the
    /// closer (a boundary), whereas inside `\[…\]` a `$` opens a genuine nested
    /// inline region (`\inferrule*[right=$\Pi$-eq]`), so the two must be scanned
    /// differently.
    ///
    /// Not every `MATH` node pushes here: [`Self::left_right`] opens one without
    /// a push, because a `\left…\right` always sits inside math already.
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
    /// last ([`Self::group`] / [`Self::math_group`]).
    ///
    /// Its **depth** ([`Self::in_group`]) is the `\end`-side twin of
    /// [`Self::environment_escapes_group`]: an `\end` reached inside a group has
    /// its `\begin` outside it, so it is macro code rather than a stray
    /// (`\StopEventually{\end{document}}`, issue #71).
    ///
    /// Its **last entry** is read by [`Self::doc_margin_exempt`] to tell a group
    /// the `.dtx` *documentation* layer opened itself from one stranded by the
    /// code layer.
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
    /// Bumped on every mutation of [`Self::plain_braces`], so a gate batch can
    /// key on the set without cloning it (`WalkKey`).
    plain_braces_version: u32,
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
    /// The `.dtx` doc-margin lines, as `(first DOC_MARGIN on the line, the line's
    /// terminating NEWLINE)`, ascending and disjoint. Pre-scanned once so
    /// [`Self::on_doc_margin_line`] is a binary search rather than a walk back to
    /// the previous newline. **Empty for every non-`.dtx` file** — only that lexer
    /// mode emits `DOC_MARGIN` — so the predicate costs nothing there.
    doc_margin_lines: Vec<(usize, usize)>,
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
    /// Dollar tokens in a `\def`-family parameter text. TeX reads these as
    /// literal delimiters before the replacement body, so they cannot open math.
    /// Pre-scanned once because a false opener can otherwise pair with a later
    /// definition's delimiter and swallow both bodies (issue #129).
    def_parameter_dollars: std::collections::HashSet<usize>,
    /// Token indices of *environment-alias openers* — bare control words whose
    /// definition body is exactly `\begin{X}` — mapped to the target environment
    /// `X` (issue #109). Pre-scanned in [`Self::new`] for the same reason as
    /// [`Self::conditional_openers`]: the definee filter is a running state over
    /// the stream that the recursive walk cannot carry.
    ///
    /// That filter is load-bearing, not defensive, and it counts *slots* rather
    /// than testing a single word ([`definition_name_slots`]). [`Self::command`]
    /// sets `in_def_body` after a `\def`-family head only when the definee is a
    /// `CONTROL_SYMBOL`, so in `\def\bea{\begin{eqnarray}}` the definee `\bea`
    /// reaches [`Self::element`] as an ordinary sibling command at brace depth 0
    /// with `in_macro_code` false. Unfiltered, the dispatch fires on it, the scan
    /// finds `\def\eea`'s definee at the same depth, and the two *definition lines*
    /// pair into an `ENVIRONMENT` — lossless and silent, but layout is destroyed.
    /// `\let\oldbea\bea` is the same failure one slot over: the *source* operand
    /// is a mention, not a call, and left live it pairs with a later `\eea` and
    /// swallows the prose in between. The braced `\newcommand{\bea}{…}` form is
    /// covered by `in_def_body` instead. Expl3 regions are excluded outright, as
    /// for conditionals.
    alias_openers: std::collections::HashMap<usize, SmolStr>,
    /// The closer mirror of [`Self::alias_openers`] (`\end{X}` bodies).
    alias_closers: std::collections::HashMap<usize, SmolStr>,
    /// Token indices of *literal* `\end{X}` closers whose `X` some alias opens,
    /// mapped to that `X` (issue #117).
    ///
    /// A begin alias stands in for `\begin{X}`, so what closes it is whatever
    /// closes an `X` — a closer alias, and equally the `\end{X}` an author
    /// writes out. Kept a separate map from [`Self::alias_closers`] because the
    /// two are consumed differently: this one's token is a `\end` carrying a
    /// `NAME_GROUP`, so [`Self::alias_environment`] emits the same two-token
    /// `END` a spelled-out environment does, and [`Self::finish_environment`]'s
    /// mirror arm must not mistake one for the other.
    ///
    /// The index is an *over*-approximation, per the pre-scan's standing rule:
    /// it is built from `peek_end_name`, which is looser than the walk's
    /// [`Self::env_end_at`], so the gate re-tests membership against that.
    literal_alias_closers: std::collections::HashMap<usize, SmolStr>,
    /// The largest index in [`Self::alias_closers`] or
    /// [`Self::literal_alias_closers`], or `None` when the file has neither.
    /// [`Self::alias_closer`] can only ever return an index in one of those two
    /// maps, so this bounds its forward scan — which is what keeps a file of
    /// openers that never pair linear instead of quadratic.
    last_alias_closer: Option<usize>,
    /// The `last_alias_closer` treatment, generalized (`TODO.md`, container
    /// stack C0): each shape gate succeeds only at one closer token shape, so
    /// truncating its scan at the last occurrence of that shape is
    /// verdict-preserving — past it, every path is a refusal, whether by anchor
    /// or by running out of range — and a file with none refuses without
    /// scanning at all. Recording may *over*-approximate (a `\fi` inside an
    /// expl3 region, a `\right` inside a brace group): a bound only needs to be
    /// at or past the last index that could ever succeed.
    ///
    /// This one is the last `]`, bounding [`Self::bracket_closes_in_text`] and
    /// [`Self::bracket_closes_before_math_end`].
    last_r_bracket: Option<usize>,
    /// Last `\]` — bounds [`Self::delim_math_closes`] for a `\[` opener.
    last_display_math_closer: Option<usize>,
    /// Last `\)` — bounds [`Self::delim_math_closes`] for a `\(` opener.
    last_inline_math_closer: Option<usize>,
    /// Last `\right` — bounds [`Self::left_right_closes`].
    last_right: Option<usize>,
    /// Last `}` — bounds [`Self::environment_escapes_group`], whose only `true`
    /// is a `}` at depth 0. Rarely effective (every `\begin{…}` opener carries a
    /// `}` in its own name group, so this index usually sits near EOF), but
    /// sound and free; the gate's residual quadratic shape is recorded in
    /// `TODO.md` (container stack, C2).
    last_r_brace: Option<usize>,
    /// Last `\fi`-flavored flow word — bounds [`Self::conditional_closer`].
    last_fi: Option<usize>,
    /// Last `$` — bounds [`Self::dollar_closes`]. The weakest of these bounds by
    /// construction, since this gate's closer is its opener's own token kind: in
    /// the adversarial shape (a file of `$` openers) the last one *is* an
    /// opener, so the bound cuts nothing. Recorded anyway, because the driver's
    /// contract is that every gate names the last index that could settle an
    /// entry, and a file whose `$`s all sit before a long tail does get the cut.
    last_dollar: Option<usize>,
    /// The most recent `ConditionalGate` batch ([`Self::gate_batch`]),
    /// memoized with the walk state its scan read. A lookup hits only when
    /// that key matches the walk's current state *and* the queried opener was
    /// settled by the batch; anything else re-batches from the queried opener.
    /// One slot is all the reuse there is: [`Self::element`] queries each
    /// opener once, in ascending order, under a stable state between
    /// re-batches. `RefCell` because the gate is `&self` — the pattern the
    /// alias gate's pre-batch memo used, with a map of settled openers where
    /// that one kept a single verdict.
    conditional_batch: std::cell::RefCell<Option<GateBatch>>,
    /// Tokens visited by the shape-gate scans, summed over the whole parse. A
    /// measurement hook for the linearity regression tests in this file's
    /// `mod tests` — never a budget (`TODO.md` rejects scan budgets as
    /// hard-coded special cases). `Cell` because the gates take `&self`, and
    /// `cfg(test)` because the counter is pure measurement: ticking it once per
    /// scanned token is a real cost in the driver's hottest loop, paid for
    /// nothing in a release build.
    #[cfg(test)]
    scan_work: std::cell::Cell<usize>,
    /// The `EnvGate` twin of [`Self::conditional_batch`]. Its verdicts are
    /// the *scan's* alone: [`Self::environment_escapes_group`]'s per-opener
    /// pre-checks (the group depth, the `.dtx` doc-margin exemption) are applied
    /// at query time, so a batch entry never carries them.
    env_batch: std::cell::RefCell<Option<GateBatch>>,
    /// The `AliasGate` twin of [`Self::conditional_batch`]. Both
    /// [`Self::starts_block_env`] and the [`Self::element`] dispatch ask about
    /// the same opener at the same cursor position, so even before the batch
    /// settled its neighbors this slot was load-bearing: without it every
    /// opener paid for its walk twice.
    alias_batch: std::cell::RefCell<Option<GateBatch>>,
    /// The `LeftRightGate` twin of [`Self::conditional_batch`]. Its openers
    /// nest densely — a `\left` whose `\right` the walk cannot reach is retried
    /// as a plain command and every `\left` after it asked in turn — so the
    /// batch is what keeps a run of them from being quadratic.
    left_right_batch: std::cell::RefCell<Option<GateBatch>>,
    /// The `TextBracketGate` twin of [`Self::conditional_batch`]. A `[` the
    /// gate refuses stays an ordinary token the walk steps over, and the next
    /// command-abutting `[` is asked in turn, so a run of them re-scanned per
    /// opener before the batch.
    text_bracket_batch: std::cell::RefCell<Option<GateBatch>>,
    /// The paragraph-permissive text-bracket twin. It stays separate because
    /// paragraph anchoring is part of a batch's policy, not its walk-state key.
    long_text_bracket_batch: std::cell::RefCell<Option<GateBatch>>,
    /// The `MathBracketGate` twin, keyed like the others — including on the
    /// enclosing math's flavor, which this gate alone reads (`WalkKey`).
    math_bracket_batch: std::cell::RefCell<Option<GateBatch>>,
    /// The arity-directed expl3 scan's matching-brace table
    /// ([`expl3::BraceMatches`]). Not a gate batch — it settles *pairings*
    /// rather than verdicts — but the same trade for the same reason: nested
    /// call sites ask about spans their enclosing ones already covered.
    brace_matches: std::cell::RefCell<Option<expl3::BraceMatches>>,
    /// Token index of the alias closer bounding the environment body currently
    /// being parsed, if any. Saved and restored around the body in
    /// [`Self::alias_environment`]. An alias environment has no `\end{…}` to stop
    /// at, so this positional bound is what terminates it — read by
    /// [`Self::at_block_end`], [`Self::trivia_run_is_separator`], and
    /// [`Self::binding_run`].
    alias_end: Option<usize>,
    /// Whether the environment body currently being parsed is a curated
    /// `statementBody` body ([`ParseCtx::is_statement_environment`], the
    /// TikZ/pgf picture family), so [`Self::parse_block`]'s run loop wraps each
    /// run up to a top-level `;`-carrying `WORD` in a `STATEMENT` node. Saved
    /// and restored around every environment body — a nested non-statement
    /// environment turns it off for its own body, a nested `scope` turns it
    /// back on — and never inherited by a `group()`/`conditional()` element
    /// loop, which is what keeps recognition to the body's own top level.
    in_statement_body: bool,
}

impl<'t> Parser<'t> {
    fn new(tokens: &'t [Token], ctx: &'t ParseCtx) -> Self {
        let pre = PreScan::run(tokens, ctx);
        Self {
            tokens,
            ctx,
            starts: pre.starts,
            pos: 0,
            events: Vec::new(),
            steps: std::cell::Cell::new(0),
            last_step_pos: std::cell::Cell::new(0),
            errors: Vec::new(),
            math_dollar: Vec::new(),
            in_def_body: false,
            demoted_envs: std::collections::HashSet::new(),
            open_envs: Vec::new(),
            group_opens: Vec::new(),
            macrocode_end: None,
            plain_braces: std::collections::HashSet::new(),
            plain_braces_version: 0,
            expl_toggles: pre.expl_toggles,
            doc_margin_lines: pre.doc_margin_lines,
            conditional_openers: pre.conditional_openers,
            def_parameter_dollars: pre.def_parameter_dollars,
            last_alias_closer: pre
                .alias_closers
                .keys()
                .chain(pre.literal_alias_closers.keys())
                .copied()
                .max(),
            last_r_bracket: pre.last_r_bracket,
            last_display_math_closer: pre.last_display_math_closer,
            last_inline_math_closer: pre.last_inline_math_closer,
            last_right: pre.last_right,
            last_r_brace: pre.last_r_brace,
            last_fi: pre.last_fi,
            last_dollar: pre.last_dollar,
            conditional_batch: std::cell::RefCell::new(None),
            #[cfg(test)]
            scan_work: std::cell::Cell::new(0),
            alias_openers: pre.alias_openers,
            alias_closers: pre.alias_closers,
            literal_alias_closers: pre.literal_alias_closers,
            alias_batch: std::cell::RefCell::new(None),
            env_batch: std::cell::RefCell::new(None),
            left_right_batch: std::cell::RefCell::new(None),
            text_bracket_batch: std::cell::RefCell::new(None),
            long_text_bracket_batch: std::cell::RefCell::new(None),
            math_bracket_batch: std::cell::RefCell::new(None),
            brace_matches: std::cell::RefCell::new(None),
            alias_end: None,
            in_statement_body: false,
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
    /// opens its physical line).
    ///
    /// Answered from the pre-scanned [`Self::doc_margin_lines`], the same posture
    /// as [`Self::in_expl_region`]. This used to walk back to the preceding
    /// `NEWLINE`, justified by doc lines being short and [`Self::in_macro_code`]
    /// reaching it only for a token already inside an expl3 region — but
    /// [`Self::doc_margin_exempt`] calls it *unconditionally*, and that runs from
    /// [`Self::environment_escapes_group`] and its `\end` mirror for every
    /// `\begin`/`\end` in the file. On a document written as one long line the
    /// walk is `O(line length)` per opener, so the pair was `O(N x line length)`.
    fn on_doc_margin_line(&self, idx: usize) -> bool {
        // The candidate is the last line whose margin opens strictly before
        // `idx`; the lines are disjoint, so no earlier one can reach. It reaches
        // when `idx` is still on it — at or before its terminating newline, which
        // is where the backward scan would have stopped.
        let n = self.doc_margin_lines.partition_point(|&(m, _)| m < idx);
        n > 0 && self.doc_margin_lines[n - 1].1 >= idx
    }

    /// Whether token `idx` is covered by the `.dtx` doc-margin exemption from the
    /// brace-group gates ([`Self::environment_escapes_group`] and its `\end`-side
    /// mirror): it sits on a documentation line *and* every group open around it
    /// was opened by the code layer.
    ///
    /// The exemption exists for braces the *code* layer stranded — a
    /// `\iffalse{\fi` editor-balance hack, a `` \char`{ `` constant, a
    /// catcode-swapped region — which keep a group open for the rest of the
    /// file and would otherwise unnest the whole doc layer behind them. A
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
            self.demoted_envs.contains(name.as_ref())
                && !self.open_envs.iter().any(|open| open == name.as_ref())
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
    /// one. See the [`Self::math_dollar`] field.
    fn in_math(&self) -> bool {
        !self.math_dollar.is_empty()
    }

    /// True when at least one brace group is open around the cursor. See the
    /// [`Self::group_opens`] field.
    fn in_group(&self) -> bool {
        !self.group_opens.is_empty()
    }

    // --- cursor primitives -------------------------------------------------

    /// Tick the stuck-loop guard, called from every lookahead primitive. Resets
    /// the budget whenever the cursor has advanced since the last tick (real
    /// progress — via `bump` or the math-word slicing path, both of which move
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

    /// The cursor is on a `\begin` that reads as an environment delimiter
    /// ([`Self::env_name_follows`]).
    fn at_env_begin(&self) -> bool {
        self.at_command(BEGIN_CMD) && self.env_name_follows(self.pos)
    }

    /// The `\end` twin of [`Self::at_env_begin`].
    fn at_env_end(&self) -> bool {
        self.at_command(END_CMD) && self.env_name_follows(self.pos)
    }

    /// [`Self::at_env_begin`] at an explicit index.
    ///
    /// Deliberately *not* routed through [`Self::at_command`]: that ticks the
    /// stuck-loop budget ([`Self::step`]), which is a peek counter for the walk,
    /// and this form is called once per token from inside the gate scans — where
    /// a visit is progress, not a non-advancing peek. Indexes directly, as every
    /// call site did before.
    fn env_begin_at(&self, idx: usize) -> bool {
        self.tokens[idx].text == BEGIN_CMD && self.env_name_follows(idx)
    }

    /// The `\end` twin of [`Self::env_begin_at`], with the same no-tick rule.
    fn env_end_at(&self, idx: usize) -> bool {
        self.tokens[idx].text == END_CMD && self.env_name_follows(idx)
    }

    // --- event emission ----------------------------------------------------

    fn bump(&mut self) {
        debug_assert!(!self.at_end(), "bump past end of input");
        self.events.push(Event::Tok(self.pos));
        self.pos += 1;
    }

    fn open(&mut self, kind: SyntaxKind) -> Marker {
        Marker::open(&mut self.events, kind)
    }

    fn close(&mut self, marker: Marker) {
        marker.complete(&mut self.events);
    }

    /// Open a wrapper after parsing its first children, when its kind is known.
    fn precede(&mut self, checkpoint: usize, kind: SyntaxKind) -> Marker {
        Marker::precede(&mut self.events, checkpoint, kind)
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

    // --- grammar -----------------------------------------------------------

    fn document(&mut self) {
        self.parse_block(Block::Document);
    }

    /// Whether the construct at token `idx` opens a *block* environment — one
    /// [`parse_block`](Self::parse_block) leaves bare rather than wrapping in a
    /// `PARAGRAPH`. Block-ness is read from the built-in signature DB
    /// ([`ParseCtx::is_block_environment`]), never from a name list here.
    ///
    /// Covers both spellings, so an alias formats like the environment it stands
    /// for: `\bea … \eea` must not be wrapped in a `PARAGRAPH` when the identical
    /// `\begin{eqnarray} … \end{eqnarray}` is not. The alias arm re-runs the shape
    /// gate, since a demoted opener is a plain command and must keep its paragraph.
    fn starts_block_env(&self, idx: usize) -> bool {
        if self.tokens.get(idx).is_some_and(|t| t.text == BEGIN_CMD) {
            return peek_begin_name(self.tokens, idx)
                .as_deref()
                .is_some_and(|name| self.ctx.is_block_environment(name));
        }
        self.alias_openers.get(&idx).is_some_and(|target| {
            self.ctx.is_block_environment(target) && self.alias_closer(idx).is_some()
        })
    }

    /// Whether the construct at token `idx` will parse as a genuine
    /// `ENVIRONMENT` — a paired `\begin{…}` or a pairing alias opener. In a
    /// `statementBody` body this is a **statement boundary**: an environment is
    /// a sibling of the statements around it, never statement content, so the
    /// run loop abandons its pending `STATEMENT` checkpoint here (the elements
    /// before it stay unwrapped) and restarts after the environment.
    ///
    /// Mirrors [`Self::element`]'s dispatch exactly, gate verdicts included
    /// (memoized, so re-asking is cheap): a *demoted* `\begin` is a plain
    /// command there and stays statement content here — the same
    /// gate-mirrors-the-walk discipline every shape gate carries.
    fn statement_boundary(&self, idx: usize) -> bool {
        if self.in_macro_code(idx) {
            return false;
        }
        if self.env_begin_at(idx) {
            return !self.environment_escapes_group(idx);
        }
        self.alias_openers.contains_key(&idx) && self.alias_closer(idx).is_some()
    }

    /// Consume a leading comment-bind located by [`Self::binding_run`]: float
    /// the trivia before `comment_start`, group the bound `%` run into a
    /// `DOC_COMMENT` node, parse the construct at `construct_pos`, and extend
    /// the construct's own node back over the comments
    /// ([`extend_back`] — the construct self-opens, so its kind is only
    /// known afterwards).
    ///
    /// The bound run becomes a named node rather than bare leaves — the
    /// named-trivia enrichment `AGENTS.md` #9 reserved — so downstream
    /// (LSP/formatter) sees the doc comment as one unit.
    ///
    /// Shared by [`Self::parse_block`] and [`Self::conditional`], which differ
    /// only in what they check *before* calling (a conditional divider is not
    /// documentable) and what they track *after*.
    fn doc_comment_bind(&mut self, comment_start: usize, construct_pos: usize) {
        while self.pos < comment_start {
            self.bump();
        }
        let checkpoint = self.events.len();
        let comment = self.open(SyntaxKind::DOC_COMMENT);
        while self.pos < construct_pos {
            self.bump();
        }
        self.close(comment);
        let construct_start = self.events.len();
        self.element();
        extend_back(&mut self.events, checkpoint, construct_start);
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
            // the signature data (`ParseCtx::is_block_environment`).
            let checkpoint = self.events.len();
            let mut nontrivia_count = 0usize;
            let mut lone_block_env = false;
            // In a curated `statementBody` body, the pending `STATEMENT`'s
            // checkpoint. Set lazily at the run's (or the previous statement's)
            // first non-trivia element so inter-statement trivia floats outside
            // the node; dropped at run end, so a run that never reaches a `;`
            // stays plain paragraph content (recognition degrades silently,
            // like every gated construct).
            let mut stmt_checkpoint: Option<usize> = None;
            loop {
                if self.at_block_end(block) {
                    break;
                }
                if self.kind().is_some_and(Self::is_trivia) && self.trivia_run_is_separator(block) {
                    break;
                }
                // Leading comment-bind: an own-line `%` run immediately before a
                // documentable construct attaches *leading* into it (see
                // `doc_comment_bind`). Block-ness is peeked from the construct's
                // index, so it reads the same before or after the bind.
                if let Some((comment_start, construct_pos, _)) = self.binding_run(self.pos) {
                    let starts_block_env = self.starts_block_env(construct_pos);
                    if self.in_statement_body {
                        if self.statement_boundary(construct_pos) {
                            stmt_checkpoint = None;
                        } else {
                            // Float the trivia before the bound `%` run first
                            // (`doc_comment_bind` would otherwise consume it
                            // after the checkpoint), so a lazily opened
                            // statement starts at its `DOC_COMMENT`.
                            while self.pos < comment_start {
                                self.bump();
                            }
                            stmt_checkpoint.get_or_insert(self.events.len());
                        }
                    }
                    self.doc_comment_bind(comment_start, construct_pos);
                    nontrivia_count += 1;
                    lone_block_env = nontrivia_count == 1 && starts_block_env;
                    continue;
                }
                let is_nontrivia = !self.kind().is_some_and(Self::is_trivia);
                // Peek block-env status *before* consuming (the name is only
                // available while still on the `\begin`).
                let starts_block_env = self.starts_block_env(self.pos);
                // Statement bookkeeping, peeked before consuming for the same
                // reason: a genuine environment is a *sibling* of the statements
                // around it (the pending run is abandoned, unwrapped), and the
                // terminator test needs the `WORD` while the cursor is on it.
                let mut terminator = false;
                if self.in_statement_body && is_nontrivia {
                    if self.statement_boundary(self.pos) {
                        stmt_checkpoint = None;
                    } else {
                        stmt_checkpoint.get_or_insert(self.events.len());
                        terminator =
                            self.kind() == Some(SyntaxKind::WORD) && self.text().contains(';');
                    }
                }
                self.element();
                if terminator && let Some(cp) = stmt_checkpoint.take() {
                    let statement = self.precede(cp, SyntaxKind::STATEMENT);
                    self.close(statement);
                }
                if is_nontrivia {
                    nontrivia_count += 1;
                    lone_block_env = nontrivia_count == 1 && starts_block_env;
                }
            }
            if !lone_block_env {
                let paragraph = self.precede(checkpoint, SyntaxKind::PARAGRAPH);
                self.close(paragraph);
            }
        }
    }

    fn at_block_end(&self, block: Block) -> bool {
        self.at_end()
            || match block {
                Block::Document => false,
                Block::Environment => {
                    // An alias environment has no `\end{…}`: its body ends at the
                    // closer the gate located. Checked first because
                    // `math_environment_body` hardcodes `Block::Environment`, so
                    // this one bound terminates both the math and the prose body.
                    self.alias_end.is_some_and(|end| self.pos >= end)
                        || (self.at_env_end() && !self.end_orphans_a_demoted_begin(self.pos))
                        || self.at_alias_end_for_open_env()
                }
                // `>=` (not `==`): defensive against an element overshooting the
                // pre-scanned terminator, so the loop still stops.
                Block::Macrocode => self.macrocode_end.is_some_and(|end| self.pos >= end),
            }
    }

    /// Whether the cursor is on a *closer alias* for the environment innermost
    /// open here — the mirror of the literal closer [`Self::closer_target`]
    /// admits (issue #117).
    ///
    /// `\def\eeq{\end{equation}}` expands to `\end{equation}`, so it closes a
    /// spelled-out `\begin{equation}` just as it closes an alias-opened one. The
    /// alias-opened direction already stops at [`Self::alias_end`], the index its
    /// gate positively located; this arm is what a `\begin{…}` needs, which pairs
    /// by default and locates nothing.
    ///
    /// Deliberately *not* generalized past the innermost environment: an alias
    /// closer naming some outer environment is a plain command here, exactly as
    /// a mismatched `\end{…}` is left for the caller to unwind.
    fn alias_end_for_open_env(&self, idx: usize) -> bool {
        self.alias_closers.get(&idx).is_some_and(|target| {
            !self.in_macro_code(idx) && self.open_envs.last().is_some_and(|open| open == target)
        })
    }

    /// [`Self::alias_end_for_open_env`] at the cursor.
    fn at_alias_end_for_open_env(&self) -> bool {
        self.alias_end_for_open_env(self.pos)
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
                    && (self.env_end_at(s.next)
                            // The alias twin: the run reaches the located closer.
                            || self.alias_end.is_some_and(|end| s.next >= end)
                            // …or a closer alias for the environment open here,
                            // which is what an unlocated `\begin{…}` stops at.
                            || self.alias_end_for_open_env(s.next))
            }
            Some(_) => false,
        }
    }

    /// One element in text mode. Always consumes at least one token.
    fn element(&mut self) {
        let Some(k) = self.kind() else { return };
        match k {
            k if Self::is_trivia(k) => self.bump(),
            SyntaxKind::CONTROL_WORD => {
                // Inside a definition body or an expl3 region, `\begin`/`\end`
                // are plain commands: the two need not balance within one group
                // (issues #45/#60), so neither opens an environment nor is
                // stray. A brace-less `\begin`/`\end` is likewise a plain
                // command (`env_name_follows`).
                if !self.in_macro_code(self.pos) && self.at_env_begin() {
                    // Shape-gated like `\[`: an environment cannot outlive the
                    // brace group it opened in, so one whose `\end` is not
                    // reachable before that group closes is macro code — a
                    // plain command, no diagnostic (issue #71).
                    if self.environment_escapes_group(self.pos) {
                        if let Some(name) = peek_end_name(self.tokens, self.pos) {
                            self.demoted_envs.insert(name.into_owned());
                        }
                        self.command();
                    } else {
                        self.environment();
                    }
                } else if !self.in_macro_code(self.pos) && self.at_env_end() {
                    // The mirror case: reached inside a group, this `\end`'s
                    // `\begin` is outside it, so it is macro code rather than
                    // stray (`\StopEventually{\end{document}}`, issue #71).
                    if (self.in_group() && !self.doc_margin_exempt(self.pos))
                        || self.end_orphans_a_demoted_begin(self.pos)
                    {
                        self.command();
                    } else {
                        self.stray_end();
                    }
                } else if let Some((target, closer)) = (!self.in_macro_code(self.pos))
                    .then(|| {
                        let target = self.alias_openers.get(&self.pos)?.clone();
                        Some((target, self.alias_closer(self.pos)?))
                    })
                    .flatten()
                {
                    // A command whose definition body is exactly `\begin{X}`, whose
                    // partner is reachable: pair the two into an `ENVIRONMENT` of
                    // `X` (issue #109). Shape-gated like `\begin` and `\if`, and
                    // like them it demotes silently when the gate refuses.
                    self.alias_environment(&target, closer);
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
                if self.def_parameter_dollars.contains(&self.pos) {
                    self.bump();
                    return;
                }
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
        let builtin_args = builtin_command_args(self.text());
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
        let consumes_control_symbol_name =
            is_def_prefix_command(self.text()) || is_command_definition_command(self.text());
        // Arity-directed expl3 attachment (decision #8's sanctioned
        // deviation): resolve the head's argspec and scan the whole unit
        // *before* any event is emitted; the replay below consumes exactly
        // the plan, so the scan mirrors the walk by construction. A
        // colon-carrying head is never a def-prefix or definition-body name
        // (both sets are colonless), so the branches cannot overlap.
        let expl3_plan = self
            .expl3_arity_slots()
            .and_then(|slots| self.scan_expl3_unit(&slots));
        let command = self.open(SyntaxKind::COMMAND);
        self.bump(); // the control word
        // A command definer may take its name as the next unbraced control
        // sequence. Consume a control-symbol name here as a plain token so it
        // is never misparsed as live syntax (`\def\[{…}` and
        // `\DeclareRobustCommand\[{…}` are not math openers). The attached
        // replacement group is then a macro-code body: the stacks-project
        // redefinition opens `trivlist` in `\def\[`'s body and closes it in
        // `\def\]`'s (issue #65), the same no-balance fact as
        // `is_definition_body_command`.
        if consumes_control_symbol_name {
            let scan = self.scan_trivia(self.pos, CommentMode::Skip);
            if scan.next_kind == Some(SyntaxKind::CONTROL_SYMBOL) && !scan.saw_blank_line {
                self.skip_trivia();
                self.bump(); // the defined name
                self.in_def_body = true;
            }
        }
        match &expl3_plan {
            Some(plan) => self.attach_expl3_arguments(plan),
            None => self.attach_arguments(bracket, builtin_args),
        }
        self.in_def_body = saved;
        self.close(command);
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
        let line_break = self.open(SyntaxKind::LINE_BREAK);
        self.bump(); // \\
        if self.kind() == Some(SyntaxKind::WORD) && self.text() == "*" {
            self.bump(); // *
        }
        if self.kind() == Some(SyntaxKind::L_BRACKET) {
            self.optional(); // [length]
        }
        self.close(line_break);
    }

    /// Greedily attach trailing `{…}` / `[…]` argument groups to the currently
    /// open node, allowing intervening trivia but ordinarily stopping at a
    /// paragraph break. Shared by `\foo` commands and `\begin{env}` (see
    /// `AGENTS.md`, Core decision #8). Arity is unknown without the semantic
    /// layer.
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
    /// - **Across a paragraph, only in the tight `[…]{…}` shape.** Both
    ///   delimiter junctions must abut, and the bracket gate must locate the
    ///   `]` before structural recovery anchors. This admits long mixed
    ///   optional/mandatory slots without consulting signature arity, while a
    ///   standalone long `[…]` remains ordinary text.
    fn attach_arguments(&mut self, bracket: BracketPolicy, args: Option<&[ArgSpec]>) {
        let mut slot = 0usize;
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
                    let domain = args
                        .and_then(|args| match_arg_slot(args, &mut slot, ArgKind::Brace))
                        .map_or(ArgumentDomain::Unknown, |spec| spec.domain);
                    self.argument_group(domain);
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
                    let mut allow_paragraphs = false;
                    if !self.in_math()
                        && self.macrocode_end.is_none()
                        && !self.bracket_closes_in_text(scan.next)
                    {
                        let long_closer = (scan.next == self.pos)
                            .then(|| self.long_bracket_closer_in_text(scan.next))
                            .flatten();
                        let tight_mandatory_suffix = long_closer.is_some_and(|closer| {
                            self.tokens.get(closer + 1).is_some_and(|token| {
                                token.kind == SyntaxKind::L_BRACE
                                    && !self.plain_braces.contains(&(closer + 1))
                            })
                        });
                        if !tight_mandatory_suffix {
                            break;
                        }
                        allow_paragraphs = true;
                    }
                    self.skip_trivia();
                    let domain = args
                        .and_then(|args| match_arg_slot(args, &mut slot, ArgKind::Bracket))
                        .map_or(ArgumentDomain::Unknown, |spec| spec.domain);
                    self.argument_optional(domain, allow_paragraphs);
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
                    if let Some(args) = args
                        && self
                            .peek_meaningful_text()
                            .is_some_and(|text| text.starts_with('{'))
                    {
                        match_verbatim_arg_slot(args, &mut slot);
                    }
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
        self.argument_group(ArgumentDomain::Unknown);
    }

    fn argument_group(&mut self, domain: ArgumentDomain) {
        debug_assert_eq!(self.kind(), Some(SyntaxKind::L_BRACE));
        let opener = self.token_span(self.pos);
        let group = self.open(SyntaxKind::GROUP);
        self.bump(); // {
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
                _ => match domain {
                    ArgumentDomain::Math => self.math_element(),
                    ArgumentDomain::Text | ArgumentDomain::Unknown => self.element(),
                },
            }
        }
        self.group_opens.pop();
        self.close(group);
    }

    /// An optional-argument group `[ … ]`.
    ///
    /// `[` and `]` are not real grouping in TeX, so this is heuristic: it ends
    /// at the first `]`, and bails defensively (rather than swallowing the
    /// document) on a structural `}`, a `\begin`/`\end`, a paragraph break, or
    /// EOF. The shape-gated tight `[…]{…}` path may admit paragraph breaks. A
    /// chunk-unmatched macrocode `}` is an ordinary token.
    fn optional(&mut self) {
        self.argument_optional(ArgumentDomain::Unknown, false);
    }

    fn argument_optional(&mut self, domain: ArgumentDomain, allow_paragraphs: bool) {
        debug_assert_eq!(self.kind(), Some(SyntaxKind::L_BRACKET));
        let opener = self.token_span(self.pos);
        let optional = self.open(SyntaxKind::OPTIONAL);
        self.bump(); // [
        loop {
            match self.kind() {
                None => {
                    self.error_at(opener, "unclosed `[`");
                    break;
                }
                Some(SyntaxKind::R_BRACE) if !self.plain_braces.contains(&self.pos) => {
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
                        && (self.at_env_begin() || self.at_env_end()) =>
                {
                    self.error_at(opener, "unclosed `[`");
                    break;
                }
                _ => {
                    // The macrocode frame terminator is absolute: an optional
                    // still open there is abandoned, never consumes the frame.
                    if (!allow_paragraphs && self.at_paragraph_break_outside_guards())
                        || self.macrocode_end.is_some_and(|end| self.pos >= end)
                    {
                        self.error_at(opener, "unclosed `[`");
                        break;
                    }
                    match domain {
                        ArgumentDomain::Math => self.math_element(),
                        ArgumentDomain::Text | ArgumentDomain::Unknown => self.element(),
                    }
                }
            }
        }
        self.close(optional);
    }

    /// The environment the token at `idx` closes, under *either* spelling: a
    /// closer alias (`\eea`), or the literal `\end{X}` an alias-opened `X` pairs
    /// with (issue #117). `None` when it closes neither.
    ///
    /// The two maps stay separate ([`Self::literal_alias_closers`]) because the
    /// consumers differ; this is the one place that reads them as one. The
    /// literal arm re-tests [`Self::env_end_at`] because the pre-scan's index is
    /// built from the looser `peek_end_name` — a `\end` the walk would treat as
    /// a plain command must not become an `END` here, or the `NAME_GROUP`
    /// [`Self::alias_environment`] then asks for is not there.
    fn closer_target(&self, idx: usize) -> Option<&str> {
        if let Some(target) = self.alias_closers.get(&idx) {
            return Some(target.as_str());
        }
        let target = self.literal_alias_closers.get(&idx)?;
        self.env_end_at(idx).then_some(target.as_str())
    }

    /// Whether the closer at `idx` is spelled out as `\end{X}` rather than as a
    /// closer alias — the one thing the two [`Self::closer_target`] arms are
    /// consumed differently for.
    fn closer_is_literal(&self, idx: usize) -> bool {
        !self.alias_closers.contains_key(&idx) && self.literal_alias_closers.contains_key(&idx)
    }

    /// `\bea … \eea`: an environment opened by a bare control word, for the
    /// closer [`Self::alias_closer`] located at token index `closer` — which is
    /// either the closer alias or a literal `\end{X}` (issue #117).
    ///
    /// Emits the *same* `ENVIRONMENT > BEGIN … END` shape a spelled-out
    /// `\begin{X} … \end{X}` does, so every consumer downstream — the formatter's
    /// lowering, folding, the outline, [`crate::ast::Environment`] — works
    /// unchanged. The only difference is that `BEGIN` holds a bare
    /// `CONTROL_WORD` instead of `\begin` plus a `NAME_GROUP`, which is why
    /// [`crate::ast::Begin::name`] falls back to the head control word; a
    /// literally-closed `END` is byte-for-byte the ordinary one.
    ///
    /// No arguments are attached to either delimiter: the alias head consumes none
    /// (that is an admission rule of the scan, `semantic::define`), and attaching
    /// them from the *target's* signature would be arity-directed grouping from
    /// scanned data, which `AGENTS.md` decision #8 holds the line on.
    fn alias_environment(&mut self, target: &str, closer: usize) {
        let environment = self.open(SyntaxKind::ENVIRONMENT);
        let begin = self.open(SyntaxKind::BEGIN);
        self.bump(); // the opening control word
        self.close(begin);

        let saved = self.alias_end.replace(closer);
        self.open_envs.push(target.to_owned());
        // Body routing reads the *target* name through the same curated-data-only
        // predicates a spelled-out environment uses, so no behavior flag ever comes
        // from the alias itself.
        let saved_stmt = self.in_statement_body;
        self.in_statement_body = self.ctx.is_statement_environment(target);
        if self.ctx.is_verbatim_environment(target) {
            self.verbatim_body(target);
        } else if self.ctx.is_math_environment(target) {
            self.math_environment_body();
        } else {
            self.parse_block(Block::Environment);
        }
        self.in_statement_body = saved_stmt;
        self.open_envs.pop();
        self.alias_end = saved;

        // The walk is bounded by `closer`, so it normally stops exactly there. It
        // may stop earlier when a nested construct re-gates and closes first — the
        // same one-directional guarantee `conditional_closer` documents — in which
        // case the closer stays a plain command and this environment simply has no
        // `END`, exactly as an unclosed `\begin` does.
        if self.pos == closer {
            let end = self.open(SyntaxKind::END);
            self.bump(); // the closing control word
            // A literal closer is a `\end` carrying its name, so it emits the
            // same `END > CONTROL_WORD NAME_GROUP` a spelled-out environment
            // does (issue #117); an alias closer is the bare word alone.
            if self.closer_is_literal(closer) {
                self.name_group();
            }
            self.close(end);
        }
        self.close(environment);
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
        let conditional = self.open(SyntaxKind::CONDITIONAL);
        let mut branch = self.open(SyntaxKind::CONDITIONAL_BRANCH);
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
                    self.close(branch);
                    branch = self.open(SyntaxKind::CONDITIONAL_BRANCH);
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
                self.doc_comment_bind(comment_start, construct_pos);
                continue;
            }
            self.element();
        }
        self.close(branch);
        if self.conditional_flow_at(self.pos) == Some(conditional::FlowWord::Fi) {
            self.flow_command();
        }
        self.close(conditional);
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
        let command = self.open(SyntaxKind::COMMAND);
        self.bump();
        self.close(command);
    }

    /// `\begin{name} … \end{name}`, with environment-mismatch recovery.
    fn environment(&mut self) {
        let environment = self.open(SyntaxKind::ENVIRONMENT);

        let begin_pos = self.pos;
        let begin_start = self.starts[self.pos];
        let begin = self.open(SyntaxKind::BEGIN);
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
        let bracket = if name
            .as_deref()
            .is_some_and(|n| self.ctx.is_math_environment(n))
        {
            BracketPolicy::Tight
        } else {
            BracketPolicy::Greedy
        };
        if !macrocode_frame {
            let builtin_args = name
                .as_deref()
                .and_then(|name| builtin().environment(name))
                .map(|sig| sig.args.as_ref());
            self.attach_arguments(bracket, builtin_args);
        }
        self.close(begin);

        if let Some(open) = name.as_deref() {
            self.open_envs.push(open.to_owned());
        }
        // Statement-body routing is per environment, never inherited: a nested
        // non-statement environment (an `itemize` in a `\node` label) parses its
        // body with the flag off, and a nested `scope` turns it back on.
        let saved_stmt = self.in_statement_body;
        self.in_statement_body = name
            .as_deref()
            .is_some_and(|n| self.ctx.is_statement_environment(n));
        if name
            .as_deref()
            .is_some_and(|n| self.ctx.is_verbatim_environment(n))
        {
            self.verbatim_body(name.as_deref().expect("verbatim name"));
        } else if name
            .as_deref()
            .is_some_and(|n| self.ctx.is_math_environment(n))
        {
            self.math_environment_body();
        } else if macrocode_frame {
            // A frame-lexed macrocode body is macro code, not document
            // structure (see `macrocode_frame` above).
            self.macrocode_body(name.as_deref().expect("macrocode name"));
        } else {
            self.parse_block(Block::Environment);
        }
        self.in_statement_body = saved_stmt;
        if name.is_some() {
            self.open_envs.pop();
        }
        self.finish_environment(&name, opener);
        self.close(environment);
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
        self.plain_braces_version += 1;
        self.macrocode_end = Some(end);
        self.in_def_body = true;

        self.parse_block(Block::Macrocode);

        self.plain_braces = saved_plain;
        self.plain_braces_version += 1;
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
            // The cursor is at a closer alias for this very environment: consume
            // the bare control word as the `END` (issue #117). Tested before the
            // `\end` arm because `peek_end_name` would read the alias's own
            // following group (`\eeq{…}`) as an environment name and report a
            // mismatch against it.
            Some(_)
                if self.alias_closers.get(&self.pos).is_some_and(|target| {
                    !self.in_macro_code(self.pos) && name.as_deref() == Some(target.as_str())
                }) =>
            {
                let end = self.open(SyntaxKind::END);
                self.bump();
                self.close(end);
            }
            // The cursor is at a `\end` (the only other non-EOF stop condition).
            Some(_) => {
                let end_name = peek_end_name(self.tokens, self.pos);
                if name.is_none() || name.as_deref() == end_name.as_deref() {
                    // Matching \end: consume it as our END.
                    let end = self.open(SyntaxKind::END);
                    self.bump(); // \end
                    self.name_group();
                    self.close(end);
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
        let end = self.open(SyntaxKind::END);
        self.bump(); // \end
        self.name_group();
        self.close(end);
    }

    /// The `{name}` group following `\begin` / `\end`. Returns the trimmed name.
    fn name_group(&mut self) -> Option<String> {
        self.skip_trivia();
        if self.kind() != Some(SyntaxKind::L_BRACE) {
            self.error("expected `{` for environment name");
            return None;
        }
        let name_group = self.open(SyntaxKind::NAME_GROUP);
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
        self.close(name_group);
        Some(name.trim().to_owned())
    }
}

/// Read the environment name from a `\begin{…}` at `begin_pos` without consuming.
/// Identical in shape to [`peek_end_name`] (skip the control word and trivia, then
/// read the `{name}` group); named separately for call-site clarity.
fn peek_begin_name(tokens: &[Token], begin_pos: usize) -> Option<Cow<'_, str>> {
    peek_end_name(tokens, begin_pos)
}

/// Read the environment name from a `\end{…}` at `end_pos` without consuming.
///
/// Borrows the token's own text for the single-token name every ordinary
/// environment has, and only allocates for one spelled across several tokens
/// (`\end{align *}`, a name holding a digit or a `-`). Three of the callers are
/// forward scans that ask once per token and only ever compare the result, so
/// the common case must not allocate.
fn peek_end_name(tokens: &[Token], end_pos: usize) -> Option<Cow<'_, str>> {
    let mut i = end_pos + 1; // past the \end control word
    while tokens.get(i).is_some_and(|t| Parser::is_trivia(t.kind)) {
        i += 1;
    }
    if tokens.get(i).map(|t| t.kind) != Some(SyntaxKind::L_BRACE) {
        return None;
    }
    i += 1;
    let start = i;
    while tokens.get(i).is_some_and(|t| t.kind != SyntaxKind::R_BRACE) {
        i += 1;
    }
    Some(match &tokens[start..i] {
        [] => Cow::Borrowed(""),
        [t] => Cow::Borrowed(t.text.trim()),
        many => {
            let mut name = String::new();
            for t in many {
                name.push_str(&t.text);
            }
            Cow::Owned(name.trim().to_owned())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::lexer::lex;

    #[test]
    fn step_guard_trips_when_wedged() {
        let tokens = lex("x");
        let ctx = ParseCtx::default();
        let p = Parser::new(&tokens, &ctx);
        p.last_step_pos.set(p.pos);
        p.steps.set(PARSER_STEP_LIMIT - 1);
        p.step(); // reaches the ceiling exactly — still allowed
        let wedged = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| p.step()));
        assert!(wedged.is_err(), "the guard must abort a non-advancing loop");
    }

    #[test]
    fn step_budget_resets_on_cursor_progress() {
        let tokens = lex("xx");
        let ctx = ParseCtx::default();
        let mut p = Parser::new(&tokens, &ctx);
        p.last_step_pos.set(p.pos);
        p.steps.set(PARSER_STEP_LIMIT - 1);
        p.pos += 1;
        p.step();
        assert_eq!(p.steps.get(), 1, "progress should reset the peek budget");
    }
}
