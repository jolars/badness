//! Structural statement segmentation for expl3 code — the S4 mechanism.
//!
//! [`segment_expl_statements`] walks a stream of in-region sibling elements (a
//! paragraph run or a brace-group body) and decides, for every gap between
//! elements, whether a statement boundary sits there. The layout loop
//! (`lower_expl_code`) then commits logical lines where the map says, instead
//! of where the *author's* newlines fell — retiring the unsafe
//! newline-vs-space trivia read (the root of the K&R↔Allman idempotency
//! family; see `formatter.md`, § Trivia-invariant layout).
//!
//! A statement is a **call unit**: a head `COMMAND` whose name has a derivable
//! argspec arity ([`expl3::expl3_slots`]) plus the elements its slots consume.
//! Consumption is a pure shape scan — no `Ir` is built here — over two sources
//! in order: the head's own greedily-attached children (the parser attaches
//! every trailing `{…}` regardless of arity, decision #8), then the following
//! siblings. Greedy attachment routinely gives an argument to the *wrong
//! owner* (`\cs_new:Nn \foo:n {body}` attaches `{body}` to `\foo:n`); when a
//! `COMMAND` node satisfies a single-token slot, its own attached children are
//! *peeled* back onto the front of the scan queue so they can satisfy the
//! outer head's remaining slots. Only the head's argspec ever drives
//! consumption — an argument's own argspec is inert data, exactly as TeX
//! grabs it.
//!
//! The trivia the scan may read is confined to *preserved* predicates:
//! - a **blank line** (a gap of two or more newlines) ends the unit where it
//!   stands — the partial unit commits as-is, pass-stably, because blank-line
//!   presence is preserved by the formatter;
//! - a **comment** is transparent to consumption (the layout loop makes it
//!   end its physical line; the unit continues), and a comment trailing a
//!   *complete* unit is pulled into the statement so it stays on the call's
//!   line — comment presence and own-line-ness are preserved predicates;
//! - a lone-newline-vs-space gap is **never** read on the structural path.
//!
//! Anything the shape scan cannot resolve — an unrecognized head (no `:`
//! suffix, or a `w`/`D`/unknown letter), a slot facing the wrong shape, a
//! docstrip `GUARD` mid-unit (guarded alternative bodies make arity lie,
//! issue #78), or the stream ending mid-unit — degrades that statement to the
//! **fallback**: the authored physical line is the statement, exactly the old
//! `SplitAtNewlines` behavior demoted to a per-line escape hatch (Tier 2; see
//! `formatter.md`, § Known violations). Recognition is re-attempted at every
//! statement start, so recognized and fallback statements interleave
//! deterministically; a recognized head *mid*-fallback-line is never split
//! out.

use std::collections::VecDeque;

use crate::ast::command_name;
use crate::parser::lexer::expl_toggle;
use crate::semantic::expl3::{self, Expl3Slot};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use super::core::{is_collapsible_trivia, is_param_digit};

/// The statement-boundary map for one element stream: `boundary_after(i)` says
/// a statement ends in the gap after element `i`. Boundaries sit on whole
/// top-level siblings — a boundary never splits a CST node, so anything the
/// greedy parser over-attached to a consumed sibling rides along in its
/// statement.
pub(crate) struct StatementMap {
    boundary_after: Vec<bool>,
    glue_before: Vec<bool>,
    glued: Vec<bool>,
    fallback: Vec<bool>,
}

impl StatementMap {
    /// Whether a statement boundary sits in the gap after element `idx`.
    pub(crate) fn boundary_after(&self, idx: usize) -> bool {
        self.boundary_after.get(idx).copied().unwrap_or(false)
    }

    /// Whether the gap *before* element `idx` must render unbreakable. Set for
    /// a recognized-head `COMMAND` sitting mid-way through a fallback
    /// statement: a width wrap at that gap would start a printed line with the
    /// recognized head, which the next pass segments as its own statement
    /// mid-way through this one and the passes disagree (`l3fp-trig.dtx`'s
    /// `\@@_sep:`-delimited protocols, `xo-or.dtx`'s `=~ \exp_not:c {…}\space`
    /// trace lines). Every other fallback gap stays breakable: a printed
    /// continuation line starting with anything unrecognized re-segments to
    /// exactly that line and renders to itself, the fallback's fixed point.
    pub(crate) fn glue_before(&self, idx: usize) -> bool {
        self.glue_before.get(idx).copied().unwrap_or(false)
    }

