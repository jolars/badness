//! Typed [`AstNode`](super::AstNode) wrappers over the BibTeX CST, with syntactic
//! accessors. The bib analog of [`crate::ast::nodes`]. Purely syntactic: they know
//! nothing about what a field or entry *means* (AGENTS.md decisions #2, #10).

use rowan::{NodeOrToken, TextRange};

use super::{AstNode, child, children};
use crate::bib::syntax::{SyntaxKind, SyntaxNode};

/// Declares a newtype wrapper over a `SyntaxNode` of exactly one bib `SyntaxKind`,
/// implementing [`AstNode`]. Only the identity is generated; accessors are
/// hand-written. The bib analog of `crate::ast::nodes::ast_node!`.
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
    /// A regular bibliographic entry: `@type{ key, fields }`.
    Entry, ENTRY
);
ast_node!(
    /// An `@string{ name = value }` macro definition.
    StringEntry, STRING_ENTRY
);
ast_node!(
    /// A `name = value` field.
    Field, FIELD
);
ast_node!(
    /// The right-hand side of a field's `=`; pieces separated by `#`.
    Value, VALUE
);
ast_node!(
    /// The type word following `@` (`article`, `string`, …).
    EntryType, ENTRY_TYPE
);
ast_node!(
    /// A cite key.
    Key, KEY
);
ast_node!(
    /// A field / macro name left of `=`.
    FieldName, FIELD_NAME
);

/// Concatenated text of a node's direct `WORD`/`NUMBER` token children. Reassembles
/// a name or key the lexer kept as one word run anyway (`westfahl:space` lexes as a
/// single `WORD`, since `:`/`-`/`.` are word characters), and tolerates the rare run
/// split across `WORD`+`NUMBER`. Structural punctuation and trivia are skipped. Kept
/// free so both the typed accessors and the free-function shims share it.
pub(crate) fn joined_words(node: &SyntaxNode) -> String {
    let mut text = String::new();
    for element in node.children_with_tokens() {
        if let NodeOrToken::Token(token) = element
            && matches!(token.kind(), SyntaxKind::WORD | SyntaxKind::NUMBER)
        {
            text.push_str(token.text());
        }
    }
    text
}

/// [`joined_words`], but `None` for an empty run — the shape every name/key accessor
/// wants.
fn nonempty_words(node: &SyntaxNode) -> Option<String> {
    let text = joined_words(node);
    (!text.is_empty()).then_some(text)
}

impl EntryType {
    /// The type word, or `None` when empty. Case is preserved; callers normalize.
    pub fn text(&self) -> Option<String> {
        nonempty_words(&self.syntax)
    }
}

impl Key {
    /// The cite-key text, or `None` when empty.
    pub fn text(&self) -> Option<String> {
        nonempty_words(&self.syntax)
    }
}

impl FieldName {
    /// The field / macro name, or `None` when empty.
    pub fn text(&self) -> Option<String> {
        nonempty_words(&self.syntax)
    }
}

impl Entry {
    /// The entry type — the word following `@` (`article`, …), or `None` for a
    /// malformed entry with no `ENTRY_TYPE` child.
    pub fn entry_type(&self) -> Option<String> {
        child::<EntryType>(&self.syntax)?.text()
    }

    /// The cite key and the byte range of its `KEY` node, or `None` when the entry has
    /// no key (a recovery case) or the key is empty.
    pub fn cite_key(&self) -> Option<(String, TextRange)> {
        let key = child::<Key>(&self.syntax)?;
        key.text().map(|text| (text, key.syntax.text_range()))
    }

    /// The `FIELD` children, in source order.
    pub fn fields(&self) -> impl Iterator<Item = Field> {
        children::<Field>(&self.syntax)
    }
}

impl StringEntry {
    /// The macro name defined by `@string{ name = value }` and the byte range of its
    /// `FIELD_NAME` node, or `None` for a malformed `@string` with no `name = …` field.
    pub fn def_name(&self) -> Option<(String, TextRange)> {
        let name = child::<Field>(&self.syntax)?.name_node()?;
        name.text().map(|text| (text, name.syntax.text_range()))
    }
}

impl Field {
    /// The field's `FIELD_NAME` node, if present.
    pub fn name_node(&self) -> Option<FieldName> {
        child::<FieldName>(&self.syntax)
    }

    /// The field name (the text of its `FIELD_NAME`), or `None` if absent.
    pub fn name(&self) -> Option<String> {
        self.name_node()?.text()
    }

    /// The `VALUE` node (the right-hand side of `=`), or `None` if absent.
    pub fn value(&self) -> Option<Value> {
        child::<Value>(&self.syntax)
    }
}

impl Value {
    /// The bare-macro *uses* inside this value: each `LITERAL` piece whose single
    /// token is a `WORD` (an unquoted, unbraced name) is an `@string` reference. A
    /// `LITERAL` wrapping a `NUMBER` is a literal number, not a macro use, and is
    /// skipped. Yields `(name, range)` with the range of the `LITERAL` piece.
    pub fn macro_uses(&self) -> impl Iterator<Item = (String, TextRange)> {
        macro_uses_of(&self.syntax)
    }
}

/// The shared body of [`Value::macro_uses`], kept kind-agnostic so the free-function
/// shim can call it on any node.
pub(crate) fn macro_uses_of(node: &SyntaxNode) -> impl Iterator<Item = (String, TextRange)> {
    node.children()
        .filter(|n| n.kind() == SyntaxKind::LITERAL)
        .filter_map(|literal| {
            let token = literal
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| matches!(t.kind(), SyntaxKind::WORD | SyntaxKind::NUMBER))?;
            (token.kind() == SyntaxKind::WORD)
                .then(|| (token.text().to_string(), literal.text_range()))
        })
}
