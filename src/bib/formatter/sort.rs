//! Deterministic ordering for the bib formatter: canonical field order within an
//! entry, and cite-key order across the file (Tenet 1 — order is the formatter's
//! call, not the author's).
//!
//! Both routines reorder existing CST nodes only; they never synthesize or mutate
//! content, so meaning is preserved and the lowering stays a pure replay of the
//! reordered nodes. They read only the syntactic accessors ([`crate::bib::ast`]) plus
//! the static signature DB — no semantic-model state, no parser changes.

use std::collections::HashMap;

use crate::bib::ast;
use crate::bib::semantic::{BibFieldDb, RequiredField};
use crate::bib::syntax::{SyntaxKind, SyntaxNode};

/// The `FIELD` children of `entry`, reordered into the canonical order for its entry
/// type: the signature DB's **required-then-optional** sequence, with any field the DB
/// does not list alphabetized after the known ones. An unknown entry type has no known
/// fields, so all of its fields fall through to the alphabetical tail.
///
/// The sort is **stable**, which is load-bearing: a repeated field name (two `note =`)
/// keeps its source order, so BibTeX's last/first-wins semantics are preserved.
pub(super) fn canonical_fields(entry: &SyntaxNode, db: &BibFieldDb) -> Vec<SyntaxNode> {
    let etype = ast::entry_type(entry).unwrap_or_default();
    let ranks = field_ranks(db, &etype);

    let mut fields: Vec<SyntaxNode> = ast::fields(entry).collect();
    fields.sort_by_cached_key(|field| {
        let name = ast::field_name(field).unwrap_or_default().to_lowercase();
        match ranks.get(name.as_str()) {
            // Known field: group 0, ordered by its position in the canonical sequence.
            Some(&rank) => (0u8, rank, String::new()),
            // Unknown field: group 1, alphabetized by name.
            None => (1u8, 0usize, name),
        }
    });
    fields
}

/// A `field-name (lowercased) → rank` map giving the canonical position of each field
/// the DB lists for `etype`: required fields first (each `OneOf` alternative in listed
/// order), then optional fields. First occurrence wins. Empty for an unknown type.
fn field_ranks<'a>(db: &'a BibFieldDb, etype: &str) -> HashMap<&'a str, usize> {
    let mut order: Vec<&str> = Vec::new();
    if let Some(sig) = db.entry(etype) {
        for req in &sig.required {
            match req {
                RequiredField::One(name) => order.push(name.as_str()),
                RequiredField::OneOf(alts) => order.extend(alts.iter().map(|a| a.as_str())),
            }
        }
        order.extend(sig.optional.iter().map(|o| o.as_str()));
    }
    let mut ranks = HashMap::new();
    for (rank, name) in order.into_iter().enumerate() {
        ranks.entry(name).or_insert(rank);
    }
    ranks
}

/// The top-level blocks of `root`, reordered so regular entries are sorted by cite key
/// (case-insensitive) while every other block stays pinned.
///
/// Non-`ENTRY` blocks (`@string`, `@preamble`, `@comment`, inter-entry `JUNK`) are
/// **fixed barriers**: they keep their absolute position. Only the maximal runs of
/// consecutive entries *between* barriers are sorted. Because an entry never crosses a
/// barrier, it stays on the same side of every `@string` definition it began on, so
/// defined-before-use is preserved, and `@preamble`/`@comment`/`JUNK` stay put.
///
/// A segment containing any `crossref`/`xdata` field is left in source order — the
/// conservative crossref guard (see [`segment_in_order`]).
pub(super) fn sorted_blocks(root: &SyntaxNode) -> Vec<SyntaxNode> {
    let blocks: Vec<SyntaxNode> = root.children().collect();
    let mut result: Vec<SyntaxNode> = Vec::with_capacity(blocks.len());

    let mut i = 0;
    while i < blocks.len() {
        if blocks[i].kind() != SyntaxKind::ENTRY {
            result.push(blocks[i].clone());
            i += 1;
            continue;
        }
        // Accumulate a maximal run of consecutive entries, then sort it.
        let start = i;
        while i < blocks.len() && blocks[i].kind() == SyntaxKind::ENTRY {
            i += 1;
        }
        result.extend(segment_in_order(&blocks[start..i]));
    }
    result
}

