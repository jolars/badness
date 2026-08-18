//! Arity-directed argument attachment for expl3 call sites — the grammar half
//! of `AGENTS.md` decision #8's sanctioned deviation. Landed through the
//! staged migration `TODO.md` recorded: the token-level scan was diffed
//! against `semantic::expl3`'s independent consumption over the gate corpora
//! (67k statement-leading heads) before any consumer flipped.
//!
//! In an expl3 region `:`/`_` are letters, so a function name lexes as one
//! `CONTROL_WORD` carrying its own argspec suffix (`\tl_set:Nn`). Attachment
//! directed by that suffix is exactly as text-pure as greed — the arity rides
//! in the head token itself — and greed is a systematically *wrong* guess in
//! the dialect: every single-token slot breaks the run, so
//! `\tl_set:Nn \l_a {x}` attaches `{x}` to the definee, and the semantic
//! layer's peel-back (`semantic::expl3::UnitCursor`) exists only to undo that
//! after the fact. This module puts the same consumption rules in front of the
//! event stream instead.
//!
//! **Shape gate and walk are one implementation.** [`Parser::scan_expl3_unit`]
//! is a pure `&self` token scan producing an [`Expl3Plan`];
//! [`Parser::attach_expl3_arguments`] replays exactly the plan's spans and
//! never re-decides a shape, so the scan mirrors the walk by construction
//! (`AGENTS.md`, "a shape gate must mirror the parse it
//! guards"). Anything the scan cannot resolve returns `None` and the head
//! falls back to plain greedy attachment with **no diagnostic** (a gated
//! construct never diagnoses). A blank line instead ends the unit *early*: the
//! plan carries the consumed prefix, the remainder parses as ordinary
//! siblings, and the partial commit is pass-stable because blank-line presence
//! is a preserved predicate — mirroring the semantic scan's `Stop::End`.
//!
//! **The trigger keys on token shape alone.** A `CONTROL_WORD` whose name
//! carries a `:` can only have lexed inside an expl3 region (out of region a
//! colon is never a control-word letter), so no region state is consulted —
//! which also covers the implicit `.dtx` regions (`Lexer::implicit_expl`)
//! that the parser's toggle index cannot see. Two deliberate exclusions:
//! the `\::n` expansion drivers (empty base name) are a runtime protocol, not
//! a call site — the semantic layer's shape rules keep them on the fallback
//! only by accident of greed, so the grammar excludes them explicitly — and
//! the formatter's *positional* toggle gate stays the formatter's alone: in a
//! false-positive region (`\def\ExplSyntaxOn{…}`) a mis-attachment is
//! tree-only and byte-invisible, the same posture as the lexer's name-only
//! toggle model (issue #69).
//!
//! Where the scan must refuse, it refuses **conservatively**: a candidate that
//! *might* form a node in the walk (a gated `\begin`, a live conditional
//! opener, a math opener whose closer is reachable, a bound `DOC_COMMENT` run)
//! aborts the unit even where the gate would demote it to a plain, consumable
//! token. A conservative refusal only costs recognition — the head stays
//! greedy, never mis-attached — and the migration oracle over the gate corpora
//! is what prices the residue before any consumer flips.

use super::Parser;
use super::trivia::{BLANK_LINE_NEWLINES, CommentMode};
use crate::semantic::expl3::{Expl3Slot, expl3_slots};
use crate::syntax::SyntaxKind;
use std::collections::HashMap;

/// The matching-brace table every group slot resolves its argument through,
/// memoized against the walk state the build read.
///
/// A group slot needs its argument's closer, and a *nested* call site asks once
/// per level over a span each outer level already covered, so a per-slot rescan
/// is quadratic in the nesting depth
/// (`expl3_arity_nested_scans_stay_linear`). One stack pass settles every pair
/// at once instead, and the whole nest then answers from the table — the trade
/// [`Parser::gated_closer`] makes for the shape gates, for the same reason.
///
/// Keyed by the two facts that decide *pairing*: [`Parser::plain_braces`] —
/// through its version counter, the [`super::WalkKey`] convention, since the
/// set is mutated per `macrocode` body — and the frame that set is scoped to.
/// The build stops at that frame rather than at the scan's own bound, so an
/// alias closer (which moves with no version bump) only ever filters at query
/// time and can never invalidate the table.
pub(super) struct BraceMatches {
    /// [`Parser::plain_braces_version`] the table was built under.
    plain_braces: u32,
    /// [`Parser::macrocode_end`] the build was bounded by.
    macrocode_end: Option<usize>,
    /// The lowest token index the table covers. A build is seeded at its first
    /// query and walks forward only; the walk's queries ascend, so this is one
    /// build per key in practice.
    from: usize,
    /// Open token index to the index of its matching `}`.
    ends: HashMap<usize, usize>,
}

