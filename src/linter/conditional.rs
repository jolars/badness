//! Shared conditional-branch pre-pass: which `\if…\else…\fi` branches a byte
//! offset sits in, so duplicate-detection rules can treat loads or labels in
//! mutually exclusive branches as non-duplicates
//! (`\iftrue\usepackage{p}\else\usepackage{p}\fi` loads `p` exactly once no
//! matter which branch TeX takes).
//!
//! Computed once per file by [`super::rules::RuleContext`] — the same
//! precomputed, read-only, tree-derived side index as `math_regions` — and
//! queried by byte offset ([`ConditionalIndex::path_at`]). Rules never track
//! conditional state themselves.
//!
//! **Pair-and-trust.** Any control word with a lowercase `if` prefix opens a
//! frame, and its branches count as mutually exclusive — `\iftrue` and a
//! `\newif`-defined `\ifmyflag` alike. Pairing `\else`/`\or`/`\fi` against
//! *every* `if*` opener (rather than a curated primitive list) is what keeps
//! the stack in sync: an unrecognized conditional's `\else` must bump its own
//! frame, never an enclosing one. The exceptions are a curated
//! [`NOT_FI_TERMINATED`] denylist of `if*`-named *macros* that take brace
//! arguments instead of a `\fi` terminator (`\iff`, `\ifthenelse`, the
//! etoolbox test family); the list is judgment-curated and trivially
//! extensible. Capital-`If` commands (`\IfFileExists`, xparse `\IfValueTF`)
//! never match the lowercase prefix.
//!
//! Two further sources of stray conditional tokens are neutralized:
//!
//! - **Operand positions.** `\ifx\ifabc\iftrue` tests two `if*`-named tokens
//!   without running them; [`OPERAND_SKIPS`] suppresses conditional
//!   interpretation for the following operand tokens (`\newif\ifmyflag` and
//!   `\let\ifpdf\iftrue` are the declaration-side analogs). `\ifcsname`
//!   skips everything up to `\endcsname`. A flow word (`\else`/`\or`/`\fi`)
//!   cancels a pending skip, so textual operands (`\if ab\else`) never eat
//!   real control flow.
//! - **Definition bodies.** Tokens inside `\newcommand{\x}{\else}` or
//!   `\def\stopit{\fi}` are code carried, not executed; the span of a
//!   definition command (per [`crate::semantic::define::is_definition_command`])
//!   is skipped wholesale.
//!
//! Known limitations, accepted rather than chased: `\def\x#1{\fi}` (parameter
//! text between name and body) may under-cover the definition span, and a live
//! opener sitting in an operand slot (`\if ab\ifsomething`) fails to open a
//! frame — in both cases the flow-word handling degrades gracefully (a stray
//! `\fi` on an empty stack is a no-op). Loads and labels *inside* definition
//! bodies keep being counted by the rules; only conditional interpretation is
//! suppressed there.

use rowan::TextSize;

use crate::ast::command_name;
use crate::semantic::define::is_definition_command;
use crate::syntax::{SyntaxKind, SyntaxNode};

/// One open conditional at a point in the document: `id` names the specific
/// `\if…\fi` instance (assigned in document order when it opens), `branch`
/// counts the `\else`/`\or` tokens seen inside it so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Frame {
    id: u32,
    branch: u32,
}

/// The conditional branch path at every state-change offset, for binary-search
/// lookup by byte offset.
pub(crate) struct ConditionalIndex {
    /// `(offset, path)` per conditional token, in strictly increasing offset
    /// order (each snapshot comes from a distinct `COMMAND` node in preorder).
    /// The path is the state *after* the token. Cloning the whole stack per
    /// state change is cheap: real documents hold a handful of conditional
    /// tokens, and nesting is shallow.
    snapshots: Vec<(TextSize, Vec<Frame>)>,
}

/// `if*`-named control words that are **not** `\fi`-terminated conditionals:
/// ordinary macros taking brace arguments (`{true}{false}`), which must not
/// open a frame. Curated — amsmath's `\iff` arrow, ifthen's `\ifthenelse`,
/// babel's `\iflanguage`, and etoolbox's test family. Extend freely; a missing
/// entry costs a desynced stack only until its `\fi`-less shape is added here.
const NOT_FI_TERMINATED: &[&str] = &[
    "iff",        // amsmath: the ⟺ arrow, not a conditional
    "ifthenelse", // ifthen/xifthen: {test}{then}{else}
    "iflanguage", // babel: {lang}{then}{else}
    "iftoggle",   // etoolbox toggles: {toggle}{then}{else}
    // etoolbox def/cs/str/bool/num/dim tests, all brace-argument shaped:
    "ifdef",
    "ifcsdef",
    "ifundef",
    "ifcsundef",
    "ifdefmacro",
    "ifcsmacro",
    "ifdefempty",
    "ifcsempty",
    "ifdefvoid",
    "ifcsvoid",
    "ifdefstring",
    "ifcsstring",
    "ifdefequal",
    "ifcsequal",
    "ifbool",
    "ifboolexpr",
    "ifboolexpe",
    "ifstrequal",
    "ifstrempty",
    "ifblank",
    "ifnumcomp",
    "ifnumequal",
    "ifnumgreater",
    "ifnumless",
    "ifnumodd",
    "ifdimcomp",
    "ifdimequal",
    "ifdimgreater",
    "ifdimless",
];

