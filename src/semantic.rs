//! Single-file semantic analysis: the per-file label/reference def-use model.
//!
//! LaTeX labels live in one document/project-global namespace — there is no
//! lexical scoping — so the model is a **flat** pair of vectors (defs + refs),
//! not a scope tree. It is built in one
//! CST walk by [`builder::build`], then a resolve pass marks each def
//! `referenced` and each ref `resolved` by matching keys. No caching lives
//! here; the [`incremental`](crate::incremental) salsa layer owns that, via the
//! `semantic_model` query.
//!
//! **Cross-file resolution is deferred.** A label defined in an `\input`-ed file
//! and referenced here resolves only once a project-level query unions label
//! sets across the include graph. This slice is per-file only — "harness + model
//! only", like
//! [`incremental`](crate::incremental) and the project graph landed.

pub mod builder;
pub mod define;
pub mod doc;
pub mod label;
pub mod load;
pub mod outline;
pub mod signature;
pub mod xparse;

pub use define::{DefSite, DefSiteKind, scan_definition_sites, scan_definitions};
pub use doc::{DocAssociation, DocKind, doc_associations};
pub use label::{
    CitationRef, GlossaryDef, GlossaryDefKind, LabelDef, LabelId, LabelRef, RefCommand, RefId,
};
pub use load::{
    DiskPackageSource, PackageSource, collect_package_signatures, disk_scope_signatures,
};
pub use outline::{LabelContext, OutlineItem, OutlineSymbol, label_context, outline};
pub use signature::{
    ArgKind, ArgSpec, CommandSig, ContentKind, EnvironmentSig, SignatureDb, Signatures,
};

use crate::syntax::SyntaxNode;

/// A file's label definitions and reference uses.
///
/// `Eq` is load-bearing: the `semantic_model` salsa query is **not** `no_eq`
/// (unlike `parsed_document`), so an edit leaving this model unchanged backdates
/// and downstream queries are not re-run.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SemanticModel {
    pub(crate) labels: Vec<LabelDef>,
    pub(crate) refs: Vec<LabelRef>,
    pub(crate) citations: Vec<CitationRef>,
    /// Glossary/acronym key definitions (`\newglossaryentry`, `\newacronym`, …).
    pub(crate) glossary_defs: Vec<GlossaryDef>,
    /// Whether the file contains a `\nocite{*}` wildcard, which pulls every entry
    /// of the bibliography into the document — so `undefined-citation` cannot flag
    /// anything in its namespace.
    pub(crate) nocite_all: bool,
}

impl SemanticModel {
    /// Build the model from a parse tree root.
    pub fn build(root: &SyntaxNode) -> Self {
        builder::build(root)
    }

    pub fn labels(&self) -> &[LabelDef] {
        &self.labels
    }

    pub fn label(&self, id: LabelId) -> &LabelDef {
        &self.labels[id.0 as usize]
    }

    pub fn refs(&self) -> &[LabelRef] {
        &self.refs
    }

    /// The citation uses (`\cite`/`\parencite`/… keys) in this file.
    pub fn citations(&self) -> &[CitationRef] {
        &self.citations
    }

    /// The glossary/acronym key definitions (`\newglossaryentry`/`\newacronym`/…)
    /// in this file.
    pub fn glossary_defs(&self) -> &[GlossaryDef] {
        &self.glossary_defs
    }

    /// Whether the file contains a `\nocite{*}` wildcard.
    pub fn has_wildcard_nocite(&self) -> bool {
        self.nocite_all
    }

    pub fn reference(&self, id: RefId) -> &LabelRef {
        &self.refs[id.0 as usize]
    }

