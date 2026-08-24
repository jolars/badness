//! Single-file semantic analysis.
//!
//! [`builder::build`] collects definitions and uses in a single CST walk, then
//! resolves them within the file. Incremental caching belongs to the root crate.
//!
//! Labels have document-wide rather than lexical scope. This model remains flat
//! and leaves cross-file resolution to the project layer.

pub mod builder;
pub mod completion;
pub mod define;
pub mod doc;
pub mod expl3;
pub mod label;
pub mod math;
pub mod mode;
pub mod outline;
pub mod pkgmeta;
pub mod signature;
pub mod tikz;
pub mod xparse;

pub use define::{DefSite, DefSiteKind, scan_definition_sites, scan_definitions};
pub use doc::{DocAssociation, DocKind, doc_associations};
pub use label::{
    CitationRef, ColorDef, ColorDefKind, GlossaryDef, GlossaryDefKind, LabelDef, LabelId, LabelRef,
    RefCommand, RefId,
};
pub use math::{
    DelimiterRole, MathAtom, MathAtomInfo, MathAtoms, MathClass, NAMED_MATH_OPERATORS, math_atoms,
    math_char_info, math_command_info,
};
pub use mode::{Mode, ModeIndex, argument_domain};
pub use outline::{LabelContext, OutlineItem, OutlineSymbol, label_context, outline};
pub use pkgmeta::{NeedsFormatDecl, OptionDecl, ProvidesDecl, ProvidesKind};
pub use signature::{
    ArgKind, ArgSpec, ArgumentDomain, CommandSig, ContentKind, EnvironmentSig, SignatureDb,
    Signatures, match_arg_slot, match_arg_slot_index, match_verbatim_arg_slot,
};

use std::collections::BTreeSet;

use smol_str::SmolStr;

use crate::declarations::ResolvedDeclarations;
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
    /// Color-name definitions (`\definecolor`, `\providecolor`, `\colorlet`),
    /// offered by color-name completion alongside the built-in list.
    pub(crate) color_defs: Vec<ColorDef>,
    /// Whether the file contains a `\nocite{*}` wildcard, which pulls every entry
    /// of the bibliography into the document — so `undefined-citation` cannot flag
    /// anything in its namespace.
    pub(crate) nocite_all: bool,
    /// The file's own `\ProvidesPackage`/`\ProvidesClass`/`\ProvidesFile` (or expl3
    /// variant) self-identification, if any (first wins). Recognized, never executed.
    pub(crate) provides: Option<ProvidesDecl>,
    /// The file's `\NeedsTeXFormat{format}[date]` declaration, if any (first wins).
    pub(crate) needs_format: Option<NeedsFormatDecl>,
    /// The file's `\DeclareOption` declarations (including the starred default handler).
    pub(crate) options: Vec<OptionDecl>,
    /// The project-declared command aliases *used in this file* whose target is a
    /// key-argument command ([`builder::key_argument_command`]). Recorded by name
    /// so a name-based gate can treat `\myref` exactly as it treats the `\cref` it
    /// was declared `like`, and holding only the names the file actually uses so an
    /// unrelated declaration edit still backdates this model.
    pub(crate) declared_key_commands: BTreeSet<SmolStr>,
}

impl SemanticModel {
    /// Build the model from a parse tree root.
    pub fn build(root: &SyntaxNode) -> Self {
        builder::build(root)
    }

    /// Build the model under a project's declared ref/cite command aliases.
    pub fn build_with_declarations(root: &SyntaxNode, declared: &ResolvedDeclarations) -> Self {
        builder::build_with_declarations(root, declared)
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

    /// Whether `name` is a command whose arguments hold opaque keys rather than
    /// typeset text — a curated one ([`builder::key_argument_command`]) or a
    /// project-declared alias of one.
    ///
    /// Answered by *name*, not by the ranges collected into [`refs`](Self::refs)
    /// and [`citations`](Self::citations): a command whose key cannot be extracted
    /// (`\myref{\textbf{a}}` yields no `LabelRef`) still has a key argument, and a
    /// declared alias must not be gated more weakly than the built-in it copies.
    pub fn is_key_argument_command(&self, name: &str) -> bool {
        builder::key_argument_command(name) || self.declared_key_commands.contains(name)
    }

    /// The glossary/acronym key definitions (`\newglossaryentry`/`\newacronym`/…)
    /// in this file.
    pub fn glossary_defs(&self) -> &[GlossaryDef] {
        &self.glossary_defs
    }

    /// The color-name definitions (`\definecolor`/`\providecolor`/`\colorlet`) in
    /// this file, offered by color-name completion.
    pub fn color_defs(&self) -> &[ColorDef] {
        &self.color_defs
    }

    /// Whether the file contains a `\nocite{*}` wildcard.
    pub fn has_wildcard_nocite(&self) -> bool {
        self.nocite_all
    }

    /// The file's `\ProvidesPackage`/`\ProvidesClass`/`\ProvidesFile` self-identification.
    pub fn provides(&self) -> Option<&ProvidesDecl> {
        self.provides.as_ref()
    }

    /// The file's `\NeedsTeXFormat` declaration.
    pub fn needs_format(&self) -> Option<&NeedsFormatDecl> {
        self.needs_format.as_ref()
    }

    /// The file's `\DeclareOption` declarations.
    pub fn options(&self) -> &[OptionDecl] {
        &self.options
    }

    pub fn reference(&self, id: RefId) -> &LabelRef {
        &self.refs[id.0 as usize]
    }

    /// Label definitions never referenced within *this* file.
    ///
    /// A per-file fact, **not** a lint signal: a label referenced only from
    /// another file looks unreferenced here. The cross-file `unreferenced-label`
    /// lint instead builds on the project-level
    /// `project::resolved_labels` (as `undefined-ref` does for refs),
    /// firing only in a closed, rooted namespace so it never false-positives on
    /// labels referenced from outside the analyzed set.
    pub fn unreferenced_labels(&self) -> impl Iterator<Item = LabelId> + '_ {
        (0..self.labels.len())
            .map(LabelId::from_index)
            .filter(move |id| !self.label(*id).referenced)
    }