    /// Whether element `idx` belongs to a recognized statement that absorbed
    /// trailing same-line material ([`absorb_trailing_junk`]) — a call unit
    /// followed by unrecognized tokens or a comment on its authored line
    /// (xparse's `\bool_if:NTF … { \cs_set:cpn } … ##1 \q_@@ …` definition
    /// trickery). Such a statement renders with every top-level gap
    /// unbreakable: its junk extent is newline-keyed (the fallback's Tier-2
    /// residue), so a width wrap moving material across a line boundary would
    /// change the extent — and with it the trailing-command glue decision —
    /// on the next pass. All-hard gaps preserve the authored line shape
    /// (node-internal layout still breaks freely and re-reads node-internal),
    /// which is a fixed point by construction.
    pub(crate) fn is_glued(&self, idx: usize) -> bool {
        self.glued.get(idx).copied().unwrap_or(false)
    }

    /// Whether element `idx` belongs to a fallback statement. A fallback line
    /// commits as a plain *greedy* fill, never the sticky fill structural
    /// statements use: greedy packing is self-fulfilling (each printed line
    /// re-segments to a fallback statement that re-fills to exactly itself),
    /// while a sticky cascade forces atoms that would fit onto their own
    /// broken lines — a shape the next pass's shorter per-line statements
    /// do not reproduce.
    pub(crate) fn is_fallback(&self, idx: usize) -> bool {
        self.fallback.get(idx).copied().unwrap_or(false)
    }
}

/// Segment an in-region element stream into statements. See the module docs
/// for the model; the caller guarantees the stream is inside an expl3 region
/// (so `:`/`_` were letters and names carry their argspec suffix).
pub(crate) fn segment_expl_statements(elements: &[SyntaxElement]) -> StatementMap {
    let mut boundary_after = vec![false; elements.len()];
    let mut glue_before = vec![false; elements.len()];
    let mut glued = vec![false; elements.len()];
    let mut fallback = vec![false; elements.len()];
    let mut i = 0;
    while i < elements.len() {
        match &elements[i] {
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => i += 1,
            // A comment, guard, or doc margin between statements ends at its
            // newline exactly as today (each is line-structured in the source);
            // the boundary keeps the next statement off its line. Comment
            // presence/own-line-ness and guard/margin column-0 are preserved
            // predicates, so the read is sanctioned.
            SyntaxElement::Token(t)
                if matches!(
                    t.kind(),
                    SyntaxKind::COMMENT | SyntaxKind::GUARD | SyntaxKind::DOC_MARGIN
                ) =>
            {
                if followed_by_newline(elements, i) {
                    boundary_after[i] = true;
                }
                i += 1;
            }
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::COMMAND => {
                // A region toggle (`\ExplSyntaxOn`, `\ProvidesExplPackage`, …)
                // is colonless but in the shared toggle name set and takes no
                // trailing call-site material beyond its greedily-attached
                // groups: a recognized zero-arity unit. Without this, every
                // region's opening line would stay a newline-keyed fallback
                // statement and strict trivia-invariance could never hold for
                // any expl3 stream.
                let slots = if node_is_expl_toggle(n) {
                    Some(Vec::new())
                } else {
                    command_name(n).and_then(|name| expl3::expl3_slots(&name))
                };
                match slots.and_then(|slots| consume_unit(elements, i, &slots)) {
                    Some(end) => {
                        let full = absorb_trailing_junk(elements, end);
                        if full > end {
                            glued[i..=full].fill(true);
                        }
                        boundary_after[full] = true;
                        i = full + 1;
                    }
                    None => {
                        i = fallback_line(
                            elements,
                            i,
                            &mut boundary_after,
                            &mut glue_before,
                            &mut fallback,
                        )
                    }
                }
            }
            _ => {
                i = fallback_line(
                    elements,
                    i,
                    &mut boundary_after,
                    &mut glue_before,
                    &mut fallback,
                )
            }
        }
    }
    StatementMap {
        boundary_after,
        glue_before,
        glued,
        fallback,
    }
}

