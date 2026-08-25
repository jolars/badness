//! Typed, read-only wrappers over the CST.
//!
//! Accessors are positional and do not assign meaning or arity to commands.
//!
//! Free functions provide the same operations for untyped [`SyntaxNode`] callers.

pub mod nodes;
pub mod tokens;

pub use nodes::{
    Begin, Command, Conditional, ConditionalBranch, End, Environment, Group, NameGroup, Optional,
};
pub use tokens::ControlWord;

use rowan::{NodeOrToken, TextRange};
use smol_str::SmolStr;

use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

/// A typed wrapper over CST nodes of a particular [`SyntaxKind`].
pub trait AstNode {
    fn can_cast(kind: SyntaxKind) -> bool
    where
        Self: Sized;
    fn cast(syntax: SyntaxNode) -> Option<Self>
    where
        Self: Sized;
    fn syntax(&self) -> &SyntaxNode;
}

/// A typed wrapper over CST tokens of a particular [`SyntaxKind`].
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

/// Returns the first child node castable to `N`.
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
//
// These stay *kind-agnostic* — they read whatever relevant child a node has rather
// than requiring the node's own kind, because callers rely on that latitude (dtx
// `\begin{macro}{\foo}` calls `nth_group` on a `BEGIN`; an xparse default body handed
// to `group_inner_source` may be an `OPTIONAL`). The typed wrapper *methods* are
// kind-checked at `cast`; the shims delegate only to the kind-agnostic navigation
// helpers and per-node body functions, never to `cast`.

/// The control-word name of a `COMMAND` node (the leading `\` stripped), or `None`
/// for a control symbol.
pub fn command_name(command: &SyntaxNode) -> Option<SmolStr> {
    child_token::<ControlWord>(command).map(|cw| SmolStr::new(cw.name()))
}

/// The range of a `COMMAND` node's leading `CONTROL_WORD` token, or `None` for a
/// control symbol.
pub fn control_word_range(command: &SyntaxNode) -> Option<TextRange> {
    child_token::<ControlWord>(command).map(|cw| cw.range())
}

/// The literal text inside the `n`-th `GROUP` argument of `command`, braces dropped.
pub fn nth_group_text(command: &SyntaxNode, n: usize) -> Option<SmolStr> {
    children::<Group>(command)
        .nth(n)
        .and_then(|g| g.inner_text())
}

/// The byte range of the content inside the `n`-th `GROUP` argument together with
/// that inner text.
pub fn nth_group_inner(command: &SyntaxNode, n: usize) -> Option<(TextRange, SmolStr)> {
    children::<Group>(command).nth(n).and_then(|g| g.inner())
}

/// The `n`-th `GROUP` argument node of `command`, if present.
pub fn nth_group(command: &SyntaxNode, n: usize) -> Option<SyntaxNode> {
    children::<Group>(command)
        .nth(n)
        .map(|g| g.syntax().clone())
}

/// The byte range of `command` spanning its control word through the end of its
/// first `{…}` group; the full command range when the first group is absent.
pub fn first_group_range(command: &SyntaxNode) -> TextRange {
    match children::<Group>(command).next() {
        Some(group) => TextRange::new(
            command.text_range().start(),
            group.syntax().text_range().end(),
        ),
        None => command.text_range(),
    }
}

/// The control-word name of a single `COMMAND` wrapped in `group`. A braced
/// l3doc `v`-type name argument (`\begin{macro}{\foo}`) captures its content as
/// one opaque `VERB` token instead of a `COMMAND` (issue #60); a control-word-
/// shaped `VERB` (`\` + letters, nothing else) reads as the same name.
pub fn group_command_name(group: &SyntaxNode) -> Option<SmolStr> {
    if let Some(name) = child::<Command>(group).and_then(|c| c.name()) {
        return Some(name);
    }
    let verb = group
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == SyntaxKind::VERB)?;
    let name = verb.text().trim().strip_prefix('\\')?;
    (!name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '@' || c == '_' || c == ':'))
    .then(|| SmolStr::new(name))
}

/// The raw inner source of `group` with its outer braces dropped, nested braces kept.
pub fn group_inner_source(group: &SyntaxNode) -> String {
    nodes::inner_source_of(group)
}

