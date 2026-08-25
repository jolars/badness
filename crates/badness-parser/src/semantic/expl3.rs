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
//! Like [`xparse`](super::xparse), the argspec is parsed rather than executed:
//! each letter describes an argument's call-site shape. No
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
        assert_eq!(expl3_slots("use_none_delimit_by_q_stop:w"), None);
        assert_eq!(expl3_slots("exp_after:wN"), None);
        assert_eq!(expl3_slots("tex_relax:D"), None);
        assert_eq!(expl3_slots("odd:TnF"), None);
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
        assert_eq!(expl3_slots("::n"), Some(vec![Group]));
        assert_eq!(expl3_slots(":::"), Some(vec![]));
    }

    #[test]
    fn conditional_branches_read_from_name_suffix() {
        assert_eq!(conditional_branches("tl_if_empty:nTF"), Some(2));
        assert_eq!(conditional_branches("bool_if:nT"), Some(1));
        assert_eq!(conditional_branches("bool_if:nF"), Some(1));
        assert_eq!(conditional_branches("str_if_eq:nnTF"), Some(2));
        assert_eq!(conditional_branches("int_compare:nNnTF"), Some(2));
        assert_eq!(conditional_branches("seq_map_inline:Nn"), None);
        assert_eq!(conditional_branches("prg_return_true:"), None);
        assert_eq!(conditional_branches("tl_new:N"), None);
        assert_eq!(conditional_branches("@ifpackageloaded"), None);
        assert_eq!(conditional_branches("IfBooleanTF"), None);
    }

    #[test]
    fn branches_survive_underivable_arity() {
        assert_eq!(expl3_slots("odd_if:wTF"), None);
        assert_eq!(conditional_branches("odd_if:wTF"), Some(2));
    }
}

/// Statement boundaries and attachment flags for an expl3 element stream.
pub struct StatementMap {
    flags: Vec<ElementFlags>,
}

#[derive(Clone, Copy, Default)]
struct ElementFlags(u8);

impl ElementFlags {
    const BOUNDARY_AFTER: u8 = 1 << 0;
    const GLUE_BEFORE: u8 = 1 << 1;
    const GLUED: u8 = 1 << 2;
    const FALLBACK: u8 = 1 << 3;

    fn contains(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    fn insert(&mut self, flag: u8) {
        self.0 |= flag;
    }
}

impl StatementMap {
    pub fn boundary_after(&self, idx: usize) -> bool {
        self.flags
            .get(idx)
            .is_some_and(|flags| flags.contains(ElementFlags::BOUNDARY_AFTER))
    }

    pub fn glue_before(&self, idx: usize) -> bool {
        self.flags
            .get(idx)
            .is_some_and(|flags| flags.contains(ElementFlags::GLUE_BEFORE))
    }

    pub fn is_glued(&self, idx: usize) -> bool {
        self.flags
            .get(idx)
            .is_some_and(|flags| flags.contains(ElementFlags::GLUED))
    }

    pub fn is_fallback(&self, idx: usize) -> bool {
        self.flags
            .get(idx)
            .is_some_and(|flags| flags.contains(ElementFlags::FALLBACK))
    }
}

/// Segments an expl3 element stream into statements.
pub fn segment_expl_statements(elements: &[SyntaxElement]) -> StatementMap {
    let mut flags = vec![ElementFlags::default(); elements.len()];
    let mut i = 0;
    while i < elements.len() {
        match &elements[i] {
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => i += 1,
            SyntaxElement::Token(t)
                if matches!(
                    t.kind(),
                    SyntaxKind::COMMENT | SyntaxKind::GUARD | SyntaxKind::DOC_MARGIN
                ) =>
            {
                if followed_by_newline(elements, i) {
                    flags[i].insert(ElementFlags::BOUNDARY_AFTER);
                }
                i += 1;
            }
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::COMMAND => {
                match expl3_unit(elements, i) {
                    Some(unit) => {
                        let end = unit.last;
                        let full = absorb_trailing_junk(elements, end);
                        if full > end {
                            for flags in &mut flags[i..=full] {
                                flags.insert(ElementFlags::GLUED);
                            }
                        }
                        flags[full].insert(ElementFlags::BOUNDARY_AFTER);
                        i = full + 1;
                    }
                    None => i = fallback_line(elements, i, &mut flags),
                }
            }
            _ => i = fallback_line(elements, i, &mut flags),
        }
    }
    StatementMap { flags }
}

fn node_is_expl_toggle(node: &SyntaxNode) -> bool {
    node.children_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == SyntaxKind::CONTROL_WORD)
        .is_some_and(|t| expl_toggle(t.text()).is_some())
}

fn is_recognized_head(node: &SyntaxNode) -> bool {
    node.kind() == SyntaxKind::COMMAND
        && (node_is_expl_toggle(node)
            || command_name(node).is_some_and(|name| expl3_slots(&name).is_some()))
}

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