/// Whether a `COMMAND`'s name token is one of the shared expl3 region-toggle
/// spellings (`parser::lexer::expl_toggle`).
fn node_is_expl_toggle(node: &SyntaxNode) -> bool {
    node.children_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == SyntaxKind::CONTROL_WORD)
        .is_some_and(|t| expl_toggle(t.text()).is_some())
}

/// Whether only inline whitespace separates element `idx` from the next
/// newline (or the stream end) — i.e. the element ends its physical line.
fn followed_by_newline(elements: &[SyntaxElement], idx: usize) -> bool {
    for element in &elements[idx + 1..] {
        match element {
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::WHITESPACE => {}
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::NEWLINE => return true,
            _ => return false,
        }
    }
    true
}

/// The fallback: the statement is the authored physical line, verbatim the old
/// `SplitAtNewlines` rule demoted to a per-statement escape hatch. Marks the
/// boundary after the line's last non-trivia element and returns the index to
/// resume the outer walk from.
fn fallback_line(
    elements: &[SyntaxElement],
    start: usize,
    boundary_after: &mut [bool],
    glue_before: &mut [bool],
    fallback: &mut [bool],
) -> usize {
    let mut last = start;
    let mut j = start;
    while j < elements.len() {
        match &elements[j] {
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {
                if t.kind() == SyntaxKind::NEWLINE {
                    boundary_after[last] = true;
                    fallback[start..=last].fill(true);
                    return j;
                }
                j += 1;
            }
            element => {
                // A recognized head mid-line must never start a printed
                // continuation line (see [`StatementMap::glue_before`]).
                if j > start
                    && let SyntaxElement::Node(n) = element
                    && n.kind() == SyntaxKind::COMMAND
                    && (node_is_expl_toggle(n)
                        || command_name(n).is_some_and(|name| expl3::expl3_slots(&name).is_some()))
                {
                    glue_before[j] = true;
                }
                last = j;
                j += 1;
            }
        }
    }
    boundary_after[last] = true;
    fallback[start..=last].fill(true);
    elements.len()
}

/// Extend a completed unit over trailing same-line *junk*: unrecognized
/// material — punctuation and words (`\int_use:N \c@… , %mc-num`'s comma),
/// unrecognized command tokens, a trailing comment — that the author wrote as
/// part of the call's line. The scan never crosses a newline (junk on a later
/// line stays its own fallback statement, and a recognized head is never
/// pulled apart from fallback material it shares a line with) and stops at
/// the next recognized head or toggle (the next call), a `{…}` group (a
/// statement-leading block keeps its continuation-hang treatment), or a guard
/// or doc margin (line-structured). This same-line read is part of the
/// fallback's Tier-2 residue, not the structural model; a comment stays
/// sanctioned either way (own-line-ness is a preserved predicate).
fn absorb_trailing_junk(elements: &[SyntaxElement], end: usize) -> usize {
    let mut end = end;
    let mut j = end + 1;
    while j < elements.len() {
        match &elements[j] {
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {
                if t.kind() == SyntaxKind::NEWLINE {
                    break;
                }
                j += 1;
            }
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::COMMENT => {
                end = j;
                break;
            }
            SyntaxElement::Token(t)
                if matches!(t.kind(), SyntaxKind::GUARD | SyntaxKind::DOC_MARGIN) =>
            {
                break;
            }
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::GROUP => break,
            SyntaxElement::Node(n)
                if n.kind() == SyntaxKind::COMMAND
                    && (node_is_expl_toggle(n)
                        || command_name(n)
                            .is_some_and(|name| expl3::expl3_slots(&name).is_some())) =>
            {
                break;
            }
            _ => {
                end = j;
                j += 1;
            }
        }
    }
    end
}

/// Why slot consumption stopped early.
enum Stop {
    /// A blank line: the unit ends here and the partial statement commits
    /// as-is (blank-line presence is a preserved predicate, so pass-stable).
    End,
    /// The shape scan cannot resolve the unit — degrade to [`fallback_line`].
    Abort,
}

