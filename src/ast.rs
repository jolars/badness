//! A typed AST layer over the CST — thin, read-only wrappers ([`AstNode`] /
//! [`AstToken`]) giving nodes a typed identity and named, *positional* accessors
//! (reading a `COMMAND`'s name and its literal `{…}` argument text, an
//! `ENVIRONMENT`'s `\begin`/`\end`, …).
//!
//! Purely syntactic: the wrappers know nothing about what any command *means*, so
//! both the syntactic `project/` layer and the semantic layer build on them without
//! meaning leaking downward (AGENTS.md decision #2). Because the CST is generic and
//! greedy (decision #8), accessors are positional ([`nodes::Command::nth_group`]) and
//! tolerate over-attached groups by construction — they never pretend arity is fixed.
//!
//! The free functions below are thin shims over the wrapper methods, kept so existing
//! `&SyntaxNode`-based call sites compile unchanged during the migration.

pub mod nodes;
pub mod tokens;

pub use nodes::{Begin, Command, End, Environment, Group, NameGroup, Optional};
pub use tokens::ControlWord;

use rowan::{NodeOrToken, TextRange};

use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

/// A typed wrapper over a CST *node* of a single [`SyntaxKind`]. Mirrors
/// rust-analyzer's `AstNode`: `cast` succeeds iff `can_cast(node.kind())`.
pub trait AstNode {
    fn can_cast(kind: SyntaxKind) -> bool
    where
        Self: Sized;
    fn cast(syntax: SyntaxNode) -> Option<Self>
    where
        Self: Sized;
    fn syntax(&self) -> &SyntaxNode;
}

/// A typed wrapper over a CST *token* of a single [`SyntaxKind`].
pub trait AstToken {
    fn can_cast(kind: SyntaxKind) -> bool
    where
        Self: Sized;
    fn cast(syntax: SyntaxToken) -> Option<Self>
    where
        Self: Sized;
    fn syntax(&self) -> &SyntaxToken;
    fn text(&self) -> &str {
        self.syntax().text()
    }
}

/// The first child node castable to `N`. Replaces the raw
/// `children().find(|c| c.kind() == X)` idiom at *field-extraction* sites.
pub fn child<N: AstNode>(parent: &SyntaxNode) -> Option<N> {
    parent.children().find_map(N::cast)
}

/// All child nodes castable to `N`, in source order.
pub fn children<N: AstNode>(parent: &SyntaxNode) -> impl Iterator<Item = N> {
    parent.children().filter_map(N::cast)
}

/// The first child token castable to `T`.
pub fn child_token<T: AstToken>(parent: &SyntaxNode) -> Option<T> {
    parent
        .children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find_map(T::cast)
}

// --- Free-function shims (see module docs) -----------------------------------

/// The control-word name of a `COMMAND` node (the leading `\` stripped), or `None`
/// for a control symbol.
pub fn command_name(command: &SyntaxNode) -> Option<String> {
    Command::cast(command.clone()).and_then(|c| c.name())
}

/// The range of a `COMMAND` node's leading `CONTROL_WORD` token, or `None` for a
/// control symbol.
pub fn control_word_range(command: &SyntaxNode) -> Option<TextRange> {
    Command::cast(command.clone()).and_then(|c| c.control_word_range())
}

/// The literal text inside the `n`-th `GROUP` argument of `command`, braces dropped.
pub fn nth_group_text(command: &SyntaxNode, n: usize) -> Option<String> {
    Command::cast(command.clone()).and_then(|c| c.nth_group_text(n))
}

/// The byte range of the content inside the `n`-th `GROUP` argument together with
/// that inner text.
pub fn nth_group_inner(command: &SyntaxNode, n: usize) -> Option<(TextRange, String)> {
    Command::cast(command.clone()).and_then(|c| c.nth_group_inner(n))
}

/// The `n`-th `GROUP` argument node of `command`, if present.
pub fn nth_group(command: &SyntaxNode, n: usize) -> Option<SyntaxNode> {
    Command::cast(command.clone()).and_then(|c| c.nth_group(n).map(|g| g.syntax().clone()))
}

/// The byte range of `command` spanning its control word through the end of its
/// first `{…}` group; the full command range when the first group is absent.
pub fn first_group_range(command: &SyntaxNode) -> TextRange {
    match Command::cast(command.clone()) {
        Some(c) => c.first_group_range(),
        None => command.text_range(),
    }
}

/// The control-word name of a single `COMMAND` wrapped in `group`.
pub fn group_command_name(group: &SyntaxNode) -> Option<String> {
    Group::cast(group.clone()).and_then(|g| g.command_name())
}

/// The raw inner source of `group` with its outer braces dropped, nested braces kept.
pub fn group_inner_source(group: &SyntaxNode) -> String {
    Group::cast(group.clone())
        .map(|g| g.inner_source())
        .unwrap_or_default()
}

/// The environment name of a `BEGIN` or `END` node — the text of its `NAME_GROUP`
/// child, braces dropped.
pub fn environment_name(begin_or_end: &SyntaxNode) -> Option<String> {
    child::<NameGroup>(begin_or_end).and_then(|g| g.text())
}

