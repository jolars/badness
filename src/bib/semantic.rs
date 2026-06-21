//! Single-file BibTeX semantic analysis: the per-file entry / cite-key / `@string`
//! model.
//!
//! The bib analog of [`crate::semantic`]. BibTeX has no lexical scoping — cite keys
//! and `@string` macros live in one file-global namespace — so the model is a
//! **flat** set of vectors (entries, `@string` defs, `@string` uses), built in one
//! CST walk by [`builder::build`], then a resolve pass flags duplicate cite keys and
//! marks each `@string` use resolved/undefined. No caching lives here; the salsa
//! layer that memoizes it (`bib_semantic_model`) is a later increment (Phase 4).
//!
//! **Cross-file resolution is out of scope for this slice.** A `@string` defined in
//! one `.bib` and used in another, or cite keys spanning a multi-file bibliography,
//! resolve only once a project-level query unions the per-file models — deferred,
//! exactly as on the LaTeX side. This is per-file only.

pub mod builder;
pub mod entry;
pub mod signature;

pub use entry::{Entry, StringDef, StringUse};
pub use signature::{BibFieldDb, EntrySig, FieldCategory, FieldSig, RequiredField, builtin};

pub(crate) use builder::MONTH_MACROS;

use crate::bib::syntax::SyntaxNode;

/// A file's regular entries, `@string` definitions, and `@string` uses.
///
/// `Eq` is load-bearing: the future `bib_semantic_model` salsa query will be **not**
/// `no_eq` (like `semantic_model`), so an edit leaving this model unchanged backdates
/// and downstream queries are not re-run.
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

    /// Entries whose cite key duplicates an earlier one (the 2nd+ occurrence).
    ///
    /// A per-file fact, **not** a lint signal: a `duplicate-key` diagnostic is built
    /// on this by the linter (Phase 3), which decides severity and how to point at
    /// each occurrence.
    pub fn duplicate_keys(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|entry| entry.duplicate)
    }

    /// `@string` uses that match no in-file definition or predefined month macro.
    ///
    /// A per-file fact, **not** a lint signal: in a multi-file bibliography the macro
    /// may be defined elsewhere. The Phase-3 `undefined-string` lint would gate on a
    /// cross-file resolution, mirroring `undefined-ref`.
    pub fn undefined_string_uses(&self) -> impl Iterator<Item = &StringUse> {
        self.string_uses.iter().filter(|u| !u.resolved)
    }

    /// `@string` definitions never referenced by any use in this file.
    ///
    /// A per-file fact, **not** a lint signal on its own: in a multi-file
    /// bibliography a `@string` defined here may be referenced from another `.bib`,
    /// so the Phase-3 `unused-string` lint that builds on this carries a single-file
    /// false-positive caveat until cross-file resolution gates it (Phase 4), exactly
    /// as [`undefined_string_uses`](Self::undefined_string_uses) is gated. Both
    /// `name` fields are lowercased, so the membership test is case-correct.
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
    fn entry_type_is_lowercased() {
        let model = model_of("@InProceedings{k, title = {X}}\n");
        assert_eq!(model.entries()[0].entry_type, "inproceedings");
    }

    #[test]
    fn keyless_entry_skipped() {
        // Recovery case: nothing after the brace, no key to record.
        let model = model_of("@misc{");
        assert_eq!(model.entries().len(), 0);
    }

    #[test]
    fn duplicate_keys_flagged_case_insensitively() {
        let model = model_of("@misc{Key, t = {a}}\n@book{key, t = {b}}\n@misc{other, t = {c}}\n");
        assert_eq!(model.entries().len(), 3);
        // First `Key` clean, second `key` duplicate, `other` clean.
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
        // `pub` is a macro use; `{Press}` and the quoted piece are not.
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