/// One argument the scan resolved, as the token span the replay consumes.
/// The *shape* is recorded because the replay must emit different events per
/// shape, never because it re-decides anything.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PlanArg {
    /// A control word consumed as a bare `COMMAND` node
    /// ([`Parser::command_bare`]): kept a node so name-keyed consumers
    /// (rename, hover, call counts) still see `\l_tmpa_tl`, but with no
    /// arguments of its own — only the head's argspec drives consumption, so
    /// this is unconditional even for `\exp_args:NNo \cs_set:Npn`.
    Command(usize),
    /// A braced `{…}` group, replayed via [`Parser::group`]. Serves the
    /// `n`-family and `T`/`F` slots, and an `N` slot TeX-faithfully (braces
    /// around an `N` argument are grabbed whole).
    Group(std::ops::Range<usize>),
    /// Raw tokens bumped bare into the head node: a control symbol (the
    /// def-prefix definee precedent — `\[` in an `N` slot is never a math
    /// opener), a single-character relation `WORD` (issue #106), `#`-parameter
    /// tokens, and parameter-text content.
    Tokens(std::ops::Range<usize>),
}

impl PlanArg {
    fn start(&self) -> usize {
        match self {
            PlanArg::Command(idx) => *idx,
            PlanArg::Group(r) | PlanArg::Tokens(r) => r.start,
        }
    }

    pub(super) fn end(&self) -> usize {
        match self {
            PlanArg::Command(idx) => idx + 1,
            PlanArg::Group(r) | PlanArg::Tokens(r) => r.end,
        }
    }
}

/// The scanned call unit for one head: what
/// [`Parser::attach_expl3_arguments`] replays.
#[derive(Debug)]
pub(super) struct Expl3Plan {
    /// The resolved arguments, in consumption order.
    pub(super) args: Vec<PlanArg>,
    /// One past the last consumed token — the gap after it stays outside the
    /// head node, exactly as greedy attachment leaves trailing trivia.
    pub(super) end: usize,
    /// `false` when a blank line ended the unit early: the plan carries the
    /// consumed prefix. Consumers of a conditional's branches then naturally
    /// see fewer trailing groups than `conditional_branches(name)` — the
    /// "report none rather than a prefix" rule reproduced with no extra state.
    /// Read by the scan's unit tests only: the replay needs just the spans,
    /// since a partial plan replays exactly like a complete one.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) complete: bool,
}

/// Why slot consumption stopped early — the token-level mirror of
/// `semantic::expl3::Stop`.
enum Stop {
    /// A blank line: the unit ends here and the consumed prefix commits.
    End,
    /// The scan cannot resolve the unit — the head falls back to greed.
    Abort,
}

