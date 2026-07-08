//! Typed [`AstNode`] wrappers over CST *nodes*, with positional/structural
//! accessors. These are a read-only typed view over the generic, greedy CST
//! (AGENTS.md decision #8): a `\section` and a `\newcommand` share the `COMMAND`
//! shape, so accessors are *positional* ([`Command::nth_group`]) and tolerate
//! greedily over-attached groups by construction. They expose structure only, never
//! command *meaning* (decision #2) — no signature-DB lookup lives here.

use rowan::{NodeOrToken, TextRange, TextSize};

use super::{AstNode, AstToken, child, children};
use crate::ast::tokens::ControlWord;
use crate::syntax::{SyntaxKind, SyntaxNode};

/// Declares a newtype wrapper over a `SyntaxNode` of exactly one `SyntaxKind`,
/// implementing [`AstNode`]. Only the *identity* (`can_cast`/`cast`/`syntax`) is
/// generated; every accessor is hand-written in a separate `impl` block. This is
/// ordinary in-tree Rust, not codegen — no build step, no generated artifacts.
macro_rules! ast_node {
    ($(#[$meta:meta])* $name:ident, $kind:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name {
            syntax: SyntaxNode,
        }

        impl AstNode for $name {
            fn can_cast(kind: SyntaxKind) -> bool {
                kind == SyntaxKind::$kind
            }

            fn cast(syntax: SyntaxNode) -> Option<Self> {
                Self::can_cast(syntax.kind()).then_some(Self { syntax })
            }

            fn syntax(&self) -> &SyntaxNode {
                &self.syntax
            }
        }
    };
}

ast_node!(
    /// A control sequence with its greedily-attached argument groups.
    Command, COMMAND
);
ast_node!(
    /// A `{ … }` group — an argument, or a nested brace group.
    Group, GROUP
);
ast_node!(
    /// A `[ … ]` optional argument.
    Optional, OPTIONAL
);
ast_node!(
    /// The `{name}` group following `\begin` / `\end`.
    NameGroup, NAME_GROUP
);
ast_node!(
    /// A `\begin{name}` node.
    Begin, BEGIN
);
ast_node!(
    /// An `\end{name}` node.
    End, END
);
ast_node!(
    /// A `\begin{…} … \end{…}` environment.
    Environment, ENVIRONMENT
);

impl Command {
    /// The leading `CONTROL_WORD` token, or `None` for a control symbol. The
    /// grammar bumps the control word as the command's first token.
    pub fn control_word(&self) -> Option<ControlWord> {
        self.syntax
            .children_with_tokens()
            .filter_map(NodeOrToken::into_token)
            .find_map(ControlWord::cast)
    }

    /// The control-word name (leading `\` stripped), or `None` for a control
    /// symbol.
    pub fn name(&self) -> Option<String> {
        self.control_word().map(|cw| cw.name())
    }

    /// The range of the leading `CONTROL_WORD` token (the `\foo` itself, backslash
    /// included), or `None` for a control symbol. Callers use this to underline just
    /// the control word rather than the whole node, which may carry greedily-attached
    /// argument groups.
    pub fn control_word_range(&self) -> Option<TextRange> {
        self.control_word().map(|cw| cw.range())
    }

    /// The `n`-th `GROUP` argument, if present. Filters `GROUP` only, so `OPTIONAL`
    /// arguments do *not* shift brace indexing (`\cmd[o]{a}` → `nth_group(0)` is
    /// `{a}`).
    pub fn nth_group(&self, n: usize) -> Option<Group> {
        self.groups().nth(n)
    }

    /// The `GROUP` argument nodes, in source order.
    pub fn groups(&self) -> impl Iterator<Item = Group> {
        children::<Group>(&self.syntax)
    }

    /// The `OPTIONAL` argument nodes, in source order.
    pub fn optionals(&self) -> impl Iterator<Item = Optional> {
        children::<Optional>(&self.syntax)
    }

