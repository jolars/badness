//! A flat document-symbol outline for `.bib` files: one symbol per regular
//! entry, keyed by its cite key.
//!
//! The bib analog of [`crate::semantic::outline`], but trivial by comparison:
//! BibTeX has no nesting, so the outline is a flat list (no children). It is
//! built from the salsa-cached [`Model`] the LSP already holds, rather than the
//! CST, since the model already carries every range an entry-level outline needs
//! (`key`, `key_range`, `range`). `@string`/`@preamble`/`@comment` blocks are
//! omitted — only regular entries are surfaced.

use rowan::TextRange;

use crate::bib::semantic::Model;

/// One outline entry: a cite key, its entry type, and the byte ranges the LSP
/// maps to an `lsp_types::DocumentSymbol`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibOutlineItem {
    /// The cite key, as authored — the symbol name.
    pub name: String,
    /// The entry type (`"article"`, …) — the symbol detail.
    pub detail: String,
    /// The byte range of the whole entry.
    pub range: TextRange,
    /// The byte range of the cite key (the selection range).
    pub selection_range: TextRange,
}

/// Build the flat outline for a `.bib` model: one item per regular entry, in
/// source order.
pub fn outline(model: &Model) -> Vec<BibOutlineItem> {
    model
        .entries()
        .iter()
        .map(|entry| BibOutlineItem {
            name: entry.key.to_string(),
            detail: entry.entry_type.to_string(),
            range: entry.range,
            selection_range: entry.key_range,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::parse;

    fn outline_of(src: &str) -> Vec<BibOutlineItem> {
        outline(&Model::build(&parse(src).syntax()))
    }

    #[test]
    fn one_item_per_entry_in_source_order() {
        let items = outline_of("@article{b, title = {B}}\n@book{a, title = {A}}\n");
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["b", "a"]);
        assert_eq!(items[0].detail, "article");
        assert_eq!(items[1].detail, "book");
    }

    #[test]
    fn string_and_comment_blocks_are_omitted() {
        let items = outline_of("@string{cup = {C}}\n@comment{x}\n@misc{k, title = {T}}\n");
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["k"]);
    }

    #[test]
    fn selection_range_is_the_key() {
        let src = "@article{key, title = {T}}\n";
        let items = outline_of(src);
        assert_eq!(items.len(), 1);
        let sel = &src[items[0].selection_range];
        assert_eq!(sel, "key");
    }
}