impl Parser<'_> {
    /// The argspec slots of the head at the cursor, when arity-directed
    /// attachment applies to it: a `CONTROL_WORD` whose name carries a
    /// derivable argspec ([`expl3_slots`]). A colon-carrying name can only
    /// have lexed in-region, so the token's own shape is the region proof.
    /// Excludes the `\::n` expansion drivers (empty base name) — see the
    /// module docs.
    pub(super) fn expl3_arity_slots(&self) -> Option<Vec<Expl3Slot>> {
        let t = self.tokens.get(self.pos)?;
        if t.kind != SyntaxKind::CONTROL_WORD {
            return None;
        }
        let name = t.text.strip_prefix('\\')?;
        if name.starts_with(':') {
            return None;
        }
        expl3_slots(name)
    }

    /// Scan the call unit headed at the cursor: consume `slots` from the raw
    /// token stream by the same rules as `semantic::expl3::consume_unit`, minus
    /// the peel-back queue (which exists only to undo greed and has no
    /// token-level analog). Pure `&self`; the walk replays the returned plan.
    /// `None` degrades the head to greedy attachment.
    pub(super) fn scan_expl3_unit(&self, slots: &[Expl3Slot]) -> Option<Expl3Plan> {
        // Inside math the math loop owns dispatch, and the enclosing math's
        // closer (`\]`, `\)`, a `$`) is a boundary the token scan cannot see —
        // a slot consuming it swallows the closer into the head and leaves the
        // math unclosed (`xo-grid.dtx`'s `\cs_set_nopar:Npn \]{…}` inside the
        // `\[…\]` the previous line's definition opened). The formatter's
        // expl3 layout does not own math bodies either, so nothing is lost by
        // refusing outright.
        if self.in_math() {
            return None;
        }
        // The macrocode frame is a hard boundary in both directions, and an
        // alias environment's body ends positionally at its closer — a unit
        // reaching either has run out of stream mid-unit (`Stop::Abort`'s
        // stream-end case).
        let bound = [self.macrocode_end, self.alias_end]
            .into_iter()
            .flatten()
            .fold(self.tokens.len(), usize::min);
        let mut scan = UnitScan {
            p: self,
            i: self.pos + 1,
            bound,
            args: Vec::new(),
        };
        let mut complete = true;
        for slot in slots {
            let took = match slot {
                Expl3Slot::SingleToken => scan.take_single_token(),
                Expl3Slot::Group | Expl3Slot::Branch => scan.take_group(),
                Expl3Slot::ParameterText => scan.take_parameter_text(),
            };
            match took {
                Ok(()) => {}
                Err(Stop::End) => {
                    complete = false;
                    break;
                }
                Err(Stop::Abort) => return None,
            }
        }
        let end = scan.args.last().map_or(self.pos + 1, PlanArg::end);
        Some(Expl3Plan {
            args: scan.args,
            end,
            complete,
        })
    }

    /// Replay a scanned plan inside the open `COMMAND` node. Consumes exactly
    /// the plan's spans — the gap before each argument is the trivia,
    /// comments, and `~` the scan skipped, bumped into the head node exactly
    /// where greedy attachment's `skip_trivia` would put them. Never
    /// re-decides a shape; the per-arg asserts are the scan-mirrors-walk
    /// tripwire.
    pub(super) fn attach_expl3_arguments(&mut self, plan: &Expl3Plan) {
        for arg in &plan.args {
            while self.pos < arg.start() {
                self.bump();
            }
            match arg {
                PlanArg::Command(idx) => {
                    debug_assert_eq!(self.pos, *idx);
                    self.command_bare();
                }
                PlanArg::Group(range) => {
                    debug_assert_eq!(self.pos, range.start);
                    self.group();
                    debug_assert_eq!(self.pos, range.end);
                }
                PlanArg::Tokens(range) => {
                    debug_assert_eq!(self.pos, range.start);
                    while self.pos < range.end {
                        self.bump();
                    }
                }
            }
        }
        debug_assert_eq!(self.pos, plan.end);
    }

    /// The index of the `}` matching the group opening at `open`, from the
    /// shared [`BraceMatches`] table, or `None` when nothing closes it before
    /// the `macrocode` frame. Bound-free on purpose — a caller applies its own
    /// bound to the answer, which is what keeps the table valid across the
    /// alias closers that move without a version bump.
    ///
    /// A miss rebuilds from `open` forward, settling every pair in the frame
    /// the way a gate batch settles every opener its scan passes. The build
    /// recycles the superseded table's map: its pairings are stale, its
    /// allocation is not ([`Parser::gated_closer`]).
    fn matching_brace(&self, open: usize) -> Option<usize> {
        if let Some(table) = self.brace_matches.borrow().as_ref()
            && table.plain_braces == self.plain_braces_version
            && table.macrocode_end == self.macrocode_end
            && open >= table.from
        {
            return table.ends.get(&open).copied();
        }
        let mut ends = self
            .brace_matches
            .borrow_mut()
            .take()
            .map_or_else(HashMap::new, |stale| {
                let mut map = stale.ends;
                map.clear();
                map
            });
        let mut stack: Vec<usize> = Vec::new();
        for j in open..self.macrocode_end.unwrap_or(self.tokens.len()) {
            self.tick_scan();
            match self.tokens[j].kind {
                SyntaxKind::L_BRACE if !self.plain_braces.contains(&j) => stack.push(j),
                SyntaxKind::R_BRACE if !self.plain_braces.contains(&j) => {
                    // An unbalanced closer belongs to a group opened before
                    // the build's seed; it pairs with nothing here.
                    if let Some(opened) = stack.pop() {
                        ends.insert(opened, j);
                    }
                }
                _ => {}
            }
        }
        let answer = ends.get(&open).copied();
        *self.brace_matches.borrow_mut() = Some(BraceMatches {
            plain_braces: self.plain_braces_version,
            macrocode_end: self.macrocode_end,
            from: open,
            ends,
        });
        answer
    }

    /// A control sequence consumed as an argument: a `COMMAND` node holding
    /// only its name token — no [`Parser::attach_arguments`], because its
    /// trailing groups belong to the outer head. Kept a node (not the
    /// def-prefix bare-token treatment) so name-keyed consumers still see it.
    fn command_bare(&mut self) {
        self.open(SyntaxKind::COMMAND);
        self.bump();
        self.close();
    }
}

