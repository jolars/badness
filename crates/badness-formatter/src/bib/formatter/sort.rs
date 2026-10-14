//! Deterministic ordering for the bib formatter: canonical field order within an
//! entry, and cite-key order across the file.
//!
//! Both routines reorder existing CST nodes only; they never synthesize or mutate
//! content, so meaning is preserved and the lowering stays a pure replay of the
//! reordered nodes. Field order is formatter-owned and independent of entry-type
//! validation, so a change to required fields cannot rearrange formatted entries.

use std::collections::HashMap;
use std::sync::LazyLock;

use super::lint_directive_verb;
use crate::bib::ast;
use crate::bib::syntax::{SyntaxKind, SyntaxNode};
use crate::directives::Verb;

// Keep related metadata together, including classic BibTeX spellings alongside
// their BibLaTeX equivalents, so every entry type has the same reading order.
const FIELD_ORDER: &[&str] = &[
    "title",
    "subtitle",
    "titleaddon",
    "shorttitle",
    "author",
    "shortauthor",
    "editor",
    "shorteditor",
    "editortype",
    "translator",
    "bookauthor",
    "journaltitle",
    "journal",
    "journalsubtitle",
    "shortjournal",
    "booktitle",
    "booksubtitle",
    "booktitleaddon",
    "maintitle",
    "mainsubtitle",
    "maintitleaddon",
    "eventtitle",
    "eventtitleaddon",
    "date",
    "year",
    "month",
    "day",
    "eventdate",
    "publisher",
    "institution",
    "school",
    "organization",
    "location",
    "address",
    "series",
    "volume",
    "volumes",
    "number",
    "issue",
    "edition",
    "version",
    "chapter",
    "part",
    "pages",
    "pagetotal",
    "eid",
    "doi",
    "eprint",
    "eprinttype",
    "archiveprefix",
    "eprintclass",
    "primaryclass",
    "isbn",
    "issn",
    "isrn",
    "url",
    "urldate",
    "type",
    "howpublished",
    "pubstate",
    "language",
    "langid",
    "note",
    "addendum",
    "abstract",
    "annotation",
    "annote",
    "keywords",
    "crossref",
    "xref",
    "xdata",
    "related",
    "relatedtype",
    "relatedstring",
    "file",
    "pdf",
];

static FIELD_RANKS: LazyLock<HashMap<&'static str, usize>> = LazyLock::new(|| {
    FIELD_ORDER
        .iter()
        .enumerate()
        .map(|(rank, &name)| (name, rank))
        .collect()
});