/// Commands whose next N command tokens are *operands* (tokens being tested or
/// aliased), not live control flow: `\if`/`\ifx`/`\ifcat` compare two tokens,
/// eTeX's `\ifdefined` tests one, `\newif\ifmyflag` declares one, and
/// `\let\ifpdf\iftrue` aliases two. `\ifcsname` is handled separately
/// (skip-until-`\endcsname`); `\ifincsname` takes no operand at all.
const OPERAND_SKIPS: &[(&str, u8)] = &[
    ("if", 2),
    ("ifx", 2),
    ("ifcat", 2),
    ("ifdefined", 1),
    ("newif", 1),
    ("let", 2),
];

/// Whether two branch paths are provably mutually exclusive: some conditional
/// instance contains both sites in *different* branches. The positional zip is
/// sound because a frame id always occupies one nesting depth — two paths that
/// share a frame share every frame above it, at the same positions.
pub(crate) fn mutually_exclusive(a: &[Frame], b: &[Frame]) -> bool {
    a.iter()
        .zip(b)
        .any(|(x, y)| x.id == y.id && x.branch != y.branch)
}

impl ConditionalIndex {
    /// Walk `root`'s `COMMAND` nodes in preorder (document order) and record a
    /// path snapshot at every conditional state change.
    pub(crate) fn compute(root: &SyntaxNode) -> Self {
        let mut stack: Vec<Frame> = Vec::new();
        let mut next_id = 0u32;
        let mut snapshots: Vec<(TextSize, Vec<Frame>)> = Vec::new();
        let mut pending_skips = 0u8;
        let mut in_csname = false;
        let mut suppress_until = TextSize::from(0);

        for node in root.descendants() {
            if node.kind() != SyntaxKind::COMMAND {
                continue;
            }
            let start = node.text_range().start();
            if start < suppress_until {
                continue;
            }
            let Some(name) = command_name(&node) else {
                continue;
            };
            if is_definition_command(&name) {
                suppress_until = suppress_until.max(definition_span_end(&node));
                continue;
            }
            let is_flow = matches!(name.as_str(), "else" | "or" | "fi");
            if in_csname {
                if name == "endcsname" {
                    in_csname = false;
                    continue;
                }
                if !is_flow {
                    continue;
                }
                // Malformed input (an `\ifcsname` never closed): a flow word
                // re-enables interpretation rather than going dark to EOF.
                in_csname = false;
            }
            if pending_skips > 0 {
                if is_flow {
                    pending_skips = 0;
                } else {
                    pending_skips -= 1;
                    continue;
                }
            }
            match name.as_str() {
                "else" | "or" => {
                    if let Some(top) = stack.last_mut() {
                        top.branch += 1;
                        snapshots.push((start, stack.clone()));
                    }
                }
                "fi" => {
                    if stack.pop().is_some() {
                        snapshots.push((start, stack.clone()));
                    }
                }
                _ if is_conditional_opener(&name) => {
                    stack.push(Frame {
                        id: next_id,
                        branch: 0,
                    });
                    next_id += 1;
                    if name == "ifcsname" {
                        in_csname = true;
                    } else if let Some(&(_, n)) = OPERAND_SKIPS.iter().find(|(op, _)| *op == name) {
                        pending_skips = n;
                    }
                    snapshots.push((start, stack.clone()));
                }
                _ => {
                    // `\newif`/`\let`: an operand skip without opening a frame.
                    if let Some(&(_, n)) = OPERAND_SKIPS.iter().find(|(op, _)| *op == name) {
                        pending_skips = n;
                    }
                }
            }
        }
        Self { snapshots }
    }

    /// The branch path in effect at byte `offset`: the latest snapshot at or
    /// before it, or the empty (unconditional) path. A load or label never
    /// shares a start offset with a conditional token, so `<=` is unambiguous.
    pub(crate) fn path_at(&self, offset: usize) -> &[Frame] {
        let offset = TextSize::from(offset as u32);
        let i = self.snapshots.partition_point(|(s, _)| *s <= offset);
        if i == 0 {
            &[]
        } else {
            &self.snapshots[i - 1].1
        }
    }
}