/// The token-level consumption cursor — `semantic::expl3::UnitCursor` minus
/// the peel queue. `i` is the next unexamined token; a `take_*` that succeeds
/// leaves `i` one past what it consumed and pushes the consumed span onto
/// `args`.
struct UnitScan<'p, 't> {
    p: &'p Parser<'t>,
    i: usize,
    bound: usize,
    args: Vec<PlanArg>,
}

impl UnitScan<'_, '_> {
    /// Scan forward to the next slot candidate, skipping inline whitespace,
    /// comments, and `~` (a `~` is a space token TeX skips before an
    /// undelimited argument, so it can never satisfy a slot; it stays in the
    /// extent for the layout loop's tilde arm). Returns the candidate's index
    /// without consuming it, plus whether the gap crossed an *own-line
    /// comment* — the slot handlers read that flag, because a comment-glued
    /// gap stops the unit only where greedy attachment would have stopped
    /// too: a `{…}` candidate past it was glued by greed (a comment is
    /// content that resets the newline run for attachment), while any other
    /// candidate keeps the own-line-comment-ends-the-unit rule.
    ///
    /// A *bare* blank line (two newlines with no comment between) stops per
    /// [`Self::gap_stop`]. An own-line comment run that binds forward into a
    /// following construct ([`Parser::binding_run`]) becomes that construct's
    /// `DOC_COMMENT`, a node the unit must not swallow, so it aborts. A
    /// docstrip `GUARD` or `DOC_MARGIN` mid-unit aborts: guarded alternative
    /// bodies make arity lie (issue #78). Running out of stream (EOF, the
    /// macrocode frame, an alias closer) aborts.
    fn advance(&mut self) -> Result<(usize, bool), Stop> {
        let mut newlines = 0usize;
        let mut crossed_comment = false;
        let gap_start = self.i;
        loop {
            if self.i >= self.bound {
                return Err(Stop::Abort);
            }
            self.p.tick_scan();
            match self.p.tokens[self.i].kind {
                SyntaxKind::NEWLINE => {
                    newlines += 1;
                    if newlines >= BLANK_LINE_NEWLINES {
                        return Err(self.gap_stop(gap_start));
                    }
                    self.i += 1;
                }
                SyntaxKind::WHITESPACE | SyntaxKind::TILDE => self.i += 1,
                SyntaxKind::COMMENT => {
                    if self.p.binding_run(self.i).is_some() {
                        return Err(Stop::Abort);
                    }
                    if newlines > 0 {
                        crossed_comment = true;
                    }
                    newlines = 0;
                    self.i += 1;
                }
                SyntaxKind::GUARD | SyntaxKind::DOC_MARGIN => return Err(Stop::Abort),
                _ => return Ok((self.i, crossed_comment)),
            }
        }
    }

    /// How a blank-line-sized gap (two or more newlines, counted across
    /// comments) stops the unit — the distinction `semantic::expl3` gets for
    /// free from its element streams and the token scan must re-derive:
    ///
    /// - Inside a brace group the element loop runs to the `}` regardless of
    ///   blank lines, so the stream continues past the gap and the unit
    ///   commits its consumed prefix ([`Stop::End`], the sanctioned partial
    ///   commit).
    /// - At paragraph level, a gap the walk treats as a *paragraph separator*
    ///   ([`Parser::trivia_run_is_separator`]: a bare blank line, or trivia
    ///   running out into the block terminator — the macrocode frame, an
    ///   alias closer, a genuine `\end`, EOF) ends the walk's element stream
    ///   right here, which the semantic scan reads as running out of stream:
    ///   [`Stop::Abort`], never a partial commit (`latex-lab-sec.dtx`'s
    ///   `#1 #2 #3 #4` head whose commented body lives in the next chunk).
    ///   A comment-glued gap that does *not* separate (content follows in the
    ///   same paragraph) keeps the own-line-comment-ends-the-unit rule:
    ///   [`Stop::End`].
    fn gap_stop(&self, gap_start: usize) -> Stop {
        if !self.p.group_opens.is_empty() {
            return Stop::End;
        }
        let s = self.p.scan_trivia(gap_start, CommentMode::Skip);
        let reaches_terminator = s.next_kind.is_none()
            || self.p.macrocode_end.is_some_and(|end| s.next >= end)
            || self.p.alias_end.is_some_and(|end| s.next >= end)
            || (s.next_kind == Some(SyntaxKind::CONTROL_WORD) && self.p.env_end_at(s.next));
        if s.saw_blank_line || reaches_terminator {
            Stop::Abort
        } else {
            Stop::End
        }
    }