/// Consume `slots` for the head at `head_idx`, returning the index of the last
/// sibling element the unit spans (the head itself for a zero-arity or
/// entirely head-internal unit), or `None` to degrade to the fallback.
fn consume_unit(elements: &[SyntaxElement], head_idx: usize, slots: &[Expl3Slot]) -> Option<usize> {
    let head = elements[head_idx].as_node()?;
    let mut cur = UnitCursor::new(elements, head_idx, head);
    for slot in slots {
        let took = match slot {
            Expl3Slot::SingleToken => cur.take_single_token(),
            Expl3Slot::Group | Expl3Slot::Branch => cur.take_group(),
            Expl3Slot::ParameterText => cur.take_parameter_text(),
        };
        match took {
            Ok(()) => {}
            Err(Stop::End) => break,
            Err(Stop::Abort) => return None,
        }
    }
    Some(cur.last_sib)
}

/// The consumption cursor: candidates come from the peel **queue** first (an
/// already-consumed `COMMAND`'s attached children), then from the sibling
/// stream. Trivia, comments, and `~` are skipped in place (a `~` is a space
/// token TeX skips before an undelimited argument, so it can never satisfy a
/// slot — it stays in the extent for the layout loop's tilde arm).
struct UnitCursor<'a> {
    elements: &'a [SyntaxElement],
    queue: VecDeque<SyntaxElement>,
    /// Next sibling index to pull from.
    sib: usize,
    /// Last sibling index consumed into the unit — the unit's extent.
    last_sib: usize,
    /// A peeked candidate not yet consumed; the index is its sibling position
    /// when it came from the sibling stream (`None` for queue candidates).
    peeked: Option<(SyntaxElement, Option<usize>)>,
}

impl<'a> UnitCursor<'a> {
    fn new(elements: &'a [SyntaxElement], head_idx: usize, head: &SyntaxNode) -> Self {
        let mut cur = UnitCursor {
            elements,
            queue: VecDeque::new(),
            sib: head_idx + 1,
            last_sib: head_idx,
            peeked: None,
        };
        cur.queue_children_after_name(head, false);
        cur
    }

    /// Push `node`'s children after its name token onto the queue — at the
    /// back when seeding from the head, at the **front** when peeling an
    /// argument (its children must be scanned before later siblings).
    fn queue_children_after_name(&mut self, node: &SyntaxNode, front: bool) {
        let mut seen_name = false;
        let mut after: Vec<SyntaxElement> = Vec::new();
        for child in node.children_with_tokens() {
            if seen_name {
                after.push(child);
            } else if matches!(
                child.kind(),
                SyntaxKind::CONTROL_WORD | SyntaxKind::CONTROL_SYMBOL
            ) {
                seen_name = true;
            }
        }
        if front {
            for el in after.into_iter().rev() {
                self.queue.push_front(el);
            }
        } else {
            self.queue.extend(after);
        }
    }

    /// The next slot candidate, without consuming it.
    fn peek(&mut self) -> Result<&SyntaxElement, Stop> {
        if self.peeked.is_none() {
            self.peeked = Some(self.advance()?);
        }
        Ok(&self.peeked.as_ref().expect("just filled").0)
    }

    /// Consume the next slot candidate, extending the unit over it.
    fn bump(&mut self) -> Result<SyntaxElement, Stop> {
        let (el, sib_idx) = match self.peeked.take() {
            Some(peeked) => peeked,
            None => self.advance()?,
        };
        if let Some(idx) = sib_idx {
            self.last_sib = idx;
        }
        Ok(el)
    }