/// The environment name of a `BEGIN` or `END` node — the text of its `NAME_GROUP`
/// child, braces dropped.
///
/// Mirrors [`Begin::name`] on the `BEGIN` side, including its fallback to the bare
/// head control word for an environment-alias delimiter (issue #109). `END` nodes
/// keep the `NAME_GROUP`-only reading, so an alias closer stays nameless.
pub fn environment_name(begin_or_end: &SyntaxNode) -> Option<String> {
    if begin_or_end.kind() == SyntaxKind::BEGIN {
        return Begin::cast(begin_or_end.clone())?.name();
    }
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
    fn nth_group_inner_none_for_parameter_token() {
        // A macro-parameter template (`\ref{#1}`, doubled `##1` in a definition
        // body) is not a flat literal — issue #104.
        assert_eq!(nth_group_inner(&command("\\ref{#1}\n"), 0), None);
        assert_eq!(nth_group_inner(&command("\\eqref{##1}\n"), 0), None);
        assert_eq!(nth_group_text(&command("\\input{#1}\n"), 0), None);
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

    #[test]
    fn name_group_rejects_parameter_token() {
        let mut builder = rowan::GreenNodeBuilder::new();
        builder.start_node(SyntaxKind::NAME_GROUP.into());
        builder.token(SyntaxKind::L_BRACE.into(), "{");
        builder.token(SyntaxKind::HASH.into(), "#");
        builder.token(SyntaxKind::WORD.into(), "1");
        builder.token(SyntaxKind::R_BRACE.into(), "}");
        builder.finish_node();
        let group = NameGroup::cast(SyntaxNode::new_root(builder.finish())).unwrap();

        assert_eq!(group.text(), None);
        assert_eq!(group.range(), None);
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
    fn free_fn_shims_stay_kind_agnostic() {
        // The shims read whatever child a node has, not gating on the node's own
        // kind: dtx `\begin{macro}{\foo}` reads the `{\foo}` GROUP off a BEGIN node,
        // not a COMMAND. The typed `Command::nth_group` would (correctly) not apply
        // here, but the free-function shim must.
        let begin = node(
            "\\begin{macro}{\\foo}\ncode\n\\end{macro}\n",
            SyntaxKind::BEGIN,
        );
        assert_eq!(
            nth_group(&begin, 0).map(|g| g.kind()),
            Some(SyntaxKind::GROUP)
        );
        assert_eq!(
            group_command_name(&nth_group(&begin, 0).unwrap()).as_deref(),
            Some("foo")
        );
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

    #[test]
    fn begin_name_falls_back_to_the_head_control_word() {
        // An environment-alias `BEGIN` (issue #109) is the bare control word with
        // no `NAME_GROUP`, so the name is read from there instead. Positional and
        // meaning-free: `Signatures::environment` is what maps it onto behavior.
        let src = "\\newcommand{\\bea}{\\begin{eqnarray}}\n\\newcommand{\\eea}{\\end{eqnarray}}\n\\bea a \\eea\n";
        let begin = node(src, SyntaxKind::BEGIN);
        assert_eq!(environment_name(&begin).as_deref(), Some("bea"));
        assert_eq!(
            Begin::cast(begin.clone()).unwrap().name().as_deref(),
            Some("bea")
        );
        // The range stays `None`: that is the contract making every name-rewriting
        // consumer (rename, change-environment, the obsolete-environment fix)
        // decline cleanly rather than emit a half-edit.
        assert!(environment_name_range(&begin).is_none());
        assert!(Begin::cast(begin).unwrap().name_range().is_none());
    }

    #[test]
    fn alias_end_stays_nameless() {
        // Only the `BEGIN` side falls back; an alias closer keeps the
        // `NAME_GROUP`-only reading.
        let src = "\\newcommand{\\bea}{\\begin{eqnarray}}\n\\newcommand{\\eea}{\\end{eqnarray}}\n\\bea a \\eea\n";
        let end = node(src, SyntaxKind::END);
        assert!(environment_name(&end).is_none());
    }

    #[test]
    fn a_real_begin_reads_its_name_group() {
        // The fallback is guarded on the head not being `\begin`, so a spelled-out
        // environment is unaffected and a malformed `\begin` still reports `None`
        // rather than claiming to be named "begin".
        let begin = node("\\begin{center}x\\end{center}", SyntaxKind::BEGIN);
        assert_eq!(environment_name(&begin).as_deref(), Some("center"));
        assert!(environment_name_range(&begin).is_some());
    }
}
