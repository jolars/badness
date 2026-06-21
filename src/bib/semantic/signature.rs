//! The built-in **BibTeX field/entry signature database**: which fields each entry
//! type requires/allows, and a coarse category per field (name list, date,
//! verbatim-ish, or plain literal). The bib analog of
//! [`crate::semantic::signature`] — the place where *meaning* is assigned to entry
//! types and field names, kept strictly out of the parser (AGENTS.md decision #2).
//!
//! Like the LaTeX side, the data is fully static, so it lives in a process-wide
//! [`LazyLock`] loaded from one curated JSON file (`data/bib_fields.json`,
//! [`include_str!`]-ed, [`serde`]-deserialized). It is consulted directly; there is
//! no per-document overlay (entry types and field names are fixed, unlike
//! user-defined commands). Categories drive the Phase-2 formatter (name-list and
//! verbatim handling) and the Phase-3 linter (missing-required / unknown-field);
//! it is loaded now and consumed there.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;
use smol_str::SmolStr;

/// The coarse role of a field's value, used by the formatter and linter. Unlisted
/// fields default to [`FieldCategory::Literal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldCategory {
    /// A `and`-separated list of person/organization names (`author`, `editor`, …).
    Name,
    /// A date or date component (`date`, `year`, `month`, `urldate`, …).
    Date,
    /// A value the formatter must not reshape (`url`, `doi`, `eprint`, `file`).
    Verbatim,
    /// Anything else — a plain literal/title field.
    Literal,
}

/// The signature of a single field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSig {
    pub category: FieldCategory,
}

/// One entry in an entry type's *required* list: either a single mandatory field or
/// a set of alternatives of which at least one must be present (e.g. `author` **or**
/// `editor`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequiredField {
    One(SmolStr),
    OneOf(Vec<SmolStr>),
}

/// The signature of an entry type: its required and optional fields. Field names are
/// lowercased (BibTeX is case-insensitive).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EntrySig {
    pub required: Vec<RequiredField>,
    pub optional: Vec<SmolStr>,
}

/// The built-in field/entry signature database. Keys (entry types and field names)
/// are stored lowercased; lookups lowercase the query.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BibFieldDb {
    entries: HashMap<SmolStr, EntrySig>,
    fields: HashMap<SmolStr, FieldSig>,
}

impl BibFieldDb {
    /// The signature of entry type `name`, if known.
    pub fn entry(&self, name: &str) -> Option<&EntrySig> {
        self.entries.get(name.to_lowercase().as_str())
    }

    /// The signature of field `name`, if it carries non-default metadata.
    pub fn field(&self, name: &str) -> Option<&FieldSig> {
        self.fields.get(name.to_lowercase().as_str())
    }

    /// The category of field `name`, defaulting to [`FieldCategory::Literal`] for an
    /// unlisted field.
    pub fn category(&self, name: &str) -> FieldCategory {
        self.field(name)
            .map_or(FieldCategory::Literal, |sig| sig.category)
    }

    /// The known entry type names.
    pub fn entry_names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(SmolStr::as_str)
    }

    /// The fields carrying explicit metadata.
    pub fn field_names(&self) -> impl Iterator<Item = &str> {
        self.fields.keys().map(SmolStr::as_str)
    }
}

/// The process-wide built-in database, parsed once from the bundled JSON.
pub fn builtin() -> &'static BibFieldDb {
    &DB
}

const BIB_FIELDS_JSON: &str = include_str!("../../../data/bib_fields.json");

static DB: LazyLock<BibFieldDb> =
    LazyLock::new(|| parse(BIB_FIELDS_JSON).expect("bundled data/bib_fields.json must be valid"));

// --- deserialization ------------------------------------------------------

/// A `required` element: a single field name, or an array of alternatives.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawRequired {
    One(String),
    OneOf(Vec<String>),
}

