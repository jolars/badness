//! Small syntactic accessors over the BibTeX CST — an entry's type and cite key,
//! its fields' names and values, an `@string` definition's name, and the bare
//! macro *uses* inside a value. Purely syntactic: they know nothing about what a
//! field or entry *means* (AGENTS.md decision #2), so the semantic layer (and
//! later the formatter/linter) build on them without meaning leaking downward.
//!
//! The bib analog of [`crate::ast`], and the seed of a bib `ast/` layer — extracted
//! here when its first consumer, the [`semantic`](crate::bib::semantic) model,
//! appeared.

use rowan::TextRange;

use crate::bib::syntax::{SyntaxKind, SyntaxNode};

/// Concatenated text of a node's direct `WORD`/`NUMBER` token children. Reassembles
/// a name or key the lexer kept as one word run anyway (`westfahl:space` lexes as a
/// single `WORD`, since `:`/`-`/`.` are word characters), and tolerates the rare run
/// split across `WORD`+`NUMBER`. Structural punctuation and trivia are skipped.
fn joined_words(node: &SyntaxNode) -> String {
    let mut text = String::new();
    for element in node.children_with_tokens() {
        if let rowan::NodeOrToken::Token(token) = element
            && matches!(token.kind(), SyntaxKind::WORD | SyntaxKind::NUMBER)
        {
            text.push_str(token.text());
        }
    }
    text
}

/// The `n`-th child node of `parent` with kind `kind`.
fn child(parent: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxNode> {
    parent.children().find(|node| node.kind() == kind)
}

/// The entry type of an `ENTRY` / `STRING_ENTRY` / … node — the word following `@`
/// (e.g. `"article"`, `"string"`). Returns `None` for a malformed entry with no
/// `ENTRY_TYPE` child. Case is preserved; callers normalize.
pub fn entry_type(entry: &SyntaxNode) -> Option<String> {
    let node = child(entry, SyntaxKind::ENTRY_TYPE)?;
    let text = joined_words(&node);
    (!text.is_empty()).then_some(text)
}

/// The cite key of a regular `ENTRY` and the byte range of its `KEY` node. Returns
/// `None` when the entry has no key (a recovery case — e.g. `@misc{` with nothing
/// after the brace) or the key is empty.
pub fn cite_key(entry: &SyntaxNode) -> Option<(String, TextRange)> {
    let key = child(entry, SyntaxKind::KEY)?;
    let text = joined_words(&key);
    (!text.is_empty()).then(|| (text, key.text_range()))
}

/// The macro name defined by a `STRING_ENTRY` (`@string{ name = value }`) and the
/// byte range of its `FIELD_NAME` node. Returns `None` for a malformed `@string`
/// with no `name = …` field.
pub fn string_def_name(string_entry: &SyntaxNode) -> Option<(String, TextRange)> {
    let field = child(string_entry, SyntaxKind::FIELD)?;
    let name_node = child(&field, SyntaxKind::FIELD_NAME)?;
    let text = joined_words(&name_node);
    (!text.is_empty()).then(|| (text, name_node.text_range()))
}

/// The `FIELD` children of an entry, in source order.
pub fn fields(entry: &SyntaxNode) -> impl Iterator<Item = SyntaxNode> {
    entry.children().filter(|n| n.kind() == SyntaxKind::FIELD)
}

/// The name of a `FIELD` (the text of its `FIELD_NAME`), or `None` if absent.
pub fn field_name(field: &SyntaxNode) -> Option<String> {
    let node = child(field, SyntaxKind::FIELD_NAME)?;
    let text = joined_words(&node);
    (!text.is_empty()).then_some(text)
}

/// The `VALUE` node of a `FIELD` (the right-hand side of `=`), or `None` if absent.
pub fn field_value(field: &SyntaxNode) -> Option<SyntaxNode> {
    child(field, SyntaxKind::VALUE)
}

/// The bare-macro *uses* inside a `VALUE`: each `LITERAL` piece whose single token is
/// a `WORD` (an unquoted, unbraced name) is an `@string` reference. A `LITERAL`
/// wrapping a `NUMBER` is a literal number, not a macro use, and is skipped. Yields
/// `(name, range)` with the range of the `LITERAL` piece.
pub fn value_macro_uses(value: &SyntaxNode) -> impl Iterator<Item = (String, TextRange)> {
    value
        .children()
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
}
