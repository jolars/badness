//! Single-file BibTeX semantic analysis.
//!
//! [`builder::build`] collects entries, `@string` definitions, and uses in one
//! CST walk. A resolve pass then marks duplicate cite keys and unresolved uses.
//!
//! BibTeX has file-global rather than lexical scope. Cross-file resolution and
//! incremental caching belong to the project layer.

pub mod builder;
pub mod entry;
pub mod signature;

pub use entry::{Entry, StringDef, StringUse};
pub use signature::{BibFieldDb, EntrySig, FieldCategory, FieldSig, RequiredField, builtin};

pub use builder::MONTH_MACROS;

use crate::bib::syntax::SyntaxNode;

/// A file's regular entries, `@string` definitions, and `@string` uses.
///
/// Equality lets incremental queries avoid recomputing unchanged dependents.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Model {
    pub(crate) entries: Vec<Entry>,
    pub(crate) string_defs: Vec<StringDef>,
    pub(crate) string_uses: Vec<StringUse>,
}

impl Model {
    /// Build the model from a bib parse-tree root.
    pub fn build(root: &SyntaxNode) -> Self {
        builder::build(root)
    }

    /// Every regular entry, in source order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Every `@string` definition, in source order.
    pub fn string_defs(&self) -> &[StringDef] {
        &self.string_defs
    }

    /// Every `@string` use, in source order.
    pub fn string_uses(&self) -> &[StringUse] {
        &self.string_uses
    }

    /// Returns entries whose cite key duplicates an earlier entry in this file.
    pub fn duplicate_keys(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|entry| entry.duplicate)
    }

    /// `@string` uses that match no in-file definition or predefined month macro.
    ///
    /// A macro may still be defined in another bibliography file.
    pub fn undefined_string_uses(&self) -> impl Iterator<Item = &StringUse> {
        self.string_uses.iter().filter(|u| !u.resolved)
    }

    /// `@string` definitions never referenced by any use in this file.
    ///
    /// A definition may still be used from another bibliography file. Names are
    /// compared in lowercase.
    pub fn unused_string_defs(&self) -> impl Iterator<Item = &StringDef> {
        let used: std::collections::HashSet<&str> =
            self.string_uses.iter().map(|u| u.name.as_str()).collect();
        self.string_defs
            .iter()
            .filter(move |d| !used.contains(d.name.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::parse;

    fn model_of(src: &str) -> Model {
        Model::build(&parse(src).syntax())
    }

    #[test]
    fn collects_entry_type_and_key() {
        let model = model_of("@article{knuth1984, title = {X}}\n");
        assert_eq!(model.entries().len(), 1);
        assert_eq!(model.entries()[0].entry_type, "article");
        assert_eq!(model.entries()[0].key, "knuth1984");
        assert!(!model.entries()[0].duplicate);
    }

    #[test]
    fn caches_cleaned_title_and_author() {
        let model =
            model_of("@article{k, title = {The {\\TeX}book}, author = {Knuth, Donald E.}}\n");
        let e = &model.entries()[0];
        assert_eq!(e.title.as_deref(), Some("The {\\TeX}book"));
        assert_eq!(e.authors.as_deref(), Some("Knuth, Donald E."));
    }

    #[test]
    fn authors_falls_back_to_editor() {
        let model = model_of("@book{k, title = {T}, editor = {Lamport, Leslie}}\n");
        let e = &model.entries()[0];
        assert_eq!(e.authors.as_deref(), Some("Lamport, Leslie"));
    }

    #[test]
    fn missing_title_and_author_stay_none() {
        let model = model_of("@misc{k, year = {2020}}\n");
        let e = &model.entries()[0];
        assert!(e.title.is_none());
        assert!(e.authors.is_none());
    }

    #[test]
    fn entry_type_is_lowercased() {
        let model = model_of("@InProceedings{k, title = {X}}\n");
        assert_eq!(model.entries()[0].entry_type, "inproceedings");
    }

    #[test]
    fn keyless_entry_skipped() {
        let model = model_of("@misc{");
        assert_eq!(model.entries().len(), 0);
    }

    #[test]
    fn duplicate_keys_flagged_case_insensitively() {
        let model = model_of("@misc{Key, t = {a}}\n@book{key, t = {b}}\n@misc{other, t = {c}}\n");
        assert_eq!(model.entries().len(), 3);
        assert!(!model.entries()[0].duplicate);
        assert!(model.entries()[1].duplicate);
        assert!(!model.entries()[2].duplicate);
        let dups: Vec<_> = model.duplicate_keys().map(|e| e.key.as_str()).collect();
        assert_eq!(dups, vec!["key"]);
    }

    #[test]
    fn string_def_collected() {
        let model = model_of("@string{cup = {Cambridge University Press}}\n");
        assert_eq!(model.string_defs().len(), 1);
        assert_eq!(model.string_defs()[0].name, "cup");
    }

    #[test]
    fn string_use_resolved_by_in_file_def() {
        let model = model_of("@string{cup = {C}}\n@book{k, publisher = cup}\n");
        assert_eq!(model.string_uses().len(), 1);
        assert_eq!(model.string_uses()[0].name, "cup");
        assert!(model.string_uses()[0].resolved);
        assert_eq!(model.undefined_string_uses().count(), 0);
    }

    #[test]
    fn month_macro_use_is_resolved() {
        let model = model_of("@article{k, month = jan}\n");
        assert_eq!(model.string_uses().len(), 1);
        assert!(model.string_uses()[0].resolved);
    }

    #[test]
    fn undefined_string_use_reported() {
        let model = model_of("@book{k, publisher = nope}\n");
        assert_eq!(model.undefined_string_uses().count(), 1);
        assert_eq!(model.string_uses()[0].name, "nope");
    }

    #[test]
    fn number_value_is_not_a_string_use() {
        let model = model_of("@article{k, year = 2020}\n");
        assert_eq!(model.string_uses().len(), 0);
    }

    #[test]
    fn unused_string_def_reported() {
        let model =
            model_of("@string{cup = {C}}\n@string{used = {U}}\n@book{k, publisher = used}\n");
        let unused: Vec<_> = model
            .unused_string_defs()
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(unused, vec!["cup"]);
    }

    #[test]
    fn all_strings_used_reports_none() {
        let model = model_of("@string{cup = {C}}\n@book{k, publisher = cup}\n");
        assert_eq!(model.unused_string_defs().count(), 0);
    }

    #[test]
    fn string_use_in_concatenation() {
        let model = model_of("@book{k, publisher = pub # { Press}}\n@string{pub = {Foo}}\n");
        let uses: Vec<_> = model
            .string_uses()
            .iter()
            .map(|u| u.name.as_str())
            .collect();
        assert_eq!(uses, vec!["pub"]);
        assert!(model.string_uses()[0].resolved);
    }
}