    /// Scan forward to the next candidate, skipping inline trivia, comments,
    /// and `~`. A blank-line gap is [`Stop::End`]; a guard or doc margin
    /// mid-unit, or the stream running out, is [`Stop::Abort`].
    fn advance(&mut self) -> Result<(SyntaxElement, Option<usize>), Stop> {
        let mut gap_newlines = 0usize;
        loop {
            let (el, sib_idx) = if let Some(el) = self.queue.pop_front() {
                (el, None)
            } else {
                let Some(el) = self.elements.get(self.sib) else {
                    return Err(Stop::Abort);
                };
                // A blank line must end the unit *before* it is crossed, so
                // peek the newline count without consuming past it.
                if let SyntaxElement::Token(t) = el
                    && t.kind() == SyntaxKind::NEWLINE
                    && gap_newlines >= 1
                {
                    return Err(Stop::End);
                }
                let idx = self.sib;
                self.sib += 1;
                (el.clone(), Some(idx))
            };
            match &el {
                SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {
                    if t.kind() == SyntaxKind::NEWLINE {
                        gap_newlines += 1;
                        if gap_newlines >= 2 {
                            return Err(Stop::End);
                        }
                    }
                }
                SyntaxElement::Token(t) if t.kind() == SyntaxKind::COMMENT => {}
                SyntaxElement::Token(t) if t.kind() == SyntaxKind::TILDE => {}
                SyntaxElement::Token(t)
                    if matches!(t.kind(), SyntaxKind::GUARD | SyntaxKind::DOC_MARGIN) =>
                {
                    return Err(Stop::Abort);
                }
                _ => return Ok((el, sib_idx)),
            }
        }
    }

    /// An `N`/`V` slot: one token — a control sequence, a `#`-parameter, a
    /// braced group (TeX-faithful: braces around an `N` argument are grabbed
    /// whole; `N` vs `n` is convention, not matching behavior), or a `COMMAND`
    /// node whose *name* satisfies the slot and whose greedily-attached
    /// children are peeled back for the head's remaining slots.
    fn take_single_token(&mut self) -> Result<(), Stop> {
        let el = self.bump()?;
        match &el {
            SyntaxElement::Token(t)
                if matches!(
                    t.kind(),
                    SyntaxKind::CONTROL_WORD | SyntaxKind::CONTROL_SYMBOL
                ) =>
            {
                Ok(())
            }
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::HASH => {
                // `#1` (or `##1` in a nested definition): hash(es) plus one
                // parameter digit read as one parameter token.
                loop {
                    let next = self.bump()?;
                    match &next {
                        SyntaxElement::Token(t) if t.kind() == SyntaxKind::HASH => {}
                        SyntaxElement::Token(t)
                            if t.kind() == SyntaxKind::WORD && is_param_digit(t) =>
                        {
                            return Ok(());
                        }
                        _ => return Err(Stop::Abort),
                    }
                }
            }
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::COMMAND => {
                self.queue_children_after_name(n, true);
                Ok(())
            }
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::GROUP => Ok(()),
            _ => Err(Stop::Abort),
        }
    }

    /// An `n`-family or `T`/`F` slot: exactly a braced group. A bare token is
    /// legal TeX for an undelimited argument, but accepting it would let
    /// sloppy shapes (and the `\::n` expansion-driver protocol) swallow the
    /// next statement's head — those stay on the fallback path instead.
    fn take_group(&mut self) -> Result<(), Stop> {
        let el = self.bump()?;
        match &el {
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::GROUP => Ok(()),
            _ => Err(Stop::Abort),
        }
    }

