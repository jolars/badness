//! Shared conditional paths for duplicate-package and duplicate-label checks.
//!
//! Computed once per file by [`super::rules::RuleContext`] and queried by byte
//! offset ([`ConditionalIndex::path_at`]). A prior occurrence establishes a
//! duplicate only when its path is an ancestor of, or identical to, the later
//! path. Separate tests are unrelated: uncertainty should not produce a warning.
//!
//! **Primitive conditionals.** Any lowercase `if`-prefixed control word opens
//! a frame unless the shared [`badness_parser::parser::conditional::OpenerScan`]
//! recognizes a brace-argument macro or an operand slot. Pairing every opener
//! keeps a user-defined conditional's `\else` from changing an enclosing frame.
//! This recognition is shared with the parser's `CONDITIONAL` shape gate.
//!
//! **Macro conditionals.** [`macro_branches`] recognizes curated commands such
//! as `\ifthenelse`, `\iftoggle`, and `\IfFileExists`, provided their arguments
//! are complete and braced. Predicates are opaque. Each branch gets a frame and
//! its own primitive scan; leaving the argument restores the enclosing state.
//! This is linter semantics over ordinary command/group syntax, not a parser
//! attachment rule. Extra greedily attached groups retain their outer context.
//!
//! **Definition bodies.** Tokens inside `\newcommand{\x}{\else}` or
//! `\def\stopit{\fi}` are carried code, not live flow. The span of a definition
//! command (per [`crate::semantic::define::is_definition_command`]) is skipped.
//! Loads and labels inside definitions are still counted by the rules.
//!
//! No predicates are evaluated or correlated, including literal `\iftrue`.
//! The analysis does not combine all branches into a guaranteed prior or infer
//! custom macro semantics. These limits prefer missed duplicates to noise.
//! Existing primitive-scan approximations remain: parameter text can under-cover
//! a `\def` body, and textual operands can consume a subsequent opener's scan
//! slot. See [`OpenerScan`] for the operand policy.

use std::collections::HashMap;

use badness_parser::parser::conditional::{FlowWord, OpenerScan, Word};
use rowan::{TextRange, TextSize, WalkEvent};

use crate::ast::{AstNode, Command, Group};
use crate::semantic::define::is_definition_command;
use crate::syntax::{SyntaxKind, SyntaxNode};

/// One open conditional at a point in the document: `id` names the specific
/// primitive or macro conditional instance; `branch` identifies one arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Frame {
    id: u32,
    branch: u32,
}

/// The conditional branch path at every state-change offset, for binary-search
/// lookup by byte offset.
pub(crate) struct ConditionalIndex {
    /// State after each branch boundary, in strictly increasing offset order.
    /// Cloning the whole stack is cheap for the shallow nesting of real files.
    snapshots: Vec<(TextSize, Vec<Frame>)>,
}

/// Whether an earlier site executes whenever the later site does, according to
/// their conditional paths. An enclosing or identical path proves this; merely
/// failing to prove exclusivity does not. Predicates are never evaluated, and
/// separate conditionals remain unrelated even when their tests look identical.
pub(crate) fn guaranteed_before(earlier: &[Frame], later: &[Frame]) -> bool {
    later.starts_with(earlier)
}

