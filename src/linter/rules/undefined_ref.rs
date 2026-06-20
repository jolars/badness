//! `undefined-ref`: a `\ref`-family key that matches no `\label` anywhere in its
//! document's label namespace.
//!
//! This is the one cross-file rule that needs a soundness gate. Flagging "defined
//! nowhere" is only safe when the analyzed namespace is **complete**: adding a
//! file can only *define* more keys, so a false positive arises exactly when a
//! defining file is missing. Two gates close that gap (both from
//! [`ResolvedLabels`]):
//!
//! - **closed** — every include in the namespace resolves to an analyzed member
//!   (no dynamic `\input{#1}`, no `\input` of a file outside the set), so no
//!   opaque file could hold the label.
//! - **rooted** — the namespace contains a document root (`\documentclass` /
//!   `\begin{document}`). A bare chapter fragment opened on its own, whose labels
//!   live in the main document, is therefore never flagged.
//!
//! Inert when no [`ResolvedLabels`] is available (stdin, or the language server
//! today). Labels defined by packages/classes are still out of reach and remain a
//! known false-positive source; `Severity::Warning` keeps that conservative.
//!
//! [`ResolvedLabels`]: crate::project::ResolvedLabels

use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Severity};

use super::{Rule, RuleContext};

pub struct UndefinedRef;

impl Rule for UndefinedRef {
    fn id(&self) -> &'static str {
        "undefined-ref"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn check_file(&self, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        // No project view, or an incomplete namespace (open, or rootless): a
        // missing key may simply live in a file we never analyzed, so stay quiet.
        let Some(resolution) = ctx.resolution else {
            return;
        };
        if !resolution.is_closed(ctx.path) || !resolution.is_root_component(ctx.path) {
            return;
        }

        sink.extend(
            ctx.model
                .refs()
                .iter()
                .filter(|reference| !resolution.is_defined(ctx.path, &reference.name))
                .map(|reference| Diagnostic {
                    rule: self.id(),
                    severity: self.default_severity(),
                    path: PathBuf::new(),
                    start: usize::from(reference.range.start()),
                    end: usize::from(reference.range.end()),
                    message: format!("reference to undefined label `{}`", reference.name),
                    fix: None,
                }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::project::ResolvedLabels;
    use crate::project::graph::{FileFacts, IncludeGraph};
    use crate::semantic::SemanticModel;
    use crate::syntax::SyntaxNode;
    use smol_str::SmolStr;

    const DOC: &str = "doc.tex";

    /// A single-file, no-includes namespace defining `labels`, optionally rooted.
    fn resolution(labels: &[&str], rooted: bool) -> ResolvedLabels {
        let graph = IncludeGraph::build(
            &[FileFacts {
                path: PathBuf::from(DOC),
                include_edges: Vec::new(),
            }],
            None,
        );
        ResolvedLabels::build(
            &[(
                PathBuf::from(DOC),
                labels.iter().map(SmolStr::new).collect(),
                rooted,
            )],
            &graph,
        )
    }

    fn findings(src: &str, resolution: Option<&ResolvedLabels>) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse(src).green);
        let model = SemanticModel::build(&root);
        let ctx = RuleContext {
            path: std::path::Path::new(DOC),
            root: &root,
            model: &model,
            resolution,
            citations: None,
        };
        let mut out = Vec::new();
        UndefinedRef.check_file(&ctx, &mut out);
        out
    }

    #[test]
    fn flags_ref_with_no_matching_label() {
        let r = resolution(&[], true);
        let out = findings("\\ref{missing}\n", Some(&r));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "undefined-ref");
        assert!(out[0].message.contains("missing"));
    }

    #[test]
    fn defined_label_is_fine() {
        let r = resolution(&["here"], true);
        assert!(findings("\\label{here}\\ref{here}\n", Some(&r)).is_empty());
    }

    #[test]
    fn inert_without_resolution() {
        assert!(findings("\\ref{missing}\n", None).is_empty());
    }

    #[test]
    fn rootless_namespace_does_not_fire() {
        // A bare fragment: the label may live in the (unanalyzed) main document.
        let r = resolution(&[], false);
        assert!(findings("\\ref{missing}\n", Some(&r)).is_empty());
    }

    #[test]
    fn cref_list_flags_each_undefined_key() {
        let r = resolution(&["a"], true);
        // `a` resolves, `b` does not.
        let out = findings("\\label{a}\\cref{a,b}\n", Some(&r));
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains('b'));
    }
}