    /// A `p` slot: TeX parameter text — everything up to (not including) the
    /// first explicit `{`, which is left for the following slot. Tokens and
    /// `[…]` are parameter text; a control sequence delimiting the text
    /// (`#1 \q_stop {body}`) has its own over-attached children peeled, so the
    /// terminating group is found wherever greedy attachment put it. The
    /// `#{`-terminated form works out to the same rule (the `{` opens the
    /// replacement text).
    fn take_parameter_text(&mut self) -> Result<(), Stop> {
        loop {
            if let SyntaxElement::Node(n) = self.peek()?
                && n.kind() == SyntaxKind::GROUP
            {
                return Ok(());
            }
            let el = self.bump()?;
            match &el {
                SyntaxElement::Token(_) => {}
                SyntaxElement::Node(n) if n.kind() == SyntaxKind::COMMAND => {
                    self.queue_children_after_name(n, true);
                }
                SyntaxElement::Node(n) if n.kind() == SyntaxKind::OPTIONAL => {}
                _ => return Err(Stop::Abort),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::syntax::SyntaxNode;

    /// Segment the first paragraph of `src` (which must open with
    /// `\ExplSyntaxOn` so the lexer treats `:`/`_` as letters) and render each
    /// statement's source text with whitespace collapsed, for stable
    /// assertions.
    fn statements(src: &str) -> Vec<String> {
        let parsed = parse(src);
        assert!(parsed.errors.is_empty(), "test source should parse cleanly");
        let root = SyntaxNode::new_root(parsed.green);
        let para = root
            .children()
            .find(|n| n.kind() == SyntaxKind::PARAGRAPH)
            .expect("a paragraph");
        let elements: Vec<SyntaxElement> = para.children_with_tokens().collect();
        statement_texts(&elements)
    }

    fn statement_texts(elements: &[SyntaxElement]) -> Vec<String> {
        let map = segment_expl_statements(elements);
        let mut out = Vec::new();
        let mut cur = String::new();
        for (i, el) in elements.iter().enumerate() {
            cur.push_str(&el.to_string());
            if map.boundary_after(i) {
                let text = normalize(&cur);
                if !text.is_empty() {
                    out.push(text);
                }
                cur.clear();
            }
        }
        let tail = normalize(&cur);
        if !tail.is_empty() {
            out.push(tail);
        }
        out
    }

    fn normalize(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn statements_are_structural_units() {
        // Mid-call newlines join; the colonless toggles fall back per-line.
        let got = statements(
            "\\ExplSyntaxOn\n\\tl_set:Nn \\l_a\n  { x }\n\\group_begin:\n\\ExplSyntaxOff\n",
        );
        assert_eq!(
            got,
            vec![
                "\\ExplSyntaxOn",
                "\\tl_set:Nn \\l_a { x }",
                "\\group_begin:",
                "\\ExplSyntaxOff",
            ]
        );
    }

    #[test]
    fn same_line_calls_split() {
        let got =
            statements("\\ExplSyntaxOn\n\\group_begin: \\int_zero:N \\l_a\n\\ExplSyntaxOff\n");
        assert_eq!(
            got,
            vec![
                "\\ExplSyntaxOn",
                "\\group_begin:",
                "\\int_zero:N \\l_a",
                "\\ExplSyntaxOff",
            ]
        );
    }

    #[test]
    fn npn_definition_is_one_unit() {
        // `N` takes `\foo:n`, `p` scans `#1`, `n` takes the body — across the
        // authored Allman break.
        let got =
            statements("\\ExplSyntaxOn\n\\cs_new:Npn \\foo:n #1\n  { body #1 }\n\\ExplSyntaxOff\n");
        assert_eq!(
            got,
            vec![
                "\\ExplSyntaxOn",
                "\\cs_new:Npn \\foo:n #1 { body #1 }",
                "\\ExplSyntaxOff",
            ]
        );
    }

    #[test]
    fn peel_back_reclaims_over_attached_group() {
        // Greedy attachment gives `{ body }` to `\foo:n`; the `N` slot takes
        // the name and the peeled group satisfies the outer `n` slot.
        let got = statements("\\ExplSyntaxOn\n\\cs_new:Nn \\foo:n\n  { body }\n\\ExplSyntaxOff\n");
        assert_eq!(
            got,
            vec![
                "\\ExplSyntaxOn",
                "\\cs_new:Nn \\foo:n { body }",
                "\\ExplSyntaxOff",
            ]
        );
    }

    #[test]
    fn exp_args_chain_is_one_unit() {
        let got = statements(
            "\\ExplSyntaxOn\n\\exp_args:NNo \\tl_set:Nn \\l_a { \\l_b }\n\\ExplSyntaxOff\n",
        );
        assert_eq!(
            got,
            vec![
                "\\ExplSyntaxOn",
                "\\exp_args:NNo \\tl_set:Nn \\l_a { \\l_b }",
                "\\ExplSyntaxOff",
            ]
        );
    }

    #[test]
    fn hash_parameter_satisfies_single_token_slot() {
        let got = statements("\\ExplSyntaxOn\n\\tl_set:Nn #1 { x }\n\\ExplSyntaxOff\n");
        assert_eq!(
            got,
            vec!["\\ExplSyntaxOn", "\\tl_set:Nn #1 { x }", "\\ExplSyntaxOff"]
        );
    }

    #[test]
    fn delimited_parameter_text_peels_the_body() {
        // `{ body }` greedily attached to `\q_stop`; the p-scan peels it and
        // stops there, leaving it for the trailing `n` slot.
        let got = statements(
            "\\ExplSyntaxOn\n\\cs_new:Npn \\foo:w #1 \\q_stop { body }\n\\ExplSyntaxOff\n",
        );
        assert_eq!(
            got,
            vec![
                "\\ExplSyntaxOn",
                "\\cs_new:Npn \\foo:w #1 \\q_stop { body }",
                "\\ExplSyntaxOff",
            ]
        );
    }

    #[test]
    fn unknown_head_falls_back_to_its_line() {
        // `\exp_after:wN` has no derivable arity: its authored line is the
        // statement, and the recognized call sharing that line is not split out.
        let got = statements(
            "\\ExplSyntaxOn\n\\exp_after:wN \\foo \\tl_set:Nn \\l_a { x }\n\\group_begin:\n\\ExplSyntaxOff\n",
        );
        assert_eq!(
            got,
            vec![
                "\\ExplSyntaxOn",
                "\\exp_after:wN \\foo \\tl_set:Nn \\l_a { x }",
                "\\group_begin:",
                "\\ExplSyntaxOff",
            ]
        );
    }

    #[test]
    fn shape_mismatch_falls_back() {
        // The `n` slot faces a command, not a group: the whole statement
        // degrades to newline splitting rather than swallowing the next head.
        let got = statements("\\ExplSyntaxOn\n\\tl_set:Nn\n\\l_a\n\\ExplSyntaxOff\n");
        assert_eq!(
            got,
            vec!["\\ExplSyntaxOn", "\\tl_set:Nn", "\\l_a", "\\ExplSyntaxOff"]
        );
    }

    #[test]
    fn trailing_comment_rides_the_statement() {
        let got = statements("\\ExplSyntaxOn\n\\tl_set:Nn \\l_a { x } % note\n\\ExplSyntaxOff\n");
        assert_eq!(
            got,
            vec![
                "\\ExplSyntaxOn",
                "\\tl_set:Nn \\l_a { x } % note",
                "\\ExplSyntaxOff",
            ]
        );
    }

    #[test]
    fn leftover_attached_group_rides_the_statement() {
        // `\use:n` has arity 1; the second group is over-attached to the head
        // node, and boundaries never split a node, so it stays in the unit.
        let got = statements("\\ExplSyntaxOn\n\\use:n { a } { b }\n\\ExplSyntaxOff\n");
        assert_eq!(
            got,
            vec!["\\ExplSyntaxOn", "\\use:n { a } { b }", "\\ExplSyntaxOff"]
        );
    }

    #[test]
    fn conditional_call_is_one_unit() {
        let got = statements(
            "\\ExplSyntaxOn\n\\str_if_eq:nnTF { a } { b }\n  { yes }\n  { no }\n\\ExplSyntaxOff\n",
        );
        assert_eq!(
            got,
            vec![
                "\\ExplSyntaxOn",
                "\\str_if_eq:nnTF { a } { b } { yes } { no }",
                "\\ExplSyntaxOff",
            ]
        );
    }

    #[test]
    fn blank_line_ends_the_unit() {
        // Inside a group body a blank line can sit mid-call: the unit commits
        // as-is before it, and the stranded group starts a fresh statement.
        let src = "\\ExplSyntaxOn\n\\use:n { \\tl_set:Nn \\l_a\n\n  { x } }\n\\ExplSyntaxOff\n";
        let parsed = parse(src);
        assert!(parsed.errors.is_empty());
        let root = SyntaxNode::new_root(parsed.green);
        let group = root
            .descendants()
            .find(|n| n.kind() == SyntaxKind::GROUP)
            .expect("a group");
        let body: Vec<SyntaxElement> = group
            .children_with_tokens()
            .filter(|el| !matches!(el.kind(), SyntaxKind::L_BRACE | SyntaxKind::R_BRACE))
            .collect();
        assert_eq!(statement_texts(&body), vec!["\\tl_set:Nn \\l_a", "{ x }"]);
    }
}