    /// Label definitions never referenced within *this* file.
    ///
    /// A per-file fact, **not** a lint signal: a label referenced only from
    /// another file looks unreferenced here. A cross-file "unused label"
    /// diagnostic would build on the project-level
    /// [`crate::project::resolved_labels`] (as `undefined-ref` does for refs),
    /// but is deferred — it can false-positive on labels referenced from outside
    /// the analyzed set.
    pub fn unreferenced_labels(&self) -> impl Iterator<Item = LabelId> + '_ {
        (0..self.labels.len())
            .map(LabelId::from_index)
            .filter(move |id| !self.label(*id).referenced)
    }

    /// References whose key matches no `\label` in *this* file.
    ///
    /// A per-file fact, **not** a lint signal: the key may be defined in an
    /// included file. The `undefined-ref` lint instead consults the cross-file
    /// [`crate::project::resolved_labels`], firing only in a closed, rooted
    /// document namespace.
    pub fn unresolved_refs(&self) -> impl Iterator<Item = RefId> + '_ {
        (0..self.refs.len())
            .map(RefId::from_index)
            .filter(move |id| !self.reference(*id).resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn model_of(src: &str) -> SemanticModel {
        SemanticModel::build(&SyntaxNode::new_root(parse(src).green))
    }

    #[test]
    fn label_creates_def() {
        let model = model_of("\\label{sec:intro}\n");
        assert_eq!(model.labels().len(), 1);
        assert_eq!(model.labels()[0].name, "sec:intro");
        assert!(!model.labels()[0].referenced);
    }

    #[test]
    fn ref_creates_use() {
        let model = model_of("\\ref{sec:intro}\n");
        assert_eq!(model.refs().len(), 1);
        assert_eq!(model.refs()[0].name, "sec:intro");
        assert_eq!(model.refs()[0].command, RefCommand::Ref);
        assert!(!model.refs()[0].resolved);
    }

    #[test]
    fn label_and_ref_resolve() {
        let model = model_of("\\label{a}\\ref{a}\n");
        assert!(model.labels()[0].referenced);
        assert!(model.refs()[0].resolved);
        assert_eq!(model.unreferenced_labels().count(), 0);
        assert_eq!(model.unresolved_refs().count(), 0);
    }

    #[test]
    fn ref_family_recognized() {
        let model = model_of(
            "\\pageref{x}\\eqref{x}\\autoref{x}\\nameref{x}\\Cref{x}\\vref{x}\\Vref{x}\\cpageref{x}\n",
        );
        let kinds: Vec<_> = model.refs().iter().map(|r| r.command).collect();
        assert_eq!(
            kinds,
            vec![
                RefCommand::PageRef,
                RefCommand::EqRef,
                RefCommand::AutoRef,
                RefCommand::NameRef,
                RefCommand::CrefUpper,
                RefCommand::Vref,
                RefCommand::VrefUpper,
                RefCommand::CpageRef,
            ]
        );
    }

    #[test]
    fn non_ref_commands_ignored() {
        let model = model_of("\\textbf{x}\\section{Hi}\\emph{y}\n");
        assert_eq!(model.labels().len(), 0);
        assert_eq!(model.refs().len(), 0);
    }

    #[test]
    fn cref_splits_comma_list() {
        let model = model_of("\\cref{a,b,c}\n");
        let names: Vec<_> = model.refs().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
        assert!(model.refs().iter().all(|r| r.command == RefCommand::Cref));
        // All split keys share the single command range.
        let range = model.refs()[0].range;
        assert!(model.refs().iter().all(|r| r.range == range));
    }

    #[test]
    fn plain_ref_does_not_split() {
        let model = model_of("\\ref{a,b}\n");
        assert_eq!(model.refs().len(), 1);
        assert_eq!(model.refs()[0].name, "a,b");
    }

    #[test]
    fn cref_empty_and_blank_keys_dropped() {
        assert_eq!(model_of("\\cref{}\n").refs().len(), 0);
        let model = model_of("\\cref{a,,b}\n");
        let names: Vec<_> = model.refs().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn unresolved_ref_when_no_label() {
        let model = model_of("\\ref{missing}\n");
        assert!(!model.refs()[0].resolved);
        assert_eq!(model.unresolved_refs().count(), 1);
    }

    #[test]
    fn unreferenced_label_reported() {
        let model = model_of("\\label{x}\n");
        assert_eq!(model.unreferenced_labels().count(), 1);
    }

    #[test]
    fn duplicate_labels_preserved() {
        let model = model_of("\\label{x}\\label{x}\\ref{x}\n");
        assert_eq!(model.labels().len(), 2);
        assert!(model.labels().iter().all(|l| l.referenced));
        assert!(model.refs()[0].resolved);
    }

    #[test]
    fn nested_macro_key_skipped() {
        let model = model_of("\\label{\\foo}\n");
        assert_eq!(model.labels().len(), 0);
    }

    #[test]
    fn label_collected_inside_environment() {
        let model = model_of("\\begin{figure}\n\\label{fig:one}\n\\end{figure}\n");
        assert_eq!(model.labels().len(), 1);
        assert_eq!(model.labels()[0].name, "fig:one");
    }
}
