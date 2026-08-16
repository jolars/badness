//! The expl3 call-site model: **argspec arity** for expl3 function names, and
//! the **statement segmentation** built on it.
//!
//! Two halves, both semantics layered on the syntax tree (like
//! [`define`](super::define)'s definition scan): [`expl3_slots`] derives
//! per-slot arity from the letters after the final `:` in `\cs_new:Npn`,
//! `\tl_if_empty:nTF`, …, and [`segment_expl_statements`] applies it to an
//! in-region element stream to produce the statement model the formatter's
//! expl3 layout consumes. Neither builds `Ir` or touches layout policy — a
//! wrong answer here can only produce ugly formatting downstream, never a
//! wrong tree or a lost byte.
//!
//! Like [`xparse`](super::xparse), the argspec is a spec mini-language that is
//! *parsed*, never executed (AGENTS.md decision #1): each letter names the
//! **shape** an argument takes at the call site, a bounded, purely lexical
//! fact — squarely decision #2's "the semantic layer assigns arity". No
//! signature database is involved: the name string alone carries the spec, so
//! there is nothing to curate and nothing to drift. Only meaningful inside an
//! expl3 region, where `:`/`_` are catcode-11 and the whole name lexes as one
//! `CONTROL_WORD` — callers of the segmentation guarantee the stream is
//! in-region (out-of-region, colon names lex split and everything degrades to
//! the fallback).
//!
//! The letter-by-letter model (interface3's argument specifiers):
//!
//! - `N`, `V` → [`Expl3Slot::SingleToken`]: one token, typically a control
//!   sequence (`V` differs from `N` only in *expansion*, not call-site shape).
//! - `n`, `c`, `v`, `o`, `x`, `e`, `f` → [`Expl3Slot::Group`]: one braced
//!   `{…}` group (again, the letters differ only in how the material is
//!   processed, which we never model).
//! - `T`, `F` → [`Expl3Slot::Branch`]: a braced conditional branch. Sanctioned
//!   only as a *trailing* run — in a standard argspec `T`/`F` are always last,
//!   so a mid-spec `T`/`F` is treated as unknown.
//! - `p` → [`Expl3Slot::ParameterText`]: TeX parameter text (`#1#2…`), which
//!   has no fixed token count but a static *end*: TeX's own rule that the
//!   parameter text runs to the first explicit `{`. The consumer scans by that
//!   shape.
//! - `w` (arbitrary delimiters) and `D` (kernel primitive) have no lexically
//!   derivable call-site shape → the whole name is unrecognized (`None`), as is
//!   any unknown letter (including one added to expl3 after this list was
//!   written — new letters degrade to unrecognized, never to a wrong arity).

use std::collections::VecDeque;

use rowan::TextRange;

use crate::ast::command_name;
use crate::parser::lexer::expl_toggle;
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, is_collapsible_trivia, is_param_digit};

/// The call-site shape of one expl3 argument slot, derived from an argspec letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expl3Slot {
    /// `N`, `V`: exactly one token, typically a control sequence.
    SingleToken,
    /// `n`, `c`, `v`, `o`, `x`, `e`, `f`: one braced `{…}` group.
    Group,
    /// `T`, `F`: a braced conditional branch (a [`Group`](Expl3Slot::Group) a
    /// consumer may lay out specially).
    Branch,
    /// `p`: TeX parameter text — the tokens up to (not including) the next
    /// explicit `{`.
    ParameterText,
}

/// The argument slots of an expl3 function name, read from its argspec suffix
/// (the substring after the *final* `:`), or `None` when the name has no
/// derivable call-site arity.
///
/// `Some` iff the name contains a `:` and every suffix letter is a fixed-shape
/// letter per the module docs; an empty suffix (`\scan_stop:`, `\group_end:`)
/// is `Some(vec![])` — a recognized zero-argument call. `None` for a colonless
/// name (`\def`, `\@ifpackageloaded`), or a spec containing `w`, `D`, a
/// mid-spec `T`/`F`, or any unknown letter.
pub fn expl3_slots(name: &str) -> Option<Vec<Expl3Slot>> {
    let argspec = name.rsplit_once(':')?.1;
    let chars: Vec<char> = argspec.chars().collect();
    let branches = chars
        .iter()
        .rev()
        .take_while(|c| matches!(c, 'T' | 'F'))
        .count();
    let mut slots = Vec::with_capacity(chars.len());
    for c in &chars[..chars.len() - branches] {
        // `T`/`F` never match here, so a *mid*-spec `T`/`F` (nonstandard) falls
        // through to unknown.
        slots.push(match c {
            'N' | 'V' => Expl3Slot::SingleToken,
            'n' | 'c' | 'v' | 'o' | 'x' | 'e' | 'f' => Expl3Slot::Group,
            'p' => Expl3Slot::ParameterText,
            _ => return None,
        });
    }
    slots.extend(std::iter::repeat_n(Expl3Slot::Branch, branches));
    Some(slots)
}

