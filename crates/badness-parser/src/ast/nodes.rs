//! Typed [`AstNode`] wrappers over the generic CST.
//!
//! Accessors are positional and tolerate greedily attached groups. They expose
//! syntax without assigning command meaning or consulting the signature database.

use rowan::{NodeOrToken, TextRange, TextSize};
use smol_str::{SmolStr, SmolStrBuilder};

use super::{AstNode, AstToken, child, child_token, children};
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
ast_node!(
    /// An `\if… … \else … \or … \fi` conditional the shape gate paired.
    Conditional, CONDITIONAL
);
ast_node!(
    /// One branch of a [`Conditional`]. The first holds the opener, its test, and
    /// the then-body; every later one opens with its `\else`/`\or` divider.
    ConditionalBranch, CONDITIONAL_BRANCH
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
    pub fn name(&self) -> Option<SmolStr> {
        self.control_word().map(|cw| SmolStr::new(cw.name()))
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
    pub fn nth_group_text(&self, n: usize) -> Option<SmolStr> {
        self.nth_group(n)?.inner_text()
    }

    /// The byte range of the content *inside* the `n`-th `GROUP` argument together
    /// with that inner text — the location-aware counterpart to
    /// [`Command::nth_group_text`]. See [`Group::inner`].
    pub fn nth_group_inner(&self, n: usize) -> Option<(TextRange, SmolStr)> {
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
    /// the group holds non-token content (a nested command — not a flat literal) or a
    /// parameter token (`\ref{#1}`, `\eqref{##1}` — a macro-parameter template whose
    /// literal value exists only at expansion time).
    pub fn inner_text(&self) -> Option<SmolStr> {
        Some(flat_inner(&self.syntax)?.text)
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
    pub fn inner(&self) -> Option<(TextRange, SmolStr)> {
        let inner = flat_inner(&self.syntax)?;
        let range = inner
            .range
            .unwrap_or_else(|| TextRange::empty(inner.empty_anchor));
        Some((range, inner.text))
    }

    /// The raw inner source of this group with its outer braces dropped, but *all*
    /// interior text preserved — nested `{…}` braces included. Unlike
    /// [`Group::inner_text`], which bails on nested nodes, this reconstructs the
    /// verbatim content needed for an xparse argument spec like `{m O{0} m}` (whose
    /// `{0}` default parses as a nested `GROUP`). Trivia is kept verbatim; the caller
    /// tokenizes the result.
    pub fn inner_source(&self) -> String {
        inner_source_of(&self.syntax)
    }

    /// The single `COMMAND` child wrapped in this group, if any.
    pub fn command(&self) -> Option<Command> {
        child::<Command>(&self.syntax)
    }

    /// The control-word name (leading `\` stripped) of a single `COMMAND` wrapped in
    /// this group, as in a `\newcommand{\foo}` name group. Returns `None` unless the
    /// group's only relevant child is exactly one control word.
    pub fn command_name(&self) -> Option<SmolStr> {
        self.command()?.name()
    }
}

/// The shared body of [`Group::inner_source`], kept kind-agnostic so the
/// free-function shim can call it on any node — an xparse default like `O{0}` parses
/// its `{0}` as a nested group but a top-level default body may be an `OPTIONAL`
/// rather than a `GROUP`. Concatenates all descendant token text, then drops a single
/// leading `{` and trailing `}` if present (a bracket-delimited `OPTIONAL` keeps its
/// brackets, matching the pre-wrapper behavior).
pub(crate) fn inner_source_of(node: &SyntaxNode) -> String {
    let mut text = String::new();
    for element in node.descendants_with_tokens() {
        if let NodeOrToken::Token(token) = element {
            text.push_str(token.text());
        }
    }
    let inner = text.strip_prefix('{').unwrap_or(&text);
    inner.strip_suffix('}').unwrap_or(inner).to_string()
}

impl NameGroup {
    /// The environment name — the literal text of this `NAME_GROUP`, braces dropped.
    /// Returns `None` when it holds non-token content or a parameter token.
    pub fn text(&self) -> Option<String> {
        Some(flat_inner(&self.syntax)?.text.to_string())
    }

    /// The byte range of the name *inside* this `NAME_GROUP` (the span between the
    /// braces) — the location-aware counterpart to [`NameGroup::text`]. Returns
    /// `None` when it holds a nested node or parameter token, or the name is empty
    /// (`\begin{}`, nothing to highlight).
    pub fn range(&self) -> Option<TextRange> {
        flat_inner(&self.syntax)?.range
    }
}

struct FlatInner {
    text: SmolStr,
    range: Option<TextRange>,
    empty_anchor: TextSize,
}

/// Reads brace-delimited content only when it is a literal token sequence.
/// Keeping rejection here ensures that text-only and range-aware accessors cannot
/// disagree about which source shapes are resolvable.
fn flat_inner(node: &SyntaxNode) -> Option<FlatInner> {
    let mut text = SmolStrBuilder::new();
    let mut start = None;
    let mut end = None;
    let mut empty_anchor = node.text_range().start();

    for element in node.children_with_tokens() {
        match element {
            NodeOrToken::Token(token) => match token.kind() {
                SyntaxKind::L_BRACE => empty_anchor = token.text_range().end(),
                SyntaxKind::R_BRACE => {}
                SyntaxKind::HASH => return None,
                _ => {
                    let token_range = token.text_range();
                    start.get_or_insert(token_range.start());
                    end = Some(token_range.end());
                    text.push_str(token.text());
                }
            },
            NodeOrToken::Node(_) => return None,
        }
    }

    Some(FlatInner {
        text: text.finish(),
        range: start
            .zip(end)
            .map(|(start, end)| TextRange::new(start, end)),
        empty_anchor,
    })
}

/// The environment an alias-delimiter node names: its bare `CONTROL_WORD` with the
/// leading `\` stripped, when that word is not `keyword` (the spelled-out
/// `\begin`/`\end`, whose name lives in a `NAME_GROUP` instead).
fn alias_delimiter_name(node: &SyntaxNode, keyword: &str) -> Option<String> {
    let head = child_token::<ControlWord>(node)?;
    let text = head.syntax().text();
    (text != keyword)
        .then(|| text.strip_prefix('\\'))
        .flatten()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

impl Begin {
    /// The `{name}` group following `\begin`.
    pub fn name_group(&self) -> Option<NameGroup> {
        child::<NameGroup>(&self.syntax)
    }

    /// The environment name (braces dropped), or `None` for a malformed `\begin`.
    ///
    /// A `BEGIN` opened by an *environment alias* (`\bea`, issue #109) carries no
    /// `NAME_GROUP` at all — the whole node is the bare control word — so the name
    /// falls back to that word with its `\` stripped. Positional and meaning-free,
    /// per decision #10: it reads the name from wherever the tree puts it and looks
    /// nothing up. `Signatures::environment` is what maps `bea` on to the target's
    /// curated behavior.
    ///
    /// The fallback is guarded on the head *not* being `\begin`, so the malformed
    /// `\begin`-without-a-name path (which builds a `BEGIN` with no `NAME_GROUP`)
    /// keeps reporting `None` rather than suddenly claiming to be named `begin`.
    pub fn name(&self) -> Option<String> {
        match self.name_group() {
            Some(group) => group.text(),
            None => alias_delimiter_name(&self.syntax, "\\begin"),
        }
    }

    /// Whether this `BEGIN` is an *environment-alias* delimiter — a bare control
    /// word standing in for `\begin{X}` — rather than a spelled-out `\begin{X}`.
    ///
    /// Purely structural (no `NAME_GROUP`, head is not `\begin`), like every other
    /// accessor here. It exists because [`name`](Self::name) makes the two shapes
    /// indistinguishable by name, and the alias table describes the *command*, not
    /// the name: a literal `\begin{bea}` written in a file that also defines `\bea`
    /// as an alias is a different, unrelated environment and must not inherit the
    /// target's behavior. `Signatures::environment_at` is the consumer.
    pub fn is_alias(&self) -> bool {
        self.name_group().is_none() && alias_delimiter_name(&self.syntax, "\\begin").is_some()
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

impl Conditional {
    /// The branches, in source order — at least one, since the grammar opens a
    /// branch before the opener.
    pub fn branches(&self) -> impl Iterator<Item = ConditionalBranch> {
        children::<ConditionalBranch>(&self.syntax)
    }

    /// The closing `\fi`, read *positionally* as the last child node rather than
    /// by matching the name: which control word closes a conditional is the
    /// grammar's call, and re-deciding it here would be the same meaning check
    /// twice (decision #10). `None` only if the gate's guarantee is ever broken,
    /// which callers must tolerate rather than assume away.
    pub fn closer(&self) -> Option<Command> {
        self.syntax.last_child().and_then(Command::cast)
    }
}

impl ConditionalBranch {
    /// The leading `\if…`/`\else`/`\or` control word of this branch, if it opens
    /// with one. Positional: the first child node, cast to a `COMMAND`.
    pub fn head(&self) -> Option<Command> {
        self.syntax.first_child().and_then(Command::cast)
    }
}