impl ConditionalIndex {
    /// Record branch boundaries in document order. Braced macro branches save
    /// and restore their enclosing state, so a predicate's control words and an
    /// incomplete primitive conditional cannot affect a sibling branch.
    pub(crate) fn compute(root: &SyntaxNode) -> Self {
        let mut stack: Vec<Frame> = Vec::new();
        let mut next_id = 0u32;
        let mut snapshots: Vec<(TextSize, Vec<Frame>)> = Vec::new();
        let mut scan = OpenerScan::new();
        let mut suppress_until = TextSize::from(0);
        let mut branch_groups = HashMap::new();
        let mut scopes: Vec<BranchScope> = Vec::new();

        for event in root.preorder() {
            let node = match event {
                WalkEvent::Enter(node) => node,
                WalkEvent::Leave(node) => {
                    if node.kind() == SyntaxKind::GROUP
                        && scopes.last().is_some_and(|s| s.group == node.text_range())
                    {
                        let scope = scopes.pop().expect("matching branch scope");
                        stack = scope.path;
                        scan = scope.scan;
                        snapshot(&mut snapshots, node.text_range().end(), &stack);
                    }
                    continue;
                }
            };
            let start = node.text_range().start();
            if start < suppress_until {
                continue;
            }
            if node.kind() == SyntaxKind::GROUP
                && let Some(frame) = branch_groups.remove(&start)
            {
                scopes.push(BranchScope {
                    group: node.text_range(),
                    path: stack.clone(),
                    scan: std::mem::take(&mut scan),
                });
                stack.push(frame);
                snapshot(&mut snapshots, start, &stack);
            }
            let Some(command) = Command::cast(node.clone()) else {
                continue;
            };
            let Some(name) = command.name() else {
                continue;
            };
            if is_definition_command(&name) {
                suppress_until = suppress_until.max(definition_span_end(&node));
                continue;
            }
            // A primitive closer inside an argument cannot close the macro's
            // branch frame or alter the conditional surrounding that argument.
            let branch_depth = scopes.last().map_or(0, |s| s.path.len() + 1);
            match scan.visit(&name) {
                Word::Flow(FlowWord::Else | FlowWord::Or) => {
                    if stack.len() > branch_depth {
                        stack.last_mut().expect("open primitive").branch += 1;
                        snapshot(&mut snapshots, start, &stack);
                    }
                }
                Word::Flow(FlowWord::Fi) => {
                    if stack.len() > branch_depth {
                        stack.pop();
                        snapshot(&mut snapshots, start, &stack);
                    }
                }
                Word::Opens => {
                    stack.push(Frame {
                        id: next_id,
                        branch: 0,
                    });
                    next_id += 1;
                    snapshot(&mut snapshots, start, &stack);
                }
                Word::Inert => {
                    if let Some(branches) = macro_branches(&command, &name) {
                        // Test arguments are data, including control sequences
                        // passed to predicates such as `\boolean`.
                        suppress_until = branches[0].start();
                        for (branch, range) in branches.into_iter().enumerate() {
                            branch_groups.insert(
                                range.start(),
                                Frame {
                                    id: next_id,
                                    branch: branch as u32,
                                },
                            );
                        }
                        next_id += 1;
                    }
                }
                Word::Suppressed => {}
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

struct BranchScope {
    group: TextRange,
    path: Vec<Frame>,
    scan: OpenerScan,
}

fn snapshot(snapshots: &mut Vec<(TextSize, Vec<Frame>)>, at: TextSize, path: &[Frame]) {
    // A group end and the following command can share an offset. Keep only the
    // final state there so binary-search lookup has one unambiguous answer.
    if let Some((last_at, last_path)) = snapshots.last_mut()
        && *last_at == at
    {
        path.clone_into(last_path);
    } else {
        snapshots.push((at, path.to_vec()));
    }
}

/// Curated semantics belong here, not in parser attachment. Require complete,
/// positionally braced arguments: filtering out optionals or guessing unbraced
/// operands would assign branch meaning to the wrong source ranges.
fn macro_branches(command: &Command, name: &str) -> Option<[TextRange; 2]> {
    let tests = match name {
        "ifthenelse" | "iflanguage" | "iftoggle" | "ifbool" | "ifboolexpr" | "ifboolexpe"
        | "ifcsdef" | "ifcsundef" | "ifcsmacro" | "ifcsempty" | "ifcsvoid" | "ifstrempty"
        | "ifblank" | "ifnumodd" | "IfFileExists" | "IfValueTF" | "IfNoValueTF" | "IfBooleanTF"
        | "IfPackageLoadedTF" | "IfClassLoadedTF" | "@ifpackageloaded" | "@ifclassloaded"
        | "@ifundefined" => 1,
        "ifstrequal" | "ifcsstring" | "ifcsequal" | "ifnumequal" | "ifnumgreater" | "ifnumless"
        | "ifdimequal" | "ifdimgreater" | "ifdimless" => 2,
        "ifnumcomp" | "ifdimcomp" => 3,
        _ => return None,
    };
    let mut arguments = command
        .syntax()
        .children_with_tokens()
        .skip(1)
        .filter(|el| !is_trivia(el.kind()));
    let mut branches = [TextRange::default(); 2];
    for i in 0..tests + 2 {
        let element = arguments.next()?;
        let group = Group::cast(element.into_node()?)?;
        if group.syntax().last_child_or_token()?.kind() != SyntaxKind::R_BRACE {
            return None;
        }
        if i >= tests {
            branches[i - tests] = group.syntax().text_range();
        }
    }
    Some(branches)
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
        assert!(!guaranteed_before(a, b));
    }

    #[test]
    fn same_branch_guarantees_a_prior_occurrence() {
        let src = "\\iftrue\\usepackage{a}\\usepackage{a}\\else x\\fi\n";
        let idx = index(src);
        let (a, b) = paths_at_loads(&idx, src);
        assert!(guaranteed_before(a, b));
    }

    #[test]
    fn ifcase_or_branches_are_pairwise_exclusive() {
        let src = "\\ifcase 0 \\usepackage{a}\\or\\usepackage{a}\\or\\usepackage{a}\\fi\n";
        let idx = index(src);
        let a = idx.path_at(offset(src, "\\usepackage", 0));
        let b = idx.path_at(offset(src, "\\usepackage", 1));
        let c = idx.path_at(offset(src, "\\usepackage", 2));
        assert!(!guaranteed_before(a, b));
        assert!(!guaranteed_before(b, c));
        assert!(!guaranteed_before(a, c));
    }

    #[test]
    fn unconditional_prior_is_guaranteed_but_conditional_prior_is_not() {
        let src = "\\iftrue\\usepackage{a}\\fi\n\\usepackage{a}\n";
        let idx = index(src);
        let (a, b) = paths_at_loads(&idx, src);
        assert!(b.is_empty());
        assert!(!guaranteed_before(a, b));
        assert!(guaranteed_before(b, a));
        assert!(guaranteed_before(b, b));
    }

    #[test]
    fn nested_conditionals_compare_by_shared_frame() {
        // Load 1 in the outer then-branch; load 2 in a conditional nested in
        // the outer else-branch. Exclusive via the shared outer frame.
        let src = "\\iftrue\\usepackage{a}\\else\\ifodd 1 \\usepackage{a}\\fi\\fi\n";
        let idx = index(src);
        let (a, b) = paths_at_loads(&idx, src);
        assert!(!guaranteed_before(a, b));
    }

    #[test]
    fn unknown_conditional_is_paired_and_trusted() {
        let src = "\\ifmyflag\\usepackage{a}\\else\\usepackage{a}\\fi\n";
        let idx = index(src);
        let (a, b) = paths_at_loads(&idx, src);
        assert!(!guaranteed_before(a, b));
    }

    #[test]
    fn unknown_conditional_nested_in_known_resyncs() {
        // The inner `\ifmyflag x\fi` pairs with itself; the outer `\iftrue`
        // branches stay exclusive.
        let src = "\\iftrue\\ifmyflag x\\fi\\usepackage{a}\\else\\usepackage{a}\\fi\n";
        let idx = index(src);
        let (a, b) = paths_at_loads(&idx, src);
        assert!(!guaranteed_before(a, b));
    }

    #[test]
    fn unknown_conditionals_else_bumps_its_own_frame() {
        // The `\else` belongs to `\ifmyflag`, so both loads stay in the outer
        // then-branch: not exclusive.
        let src = "\\iftrue\\usepackage{a}\\ifmyflag\\else\\usepackage{a}\\fi\\fi\n";
        let idx = index(src);
        let (a, b) = paths_at_loads(&idx, src);
        assert!(guaranteed_before(a, b));
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
        assert!(!guaranteed_before(a, b));
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
        assert!(!guaranteed_before(a, b));
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
    fn macro_branches_do_not_escape_their_groups() {
        let src = "\\ifthenelse{\\boolean{x}}{a}{b} $a \\iff b$\n done";
        let idx = index(src);
        assert!(idx.path_at(offset(src, "done", 0)).is_empty());
        assert!(!idx.snapshots.is_empty());
    }

    #[test]
    fn braced_macros_track_branch_slots_and_restore_the_outer_path() {
        for head in [
            "\\ifthenelse{\\boolean{x}}",
            "\\iftoggle{x}",
            "\\ifbool{x}",
            "\\ifboolexpr{bool {x}}",
            "\\iflanguage{english}",
            "\\ifstrequal{a}{b}",
            "\\ifnumcomp{1}{<}{2}",
            "\\IfFileExists{example.tex}",
            "\\IfNoValueTF{#1}",
        ] {
            let src = format!(
                "\\ifouter {head}% comment before the branches\n\
                 {{\\usepackage{{a}}\\usepackage{{a}}}}% comment between branches\n\
                 {{\\usepackage{{a}}}}\\usepackage{{a}}\\fi done"
            );
            let idx = index(&src);
            let paths: Vec<_> = (0..4)
                .map(|n| idx.path_at(offset(&src, "\\usepackage", n)))
                .collect();
            assert!(guaranteed_before(paths[0], paths[1]), "{src}");
            assert!(!guaranteed_before(paths[0], paths[2]), "{src}");
            assert!(!guaranteed_before(paths[0], paths[3]), "{src}");
            assert_eq!(paths[0].len(), 2, "{src}");
            assert_eq!(paths[3].len(), 1, "{src}");
            assert!(idx.path_at(offset(&src, "done", 0)).is_empty(), "{src}");
            assert!(idx.snapshots.windows(2).all(|pair| pair[0].0 < pair[1].0));
        }
    }

    #[test]
    fn nested_macro_branches_retain_their_enclosing_path() {
        let src = "\\ifthenelse{x}{\\usepackage{a}\\iftoggle{y}{\\usepackage{a}}{\\usepackage{a}}}{\\usepackage{a}}";
        let idx = index(src);
        let path = |n| idx.path_at(offset(src, "\\usepackage", n));
        assert!(guaranteed_before(path(0), path(1)));
        assert!(guaranteed_before(path(0), path(2)));
        assert!(!guaranteed_before(path(1), path(2)));
        assert!(!guaranteed_before(path(0), path(3)));
    }

    #[test]
    fn extra_attached_groups_are_not_branches() {
        let src = "\\ifthenelse{x}{\\usepackage{a}}{}{\\usepackage{a}}";
        let idx = index(src);
        let (a, b) = paths_at_loads(&idx, src);
        assert_eq!(a.len(), 1);
        assert!(b.is_empty());
    }

    #[test]
    fn malformed_macro_arguments_do_not_get_branch_positions() {
        for src in [
            "\\ifthenelse[x]{test}{\\usepackage{a}}{\\usepackage{a}}",
            "\\ifthenelse{x}{\\usepackage{a}}",
            "\\ifthenelse{x}{\\usepackage{a}}{unclosed",
        ] {
            let idx = index(src);
            assert!(
                idx.path_at(offset(src, "\\usepackage", 0)).is_empty(),
                "{src}"
            );
        }
    }

    #[test]
    fn macro_predicates_and_definition_bodies_do_not_change_outer_state() {
        let src = "\\ifouter\\usepackage{a}\\ifthenelse{\\iffoo\\else\\fi}{}{}\\newcommand{\\x}{\\ifthenelse{x}{\\fi}{\\else}}\\usepackage{a}\\fi done";
        let idx = index(src);
        let (a, b) = paths_at_loads(&idx, src);
        assert_eq!(a, b);
        assert_eq!(a.len(), 1);
        assert!(idx.path_at(offset(src, "done", 0)).is_empty());
    }

    #[test]
    fn primitive_state_cannot_escape_a_macro_branch() {
        for body in ["\\iffoo", "\\else\\or\\fi", "\\ifdefined", "\\ifcsname"] {
            let src = format!(
                "\\ifouter\\ifthenelse{{x}}{{{body}}}{{\\usepackage{{a}}\\ifinner\\usepackage{{a}}\\fi}}\\usepackage{{a}}\\fi done"
            );
            let idx = index(&src);
            let a = idx.path_at(offset(&src, "\\usepackage", 0));
            let b = idx.path_at(offset(&src, "\\usepackage", 1));
            let c = idx.path_at(offset(&src, "\\usepackage", 2));
            assert_eq!(a.len(), 2, "{src}");
            assert_eq!(b.len(), 3, "{src}");
            assert_eq!(c.len(), 1, "{src}");
            assert!(guaranteed_before(a, b), "{src}");
            assert!(!guaranteed_before(a, c), "{src}");
            assert!(idx.path_at(offset(&src, "done", 0)).is_empty(), "{src}");
        }
    }

    #[test]
    fn stray_flow_words_are_no_ops() {
        let src = "\\else\\or\\fi\n done";
        let idx = index(src);
        assert!(idx.snapshots.is_empty());
        assert!(idx.path_at(offset(src, "done", 0)).is_empty());
    }
}
