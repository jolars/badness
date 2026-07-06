//! Small generic accessors over the CST — reading a `COMMAND` node's name and
//! its literal `{…}` argument text. Purely syntactic: these know nothing about
//! what any command *means*, so both the syntactic `project/` layer and the
//! semantic layer build on them without meaning leaking downward (AGENTS.md
//! decision #2). This is the seed of an `ast/` layer; it was
//! extracted when its second consumer (the semantic label/reference model)
//! appeared.

use rowan::{TextRange, TextSize};

use crate::syntax::{SyntaxKind, SyntaxNode};

/// The control-word name of a `COMMAND` node (the leading `\` stripped), or
/// `None` for a control symbol. The grammar bumps the control word as the
/// command's first token (`grammar.rs`, `fn command`).
pub fn command_name(command: &SyntaxNode) -> Option<String> {
    command
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| token.kind() == SyntaxKind::CONTROL_WORD)
        .map(|token| token.text().trim_start_matches('\\').to_string())
}

/// The range of a `COMMAND` node's leading `CONTROL_WORD` token (the `\foo`
/// itself, backslash included), or `None` for a control symbol. Rules use this
/// to underline just the control word and to scope a control-word swap fix,
/// rather than the whole node (which may carry greedily-attached argument
/// groups).
pub fn control_word_range(command: &SyntaxNode) -> Option<TextRange> {
    command
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| token.kind() == SyntaxKind::CONTROL_WORD)
        .map(|token| token.text_range())
}

/// The literal text inside the `n`-th `GROUP` argument of `command`, with the
/// enclosing braces dropped. Concatenates the group's inner token text so
/// content split across `WORD`/`.`/`/`/`UNDERSCORE` tokens (e.g.
/// `chapters/my_file`, `sec:intro`) reassembles. Returns `None` when there is
/// no `n`-th group, or when it holds non-token content (a nested command — not
/// a flat literal).
pub fn nth_group_text(command: &SyntaxNode, n: usize) -> Option<String> {
    let group = command
        .children()
        .filter(|child| child.kind() == SyntaxKind::GROUP)
        .nth(n)?;

    let mut text = String::new();
    for element in group.children_with_tokens() {
        match element {
            rowan::NodeOrToken::Token(token) => match token.kind() {
                SyntaxKind::L_BRACE | SyntaxKind::R_BRACE => {}
                _ => text.push_str(token.text()),
            },
            // A nested node (e.g. a COMMAND) means the argument isn't a flat
            // literal; treat the whole thing as unresolvable.
            rowan::NodeOrToken::Node(_) => return None,
        }
    }
    Some(text)
}

/// The byte range of the content *inside* the `n`-th `GROUP` argument (the span
/// between the braces) together with that inner text — the location-aware
/// counterpart to [`nth_group_text`]. The inner range runs from the first inner
/// token's start to the last inner token's end; an empty group (`{}`) yields a
/// zero-width range just after the `{`. Returns `None` under exactly the same
/// conditions as [`nth_group_text`] (no `n`-th group, or non-token content such as
/// a nested command), so callers see the same skip-on-nested-macro behavior.
///
/// The text/range correspondence is exact: in the success path the group holds
/// only flat tokens, so its inner bytes are contiguous and per-key sub-ranges can
/// be sliced off `inner_range` by byte offset (used by the semantic builder to give
/// each key in a `\cref{a,b}` its own precise span).
pub fn nth_group_inner(command: &SyntaxNode, n: usize) -> Option<(TextRange, String)> {
    let group = command
        .children()
        .filter(|child| child.kind() == SyntaxKind::GROUP)
        .nth(n)?;

    let mut text = String::new();
    let mut start: Option<TextSize> = None;
    let mut end: Option<TextSize> = None;
    // Fallback anchor for an empty group: the byte just after the opening brace.
    let mut after_l_brace = group.text_range().start();
    for element in group.children_with_tokens() {
        match element {
            rowan::NodeOrToken::Token(token) => match token.kind() {
                SyntaxKind::L_BRACE => after_l_brace = token.text_range().end(),
                SyntaxKind::R_BRACE => {}
                _ => {
                    let range = token.text_range();
                    start.get_or_insert(range.start());
                    end = Some(range.end());
                    text.push_str(token.text());
                }
            },
            // A nested node (e.g. a COMMAND) means the argument isn't a flat
            // literal; treat the whole thing as unresolvable, like `nth_group_text`.
            rowan::NodeOrToken::Node(_) => return None,
        }
    }
    let range = match (start, end) {
        (Some(start), Some(end)) => TextRange::new(start, end),
        _ => TextRange::empty(after_l_brace),
    };
    Some((range, text))
}