/// One run of consecutive entries, sorted by cite key (case-insensitive, stable) —
/// unless any entry in the run carries a `crossref`/`xdata` field, in which case the
/// run is returned untouched.
///
/// The guard is the safe v1 of the cross-reference constraint (a referenced parent
/// must stay after its children): skipping any run that contains a cross-reference
/// *source* guarantees we never reorder a parent ahead of a child within the run, and
/// the barrier segmentation fixes cross-run order. A precise topological sort over the
/// key graph is a future refinement.
fn segment_in_order(segment: &[SyntaxNode]) -> Vec<SyntaxNode> {
    let mut entries = segment.to_vec();
    if segment.iter().any(has_cross_reference) {
        return entries;
    }
    entries.sort_by_cached_key(|entry| {
        ast::cite_key(entry)
            .map(|(key, _)| key.to_lowercase())
            .unwrap_or_default()
    });
    entries
}

/// Whether `entry` has a `crossref` or `xdata` field (case-insensitive). Only the
/// field's presence matters for the guard; its value is irrelevant.
fn has_cross_reference(entry: &SyntaxNode) -> bool {
    ast::fields(entry).any(|field| {
        ast::field_name(&field).is_some_and(|name| {
            let lc = name.to_lowercase();
            lc == "crossref" || lc == "xdata"
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::parse;
    use crate::bib::semantic::builtin;

    /// The first `ENTRY` node in a freshly parsed `src`.
    fn entry(src: &str) -> SyntaxNode {
        parse(src)
            .syntax()
            .descendants()
            .find(|n| n.kind() == SyntaxKind::ENTRY)
            .expect("an ENTRY node")
    }

    /// The lowercased field names of `entry` after canonical reordering.
    fn ordered_names(entry: &SyntaxNode) -> Vec<String> {
        canonical_fields(entry, builtin())
            .iter()
            .map(|f| ast::field_name(f).unwrap_or_default().to_lowercase())
            .collect()
    }

    /// The cite keys of the sorted top-level blocks of `src` (entries only).
    fn ordered_keys(src: &str) -> Vec<String> {
        sorted_blocks(&parse(src).syntax())
            .iter()
            .filter_map(|b| ast::cite_key(b).map(|(k, _)| k))
            .collect()
    }

    #[test]
    fn fields_sorted_to_canonical_order() {
        // article required: author, title, journaltitle, (date|year).
        let e = entry("@article{k, year = 2020, title = {T}, author = {A}}\n");
        assert_eq!(ordered_names(&e), ["author", "title", "year"]);
    }

    #[test]
    fn unknown_fields_alphabetized_after_known() {
        let e = entry("@article{k, zzz = {z}, author = {A}, aaa = {a}}\n");
        assert_eq!(ordered_names(&e), ["author", "aaa", "zzz"]);
    }

    #[test]
    fn unknown_entry_type_is_fully_alphabetical() {
        let e = entry("@weirdtype{k, charlie = {c}, alpha = {a}, bravo = {b}}\n");
        assert_eq!(ordered_names(&e), ["alpha", "bravo", "charlie"]);
    }

    #[test]
    fn duplicate_field_names_keep_source_order() {
        let e = entry("@misc{k, note = {first}, note = {second}}\n");
        let values: Vec<String> = canonical_fields(&e, builtin())
            .iter()
            .filter(|f| ast::field_name(f).as_deref() == Some("note"))
            .map(|f| ast::field_value(f).unwrap().to_string())
            .collect();
        assert_eq!(values, ["{first}", "{second}"]);
    }

    #[test]
    fn entries_sorted_by_key_case_insensitive() {
        let keys = ordered_keys("@misc{Charlie}\n@misc{alpha}\n@misc{Bravo}\n");
        assert_eq!(keys, ["alpha", "Bravo", "Charlie"]);
    }

    #[test]
    fn string_def_is_a_barrier() {
        // `apple` cannot migrate ahead of `zoo` across the `@string` barrier.
        let blocks =
            sorted_blocks(&parse("@misc{zoo}\n@string{m = \"x\"}\n@misc{apple}\n").syntax());
        assert_eq!(ast::cite_key(&blocks[0]).unwrap().0, "zoo");
        assert_eq!(blocks[1].kind(), SyntaxKind::STRING_ENTRY);
        assert_eq!(ast::cite_key(&blocks[2]).unwrap().0, "apple");
    }

    #[test]
    fn crossref_segment_left_in_source_order() {
        // `zzz` sorts after `proc`, but the crossref guard keeps source order.
        let keys = ordered_keys(
            "@inproceedings{zzz, crossref = {proc}}\n@proceedings{proc, title = {P}}\n",
        );
        assert_eq!(keys, ["zzz", "proc"]);
    }
}