/// Pair-and-trust: a lowercase-`if`-prefixed name opens a frame unless it is a
/// known brace-argument macro.
fn is_conditional_opener(name: &str) -> bool {
    name.starts_with("if") && !NOT_FI_TERMINATED.contains(&name)
}

/// The end of a definition command's span, mirroring the definition scanner's
/// braced/unbraced dispatch (`semantic::define::resolve_command_def`): with a
/// braced name (`\newcommand{\x}{…}`, `\newenvironment{e}{…}{…}`) every group
/// hangs off the definition command itself; unbraced (`\def\stopit{\fi}`) the
/// body attaches to the adjacent sibling `COMMAND`.
fn definition_span_end(command: &SyntaxNode) -> TextSize {
    let own = command.text_range().end();
    if command.children().any(|c| c.kind() == SyntaxKind::GROUP) {
        return own;
    }
    match adjacent_sibling_command(command) {
        Some(sibling) => own.max(sibling.text_range().end()),
        None => own,
    }
}

/// The immediately-following sibling `COMMAND`, separated by trivia only.
/// Mirrors the definition scanner's private helper of the same name.
fn adjacent_sibling_command(command: &SyntaxNode) -> Option<SyntaxNode> {
    let mut next = command.next_sibling_or_token();
    while let Some(element) = next {
        match element {
            rowan::NodeOrToken::Token(token) if is_trivia(token.kind()) => {
                next = token.next_sibling_or_token();
            }
            rowan::NodeOrToken::Node(node) if node.kind() == SyntaxKind::COMMAND => {
                return Some(node);
            }
            _ => return None,
        }
    }
    None
}