/// The `n`-th `GROUP` argument node of `command`, if present. The thin node-level
/// counterpart to [`nth_group_text`], for callers that must inspect a group's
/// structure (a nested `\name` command, or raw source) rather than flatten it to a
/// literal.
pub fn nth_group(command: &SyntaxNode, n: usize) -> Option<SyntaxNode> {
    command
        .children()
        .filter(|child| child.kind() == SyntaxKind::GROUP)
        .nth(n)
}

/// The byte range of `command` spanning its control word through the end of its
/// *first* `{…}` group — e.g. `\label{key}` up to the closing brace of `{key}`.
/// Deliberately not [`SyntaxNode::text_range`], which the greedy parser may
/// stretch over a *second* group it attached without knowing the command's arity
/// (`\label{a}\n{…}`; AGENTS.md decision #8). Falls back to the full command range
/// when the first group is absent.
pub fn first_group_range(command: &SyntaxNode) -> TextRange {
    match nth_group(command, 0) {
        Some(group) => TextRange::new(command.text_range().start(), group.text_range().end()),
        None => command.text_range(),
    }
}

/// The control-word name (leading `\` stripped) of a single `COMMAND` wrapped in
/// `group`, as in a `\newcommand{\foo}` / `\NewDocumentCommand{\foo}` name group.
/// Returns `None` unless the group's only non-trivia child is exactly one control
/// word — anything else (plain text, multiple tokens, a control symbol) is not a
/// definable command name we extract.
pub fn group_command_name(group: &SyntaxNode) -> Option<String> {
    let command = group
        .children()
        .find(|child| child.kind() == SyntaxKind::COMMAND)?;
    command_name(&command)
}

/// The raw inner source of `group` with its outer `L_BRACE`/`R_BRACE` dropped, but
/// *all* interior text preserved — nested `{…}` braces included. Unlike
/// [`nth_group_text`], which bails on nested nodes, this reconstructs the verbatim
/// content needed for an xparse argument spec like `{m O{0} m}` (whose `{0}` default
/// parses as a nested `GROUP`). Trivia (whitespace/newlines) is kept verbatim; the
/// caller tokenizes the result.
pub fn group_inner_source(group: &SyntaxNode) -> String {
    let mut text = String::new();
    for element in group.descendants_with_tokens() {
        if let rowan::NodeOrToken::Token(token) = element {
            text.push_str(token.text());
        }
    }
    // Drop the outer braces the group carries as its first/last tokens (tolerating
    // a malformed group missing one).
    let inner = text.strip_prefix('{').unwrap_or(&text);
    inner.strip_suffix('}').unwrap_or(inner).to_string()
}

/// The environment name of a `BEGIN` or `END` node — the literal text of its
/// `NAME_GROUP` child, braces dropped. Returns `None` when the node has no
/// `NAME_GROUP` (a malformed `\begin`) or it holds non-token content. The grammar
/// emits the name as a `NAME_GROUP` (`grammar.rs`, `fn name_group`).
pub fn environment_name(begin_or_end: &SyntaxNode) -> Option<String> {
    let group = begin_or_end
        .children()
        .find(|child| child.kind() == SyntaxKind::NAME_GROUP)?;

    let mut text = String::new();
    for element in group.children_with_tokens() {
        match element {
            rowan::NodeOrToken::Token(token) => match token.kind() {
                SyntaxKind::L_BRACE | SyntaxKind::R_BRACE => {}
                _ => text.push_str(token.text()),
            },
            rowan::NodeOrToken::Node(_) => return None,
        }
    }
    Some(text)
}

/// The byte range of the environment name *inside* a `BEGIN` or `END` node's
/// `NAME_GROUP` (the span between the braces, e.g. `equation` in
/// `\begin{equation}`) — the location-aware counterpart to [`environment_name`].
/// The range runs from the first inner token's start to the last inner token's
/// end, matching the brace-dropping in [`environment_name`] and mirroring
/// [`nth_group_inner`]. Returns `None` when the node has no `NAME_GROUP` (a
/// malformed `\begin`), it holds a nested node, or the name is empty (`\begin{}`,
/// nothing to highlight).
pub fn environment_name_range(begin_or_end: &SyntaxNode) -> Option<TextRange> {
    let group = begin_or_end
        .children()
        .find(|child| child.kind() == SyntaxKind::NAME_GROUP)?;

    let mut start: Option<TextSize> = None;
    let mut end: Option<TextSize> = None;
    for element in group.children_with_tokens() {
        match element {
            rowan::NodeOrToken::Token(token) => match token.kind() {
                SyntaxKind::L_BRACE | SyntaxKind::R_BRACE => {}
                _ => {
                    let range = token.text_range();
                    start.get_or_insert(range.start());
                    end = Some(range.end());
                }
            },
            rowan::NodeOrToken::Node(_) => return None,
        }
    }
    Some(TextRange::new(start?, end?))
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
}
