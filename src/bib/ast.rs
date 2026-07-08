//! A typed AST layer over the BibTeX CST — the bib analog of [`crate::ast`]. Thin,
//! read-only wrappers ([`AstNode`]) giving nodes a typed identity and syntactic
//! accessors (an entry's type and cite key, its fields' names and values, an
//! `@string` definition's name, the bare macro *uses* inside a value).
//!
//! Purely syntactic: they know nothing about what a field or entry *means* (AGENTS.md
//! decisions #2, #10), so the semantic layer, formatter, and linter build on them
//! without meaning leaking downward.
//!
//! The free functions below are thin, kind-agnostic shims over the wrapper methods,
//! kept so existing `&SyntaxNode`-based call sites compile unchanged.

pub mod nodes;

pub use nodes::{Entry, EntryType, Field, FieldName, Key, StringEntry, Value};

use rowan::TextRange;

use crate::bib::syntax::{SyntaxKind, SyntaxNode};

/// A typed wrapper over a bib CST *node* of a single [`SyntaxKind`]. Mirrors
/// rust-analyzer's `AstNode` (and [`crate::ast::AstNode`]): `cast` succeeds iff
/// `can_cast(node.kind())`. Re-declared here rather than shared with the LaTeX side
/// because the two CSTs have distinct `SyntaxKind` enums and `Language` markers.
pub trait AstNode {
    fn can_cast(kind: SyntaxKind) -> bool
    where
        Self: Sized;
    fn cast(syntax: SyntaxNode) -> Option<Self>
    where
        Self: Sized;
    fn syntax(&self) -> &SyntaxNode;
}

/// The first child node castable to `N`.
pub fn child<N: AstNode>(parent: &SyntaxNode) -> Option<N> {
    parent.children().find_map(N::cast)
}

/// All child nodes castable to `N`, in source order.
pub fn children<N: AstNode>(parent: &SyntaxNode) -> impl Iterator<Item = N> {
    parent.children().filter_map(N::cast)
}

// --- Free-function shims (kind-agnostic; see module docs) ---------------------

/// The entry type of an `ENTRY` / `STRING_ENTRY` / … node — the word following `@`
/// (e.g. `"article"`, `"string"`). `None` for a malformed entry with no `ENTRY_TYPE`
/// child. Case is preserved; callers normalize.
pub fn entry_type(entry: &SyntaxNode) -> Option<String> {
    child::<EntryType>(entry).and_then(|t| t.text())
}

/// The cite key of a regular `ENTRY` and the byte range of its `KEY` node. `None` when
/// the entry has no key (a recovery case) or the key is empty.
pub fn cite_key(entry: &SyntaxNode) -> Option<(String, TextRange)> {
    let key = child::<Key>(entry)?;
    key.text().map(|text| (text, key.syntax().text_range()))
}

/// The macro name defined by a `STRING_ENTRY` (`@string{ name = value }`) and the
/// byte range of its `FIELD_NAME` node. `None` for a malformed `@string` with no
/// `name = …` field.
pub fn string_def_name(string_entry: &SyntaxNode) -> Option<(String, TextRange)> {
    let name = child::<Field>(string_entry)?.name_node()?;
    name.text().map(|text| (text, name.syntax().text_range()))
}

/// The `FIELD` children of an entry, in source order.
pub fn fields(entry: &SyntaxNode) -> impl Iterator<Item = SyntaxNode> {
    children::<Field>(entry).map(|f| f.syntax().clone())
}

/// The name of a `FIELD` (the text of its `FIELD_NAME`), or `None` if absent.
pub fn field_name(field: &SyntaxNode) -> Option<String> {
    child::<FieldName>(field).and_then(|n| n.text())
}

/// The `VALUE` node of a `FIELD` (the right-hand side of `=`), or `None` if absent.
pub fn field_value(field: &SyntaxNode) -> Option<SyntaxNode> {
    child::<Value>(field).map(|v| v.syntax().clone())
}

/// The bare-macro *uses* inside a `VALUE`: each `LITERAL` piece whose single token is
/// a `WORD` (an unquoted, unbraced name) is an `@string` reference. A `LITERAL`
/// wrapping a `NUMBER` is a literal number, not a macro use, and is skipped. Yields
/// `(name, range)` with the range of the `LITERAL` piece.
pub fn value_macro_uses(value: &SyntaxNode) -> impl Iterator<Item = (String, TextRange)> {
    nodes::macro_uses_of(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::parse;

    /// The first node of `kind` in a freshly parsed `src`.
    fn node(src: &str, kind: SyntaxKind) -> SyntaxNode {
        parse(src)
            .syntax()
            .descendants()
            .find(|n| n.kind() == kind)
            .unwrap_or_else(|| panic!("a {kind:?} node"))
    }

    #[test]
    fn entry_type_reads_word() {
        let entry = node("@article{k, title = {Hi}}\n", SyntaxKind::ENTRY);
        assert_eq!(entry_type(&entry).as_deref(), Some("article"));
    }

    #[test]
    fn cite_key_reassembles_colon_key() {
        let entry = node("@book{westfahl:space, title = {X}}\n", SyntaxKind::ENTRY);
        let (key, _range) = cite_key(&entry).expect("a key");
        assert_eq!(key, "westfahl:space");
    }

    #[test]
    fn cite_key_none_without_key() {
        let entry = node("@misc{", SyntaxKind::ENTRY);
        assert_eq!(cite_key(&entry), None);
    }

    #[test]
    fn string_def_name_reads_field_name() {
        let s = node("@string{jan = \"January\"}\n", SyntaxKind::STRING_ENTRY);
        let (name, _range) = string_def_name(&s).expect("a name");
        assert_eq!(name, "jan");
    }

    #[test]
    fn fields_and_names() {
        let entry = node("@misc{k, a = {x}, b = 3}\n", SyntaxKind::ENTRY);
        let names: Vec<_> = fields(&entry).filter_map(|f| field_name(&f)).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn value_macro_uses_finds_word_not_number() {
        // `t = pub # {x} # 2020`: only `pub` is a macro use; `2020` is a number.
        let field = node("@misc{k, t = pub # {x} # 2020}\n", SyntaxKind::FIELD);
        let value = field_value(&field).expect("a value");
        let uses: Vec<_> = value_macro_uses(&value).map(|(n, _)| n).collect();
        assert_eq!(uses, vec!["pub"]);
    }

    // --- Wrapper-native tests --------------------------------------------------

    #[test]
    fn cast_is_kind_exact() {
        let entry = node("@article{k, title = {Hi}}\n", SyntaxKind::ENTRY);
        assert!(Entry::cast(entry.clone()).is_some());
        assert!(Field::cast(entry).is_none());
    }

    #[test]
    fn entry_wrapper_reads_type_key_and_fields() {
        let entry = Entry::cast(node("@article{k, a = {x}, b = 3}\n", SyntaxKind::ENTRY)).unwrap();
        assert_eq!(entry.entry_type().as_deref(), Some("article"));
        assert_eq!(entry.cite_key().map(|(k, _)| k).as_deref(), Some("k"));
        let names: Vec<_> = entry.fields().filter_map(|f| f.name()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }
}