    /// Consume the single token at `idx` into the current [`PlanArg::Tokens`]
    /// span (extending it when contiguous).
    fn push_token(&mut self, idx: usize) {
        match self.args.last_mut() {
            Some(PlanArg::Tokens(r)) if r.end == idx => r.end = idx + 1,
            _ => self.args.push(PlanArg::Tokens(idx..idx + 1)),
        }
        self.i = idx + 1;
    }

    /// Whether the control word at `idx` would form a node (or a diagnostic)
    /// in the walk rather than parse as a plain, consumable command. `\begin`
    /// and `\end` outside macro code route through the environment machinery;
    /// a live conditional opener may become a `CONDITIONAL`; an alias
    /// opener/closer an `ENVIRONMENT`. Each of those is shape-gated and the
    /// gate may still demote it to a plain command — the scan refuses
    /// conservatively rather than re-running the gate (see the module docs).
    fn control_word_forms_node(&self, idx: usize) -> bool {
        let text = self.p.tokens[idx].text.as_str();
        if (text == super::BEGIN_CMD || text == super::END_CMD) && !self.p.in_macro_code(idx) {
            return true;
        }
        self.p.conditional_openers.contains(&idx)
            || self.p.alias_openers.contains_key(&idx)
            || self.p.alias_closers.contains_key(&idx)
    }

    /// Whether the control symbol at `idx` would form a node in the walk:
    /// `\\` is always a `LINE_BREAK`; `\[`/`\(` open math when their closer is
    /// reachable (the same gate the walk runs) and are plain data tokens
    /// otherwise (`\char_set_catcode_letter:N \)`, issue #60).
    fn control_symbol_forms_node(&self, idx: usize) -> bool {
        match self.p.tokens[idx].text.as_str() {
            "\\\\" => true,
            "\\[" => self.p.delim_math_closes(idx, "\\]"),
            "\\(" => self.p.delim_math_closes(idx, "\\)"),
            _ => false,
        }
    }

    /// An `N`/`V` slot: one token — a control sequence, a single-character
    /// `WORD` (the relation in `\int_compare:nNnTF {a} = {1}`, issue #106; a
    /// longer run would take material TeX leaves for the next slot, so it
    /// aborts), a `#`-parameter (hashes plus one parameter digit), or a whole
    /// braced group (TeX-faithful: `N` vs `n` is convention, not matching
    /// behavior).
    fn take_single_token(&mut self) -> Result<(), Stop> {
        let (idx, crossed) = self.advance()?;
        let t = &self.p.tokens[idx];
        // Past an own-line comment, only what greedy attachment glued is
        // consumable — a real braced candidate; anything else ends the unit
        // at the comment (`semantic::expl3`'s own-line-comment rule).
        if crossed && !(t.kind == SyntaxKind::L_BRACE && !self.p.plain_braces.contains(&idx)) {
            return Err(Stop::End);
        }
        match t.kind {
            SyntaxKind::CONTROL_WORD if !self.control_word_forms_node(idx) => {
                self.args.push(PlanArg::Command(idx));
                self.i = idx + 1;
                Ok(())
            }
            SyntaxKind::CONTROL_SYMBOL if !self.control_symbol_forms_node(idx) => {
                self.push_token(idx);
                Ok(())
            }
            SyntaxKind::WORD if t.text.chars().count() == 1 => {
                self.push_token(idx);
                Ok(())
            }
            SyntaxKind::HASH => {
                self.push_token(idx);
                loop {
                    let (next, crossed) = self.advance()?;
                    let t = &self.p.tokens[next];
                    if crossed {
                        return Err(Stop::End);
                    }
                    match t.kind {
                        SyntaxKind::HASH => self.push_token(next),
                        SyntaxKind::WORD if is_param_digit_text(&t.text) => {
                            self.push_token(next);
                            return Ok(());
                        }
                        _ => return Err(Stop::Abort),
                    }
                }
            }
            SyntaxKind::L_BRACE => {
                let end = self.group_end(idx).ok_or(Stop::Abort)?;
                self.args.push(PlanArg::Group(idx..end));
                self.i = end;
                Ok(())
            }
            _ => Err(Stop::Abort),
        }
    }

    /// An `n`-family or `T`/`F` slot: exactly a braced group. A bare token is
    /// legal TeX for an undelimited argument, but accepting it would let
    /// sloppy shapes swallow the next statement's head — those stay greedy,
    /// matching the semantic scan.
    fn take_group(&mut self) -> Result<(), Stop> {
        let (idx, crossed) = self.advance()?;
        if self.p.tokens[idx].kind != SyntaxKind::L_BRACE {
            // Past an own-line comment the unit ends rather than aborts: the
            // comment bounded the gap, and the consumed prefix commits.
            return Err(if crossed { Stop::End } else { Stop::Abort });
        }
        let end = self.group_end(idx).ok_or(Stop::Abort)?;
        self.args.push(PlanArg::Group(idx..end));
        self.i = end;
        Ok(())
    }