/// The byte range of the environment name inside a `BEGIN` or `END` node's
/// `NAME_GROUP`.
pub fn environment_name_range(begin_or_end: &SyntaxNode) -> Option<TextRange> {
    child::<NameGroup>(begin_or_end).and_then(|g| g.range())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn command(src: &str) -> SyntaxNode {
        SyntaxNode::new_root(parse(src).green)
            .descendants()
            .find(|node| node.kind() == SyntaxKind::COMMAND)
            .expect("a COMMAND node")
    }

    fn node(src: &str, kind: SyntaxKind) -> SyntaxNode {
        SyntaxNode::new_root(parse(src).green)
            .descendants()
            .find(|n| n.kind() == kind)
            .expect("a matching node")
    }

    #[test]
    fn command_name_strips_backslash() {
        assert_eq!(
            command_name(&command("\\section{Hi}\n")).as_deref(),
            Some("section")
        );
    }

    #[test]
    fn nth_group_text_reassembles_inner_tokens() {
        assert_eq!(
            nth_group_text(&command("\\label{sec:intro}\n"), 0).as_deref(),
            Some("sec:intro")
        );
    }

    #[test]
    fn nth_group_inner_spans_only_the_key() {
        // The inner range must cover `sec:intro` exactly, excluding the braces.
        let src = "\\label{sec:intro}\n";
        let cmd = command(src);
        let (range, text) = nth_group_inner(&cmd, 0).expect("an inner span");
        assert_eq!(text, "sec:intro");
        assert_eq!(&src[range], "sec:intro");
    }

    #[test]
    fn nth_group_inner_empty_group_is_zero_width_after_brace() {
        let cmd = command("\\label{}\n");
        let (range, text) = nth_group_inner(&cmd, 0).expect("an inner span");
        assert!(text.is_empty());
        assert!(range.is_empty());
    }

    #[test]
    fn nth_group_inner_none_for_nested_command() {
        assert_eq!(nth_group_inner(&command("\\input{\\jobname}\n"), 0), None);
    }

    #[test]
    fn nth_group_text_none_for_nested_command() {
        assert_eq!(nth_group_text(&command("\\input{\\jobname}\n"), 0), None);
    }

    #[test]
    fn nth_group_text_none_when_group_absent() {
        assert_eq!(nth_group_text(&command("\\input\n"), 0), None);
    }

    #[test]
    fn group_command_name_reads_braced_control_word() {
        let cmd = command("\\newcommand{\\foo}{x}\n");
        let name = nth_group(&cmd, 0).and_then(|g| group_command_name(&g));
        assert_eq!(name.as_deref(), Some("foo"));
    }

    #[test]
    fn group_command_name_none_for_plain_text() {
        let cmd = command("\\newenvironment{thm}{a}{b}\n");
        let name = nth_group(&cmd, 0).and_then(|g| group_command_name(&g));
        assert_eq!(name, None);
    }

    #[test]
    fn group_inner_source_keeps_nested_braces() {
        // The xparse spec group `{m O{d} m}` parses the `{d}` default as a nested
        // GROUP; `nth_group_text` would reject it, but the raw source survives.
        let cmd = command("\\NewDocumentCommand{\\foo}{m O{d} m}{x}\n");
        let spec = nth_group(&cmd, 1).map(|g| group_inner_source(&g));
        assert_eq!(spec.as_deref(), Some("m O{d} m"));
        assert_eq!(nth_group_text(&cmd, 1), None);
    }

    #[test]
    fn environment_name_range_spans_only_the_name() {
        let src = "\\begin{equation}\nx\n\\end{equation}\n";
        let begin = node(src, SyntaxKind::BEGIN);
        let range = environment_name_range(&begin).expect("a name span");
        assert_eq!(&src[range], "equation");

        let end = node(src, SyntaxKind::END);
        let range = environment_name_range(&end).expect("a name span");
        assert_eq!(&src[range], "equation");
    }

    #[test]
    fn environment_name_range_none_for_empty_name() {
        assert_eq!(
            environment_name_range(&node("\\begin{}\n\\end{}\n", SyntaxKind::BEGIN)),
            None
        );
    }

    // --- Wrapper-native tests --------------------------------------------------

    #[test]
    fn cast_is_kind_exact() {
        let cmd = command("\\section{Hi}\n");
        assert!(Command::cast(cmd.clone()).is_some());
        assert!(Group::cast(cmd.clone()).is_none());
        let group = nth_group(&cmd, 0).unwrap();
        assert!(Group::cast(group.clone()).is_some());
        assert!(Command::cast(group).is_none());
    }

    #[test]
    fn typed_nth_group_is_a_group_node() {
        let cmd = Command::cast(command("\\label{k}\n")).unwrap();
        let group = cmd.nth_group(0).unwrap();
        assert_eq!(group.syntax().kind(), SyntaxKind::GROUP);
    }

    #[test]
    fn optionals_do_not_shift_group_indexing() {
        // `\cmd[o]{a}` — the GROUP index ignores the OPTIONAL. define.rs relies on it.
        let cmd = Command::cast(command("\\cmd[o]{a}\n")).unwrap();
        assert_eq!(cmd.nth_group_text(0).as_deref(), Some("a"));
        assert_eq!(cmd.optionals().count(), 1);
    }

    #[test]
    fn first_group_range_stops_at_first_group() {
        // Greedy over-attachment (decision #8): `\label{a}\n{b}` attaches `{b}` too.
        let src = "\\label{a}\n{b}\n";
        let cmd = Command::cast(command(src)).unwrap();
        assert_eq!(&src[cmd.first_group_range()], "\\label{a}");
        assert_eq!(cmd.nth_group_text(1).as_deref(), Some("b"));
    }

    #[test]
    fn environment_wrapper_reaches_begin_and_end() {
        let env = Environment::cast(node(
            "\\begin{equation}\nx\n\\end{equation}\n",
            SyntaxKind::ENVIRONMENT,
        ))
        .unwrap();
        assert_eq!(
            env.begin().and_then(|b| b.name()).as_deref(),
            Some("equation")
        );
        assert_eq!(
            env.end().and_then(|e| e.name()).as_deref(),
            Some("equation")
        );
        assert_eq!(env.name().as_deref(), Some("equation"));
    }
}