    /// The literal text inside the `n`-th `GROUP` argument, braces dropped. Returns
    /// `None` when there is no `n`-th group or it holds non-token content (a nested
    /// command — not a flat literal). See [`Group::inner_text`].
    pub fn nth_group_text(&self, n: usize) -> Option<String> {
        self.nth_group(n)?.inner_text()
    }

    /// The byte range of the content *inside* the `n`-th `GROUP` argument together
    /// with that inner text — the location-aware counterpart to
    /// [`Command::nth_group_text`]. See [`Group::inner`].
    pub fn nth_group_inner(&self, n: usize) -> Option<(TextRange, String)> {
        self.nth_group(n)?.inner()
    }

    /// The byte range of this command spanning its control word through the end of
    /// its *first* `{…}` group — e.g. `\label{key}` up to the closing brace of
    /// `{key}`. Deliberately not [`SyntaxNode::text_range`], which the greedy parser
    /// may stretch over a *second* group it attached without knowing arity
    /// (`\label{a}\n{…}`; decision #8). Falls back to the full command range when the
    /// first group is absent.
    pub fn first_group_range(&self) -> TextRange {
        match self.nth_group(0) {
            Some(group) => TextRange::new(
                self.syntax.text_range().start(),
                group.syntax.text_range().end(),
            ),
            None => self.syntax.text_range(),
        }
    }
}

impl Group {
    /// The literal text inside this group, with the enclosing braces dropped.
    /// Concatenates the inner token text so content split across `WORD`/`.`/`/`/…
    /// tokens (e.g. `chapters/my_file`, `sec:intro`) reassembles. Returns `None` when
    /// the group holds non-token content (a nested command — not a flat literal).
    pub fn inner_text(&self) -> Option<String> {
        let mut text = String::new();
        for element in self.syntax.children_with_tokens() {
            match element {
                NodeOrToken::Token(token) => match token.kind() {
                    SyntaxKind::L_BRACE | SyntaxKind::R_BRACE => {}
                    _ => text.push_str(token.text()),
                },
                // A nested node (e.g. a COMMAND) means the argument isn't a flat
                // literal; treat the whole thing as unresolvable.
                NodeOrToken::Node(_) => return None,
            }
        }
        Some(text)
    }

    /// The byte range of the content *inside* this group (the span between the
    /// braces) together with that inner text — the location-aware counterpart to
    /// [`Group::inner_text`]. The inner range runs from the first inner token's start
    /// to the last inner token's end; an empty group (`{}`) yields a zero-width range
    /// just after the `{`. Returns `None` under the same conditions as
    /// [`Group::inner_text`].
    ///
    /// The text/range correspondence is exact: in the success path the group holds
    /// only flat tokens, so its inner bytes are contiguous and per-key sub-ranges can
    /// be sliced off the range by byte offset (used by the semantic builder to give
    /// each key in a `\cref{a,b}` its own precise span).
    pub fn inner(&self) -> Option<(TextRange, String)> {
        let mut text = String::new();
        let mut start: Option<TextSize> = None;
        let mut end: Option<TextSize> = None;
        // Fallback anchor for an empty group: the byte just after the opening brace.
        let mut after_l_brace = self.syntax.text_range().start();
        for element in self.syntax.children_with_tokens() {
            match element {
                NodeOrToken::Token(token) => match token.kind() {
                    SyntaxKind::L_BRACE => after_l_brace = token.text_range().end(),
                    SyntaxKind::R_BRACE => {}
                    _ => {
                        let range = token.text_range();
                        start.get_or_insert(range.start());
                        end = Some(range.end());
                        text.push_str(token.text());
                    }
                },
                // A nested node means the argument isn't a flat literal; treat the
                // whole thing as unresolvable, like `inner_text`.
                NodeOrToken::Node(_) => return None,
            }
        }
        let range = match (start, end) {
            (Some(start), Some(end)) => TextRange::new(start, end),
            _ => TextRange::empty(after_l_brace),
        };
        Some((range, text))
    }

