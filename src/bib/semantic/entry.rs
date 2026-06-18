//! Data types collected by the bib [`semantic`](super) model: regular entries,
//! `@string` definitions, and `@string` uses. The bib analog of
//! [`crate::semantic::label`] — plain records keyed by position in the model's
//! vectors, with the resolve-pass flags (`duplicate`, `resolved`) filled in by
//! [`super::builder`].

use rowan::TextRange;
use smol_str::SmolStr;

/// A regular bibliographic entry (`@article{key, …}`). Entry type and key are
/// lowercased and as-authored respectively — `entry_type` is normalized (BibTeX is
/// case-insensitive), `key` keeps its source casing (cite keys are case-sensitive in
/// practice, though BibTeX folds them; we preserve and compare case-insensitively in
/// the resolver).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The entry type, lowercased (`"article"`, `"inproceedings"`).
    pub entry_type: SmolStr,
    /// The cite key as authored.
    pub key: SmolStr,
    /// The byte range of the `KEY` node.
    pub key_range: TextRange,
    /// The byte range of the whole entry.
    pub range: TextRange,
    /// `true` when an earlier entry already defined this key (case-insensitive); set
    /// by the resolve pass. The *first* occurrence stays `false`.
    pub duplicate: bool,
}

/// An `@string{ name = value }` macro definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringDef {
    /// The macro name, lowercased.
    pub name: SmolStr,
    /// The byte range of the name (`FIELD_NAME`) node.
    pub range: TextRange,
}

/// A use of an `@string` macro — a bare (unquoted, unbraced) name inside a field
/// value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringUse {
    /// The referenced macro name, lowercased.
    pub name: SmolStr,
    /// The byte range of the `LITERAL` piece.
    pub range: TextRange,
    /// `true` when the name matches a `@string` definition in this file or a
    /// predefined month macro; set by the resolve pass.
    pub resolved: bool,
}
