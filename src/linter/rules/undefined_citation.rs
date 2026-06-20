//! `undefined-citation`: a `\cite`-family key that matches no entry anywhere in
//! its document's bibliography.
//!
//! The bibliographic analog of [`undefined-ref`](super::undefined_ref): flagging
//! "defined nowhere" is sound only when the namespace is **complete**, so the same
//! two gates apply (from [`ResolvedCitations`]), plus a bibliography-specific one:
//!
//! - **closed** — every `.tex` include *and* every `\bibliography`/`\addbibresource`
//!   resource resolves to an analyzed file, so no opaque file could hold the key.
//! - **rooted** — the namespace contains a document root.
//! - **no `\nocite{*}`** — a wildcard pulls in the entire bibliography, so every
//!   key is "used" and nothing can be undefined.
//!
//! Inert when no [`ResolvedCitations`] is available (stdin, or the language server
//! today). `Severity::Warning`, conservative like `undefined-ref`.
//!
//! [`ResolvedCitations`]: crate::project::ResolvedCitations

use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Severity};

use super::{Rule, RuleContext};

pub struct UndefinedCitation;

impl Rule for UndefinedCitation {
    fn id(&self) -> &'static str {
        "undefined-citation"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn check_file(&self, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        // No project view, an incomplete namespace, or a `\nocite{*}` wildcard:
        // a missing key may live in a `.bib` we never analyzed, or all entries are
        // pulled in — either way, stay quiet.
        let Some(citations) = ctx.citations else {
            return;
        };
        if !citations.is_closed(ctx.path)
            || !citations.is_root_component(ctx.path)
            || citations.has_wildcard_nocite(ctx.path)
        {
            return;
        }

        sink.extend(
            ctx.model
                .citations()
                .iter()
                .filter(|cite| !citations.is_defined(ctx.path, &cite.name))
                .map(|cite| Diagnostic {
                    rule: self.id(),
                    severity: self.default_severity(),
                    path: PathBuf::new(),
                    start: usize::from(cite.range.start()),
                    end: usize::from(cite.range.end()),
                    message: format!("citation of undefined key `{}`", cite.name),
                    fix: None,
                }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::project::ResolvedCitations;
    use crate::project::citations::CiteFileFacts;
    use crate::project::graph::{FileFacts, IncludeGraph};
    use crate::project::include::BibTarget;
    use crate::semantic::SemanticModel;
    use crate::syntax::SyntaxNode;
    use smol_str::SmolStr;
    use std::collections::HashMap;
    use std::path::PathBuf;

    const DOC: &str = "doc.tex";
    const BIB: &str = "refs.bib";

    /// A single-file namespace whose `refs.bib` defines `keys`, optionally rooted
    /// and/or with a `\nocite{*}` wildcard.
    fn resolution(keys: &[&str], rooted: bool, wildcard: bool) -> ResolvedCitations {
        let graph = IncludeGraph::build(
            &[FileFacts {
                path: PathBuf::from(DOC),
                include_edges: Vec::new(),
            }],
            None,
        );
        let mut bib = HashMap::new();
        bib.insert(PathBuf::from(BIB), keys.iter().map(SmolStr::new).collect());
        ResolvedCitations::build(
            &[CiteFileFacts {
                path: PathBuf::from(DOC),
                bib_targets: vec![BibTarget::Path(PathBuf::from(BIB))],
                nocite_all: wildcard,
                is_document_root: rooted,
            }],
            &graph,
            &bib,
        )
    }

    fn findings(src: &str, citations: Option<&ResolvedCitations>) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse(src).green);
        let model = SemanticModel::build(&root);
        let ctx = RuleContext {
            path: std::path::Path::new(DOC),
            root: &root,
            model: &model,
            resolution: None,
            citations,
        };
        let mut out = Vec::new();
        UndefinedCitation.check_file(&ctx, &mut out);
        out
    }

    #[test]
    fn flags_cite_with_no_matching_entry() {
        let r = resolution(&["knuth1984"], true, false);
        let out = findings("\\cite{missing}\n", Some(&r));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "undefined-citation");
        assert!(out[0].message.contains("missing"));
    }

    #[test]
    fn defined_key_is_fine() {
        let r = resolution(&["knuth1984"], true, false);
        assert!(findings("\\cite{knuth1984}\n", Some(&r)).is_empty());
    }

    #[test]
    fn inert_without_resolution() {
        assert!(findings("\\cite{missing}\n", None).is_empty());
    }

    #[test]
    fn rootless_namespace_does_not_fire() {
        let r = resolution(&[], false, false);
        assert!(findings("\\cite{missing}\n", Some(&r)).is_empty());
    }

    #[test]
    fn wildcard_nocite_suppresses() {
        let r = resolution(&[], true, true);
        assert!(findings("\\cite{anything}\n", Some(&r)).is_empty());
    }

    #[test]
    fn cite_list_flags_each_undefined_key() {
        let r = resolution(&["a"], true, false);
        let out = findings("\\cite{a,b}\n", Some(&r));
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains('b'));
    }

    #[test]
    fn natbib_and_biblatex_commands_recognized() {
        let r = resolution(&["a"], true, false);
        // `\citep`, `\textcite`, `\parencite` of a missing key all fire.
        let out = findings("\\citep{x}\\textcite{y}\\parencite{z}\n", Some(&r));
        assert_eq!(out.len(), 3);
    }
}