/// The `FIELD` children of `entry`, reordered into the shared title-first order.
/// Fields absent from [`FIELD_ORDER`] are alphabetized after the listed fields,
/// including in entries whose types are unknown.
///
/// The sort is **stable**, which is load-bearing: a repeated field name (two `note =`)
/// keeps its source order, so BibTeX's last/first-wins semantics are preserved.
pub(super) fn canonical_fields(entry: &SyntaxNode) -> Vec<SyntaxNode> {
    let mut fields: Vec<SyntaxNode> = ast::fields(entry).collect();
    fields.sort_by_cached_key(|field| {
        let name = ast::field_name(field).unwrap_or_default().to_lowercase();
        match FIELD_RANKS.get(name.as_str()) {
            // Known field: group 0, ordered by its position in the canonical sequence.
            Some(&rank) => (0u8, rank, String::new()),
            // Unknown field: group 1, alphabetized by name.
            None => (1u8, 0usize, name),
        }
    });
    fields
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
/// An entry targeted by a lint `skip` directive is pinned as another barrier. Moving
/// another entry ahead of it would silently retarget the directive after formatting.
///
/// A segment containing any `crossref`/`xdata` field is left in source order — the
/// conservative crossref guard (see [`segment_in_order`]).
pub(super) fn sorted_blocks(root: &SyntaxNode) -> Vec<SyntaxNode> {
    let blocks: Vec<SyntaxNode> = root.children().collect();
    let pinned = skip_targets(&blocks);
    let mut result: Vec<SyntaxNode> = Vec::with_capacity(blocks.len());

    let mut i = 0;
    while i < blocks.len() {
        if blocks[i].kind() != SyntaxKind::ENTRY || pinned[i] {
            result.push(blocks[i].clone());
            i += 1;
            continue;
        }
        // Accumulate a maximal run of consecutive entries, then sort it.
        let start = i;
        while i < blocks.len() && blocks[i].kind() == SyntaxKind::ENTRY && !pinned[i] {
            i += 1;
        }
        result.extend(segment_in_order(&blocks[start..i]));
    }
    result
}

/// Mark regular entries targeted by lint `skip` directives. Every `@comment` entry is
/// transparent to directive attachment, so multiple stacked directives—and even an
/// ordinary structured comment between a directive and its target—lead to the same
/// next meaningful block. Other block kinds are already fixed barriers and need no
/// additional marker.
fn skip_targets(blocks: &[SyntaxNode]) -> Vec<bool> {
    let mut pinned = vec![false; blocks.len()];
    let mut pending_skip = false;
    for (index, block) in blocks.iter().enumerate() {
        if block.kind() == SyntaxKind::COMMENT_ENTRY {
            pending_skip |= matches!(lint_directive_verb(block), Some(Verb::Skip));
            continue;
        }
        if pending_skip && block.kind() == SyntaxKind::ENTRY {
            pinned[index] = true;
        }
        pending_skip = false;
    }
    pinned
}

/// One run of consecutive entries, sorted by cite key (case-insensitive, stable) —
/// unless any entry in the run carries a `crossref`/`xdata` field, in which case the
/// run is returned untouched.
///
/// The guard is the safe v1 of the cross-reference constraint (a referenced parent
/// must stay after its children): skipping any run that contains a cross-reference
/// *source* guarantees we never reorder a parent ahead of a child within the run, and
/// barrier segmentation fixes cross-run order.
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
        canonical_fields(entry)
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
        let e = entry("@article{k, year = 2020, title = {T}, author = {A}}\n");
        assert_eq!(ordered_names(&e), ["title", "author", "year"]);
    }

    #[test]
    fn title_first_order_is_shared_by_all_entry_types() {
        for etype in builtin().entry_names().chain(["weirdtype"]) {
            let e = entry(&format!(
                "@{etype}{{k, url = {{U}}, date = {{2020}}, author = {{A}}, title = {{T}}}}\n"
            ));
            assert_eq!(
                ordered_names(&e),
                ["title", "author", "date", "url"],
                "field order differs for @{etype}"
            );
        }
    }

    #[test]
    fn unknown_fields_alphabetized_after_known() {
        let e = entry("@article{k, zzz = {z}, author = {A}, aaa = {a}}\n");
        assert_eq!(ordered_names(&e), ["author", "aaa", "zzz"]);
    }

    #[test]
    fn unknown_entry_type_uses_shared_order_and_alphabetical_tail() {
        let e = entry(
            "@weirdtype{k, charlie = {c}, Author = {A}, alpha = {a}, TITLE = {T}, bravo = {b}}\n",
        );
        assert_eq!(
            ordered_names(&e),
            ["title", "author", "alpha", "bravo", "charlie"]
        );
    }

    #[test]
    fn duplicate_field_names_keep_source_order() {
        let e = entry("@misc{k, note = {first}, title = {T}, NOTE = {second}}\n");
        let values: Vec<String> = canonical_fields(&e)
            .iter()
            .filter(|f| ast::field_name(f).is_some_and(|name| name.eq_ignore_ascii_case("note")))
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
    fn lint_skip_target_is_not_reordered() {
        let keys = ordered_keys(
            "@comment{badness-lint skip unknown-field}\n\
             @misc{zulu, unknown = {U}}\n\
             @misc{alpha, title = {A}}\n",
        );
        assert_eq!(keys, ["zulu", "alpha"]);
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