    /// References whose key matches no `\label` in *this* file.
    ///
    /// A per-file fact, **not** a lint signal: the key may be defined in an
    /// included file. The `undefined-ref` lint instead consults the cross-file
    /// `project::resolved_labels`, firing only in a closed, rooted
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
    use crate::declarations::Declarations;
    use crate::parser::parse;

    fn model_of(src: &str) -> SemanticModel {
        SemanticModel::build(&SyntaxNode::new_root(parse(src).green))
    }

    fn declared_model(src: &str, json: &str) -> SemanticModel {
        let declared = serde_json::from_str::<Declarations>(json)
            .expect("declarations deserialize")
            .resolve()
            .expect("declarations resolve");
        SemanticModel::build_with_declarations(&SyntaxNode::new_root(parse(src).green), &declared)
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
    fn declared_reference_inherits_target_key_cardinality() {
        let model = declared_model(
            "\\label{a}\\label{b}\\one{a,b}\\many{a,b}\n",
            r#"{"commands": {
                 "one": {"like": "eqref"},
                 "many": {"like": "cref"}
               }}"#,
        );
        let names: Vec<_> = model.refs().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["a,b", "a", "b"]);
        assert_eq!(model.refs()[0].command, RefCommand::EqRef);
        assert!(
            model.refs()[1..]
                .iter()
                .all(|reference| reference.command == RefCommand::Cref)
        );
        assert!(model.labels().iter().all(|label| label.referenced));
    }

    #[test]
    fn declared_environment_inherits_label_key_behavior() {
        let model = declared_model(
            "\\begin{mylisting}[label=custom:list]\nbody\n\\end{mylisting}\n",
            r#"{"environments": {"mylisting": {"like": "lstlisting"}}}"#,
        );
        assert_eq!(model.labels().len(), 1);
        assert_eq!(model.labels()[0].name, "custom:list");
    }

    #[test]
    fn declared_citation_and_nocite_aliases_are_collected() {
        let model = declared_model(
            "\\sources{one,two}\\everything{*}\n",
            r#"{"commands": {
                 "sources": {"like": "parencite"},
                 "everything": {"like": "nocite"}
               }}"#,
        );
        let names: Vec<_> = model
            .citations()
            .iter()
            .map(|citation| citation.name.as_str())
            .collect();
        assert_eq!(names, vec!["one", "two"]);
        assert!(
            model
                .citations()
                .iter()
                .all(|citation| citation.command == "sources")
        );
        assert!(model.has_wildcard_nocite());
    }

    /// The name-based gate the linter uses. It must answer for an alias whose key
    /// never became a `LabelRef` — a nested-macro key is skipped by the collector,
    /// but the argument is a key argument all the same.
    #[test]
    fn a_declared_alias_is_a_key_argument_command_by_name() {
        let model = declared_model(
            "\\myref{\\textbf{a}}\\wrapper{a}\n",
            r#"{"commands": {
                 "myref": {"like": "cref"},
                 "wrapper": {"like": "citeauthor"}
               }}"#,
        );
        assert!(model.refs().is_empty(), "the nested-macro key is skipped");
        assert!(model.is_key_argument_command("myref"));
        assert!(model.is_key_argument_command("wrapper"));
        assert!(
            model.is_key_argument_command("cref"),
            "built-ins still pass"
        );
        assert!(!model.is_key_argument_command("emph"));
        assert!(
            !SemanticModel::build(&SyntaxNode::new_root(parse("\\myref{a}\n").green))
                .is_key_argument_command("myref"),
            "undeclared, the alias is an ordinary command"
        );
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