    /// A `p` slot: TeX parameter text — everything up to (not including) the
    /// first explicit `{`, which is left for the following slot (the
    /// `#{`-terminated form works out to the same rule). Tokens and brackets
    /// are parameter text; a delimiting control word is consumed as a bare
    /// command; a chunk-plain brace is an ordinary token and stays parameter
    /// text. An unbalanced `}` means the enclosing group is closing mid-text
    /// (the stream-end case), and a `$` may open math in the walk — both
    /// abort.
    fn take_parameter_text(&mut self) -> Result<(), Stop> {
        loop {
            let (idx, crossed) = self.advance()?;
            let t = &self.p.tokens[idx];
            if crossed && !(t.kind == SyntaxKind::L_BRACE && !self.p.plain_braces.contains(&idx)) {
                return Err(Stop::End);
            }
            match t.kind {
                SyntaxKind::L_BRACE if !self.p.plain_braces.contains(&idx) => return Ok(()),
                SyntaxKind::R_BRACE if !self.p.plain_braces.contains(&idx) => {
                    return Err(Stop::Abort);
                }
                SyntaxKind::DOLLAR => return Err(Stop::Abort),
                SyntaxKind::CONTROL_WORD => {
                    if self.control_word_forms_node(idx) {
                        return Err(Stop::Abort);
                    }
                    self.args.push(PlanArg::Command(idx));
                    self.i = idx + 1;
                }
                SyntaxKind::CONTROL_SYMBOL => {
                    if self.control_symbol_forms_node(idx) {
                        return Err(Stop::Abort);
                    }
                    self.push_token(idx);
                }
                _ => self.push_token(idx),
            }
        }
    }

    /// One past the matching `}` of the group opening at `open`, counting only
    /// the braces that really form groups (chunk-plain ones are ordinary
    /// tokens, exactly as [`Parser::group`] treats them via the walk's
    /// dispatch). `None` when no closer is reachable before the bound: the
    /// walk would error-recover an unclosed group there, and the scan refuses
    /// instead so the head stays greedy.
    fn group_end(&self, open: usize) -> Option<usize> {
        if self.p.plain_braces.contains(&open) {
            return None;
        }
        // The table is built to the `macrocode` frame, so this scan's own
        // bound (an alias closer, the frame) is applied here: a closer past it
        // is one the walk would not reach, and the head stays greedy.
        let close = self.p.matching_brace(open)?;
        (close < self.bound).then_some(close + 1)
    }
}