/// The number of trailing `T`/`F` branch arguments of an expl3 conditional, read
/// from the command *name*'s argspec (the substring after the final `:`).
/// `\tl_if_empty:nTF` → `Some(2)`, `\bool_if:nT`/`:nF` → `Some(1)`; `None` for any
/// name without a `:`-argspec ending in `T`/`F` — a non-conditional expl3 function
/// (`\seq_new:N`), or a LaTeX2e command with no colon (`\@ifpackageloaded`). In an
/// expl3 argspec `T`/`F` denote *only* the true/false branch slots, so a trailing
/// `T`/`F` run is exactly the branch count.
///
/// Deliberately **not** derived from [`expl3_slots`]: this counts the raw
/// trailing run, so a name whose *earlier* letters make the arity unrecognized
/// (a hypothetical `:wTF` shape) still reports its branches — the conditional
/// layout keys on the branches alone and must not regress when the full arity
/// model bows out.
pub fn conditional_branches(name: &str) -> Option<usize> {
    let argspec = name.rsplit_once(':')?.1;
    let n = argspec
        .chars()
        .rev()
        .take_while(|c| *c == 'T' || *c == 'F')
        .count();
    (n > 0).then_some(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Expl3Slot::*;

    #[test]
    fn slots_read_from_name_suffix() {
        assert_eq!(
            expl3_slots("cs_new:Npn"),
            Some(vec![SingleToken, ParameterText, Group])
        );
        assert_eq!(
            expl3_slots("str_if_eq:nnTF"),
            Some(vec![Group, Group, Branch, Branch])
        );
        assert_eq!(
            expl3_slots("prop_get:NnNTF"),
            Some(vec![SingleToken, Group, SingleToken, Branch, Branch])
        );
        assert_eq!(expl3_slots("tl_set:Nn"), Some(vec![SingleToken, Group]));
        assert_eq!(
            expl3_slots("exp_args:NNo"),
            Some(vec![SingleToken, SingleToken, Group])
        );
        assert_eq!(expl3_slots("tl_set:Nv"), Some(vec![SingleToken, Group]));
        assert_eq!(expl3_slots("use:c"), Some(vec![Group]));
        assert_eq!(expl3_slots("tl_set:Nx"), Some(vec![SingleToken, Group]));
    }

    #[test]
    fn zero_argument_names_are_recognized() {
        assert_eq!(expl3_slots("scan_stop:"), Some(vec![]));
        assert_eq!(expl3_slots("group_begin:"), Some(vec![]));
        assert_eq!(expl3_slots("prg_return_true:"), Some(vec![]));
    }

    #[test]
    fn underivable_specs_are_unrecognized() {
        // `w`: arbitrary delimiters; `D`: kernel primitive of arbitrary arity.
        assert_eq!(expl3_slots("use_none_delimit_by_q_stop:w"), None);
        assert_eq!(expl3_slots("exp_after:wN"), None);
        assert_eq!(expl3_slots("tex_relax:D"), None);
        // Mid-spec `T`/`F` is nonstandard, so unknown.
        assert_eq!(expl3_slots("odd:TnF"), None);
        // Unknown letter anywhere bows out entirely — never a partial arity.
        assert_eq!(expl3_slots("odd:nZn"), None);
    }

    #[test]
    fn colonless_names_are_unrecognized() {
        assert_eq!(expl3_slots("def"), None);
        assert_eq!(expl3_slots("@ifpackageloaded"), None);
        assert_eq!(expl3_slots("IfBooleanTF"), None);
        assert_eq!(expl3_slots("l_tmpa_tl"), None);
    }

    #[test]
    fn exp_internal_drivers() {
        // The `\::n` expansion drivers: name is empty, spec is real. Their
        // runtime protocol is nothing like a call site, but the greedy shape
        // rules in the consumer keep them on the fallback path anyway; the
        // lexical read here is just the suffix.
        assert_eq!(expl3_slots("::n"), Some(vec![Group]));
        assert_eq!(expl3_slots(":::"), Some(vec![]));
    }

    #[test]
    fn conditional_branches_read_from_name_suffix() {
        // Trailing `T`/`F` run in the argspec (after the final `:`) is the branch
        // count; non-conditionals and colonless 2e names are `None`.
        assert_eq!(conditional_branches("tl_if_empty:nTF"), Some(2));
        assert_eq!(conditional_branches("bool_if:nT"), Some(1));
        assert_eq!(conditional_branches("bool_if:nF"), Some(1));
        assert_eq!(conditional_branches("str_if_eq:nnTF"), Some(2));
        assert_eq!(conditional_branches("int_compare:nNnTF"), Some(2));
        assert_eq!(conditional_branches("seq_map_inline:Nn"), None);
        assert_eq!(conditional_branches("prg_return_true:"), None);
        assert_eq!(conditional_branches("tl_new:N"), None);
        // A LaTeX2e conditional has no `:`-argspec, so it is never matched (issue
        // #94's `\@ifpackageloaded` stays on the width path).
        assert_eq!(conditional_branches("@ifpackageloaded"), None);
        assert_eq!(conditional_branches("IfBooleanTF"), None);
    }

    #[test]
    fn branches_survive_underivable_arity() {
        // The documented asymmetry: arity bows out, branch count must not.
        assert_eq!(expl3_slots("odd_if:wTF"), None);
        assert_eq!(conditional_branches("odd_if:wTF"), Some(2));
    }
}

// --- Statement segmentation -------------------------------------------------
//
// Structural statement segmentation for expl3 code — the mechanism that
// retired the newline-keyed `Statements::SplitAtNewlines` boundary.
//
// [`segment_expl_statements`] walks a stream of in-region sibling elements (a
// paragraph run or a brace-group body) and decides, for every gap between
// elements, whether a statement boundary sits there. The layout loop
// (`lower_expl_code`) then commits logical lines where the map says, instead
// of where the *author's* newlines fell — retiring the unsafe
// newline-vs-space trivia read (the root of the K&R↔Allman idempotency
// family; see `formatter.md`, § Trivia-invariant layout).
//
// A statement is a **call unit**: a head `COMMAND` whose name has a derivable
// argspec arity ([`expl3_slots`]) plus the elements its slots consume.
// Consumption is a pure shape scan — no `Ir` is built here — over two sources
// in order: the head's own greedily-attached children (the parser attaches
// every trailing `{…}` regardless of arity, decision #8), then the following
// siblings. Greedy attachment routinely gives an argument to the *wrong
// owner* (`\cs_new:Nn \foo:n {body}` attaches `{body}` to `\foo:n`); when a
// `COMMAND` node satisfies a single-token slot, its own attached children are
// *peeled* back onto the front of the scan queue so they can satisfy the
// outer head's remaining slots. Only the head's argspec ever drives
// consumption — an argument's own argspec is inert data, exactly as TeX
// grabs it.
//
// The trivia the scan may read is confined to *preserved* predicates:
// - a **blank line** (a gap of two or more newlines) ends the unit where it
//   stands — the partial unit commits as-is, pass-stably, because blank-line
//   presence is preserved by the formatter;
// - a **comment** sharing a line with consumed material is transparent to
//   consumption (the layout loop makes it end its physical line; the unit
//   continues), while an **own-line** comment mid-unit ends the unit where it
//   stands exactly like a blank line — its flanking newlines bound the gap
//   (`advance` counts them across the skipped comment), so the partial unit
//   commits pass-stably. When the comment rides *inside* a greedily-attached
//   sibling, the committed unit still carries that sibling whole (boundaries
//   never split a node), so the call's text stays together anyway. A comment
//   trailing a *complete* unit is pulled into the statement so it stays on
//   the call's line. Comment presence and own-line-ness are preserved
//   predicates;
// - a lone-newline-vs-space gap is **never** read on the structural path.
//
// Anything the shape scan cannot resolve — an unrecognized head (no `:`
// suffix, or a `w`/`D`/unknown letter), a slot facing the wrong shape, a
// docstrip `GUARD` mid-unit (guarded alternative bodies make arity lie,
// issue #78), or the stream ending mid-unit — degrades that statement to the
// **fallback**: the authored physical line is the statement, exactly the old
// `SplitAtNewlines` behavior demoted to a per-line escape hatch (Tier 2; see
// `formatter.md`, § Trivia-invariant layout). Recognition is re-attempted at every
// statement start, so recognized and fallback statements interleave
// deterministically; a recognized head *mid*-fallback-line is never split
// out.

/// The statement-boundary map for one element stream: `boundary_after(i)` says
/// a statement ends in the gap after element `i`. Boundaries sit on whole
/// top-level siblings — a boundary never splits a CST node, so anything the
/// greedy parser over-attached to a consumed sibling rides along in its
/// statement.
pub struct StatementMap {
    boundary_after: Vec<bool>,
    glue_before: Vec<bool>,
    glued: Vec<bool>,
    fallback: Vec<bool>,
}

impl StatementMap {
    /// Whether a statement boundary sits in the gap after element `idx`.
    pub fn boundary_after(&self, idx: usize) -> bool {
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
    pub fn glue_before(&self, idx: usize) -> bool {
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
    pub fn is_glued(&self, idx: usize) -> bool {
        self.glued.get(idx).copied().unwrap_or(false)
    }

    /// Whether element `idx` belongs to a fallback statement. A fallback line
    /// commits as a plain *greedy* fill, never the sticky fill structural
    /// statements use: greedy packing is self-fulfilling (each printed line
    /// re-segments to a fallback statement that re-fills to exactly itself),
    /// while a sticky cascade forces atoms that would fit onto their own
    /// broken lines — a shape the next pass's shorter per-line statements
    /// do not reproduce.
    pub fn is_fallback(&self, idx: usize) -> bool {
        self.fallback.get(idx).copied().unwrap_or(false)
    }
}

/// Segment an in-region element stream into statements. See the module docs
/// for the model; the caller guarantees the stream is inside an expl3 region
/// (so `:`/`_` were letters and names carry their argspec suffix).
pub fn segment_expl_statements(elements: &[SyntaxElement]) -> StatementMap {
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
                // groups: a recognized zero-arity unit — handled inside
                // [`expl3_unit`], which resolves the whole shape. Without it,
                // every region's opening line would stay a newline-keyed
                // fallback statement and strict trivia-invariance could never
                // hold for any expl3 stream.
                match expl3_unit(elements, i) {
                    Some(unit) => {
                        let end = unit.last;
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
                        || command_name(n).is_some_and(|name| expl3_slots(&name).is_some()))
                {
                    glue_before[j] = true;
                }
                last = j;
                j += 1;
                // Arity attachment consumes a bare-token or bare-command slot
                // across a single authored newline (attachment must stay
                // newline-insensitive), so a node can now *carry* the newline
                // that used to end this physical line — fusing two authored
                // lines into one fallback statement and voiding the per-line
                // fixed point. End the statement after such a node instead.
                // The predicate is the narrowest that names the novel shape —
                // a direct-child newline whose next argument is *not* a
                // `{…}`/`[…]` (greedy attachment always produced those, and
                // those keep today's behavior) — and it is Tier-2 sound: the
                // node's interior layout is width-driven from a column the
                // hard gaps fix, so whether the break re-renders (and with it
                // this boundary) is a pure function of the tree and width,
                // reproduced identically on every pass.
                if let SyntaxElement::Node(n) = element
                    && n.kind() == SyntaxKind::COMMAND
                    && node_carries_bare_line_break(n)
                {
                    boundary_after[last] = true;
                    fallback[start..=last].fill(true);
                    return j;
                }
            }
        }
    }
    boundary_after[last] = true;
    fallback[start..=last].fill(true);
    elements.len()
}

/// Whether a command node holds a direct-child newline whose next non-trivia
/// direct child is not a braced or bracketed argument — the shape only
/// arity-directed slot consumption produces (a bare `#1`, relation, or command
/// argument taken across an authored line break), never greedy attachment,
/// which crossed newlines only on its way to a `{…}`/`[…]`. Scoped to
/// `COMMAND` nodes by the caller: a plain multi-line `GROUP` in a fallback
/// line is interior layout the per-line model always tolerated. See the
/// caller for why a fallback statement must end after such a node.
fn node_carries_bare_line_break(node: &SyntaxNode) -> bool {
    let mut after_newline = false;
    for child in node.children_with_tokens() {
        match &child {
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::NEWLINE => after_newline = true,
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {}
            SyntaxElement::Node(n)
                if matches!(n.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL) =>
            {
                after_newline = false;
            }
            _ => {
                if after_newline {
                    return true;
                }
            }
        }
    }
    false
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
                        || command_name(n).is_some_and(|name| expl3_slots(&name).is_some())) =>
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

/// Consume `slots` for the head at `head_idx`, returning the resolved unit, or
/// `None` to degrade to the fallback.
fn consume_unit(
    elements: &[SyntaxElement],
    head_idx: usize,
    slots: &[Expl3Slot],
) -> Option<Expl3Unit> {
    let head = elements[head_idx].as_node()?;
    let mut cur = UnitCursor::new(elements, head_idx, head);
    let mut branches = Vec::new();
    let mut complete = true;
    for slot in slots {
        let took = match slot {
            Expl3Slot::SingleToken => cur.take_single_token(),
            Expl3Slot::Group => cur.take_group().map(|_| ()),
            // The one slot whose *identity* escapes the scan: a branch may live
            // inside a peeled sibling, so its range is the only handle a consumer
            // can use to find it again (see [`Expl3Unit::branches`]).
            Expl3Slot::Branch => cur.take_group().map(|el| branches.push(el.text_range())),
            Expl3Slot::ParameterText => cur.take_parameter_text(),
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
    // The unit extends over the *greedy-attachable tail*: the trailing
    // `{…}`/`[…]` material that greedy attachment hangs off the unit's last
    // command. Over the greedy tree this is provably a no-op — anything
    // attachable after an unbroken command chain is already *inside* a
    // consumed node (that is what greedy means), so a sibling attachable only
    // exists past a chain-breaking token, where the extension refuses. Over an
    // arity-attached tree (decision #8) the same material sits as siblings —
    // a head owns exactly its argspec — and the extension is what keeps the
    // statement *extent* identical across the migration: TeX consumes those
    // groups through the argument command at runtime
    // (`\exp_not:N \tl_if_blank:nF {#1}` is one conceptual step), which is
    // also why the greedy-era extent covered them. Partial units extend too —
    // a comment-glued gap ends the *unit* but not greedy attachment, whose
    // own gap rules (a comment is content that resets the newline run) the
    // extension carries; a genuinely blank-cut unit stops right there, since
    // greedy attachment stops at the same blank line.
    cur.extend_over_attachable_tail();
    Some(Expl3Unit {
        last: cur.last_sib,
        // A blank line cut the unit short, so the branch list is partial. Report
        // none rather than a prefix: a layout keyed on "the branches" must never
        // see two of a `TF` call's three.
        branches: if complete { branches } else { Vec::new() },
    })
}

/// The resolved shape of one expl3 call unit — what [`consume_unit`]'s slot scan
/// learns, kept rather than discarded.
///
/// [`segment_expl_statements`] needs only `last`; the formatter's conditional
/// layout needs `branches`, because *where* greedy attachment put a branch group
/// is an accident of the surrounding tokens and must not be a layout input. In
/// `\tl_if_empty:nTF {#1} {T} {F}` the branches hang off the head command, but a
/// single-token slot breaks attachment and hands them to a sibling
/// (`\seq_if_in:NnTF \l_seq {item} {T} {F}` peels all three off `\l_seq`) or to
/// the stream itself (`\int_compare:nNnTF {a} = {1} {T} {F}`, where the relation
/// is a `WORD`). The scan resolves all three the same way, so the branch ranges
/// are the one handle that works for every shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expl3Unit {
    /// Last sibling index the unit spans (the head itself for a zero-arity or
    /// entirely head-internal unit).
    pub last: usize,
    /// The `T`/`F` branch groups, in argspec order. Empty for a non-conditional
    /// head, and also for a unit a blank line cut short before every branch slot
    /// was filled.
    pub branches: Vec<TextRange>,
}

/// Resolve the expl3 call unit headed by `elements[head_idx]`, or `None` when the
/// shape scan cannot (an unrecognized head, a slot facing the wrong shape, a
/// docstrip guard mid-unit, or the stream ending mid-unit) — exactly the
/// conditions under which [`segment_expl_statements`] degrades that statement to
/// the fallback.
///
/// Public so the formatter can ask about one head directly, without a
/// [`StatementMap`]: the conditional layout runs inside a command's attached
/// arguments too, where there are no statements to segment.
pub fn expl3_unit(elements: &[SyntaxElement], head_idx: usize) -> Option<Expl3Unit> {
    let node = elements.get(head_idx)?.as_node()?;
    if node.kind() != SyntaxKind::COMMAND {
        return None;
    }
    let slots = if node_is_expl_toggle(node) {
        Vec::new()
    } else {
        expl3_slots(&command_name(node)?)?
    };
    consume_unit(elements, head_idx, &slots)
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
    /// Whether the unit's textual tail ends in an unbroken *attachment chain*:
    /// the head or a consumed `COMMAND`, followed by nothing but groups and
    /// optionals — the shape greedy attachment hangs further `{…}` material
    /// off. Any bare-token candidate (a relation `WORD`, a `#`-parameter, a
    /// control symbol) breaks the chain, exactly where greedy attachment
    /// stops. Read by [`Self::extend_over_attachable_tail`].
    chain: bool,
}

impl<'a> UnitCursor<'a> {
    fn new(elements: &'a [SyntaxElement], head_idx: usize, head: &SyntaxNode) -> Self {
        let mut cur = UnitCursor {
            elements,
            queue: VecDeque::new(),
            sib: head_idx + 1,
            last_sib: head_idx,
            peeked: None,
            chain: true,
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

    /// An `N`/`V` slot: one token — a control sequence, a single character, a
    /// `#`-parameter, a braced group (TeX-faithful: braces around an `N`
    /// argument are grabbed whole; `N` vs `n` is convention, not matching
    /// behavior), or a `COMMAND` node whose *name* satisfies the slot and whose
    /// greedily-attached children are peeled back for the head's remaining
    /// slots.
    fn take_single_token(&mut self) -> Result<(), Stop> {
        let el = self.bump()?;
        match &el {
            SyntaxElement::Token(t)
                if matches!(
                    t.kind(),
                    SyntaxKind::CONTROL_WORD | SyntaxKind::CONTROL_SYMBOL
                ) =>
            {
                // A bare control-sequence token (a peeled definee, an orphan
                // `\)` kept as data) is not a node greedy hangs arguments off.
                self.chain = false;
                Ok(())
            }
            // A relation character: `\int_compare:nNnTF { … } = { 1 } {T} {F}`
            // (issue #106). TeX grabs one character for an undelimited
            // argument, so only a *single-character* `WORD` satisfies the slot
            // — the lexer packs a run of characters into one token, and
            // consuming a multi-character run would take material TeX leaves
            // for the next slot. That shape aborts to the fallback instead.
            SyntaxElement::Token(t)
                if t.kind() == SyntaxKind::WORD && t.text().chars().count() == 1 =>
            {
                self.chain = false;
                Ok(())
            }
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::HASH => {
                self.chain = false;
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
                self.chain = true;
                Ok(())
            }
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::GROUP => Ok(()),
            _ => Err(Stop::Abort),
        }
    }

    /// An `n`-family or `T`/`F` slot: exactly a braced group, returned so a
    /// `T`/`F` slot can record which one it took. A bare token is legal TeX for
    /// an undelimited argument, but accepting it would let sloppy shapes (and
    /// the `\::n` expansion-driver protocol) swallow the next statement's head —
    /// those stay on the fallback path instead.
    fn take_group(&mut self) -> Result<SyntaxElement, Stop> {
        let el = self.bump()?;
        match &el {
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::GROUP => Ok(el),
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
                SyntaxElement::Token(_) => self.chain = false,
                SyntaxElement::Node(n) if n.kind() == SyntaxKind::COMMAND => {
                    self.queue_children_after_name(n, true);
                    self.chain = true;
                }
                SyntaxElement::Node(n) if n.kind() == SyntaxKind::OPTIONAL => {}
                _ => return Err(Stop::Abort),
            }
        }
    }

    /// Extend a complete unit over its greedy-attachable tail: the `{…}`/`[…]`
    /// nodes that follow the unit's last command with nothing but attachable
    /// material between — exactly the run greedy attachment would hang off it.
    /// See the call site in [`consume_unit`] for why this is a no-op over the
    /// greedy tree and load-bearing over an arity-attached one.
    ///
    /// The gap rules here are *greedy's*, not [`Self::advance`]'s: attachment
    /// crosses comments, guards, and doc margins, and a comment resets the
    /// newline run (it is content on its line), so only a bare blank line
    /// stops the extension — mirroring `peek_meaningful`'s `saw_blank_line`.
    fn extend_over_attachable_tail(&mut self) {
        // Whatever is still queued (material greedy attached to a consumed
        // argument beyond the head's slots) already rides the extent through
        // its owner's node; walk it only to keep the chain state honest. A
        // queue-peeked candidate is the same case; a *sibling* peek is simply
        // dropped — the rescan below starts after the last consumed sibling,
        // so it re-encounters the element under the extension's own rules.
        if let Some((el, sib_idx)) = self.peeked.take()
            && sib_idx.is_none()
        {
            self.update_chain(&el);
        }
        while let Some(el) = self.queue.pop_front() {
            self.update_chain(&el);
        }
        if !self.chain {
            return;
        }
        let mut newlines = 0usize;
        let mut i = self.last_sib + 1;
        while let Some(el) = self.elements.get(i) {
            match el {
                SyntaxElement::Token(t) => match t.kind() {
                    SyntaxKind::NEWLINE => {
                        newlines += 1;
                        if newlines >= 2 {
                            return;
                        }
                    }
                    SyntaxKind::COMMENT => newlines = 0,
                    SyntaxKind::WHITESPACE | SyntaxKind::GUARD | SyntaxKind::DOC_MARGIN => {}
                    _ => return,
                },
                SyntaxElement::Node(n)
                    if matches!(n.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL) =>
                {
                    self.last_sib = i;
                    newlines = 0;
                }
                SyntaxElement::Node(_) => return,
            }
            i += 1;
        }
    }

    /// The [`Self::chain`] update for one already-consumed element, shared by
    /// the tail walk over the leftover queue.
    fn update_chain(&mut self, el: &SyntaxElement) {
        match el {
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::COMMAND => self.chain = true,
            SyntaxElement::Node(n)
                if matches!(n.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL) => {}
            SyntaxElement::Token(t)
                if is_collapsible_trivia(t.kind())
                    || matches!(
                        t.kind(),
                        SyntaxKind::COMMENT
                            | SyntaxKind::TILDE
                            | SyntaxKind::GUARD
                            | SyntaxKind::DOC_MARGIN
                    ) => {}
            _ => self.chain = false,
        }
    }
}

#[cfg(test)]
mod segmentation_tests {
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
    fn relation_character_satisfies_single_token_slot() {
        // `\int_compare:nNnTF`'s `N` slot is the relation `=` (issue #106).
        // Without it the whole conditional degraded to the newline-keyed
        // fallback, so the trailing call's line was authored, not derived.
        let got = statements(
            "\\ExplSyntaxOn\n\\int_compare:nNnTF { \\l_a } = { 1 } { yes } { no } \\foo:\n\\ExplSyntaxOff\n",
        );
        assert_eq!(
            got,
            vec![
                "\\ExplSyntaxOn",
                "\\int_compare:nNnTF { \\l_a } = { 1 } { yes } { no }",
                "\\foo:",
                "\\ExplSyntaxOff",
            ]
        );
    }

    #[test]
    fn relation_character_unit_is_newline_invariant() {
        // The same call broken across lines segments identically — the point
        // of the structural model.
        let inline = statements(
            "\\ExplSyntaxOn\n\\int_compare:nNnTF { \\l_a } = { 1 } { yes } { no } \\foo:\n\\ExplSyntaxOff\n",
        );
        let broken = statements(
            "\\ExplSyntaxOn\n\\int_compare:nNnTF { \\l_a } = { 1 }\n  { yes } { no }\n\\foo:\n\\ExplSyntaxOff\n",
        );
        assert_eq!(inline, broken);
    }

    #[test]
    fn multi_character_word_does_not_satisfy_single_token_slot() {
        // TeX grabs one character for an undelimited argument, so a lexed run
        // of characters is the wrong shape and degrades to the fallback (here:
        // the authored line).
        let got = statements(
            "\\ExplSyntaxOn\n\\int_compare:nNnT { \\l_a } <= { 1 } { yes }\n\\foo:\n\\ExplSyntaxOff\n",
        );
        assert_eq!(
            got,
            vec![
                "\\ExplSyntaxOn",
                "\\int_compare:nNnT { \\l_a } <= { 1 } { yes }",
                "\\foo:",
                "\\ExplSyntaxOff",
            ]
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

    /// The source text of each `T`/`F` branch [`expl3_unit`] resolved for the
    /// head at index `head`, whitespace-collapsed for stable assertions.
    fn branch_texts(src: &str, head: usize) -> Option<Vec<String>> {
        let parsed = parse(src);
        assert!(parsed.errors.is_empty(), "test source should parse cleanly");
        let root = SyntaxNode::new_root(parsed.green);
        let para = root
            .children()
            .find(|n| n.kind() == SyntaxKind::PARAGRAPH)
            .expect("a paragraph");
        let elements: Vec<SyntaxElement> = para.children_with_tokens().collect();
        let unit = expl3_unit(&elements, head)?;
        Some(
            unit.branches
                .iter()
                .map(|range| normalize(&root.text().slice(*range).to_string()))
                .collect(),
        )
    }

    /// The sibling index of the `COMMAND` named `name` in the first paragraph.
    /// Keyed on the name rather than on position because the leading
    /// `\ExplSyntaxOn` is itself a `COMMAND` — and a recognized zero-arity unit.
    fn head_of(src: &str, name: &str) -> usize {
        let parsed = parse(src);
        let root = SyntaxNode::new_root(parsed.green);
        let para = root
            .children()
            .find(|n| n.kind() == SyntaxKind::PARAGRAPH)
            .expect("a paragraph");
        para.children_with_tokens()
            .position(|el| {
                el.as_node().is_some_and(|n| {
                    n.kind() == SyntaxKind::COMMAND
                        && command_name(n).is_some_and(|got| got == name)
                })
            })
            .unwrap_or_else(|| panic!("no command named {name}"))
    }

    #[test]
    fn branches_are_resolved_wherever_attachment_put_them() {
        // The point of [`Expl3Unit::branches`]: the same call shape, with the
        // branch groups on the head, peeled off one sibling, split across two,
        // and at the stream level — all four resolve to the same two branches.
        let head_attached = "\\ExplSyntaxOn\n\\tl_if_empty:nTF {#1} { T } { F }\n";
        assert_eq!(
            branch_texts(head_attached, head_of(head_attached, "tl_if_empty:nTF")),
            Some(vec!["{ T }".to_string(), "{ F }".to_string()])
        );

        // `\l_seq` swallowed all three trailing groups; the `n` slot takes the
        // first back off the peel queue and the branches are the other two.
        let one_sibling = "\\ExplSyntaxOn\n\\seq_if_in:NnTF \\l_seq {item} { T } { F }\n";
        assert_eq!(
            branch_texts(one_sibling, head_of(one_sibling, "seq_if_in:NnTF")),
            Some(vec!["{ T }".to_string(), "{ F }".to_string()])
        );

        // The TODO's own example: `{k}` on `\p`, both branches on `\l`.
        let two_siblings = "\\ExplSyntaxOn\n\\prop_get:NnNTF \\p {k} \\l { T } { F }\n";
        assert_eq!(
            branch_texts(two_siblings, head_of(two_siblings, "prop_get:NnNTF")),
            Some(vec!["{ T }".to_string(), "{ F }".to_string()])
        );

        // A `WORD` relation breaks attachment outright, so every group after it
        // is a top-level sibling (issue #106).
        let stream_level = "\\ExplSyntaxOn\n\\int_compare:nNnTF {a} = { 1 } { T } { F }\n";
        assert_eq!(
            branch_texts(stream_level, head_of(stream_level, "int_compare:nNnTF")),
            Some(vec!["{ T }".to_string(), "{ F }".to_string()])
        );
    }

    #[test]
    fn a_non_conditional_unit_has_no_branches() {
        let src = "\\ExplSyntaxOn\n\\tl_set:Nn \\l_a { x }\n";
        assert_eq!(branch_texts(src, head_of(src, "tl_set:Nn")), Some(vec![]));
    }

    #[test]
    fn an_underivable_head_resolves_no_unit() {
        // `conditional_branches` still reports 2 for `:wTF`
        // ([`branches_survive_underivable_arity`]), but the arity model bows out,
        // so there is no unit and no branch list — the consumer must not be handed
        // a guess.
        let src = "\\ExplSyntaxOn\n\\odd_if:wTF \\a \\b { T } { F }\n";
        assert_eq!(branch_texts(src, head_of(src, "odd_if:wTF")), None);
    }

    #[test]
    fn a_blank_line_cut_unit_reports_no_branches() {
        // The unit still commits as far as it got (`last` is real), but a partial
        // branch list must never drive a layout keyed on "the branches" — a `TF`
        // call would otherwise explode with one of its two. Inside a group body,
        // because at the *stream* level a blank line ends the paragraph and the
        // unit aborts on the stream end instead ([`Stop::Abort`], no unit at all).
        let src = "\\ExplSyntaxOn\n\\use:n { \\prop_get:NnNTF \\p {k} \\l { T }\n\n{ F } }\n";
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
        let head = body
            .iter()
            .position(|el| el.as_node().is_some())
            .expect("the head command");
        let unit = expl3_unit(&body, head).expect("the partial unit still resolves");
        assert_eq!(unit.branches, vec![]);
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

    #[test]
    fn guard_mid_unit_aborts_to_fallback() {
        // A docstrip guard inside the unit (issue #78: guarded alternative
        // bodies make arity lie) aborts consumption; the statement degrades to
        // the fallback, and the guard-bearing sibling rides it whole because
        // boundaries never split a node.
        use crate::parser::lexer::LexConfig;
        use crate::parser::{LatexFlavor, parse_with_flavor};
        let src = "% \\begin{macrocode}\n\\ExplSyntaxOn\n\\tl_set:Nn \\l_a\n%<latexrelease>  { x }\n\\ExplSyntaxOff\n% \\end{macrocode}\n";
        let config = LexConfig {
            flavor: LatexFlavor::Package,
            dtx: true,
        };
        let parsed = parse_with_flavor(src, config);
        assert!(parsed.errors.is_empty(), "test source should parse cleanly");
        let root = SyntaxNode::new_root(parsed.green);
        let para = root
            .descendants()
            .find(|n| n.kind() == SyntaxKind::PARAGRAPH)
            .expect("a paragraph");
        let elements: Vec<SyntaxElement> = para.children_with_tokens().collect();
        let map = segment_expl_statements(&elements);
        assert_eq!(
            statement_texts(&elements),
            vec![
                "\\ExplSyntaxOn",
                "\\tl_set:Nn \\l_a %<latexrelease> { x }",
                "\\ExplSyntaxOff",
            ]
        );
        let guarded_end = elements
            .iter()
            .position(|el| el.to_string().contains("latexrelease"))
            .expect("the guarded sibling");
        assert!(
            map.is_fallback(guarded_end),
            "the aborted unit must be a fallback statement"
        );
    }

    #[test]
    fn e_and_f_letters_consume_braced_groups() {
        let got = statements(
            "\\ExplSyntaxOn\n\\tl_set:Ne \\l_a\n  { x }\n\\tl_set:Nf \\l_b\n  { y }\n\\ExplSyntaxOff\n",
        );
        assert_eq!(
            got,
            vec![
                "\\ExplSyntaxOn",
                "\\tl_set:Ne \\l_a { x }",
                "\\tl_set:Nf \\l_b { y }",
                "\\ExplSyntaxOff",
            ]
        );
    }

    #[test]
    fn stream_ending_mid_unit_falls_back() {
        // The `n` slot is still open when the group body runs out: the unit
        // aborts to the fallback rather than committing a partial unit.
        let src = "\\ExplSyntaxOn\n\\use:n { \\tl_set:Nn \\l_a }\n\\ExplSyntaxOff\n";
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
        let map = segment_expl_statements(&body);
        assert_eq!(statement_texts(&body), vec!["\\tl_set:Nn \\l_a"]);
        let head = body
            .iter()
            .position(|el| el.as_node().is_some())
            .expect("the head command");
        assert!(
            map.is_fallback(head),
            "a unit cut off by the stream end must be a fallback statement"
        );
    }

    #[test]
    fn a_multi_line_group_node_does_not_end_a_fallback_line() {
        // [`fallback_line`] scans *sibling* `NEWLINE` tokens only, so a group
        // whose body spans several source lines carries those newlines inside
        // the node and the fallback statement runs straight past it: the group
        // and the following recognized head are one statement, and that head
        // still owes an unbreakable `glue_before` space. The formatter's
        // hanging-group dispatch relies on this — a forced-break commit there
        // would split a pair the segmentation kept together (latex2e's
        // `lipsum.sty`).
        // The `>` keeps the block a *sibling* of the head rather than a
        // greedily-attached argument, as in `\int_do_until:nNnn`'s real shape.
        let src = "\\ExplSyntaxOn\n\
                   \\int_do_until:w { \\l_tmpa_int } > {#2}\n\
                   { \\lipsum_add:V { \\l_tmpa_int }\n\
                   \\int_incr:N \\l_tmpa_int } \\tl_put_right:NV \\l_a \\l_b\n\
                   \\ExplSyntaxOff\n";
        let parsed = parse(src);
        assert!(parsed.errors.is_empty());
        let root = SyntaxNode::new_root(parsed.green);
        let elements: Vec<SyntaxElement> = root
            .first_child()
            .expect("the paragraph")
            .children_with_tokens()
            .collect();
        let map = segment_expl_statements(&elements);

        // `\int_do_until:w` is underivable (`w`), so its line degrades to the
        // fallback. The block starts the next fallback line, which then runs
        // past the block's *internal* newlines and absorbs the
        // `\tl_put_right:NV` call sharing the block's closing line.
        assert_eq!(
            statement_texts(&elements),
            vec![
                "\\ExplSyntaxOn",
                "\\int_do_until:w { \\l_tmpa_int } > {#2}",
                "{ \\lipsum_add:V { \\l_tmpa_int } \\int_incr:N \\l_tmpa_int } \
                 \\tl_put_right:NV \\l_a \\l_b",
                "\\ExplSyntaxOff",
            ]
        );

        let group = elements
            .iter()
            .position(|el| el.kind() == SyntaxKind::GROUP && el.to_string().contains('\n'))
            .expect("the multi-line group");
        assert!(
            map.is_fallback(group),
            "the group belongs to a fallback statement"
        );
        assert!(
            !map.boundary_after(group),
            "a multi-line group's own newlines must not end the fallback line"
        );

        let head = elements
            .iter()
            .skip(group)
            .position(|el| {
                el.as_node()
                    .is_some_and(|n| n.kind() == SyntaxKind::COMMAND)
            })
            .map(|off| group + off)
            .expect("the trailing recognized head");
        assert!(
            map.glue_before(head),
            "a recognized head mid-fallback-line owes an unbreakable gap"
        );
    }

    #[test]
    fn own_line_comment_in_attached_span_rides_the_sibling() {
        // The own-line comment's flanking newlines bound the gap like a blank
        // line, ending the unit at the `N` slot — but greedy attachment put
        // the comment *and* the group inside the `\l_a` sibling, and
        // boundaries never split a node, so the committed partial unit still
        // carries the whole sibling. Pass-stable either way (comment
        // own-line-ness is a preserved predicate).
        let got =
            statements("\\ExplSyntaxOn\n\\tl_set:Nn \\l_a\n% note\n  { x }\n\\ExplSyntaxOff\n");
        assert_eq!(
            got,
            vec![
                "\\ExplSyntaxOn",
                "\\tl_set:Nn \\l_a % note { x }",
                "\\ExplSyntaxOff",
            ]
        );
    }

    #[test]
    fn own_line_comment_at_sibling_level_ends_the_unit() {
        // Before a candidate no comment can bind to (`#1` parameter text, not
        // a `COMMAND`), the own-line comment stays a sibling: the unit ends at
        // the gap, the comment keeps its own line, and the leftover material
        // falls back per-line.
        let got = statements(
            "\\ExplSyntaxOn\n\\cs_new:Npn \\foo:n\n% note\n#1 { body }\n\\ExplSyntaxOff\n",
        );
        assert_eq!(
            got,
            vec![
                "\\ExplSyntaxOn",
                "\\cs_new:Npn \\foo:n",
                "% note",
                "#1 { body }",
                "\\ExplSyntaxOff",
            ]
        );
    }
}