#[derive(Deserialize, Default)]
struct RawEntry {
    #[serde(default)]
    required: Vec<RawRequired>,
    #[serde(default)]
    optional: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawCategory {
    Name,
    Date,
    Verbatim,
    Literal,
}

#[derive(Deserialize)]
struct RawField {
    category: RawCategory,
}

#[derive(Deserialize, Default)]
struct RawDb {
    #[serde(default)]
    entries: HashMap<String, RawEntry>,
    #[serde(default)]
    fields: HashMap<String, RawField>,
}

fn lower(s: String) -> SmolStr {
    SmolStr::new(s.to_lowercase())
}

impl From<RawRequired> for RequiredField {
    fn from(raw: RawRequired) -> Self {
        match raw {
            RawRequired::One(name) => RequiredField::One(lower(name)),
            RawRequired::OneOf(names) => {
                RequiredField::OneOf(names.into_iter().map(lower).collect())
            }
        }
    }
}

impl From<RawEntry> for EntrySig {
    fn from(raw: RawEntry) -> Self {
        EntrySig {
            required: raw.required.into_iter().map(Into::into).collect(),
            optional: raw.optional.into_iter().map(lower).collect(),
        }
    }
}

impl From<RawCategory> for FieldCategory {
    fn from(raw: RawCategory) -> Self {
        match raw {
            RawCategory::Name => FieldCategory::Name,
            RawCategory::Date => FieldCategory::Date,
            RawCategory::Verbatim => FieldCategory::Verbatim,
            RawCategory::Literal => FieldCategory::Literal,
        }
    }
}

impl From<RawField> for FieldSig {
    fn from(raw: RawField) -> Self {
        FieldSig {
            category: raw.category.into(),
        }
    }
}

fn parse(json: &str) -> serde_json::Result<BibFieldDb> {
    let raw: RawDb = serde_json::from_str(json)?;
    Ok(BibFieldDb {
        entries: raw
            .entries
            .into_iter()
            .map(|(name, sig)| (lower(name), sig.into()))
            .collect(),
        fields: raw
            .fields
            .into_iter()
            .map(|(name, sig)| (lower(name), sig.into()))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_json_parses() {
        // `builtin()` would panic on malformed/incomplete JSON.
        let db = builtin();
        assert!(db.entry_names().count() > 10);
    }

    #[test]
    fn covers_the_full_biblatex_data_model() {
        // Entry types and fields are taken verbatim from blx-dm.def. Spot-check
        // types and fields that the original hand-curated table lacked.
        let db = builtin();
        for ty in [
            "software",
            "reference",
            "dataset",
            "online",
            "suppperiodical",
        ] {
            assert!(db.entry(ty).is_some(), "missing entry type `{ty}`");
        }
        // `software` requires `title` (data model mandatory constraint).
        assert!(
            db.entry("software")
                .unwrap()
                .required
                .contains(&RequiredField::One(SmolStr::new("title")))
        );
        // Standard fields absent from the original table, now globally known.
        for f in [
            "langid",
            "shortjournal",
            "shorttitle",
            "pubstate",
            "urlyear",
        ] {
            assert!(db.field(f).is_some(), "missing field `{f}`");
        }
        assert_eq!(db.category("urlyear"), FieldCategory::Date);
        assert_eq!(db.category("shortauthor"), FieldCategory::Name);
    }

    #[test]
    fn new_data_model_types_use_oneof_date_constraints() {
        // A type added from the data model carries the `date`-or-`year` alternation
        // from its `\constraintfieldsxor`.
        let suppbook = builtin().entry("suppbook").expect("suppbook entry");
        assert!(suppbook.required.iter().any(|r| matches!(
            r,
            RequiredField::OneOf(alts) if alts.iter().any(|a| a == "date")
        )));
    }

    #[test]
    fn existing_types_required_aligned_to_data_model() {
        let db = builtin();
        let one = |s: &str| RequiredField::One(SmolStr::new(s));

        // `book` requires `author` specifically, not author-or-editor (edited
        // volumes are `@collection`).
        assert!(db.entry("book").unwrap().required.contains(&one("author")));

        // `incollection` and `periodical` mandate `editor` per the data model.
        assert!(
            db.entry("incollection")
                .unwrap()
                .required
                .contains(&one("editor"))
        );
        assert!(
            db.entry("periodical")
                .unwrap()
                .required
                .contains(&one("editor"))
        );

        // `online` mandates url OR doi OR eprint (a `\constraintfieldsor`).
        assert!(
            db.entry("online")
                .unwrap()
                .required
                .iter()
                .any(|r| matches!(
                    r,
                    RequiredField::OneOf(alts)
                        if alts.iter().any(|a| a == "url") && alts.iter().any(|a| a == "eprint")
                ))
        );

        // `misc` is in the data model's date-mandatory constraint list.
        assert!(db.entry("misc").unwrap().required.iter().any(|r| matches!(
            r, RequiredField::OneOf(alts) if alts.iter().any(|a| a == "date")
        )));

        // Classic-BibTeX-only types are absent from the model and keep `school`.
        assert!(
            db.entry("mastersthesis")
                .unwrap()
                .required
                .contains(&one("school"))
        );
    }

    #[test]
    fn article_required_fields() {
        let article = builtin().entry("article").expect("article entry");
        assert!(
            article
                .required
                .contains(&RequiredField::One(SmolStr::new("author")))
        );
        assert!(
            article
                .required
                .contains(&RequiredField::One(SmolStr::new("title")))
        );
        // `date` OR `year` is an alternation, not a single required field.
        assert!(article.required.iter().any(|r| matches!(
            r,
            RequiredField::OneOf(alts) if alts.iter().any(|a| a == "date")
        )));
    }

    #[test]
    fn entry_lookup_is_case_insensitive() {
        assert_eq!(builtin().entry("Article"), builtin().entry("article"));
        assert!(builtin().entry("InProceedings").is_some());
    }

    #[test]
    fn field_categories() {
        let db = builtin();
        assert_eq!(db.category("author"), FieldCategory::Name);
        assert_eq!(db.category("Editor"), FieldCategory::Name);
        assert_eq!(db.category("year"), FieldCategory::Date);
        assert_eq!(db.category("url"), FieldCategory::Verbatim);
        assert_eq!(db.category("doi"), FieldCategory::Verbatim);
        // Unlisted field falls back to Literal.
        assert_eq!(db.category("title"), FieldCategory::Literal);
        assert_eq!(db.category("totallyunknownfield"), FieldCategory::Literal);
    }
}