/// The token-text twin of `syntax::is_param_digit` (which takes a red-tree
/// token this scan does not have): a single TeX parameter digit `1`..=`9`.
fn is_param_digit_text(text: &str) -> bool {
    matches!(text.as_bytes(), [b'1'..=b'9'])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::lexer::{ParseCtx, lex};

    /// Lex `input` (which must open a region so `:`/`_` are letters), park the
    /// parser on the token whose text is `head`, and scan its unit. Renders
    /// each resolved argument as `kind:text` for compact assertions.
    fn scan(input: &str, head: &str) -> Option<(Vec<String>, bool)> {
        let tokens = lex(input);
        let ctx = ParseCtx::default();
        let mut p = Parser::new(&tokens, &ctx);
        p.pos = tokens
            .iter()
            .position(|t| t.text == head)
            .expect("head token present");
        let slots = p.expl3_arity_slots()?;
        let plan = p.scan_expl3_unit(&slots)?;
        let text = |r: &std::ops::Range<usize>| {
            tokens[r.clone()]
                .iter()
                .map(|t| t.text.as_str())
                .collect::<String>()
        };
        let args = plan
            .args
            .iter()
            .map(|arg| match arg {
                PlanArg::Command(i) => format!("cmd:{}", tokens[*i].text),
                PlanArg::Group(r) => format!("group:{}", text(r)),
                PlanArg::Tokens(r) => format!("tok:{}", text(r)),
            })
            .collect();
        Some((args, plan.complete))
    }

    const ON: &str = "\\ExplSyntaxOn\n";

    #[test]
    fn single_token_and_group_slots_resolve() {
        let (args, complete) = scan(
            &format!("{ON}\\tl_set:Nn \\l_tmpa_tl {{ x }}\n"),
            "\\tl_set:Nn",
        )
        .expect("recognized");
        assert_eq!(args, ["cmd:\\l_tmpa_tl", "group:{ x }"]);
        assert!(complete);
    }

    #[test]
    fn relation_word_satisfies_a_single_token_slot() {
        // Issue #106: TeX grabs one character for an undelimited argument.
        let (args, _) = scan(
            &format!("{ON}\\int_compare:nNnTF {{ a }} = {{ 1 }} {{ T }} {{ F }}\n"),
            "\\int_compare:nNnTF",
        )
        .expect("recognized");
        assert_eq!(
            args,
            [
                "group:{ a }",
                "tok:=",
                "group:{ 1 }",
                "group:{ T }",
                "group:{ F }"
            ]
        );
    }

    #[test]
    fn multi_character_word_aborts_the_slot() {
        // A longer run would take material TeX leaves for the next slot.
        assert!(
            scan(
                &format!("{ON}\\int_compare:nNnTF {{ a }} == {{ 1 }} {{ T }} {{ F }}\n"),
                "\\int_compare:nNnTF",
            )
            .is_none()
        );
    }

    #[test]
    fn parameter_text_runs_to_the_first_explicit_brace() {
        let (args, _) = scan(
            &format!("{ON}\\cs_new:Npn \\foo:nn #1#2 {{ body }}\n"),
            "\\cs_new:Npn",
        )
        .expect("recognized");
        assert_eq!(args, ["cmd:\\foo:nn", "tok:#1#2", "group:{ body }"]);
    }

    #[test]
    fn delimiting_control_word_is_parameter_text() {
        // `#1 \q_stop {body}`: the delimiter is consumed as a bare command and
        // the terminating group is left for the `n` slot — the same extent the
        // semantic peel-back produces.
        let (args, _) = scan(
            &format!("{ON}\\cs_new:Npn \\foo:w #1 \\q_stop {{ body }}\n"),
            "\\cs_new:Npn",
        )
        .expect("recognized");
        assert_eq!(
            args,
            ["cmd:\\foo:w", "tok:#1", "cmd:\\q_stop", "group:{ body }"]
        );
    }

    #[test]
    fn hash_parameter_satisfies_a_single_token_slot() {
        let (args, _) =
            scan(&format!("{ON}\\tl_set:Nn #1 {{ x }}\n"), "\\tl_set:Nn").expect("recognized");
        assert_eq!(args, ["tok:#1", "group:{ x }"]);
    }

    #[test]
    fn braces_around_a_single_token_slot_are_grabbed_whole() {
        let (args, _) = scan(
            &format!("{ON}\\tl_set:Nn {{ \\l_tmpa_tl }} {{ x }}\n"),
            "\\tl_set:Nn",
        )
        .expect("recognized");
        assert_eq!(args, ["group:{ \\l_tmpa_tl }", "group:{ x }"]);
    }

    #[test]
    fn blank_line_at_paragraph_level_aborts() {
        // A paragraph separator ends the walk's element stream, which the
        // semantic scan reads as running out of stream — the head stays
        // greedy, exactly as `attach_arguments` stops at a paragraph break.
        assert!(
            scan(
                &format!("{ON}\\tl_set:Nn \\l_tmpa_tl\n\n{{ x }}\n"),
                "\\tl_set:Nn",
            )
            .is_none()
        );
    }

    #[test]
    fn blank_line_in_a_group_body_commits_the_prefix() {
        // Inside a brace group the element loop runs to the `}` regardless of
        // blank lines, so the stream continues and the unit commits what it
        // consumed (the sanctioned partial commit). The scan is parked
        // mid-walk, so the enclosing-group context is simulated the way the
        // walk would carry it.
        let input = format!("{ON}{{ \\tl_set:Nn \\l_tmpa_tl\n\n{{ x }} }}\n");
        let tokens = lex(&input);
        let ctx = ParseCtx::default();
        let mut p = Parser::new(&tokens, &ctx);
        p.pos = tokens
            .iter()
            .position(|t| t.text == "\\tl_set:Nn")
            .expect("head token present");
        p.group_opens.push(0);
        let slots = p.expl3_arity_slots().expect("derivable");
        let plan = p.scan_expl3_unit(&slots).expect("recognized");
        assert_eq!(plan.args.len(), 1, "only the N slot is consumed");
        assert!(!plan.complete);
    }

    #[test]
    fn braced_candidate_past_an_own_line_comment_is_consumed() {
        // Greedy attachment crosses an own-line comment to a following `{…}`
        // (a comment is content that resets the newline run), so the glued
        // group is consumable — the comment rides the unit, exactly as it
        // rode the attached sibling before the migration.
        let (args, complete) = scan(
            &format!("{ON}\\tl_set:Nn \\l_tmpa_tl\n% doc\n{{ x }}\n"),
            "\\tl_set:Nn",
        )
        .expect("recognized");
        assert_eq!(args, ["cmd:\\l_tmpa_tl", "group:{ x }"]);
        assert!(complete);
    }

    #[test]
    fn non_braced_candidate_past_an_own_line_comment_ends_the_unit() {
        // Anything greedy would not have glued keeps the own-line-comment
        // rule: the unit ends at the gap with its consumed prefix.
        let (args, complete) = scan(
            &format!("{ON}\\cs_new:Npn \\module_foo:n\n% doc\n#1 {{ x }}\n"),
            "\\cs_new:Npn",
        )
        .expect("recognized");
        assert_eq!(args, ["cmd:\\module_foo:n"]);
        assert!(!complete);
    }

    #[test]
    fn trailing_same_line_comment_is_transparent() {
        let (args, complete) = scan(
            &format!("{ON}\\tl_set:Nn \\l_tmpa_tl % why\n{{ x }}\n"),
            "\\tl_set:Nn",
        )
        .expect("recognized");
        assert_eq!(args, ["cmd:\\l_tmpa_tl", "group:{ x }"]);
        assert!(complete);
    }

    #[test]
    fn binding_comment_run_aborts_the_unit() {
        // An own-line comment run before a control word becomes that word's
        // `DOC_COMMENT` (AGENTS.md #9) — a node the unit must not swallow.
        assert!(
            scan(
                &format!("{ON}\\tl_set:Nn\n% doc\n\\l_tmpa_tl {{ x }}\n"),
                "\\tl_set:Nn",
            )
            .is_none()
        );
    }

    #[test]
    fn tilde_is_skipped_like_the_space_it_is() {
        let (args, _) = scan(
            &format!("{ON}\\tl_set:Nn ~ \\l_tmpa_tl ~ {{ x }}\n"),
            "\\tl_set:Nn",
        )
        .expect("recognized");
        assert_eq!(args, ["cmd:\\l_tmpa_tl", "group:{ x }"]);
    }

    #[test]
    fn group_slot_facing_a_bare_token_aborts() {
        assert!(scan(&format!("{ON}\\tl_set:Nn \\l_a \\foo\n"), "\\tl_set:Nn").is_none());
    }

    #[test]
    fn stream_ending_mid_unit_aborts() {
        assert!(scan(&format!("{ON}\\tl_set:Nn \\l_a"), "\\tl_set:Nn").is_none());
    }

    #[test]
    fn unclosed_group_aborts() {
        assert!(scan(&format!("{ON}\\tl_set:Nn \\l_a {{ x\n"), "\\tl_set:Nn").is_none());
    }

    #[test]
    fn enclosing_group_closing_mid_parameter_text_aborts() {
        assert!(
            scan(
                &format!("{ON}{{ \\cs_new:Npn \\foo:n #1 }}\n"),
                "\\cs_new:Npn"
            )
            .is_none()
        );
    }

    #[test]
    fn zero_arity_head_consumes_nothing() {
        let (args, complete) =
            scan(&format!("{ON}\\scan_stop: {{ x }}\n"), "\\scan_stop:").expect("recognized");
        assert!(args.is_empty());
        assert!(complete);
    }

    #[test]
    fn expansion_drivers_are_excluded() {
        // `\::n` parses a real spec (`expl3_slots("::n")` is `Some`), but its
        // runtime protocol is nothing like a call site — excluded by the
        // empty-base-name check, staying greedy.
        assert!(scan(&format!("{ON}\\::n {{ x }}\n"), "\\::n").is_none());
    }

    #[test]
    fn underivable_heads_have_no_slots() {
        assert!(scan(&format!("{ON}\\exp_after:wN \\foo\n"), "\\exp_after:wN").is_none());
        assert!(scan(&format!("{ON}\\l_tmpa_tl x\n"), "\\l_tmpa_tl").is_none());
    }

    #[test]
    fn line_break_control_symbol_aborts_a_single_token_slot() {
        // `\\` always forms a `LINE_BREAK` node in the walk.
        assert!(scan(&format!("{ON}\\tl_set:Nn \\\\ {{ x }}\n"), "\\tl_set:Nn").is_none());
    }

    #[test]
    fn plain_control_symbol_satisfies_a_single_token_slot() {
        // An orphan `\)` is data in macro code (issue #60) — a bare token the
        // slot consumes, the def-prefix definee treatment.
        let (args, _) =
            scan(&format!("{ON}\\tl_set:Nn \\) {{ x }}\n"), "\\tl_set:Nn").expect("recognized");
        assert_eq!(args, ["tok:\\)", "group:{ x }"]);
    }
}