/// Whether `kind` is trivia (whitespace/newline/comment); the fixed trivia set
/// of AGENTS.md decision #9, as in the sibling rule-local copies.
fn is_trivia(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn index(src: &str) -> ConditionalIndex {
        let root = SyntaxNode::new_root(parse(src).green);
        ConditionalIndex::compute(&root)
    }

    /// The byte offset of `needle`'s `n`-th occurrence in `src`.
    fn offset(src: &str, needle: &str, n: usize) -> usize {
        src.match_indices(needle)
            .nth(n)
            .map(|(i, _)| i)
            .unwrap_or_else(|| panic!("occurrence {n} of {needle:?} in {src:?}"))
    }

    /// Branch paths at the two occurrences of `\usepackage` in `src`.
    fn paths_at_loads<'a>(idx: &'a ConditionalIndex, src: &str) -> (&'a [Frame], &'a [Frame]) {
        (
            idx.path_at(offset(src, "\\usepackage", 0)),
            idx.path_at(offset(src, "\\usepackage", 1)),
        )
    }

    #[test]
    fn if_else_branches_are_mutually_exclusive() {
        let src = "\\iftrue\\usepackage{a}\\else\\usepackage{a}\\fi\n";
        let idx = index(src);
        let (a, b) = paths_at_loads(&idx, src);
        assert!(mutually_exclusive(a, b));
    }

    #[test]
    fn same_branch_is_not_exclusive() {
        let src = "\\iftrue\\usepackage{a}\\usepackage{a}\\else x\\fi\n";
        let idx = index(src);
        let (a, b) = paths_at_loads(&idx, src);
        assert!(!mutually_exclusive(a, b));
    }

    #[test]
    fn ifcase_or_branches_are_pairwise_exclusive() {
        let src = "\\ifcase 0 \\usepackage{a}\\or\\usepackage{a}\\or\\usepackage{a}\\fi\n";
        let idx = index(src);
        let a = idx.path_at(offset(src, "\\usepackage", 0));
        let b = idx.path_at(offset(src, "\\usepackage", 1));
        let c = idx.path_at(offset(src, "\\usepackage", 2));
        assert!(mutually_exclusive(a, b));
        assert!(mutually_exclusive(b, c));
        assert!(mutually_exclusive(a, c));
    }

    #[test]
    fn unconditional_site_is_never_exclusive() {
        let src = "\\iftrue\\usepackage{a}\\fi\n\\usepackage{a}\n";
        let idx = index(src);
        let (a, b) = paths_at_loads(&idx, src);
        assert!(b.is_empty());
        assert!(!mutually_exclusive(a, b));
        assert!(!mutually_exclusive(b, b));
    }

    #[test]
    fn nested_conditionals_compare_by_shared_frame() {
        // Load 1 in the outer then-branch; load 2 in a conditional nested in
        // the outer else-branch. Exclusive via the shared outer frame.
        let src = "\\iftrue\\usepackage{a}\\else\\ifodd 1 \\usepackage{a}\\fi\\fi\n";
        let idx = index(src);
        let (a, b) = paths_at_loads(&idx, src);
        assert!(mutually_exclusive(a, b));
    }

    #[test]
    fn unknown_conditional_is_paired_and_trusted() {
        let src = "\\ifmyflag\\usepackage{a}\\else\\usepackage{a}\\fi\n";
        let idx = index(src);
        let (a, b) = paths_at_loads(&idx, src);
        assert!(mutually_exclusive(a, b));
    }

    #[test]
    fn unknown_conditional_nested_in_known_resyncs() {
        // The inner `\ifmyflag x\fi` pairs with itself; the outer `\iftrue`
        // branches stay exclusive.
        let src = "\\iftrue\\ifmyflag x\\fi\\usepackage{a}\\else\\usepackage{a}\\fi\n";
        let idx = index(src);
        let (a, b) = paths_at_loads(&idx, src);
        assert!(mutually_exclusive(a, b));
    }

    #[test]
    fn unknown_conditionals_else_bumps_its_own_frame() {
        // The `\else` belongs to `\ifmyflag`, so both loads stay in the outer
        // then-branch: not exclusive.
        let src = "\\iftrue\\usepackage{a}\\ifmyflag\\else\\usepackage{a}\\fi\\fi\n";
        let idx = index(src);
        let (a, b) = paths_at_loads(&idx, src);
        assert!(!mutually_exclusive(a, b));
    }

    #[test]
    fn ifx_operands_open_no_frames() {
        let src = "\\ifx\\ifabc\\ifxyz x\\fi\n done";
        let idx = index(src);
        // Inside the body exactly the `\ifx` frame is open.
        assert_eq!(idx.path_at(offset(src, "x\\fi", 0)).len(), 1);
        assert!(idx.path_at(offset(src, "done", 0)).is_empty());
    }

    #[test]
    fn ifdefined_operand_opens_no_frame() {
        let src = "\\ifdefined\\iffalse x\\fi\n done";
        let idx = index(src);
        assert!(idx.path_at(offset(src, "done", 0)).is_empty());
    }

    #[test]
    fn textual_operands_do_not_eat_the_else() {
        // `\if ab` has character operands; the pending skip must not swallow
        // `\else`, so the two loads are exclusive.
        let src = "\\if ab\\usepackage{a}\\else\\usepackage{a}\\fi\n";
        let idx = index(src);
        let (a, b) = paths_at_loads(&idx, src);
        assert!(mutually_exclusive(a, b));
    }

    #[test]
    fn newif_declaration_opens_no_frame() {
        let src = "\\newif\\ifmyflag\n done";
        let idx = index(src);
        assert!(idx.path_at(offset(src, "done", 0)).is_empty());
    }

    #[test]
    fn let_aliasing_opens_no_frame() {
        let src = "\\let\\ifabc\\iftrue\n done";
        let idx = index(src);
        assert!(idx.path_at(offset(src, "done", 0)).is_empty());
    }

    #[test]
    fn ifcsname_material_is_skipped_and_pairs() {
        let src = "\\ifcsname iftex\\endcsname\\usepackage{a}\\else\\usepackage{a}\\fi\n done";
        let idx = index(src);
        let (a, b) = paths_at_loads(&idx, src);
        assert!(mutually_exclusive(a, b));
        assert!(idx.path_at(offset(src, "done", 0)).is_empty());
    }

    #[test]
    fn definition_bodies_change_no_state() {
        let src = "\\iftrue x\\newcommand{\\x}{\\else}\\def\\stopit{\\fi} y\\fi\n done";
        let idx = index(src);
        // Between the definitions and the real `\fi` the frame is still open
        // in branch 0…
        assert_eq!(idx.path_at(offset(src, " y", 0)).len(), 1);
        // …and the real `\fi` closes it.
        assert!(idx.path_at(offset(src, "done", 0)).is_empty());
    }

    #[test]
    fn denylisted_macros_open_no_frames() {
        let src = "\\ifthenelse{\\boolean{x}}{a}{b} $a \\iff b$\n done";
        let idx = index(src);
        assert!(idx.path_at(offset(src, "done", 0)).is_empty());
        assert!(idx.snapshots.is_empty());
    }

    #[test]
    fn stray_flow_words_are_no_ops() {
        let src = "\\else\\or\\fi\n done";
        let idx = index(src);
        assert!(idx.snapshots.is_empty());
        assert!(idx.path_at(offset(src, "done", 0)).is_empty());
    }
}