    /// The raw inner source of this group with its outer braces dropped, but *all*
    /// interior text preserved — nested `{…}` braces included. Unlike
    /// [`Group::inner_text`], which bails on nested nodes, this reconstructs the
    /// verbatim content needed for an xparse argument spec like `{m O{0} m}` (whose
    /// `{0}` default parses as a nested `GROUP`). Trivia is kept verbatim; the caller
    /// tokenizes the result.
    pub fn inner_source(&self) -> String {
        let mut text = String::new();
        for element in self.syntax.descendants_with_tokens() {
            if let NodeOrToken::Token(token) = element {
                text.push_str(token.text());
            }
        }
        // Drop the outer braces the group carries as its first/last tokens
        // (tolerating a malformed group missing one).
        let inner = text.strip_prefix('{').unwrap_or(&text);
        inner.strip_suffix('}').unwrap_or(inner).to_string()
    }

    /// The single `COMMAND` child wrapped in this group, if any.
    pub fn command(&self) -> Option<Command> {
        child::<Command>(&self.syntax)
    }

    /// The control-word name (leading `\` stripped) of a single `COMMAND` wrapped in
    /// this group, as in a `\newcommand{\foo}` name group. Returns `None` unless the
    /// group's only relevant child is exactly one control word.
    pub fn command_name(&self) -> Option<String> {
        self.command()?.name()
    }
}

impl NameGroup {
    /// The environment name — the literal text of this `NAME_GROUP`, braces dropped.
    /// Returns `None` when it holds non-token content.
    pub fn text(&self) -> Option<String> {
        let mut text = String::new();
        for element in self.syntax.children_with_tokens() {
            match element {
                NodeOrToken::Token(token) => match token.kind() {
                    SyntaxKind::L_BRACE | SyntaxKind::R_BRACE => {}
                    _ => text.push_str(token.text()),
                },
                NodeOrToken::Node(_) => return None,
            }
        }
        Some(text)
    }

    /// The byte range of the name *inside* this `NAME_GROUP` (the span between the
    /// braces) — the location-aware counterpart to [`NameGroup::text`]. Returns
    /// `None` when it holds a nested node or the name is empty (`\begin{}`, nothing to
    /// highlight).
    pub fn range(&self) -> Option<TextRange> {
        let mut start: Option<TextSize> = None;
        let mut end: Option<TextSize> = None;
        for element in self.syntax.children_with_tokens() {
            match element {
                NodeOrToken::Token(token) => match token.kind() {
                    SyntaxKind::L_BRACE | SyntaxKind::R_BRACE => {}
                    _ => {
                        let range = token.text_range();
                        start.get_or_insert(range.start());
                        end = Some(range.end());
                    }
                },
                NodeOrToken::Node(_) => return None,
            }
        }
        Some(TextRange::new(start?, end?))
    }
}

impl Begin {
    /// The `{name}` group following `\begin`.
    pub fn name_group(&self) -> Option<NameGroup> {
        child::<NameGroup>(&self.syntax)
    }

    /// The environment name (braces dropped), or `None` for a malformed `\begin`.
    pub fn name(&self) -> Option<String> {
        self.name_group()?.text()
    }

    /// The byte range of the environment name inside the `NAME_GROUP`.
    pub fn name_range(&self) -> Option<TextRange> {
        self.name_group()?.range()
    }
}

impl End {
    /// The `{name}` group following `\end`.
    pub fn name_group(&self) -> Option<NameGroup> {
        child::<NameGroup>(&self.syntax)
    }

    /// The environment name (braces dropped), or `None` for a malformed `\end`.
    pub fn name(&self) -> Option<String> {
        self.name_group()?.text()
    }

    /// The byte range of the environment name inside the `NAME_GROUP`.
    pub fn name_range(&self) -> Option<TextRange> {
        self.name_group()?.range()
    }
}

impl Environment {
    /// The `\begin{…}` node, replacing the raw `children().find(==BEGIN)` idiom.
    pub fn begin(&self) -> Option<Begin> {
        child::<Begin>(&self.syntax)
    }

    /// The `\end{…}` node.
    pub fn end(&self) -> Option<End> {
        child::<End>(&self.syntax)
    }

    /// The environment name, read from the `\begin` node.
    pub fn name(&self) -> Option<String> {
        self.begin()?.name()
    }
}