fn fallback_line(elements: &[SyntaxElement], start: usize, flags: &mut [ElementFlags]) -> usize {
    let mut last = start;
    let mut j = start;
    while j < elements.len() {
        match &elements[j] {
            SyntaxElement::Token(t) if is_collapsible_trivia(t.kind()) => {
                if t.kind() == SyntaxKind::NEWLINE {
                    flags[last].insert(ElementFlags::BOUNDARY_AFTER);
                    for flags in &mut flags[start..=last] {
                        flags.insert(ElementFlags::FALLBACK);
                    }
                    return j;
                }
                j += 1;
            }
            element => {
                if j > start
                    && let SyntaxElement::Node(n) = element
                    && is_recognized_head(n)
                {
                    flags[j].insert(ElementFlags::GLUE_BEFORE);
                }
                last = j;
                j += 1;
                if let SyntaxElement::Node(n) = element
                    && n.kind() == SyntaxKind::COMMAND
                    && node_carries_bare_line_break(n)
                {
                    flags[last].insert(ElementFlags::BOUNDARY_AFTER);
                    for flags in &mut flags[start..=last] {
                        flags.insert(ElementFlags::FALLBACK);
                    }
                    return j;
                }
            }
        }
    }
    flags[last].insert(ElementFlags::BOUNDARY_AFTER);
    for flags in &mut flags[start..=last] {
        flags.insert(ElementFlags::FALLBACK);
    }
    elements.len()
}

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
            SyntaxElement::Node(n) if is_recognized_head(n) => {
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

enum Stop {
    End,
    Abort,
}

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
    cur.extend_over_attachable_tail();
    Some(Expl3Unit {
        last: cur.last_sib,
        branches: if complete { branches } else { Vec::new() },
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// The span and conditional branches of one parsed expl3 unit.
pub struct Expl3Unit {
    pub last: usize,
    pub branches: Vec<TextRange>,
}

/// Resolves the expl3 unit beginning at `head_idx`.
pub fn expl3_unit(elements: &[SyntaxElement], head_idx: usize) -> Option<Expl3Unit> {
    let node = elements.get(head_idx)?.as_node()?;
    if !is_recognized_head(node) {
        return None;
    }
    let slots = if node_is_expl_toggle(node) {
        Vec::new()
    } else {
        expl3_slots(&command_name(node)?)?
    };
    consume_unit(elements, head_idx, &slots)
}

struct UnitCursor<'a> {
    elements: &'a [SyntaxElement],
    queue: VecDeque<SyntaxElement>,
    sib: usize,
    last_sib: usize,
    peeked: Option<(SyntaxElement, Option<usize>)>,
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

    fn peek(&mut self) -> Result<&SyntaxElement, Stop> {
        if self.peeked.is_none() {
            self.peeked = Some(self.advance()?);
        }
        Ok(&self.peeked.as_ref().expect("just filled").0)
    }

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

    fn advance(&mut self) -> Result<(SyntaxElement, Option<usize>), Stop> {
        let mut gap_newlines = 0usize;
        loop {
            let (el, sib_idx) = if let Some(el) = self.queue.pop_front() {
                (el, None)
            } else {
                let Some(el) = self.elements.get(self.sib) else {
                    return Err(Stop::Abort);
                };
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

    fn take_single_token(&mut self) -> Result<(), Stop> {
        let el = self.bump()?;
        match &el {
            SyntaxElement::Token(t)
                if matches!(
                    t.kind(),
                    SyntaxKind::CONTROL_WORD | SyntaxKind::CONTROL_SYMBOL
                ) =>
            {
                self.chain = false;
                Ok(())
            }
            SyntaxElement::Token(t)
                if t.kind() == SyntaxKind::WORD && t.text().chars().count() == 1 =>
            {
                self.chain = false;
                Ok(())
            }
            SyntaxElement::Token(t) if t.kind() == SyntaxKind::HASH => {
                self.chain = false;
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

    fn take_group(&mut self) -> Result<SyntaxElement, Stop> {
        let el = self.bump()?;
        match &el {
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::GROUP => Ok(el),
            _ => Err(Stop::Abort),
        }
    }

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

    fn extend_over_attachable_tail(&mut self) {
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
    fn comment_in_a_consumed_slot_ends_the_fallback_line() {
        let got = statements(
            "\\ExplSyntaxOn\n\\exp_after:wN \\foo \\tl_set:Nn \\l_a\n% doc\n{ x } \\group_begin:\n\\ExplSyntaxOff\n",
        );
        assert_eq!(
            got,
            vec![
                "\\ExplSyntaxOn",
                "\\exp_after:wN \\foo \\tl_set:Nn \\l_a % doc { x }",
                "\\group_begin:",
                "\\ExplSyntaxOff",
            ]
        );
    }

    #[test]
    fn unknown_head_falls_back_to_its_line() {
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
        let head_attached = "\\ExplSyntaxOn\n\\tl_if_empty:nTF {#1} { T } { F }\n";
        assert_eq!(
            branch_texts(head_attached, head_of(head_attached, "tl_if_empty:nTF")),
            Some(vec!["{ T }".to_string(), "{ F }".to_string()])
        );

        let one_sibling = "\\ExplSyntaxOn\n\\seq_if_in:NnTF \\l_seq {item} { T } { F }\n";
        assert_eq!(
            branch_texts(one_sibling, head_of(one_sibling, "seq_if_in:NnTF")),
            Some(vec!["{ T }".to_string(), "{ F }".to_string()])
        );

        let two_siblings = "\\ExplSyntaxOn\n\\prop_get:NnNTF \\p {k} \\l { T } { F }\n";
        assert_eq!(
            branch_texts(two_siblings, head_of(two_siblings, "prop_get:NnNTF")),
            Some(vec!["{ T }".to_string(), "{ F }".to_string()])
        );

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
        let src = "\\ExplSyntaxOn\n\\odd_if:wTF \\a \\b { T } { F }\n";
        assert_eq!(branch_texts(src, head_of(src, "odd_if:wTF")), None);
    }

    #[test]
    fn a_blank_line_cut_unit_reports_no_branches() {
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
