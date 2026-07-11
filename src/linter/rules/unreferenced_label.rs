//! `unreferenced-label`: a `\label` never targeted by any `\ref`-family command
//! anywhere in its document's label namespace.
//!
//! The mirror image of [`undefined-ref`](super::undefined_ref): that rule flags a
//! *reference* with no definition, this one flags a *definition* with no
//! reference. Both consult the cross-file [`ResolvedLabels`] and share the exact
//! same soundness gate, because both are only trustworthy over a **complete**
//! namespace:
//!
//! - **closed** — every include resolves to an analyzed member, so no opaque file
//!   could hold the missing `\ref` (a `\label` referenced only from an
//!   un-analyzed `\input` would otherwise be a false positive).
//! - **rooted** — the namespace contains a document root. A bare chapter fragment
//!   opened on its own, whose labels are referenced from the main document, is
//!   never flagged.
//!
//! Inert when no [`ResolvedLabels`] is available (stdin, or the language server
//! today). Report-only: two resolutions are always valid (delete the dead
//! `\label`, or add the missing `\ref`), so there is no single correct-by-
//! construction rewrite — no autofix (see [`crate::linter`] tenet 1).
//! `Severity::Warning` keeps a stray package-defined reference target
//! conservative.
//!
//! [`ResolvedLabels`]: crate::project::ResolvedLabels

use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Severity};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "A label that no `\\ref`-family command ever targets:",
    source: "\\section{Intro}\\label{sec:intro}\n",
}];

pub struct UnreferencedLabel;

impl Rule for UnreferencedLabel {
    fn id(&self) -> &'static str {
        "unreferenced-label"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a `\\label` that no `\\ref`-family command targets anywhere in the \
         document. The mirror of `undefined-ref`, and sound only when the label \
         namespace is complete, so it stays silent unless the project view is \
         **closed** (every include resolves to an analyzed file) and **rooted**. \
         Inert on stdin or wherever no cross-file label resolution is available. \
         Report-only: removing the dead label or adding a reference are both \
         valid, so there is no autofix."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn check_file(&self, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        // No project view, or an incomplete namespace (open, or rootless): a
        // reference may live in a file we never analyzed, so stay quiet.
        let Some(resolution) = ctx.resolution else {
            return;
        };
        if !resolution.is_closed(ctx.path) || !resolution.is_root_component(ctx.path) {
            return;
        }

        sink.extend(
            ctx.model
                .labels()
                .iter()
                .filter(|label| !resolution.is_referenced(ctx.path, &label.name))
                .map(|label| Diagnostic {
                    rule: self.id(),
                    severity: self.default_severity(),
                    path: PathBuf::new(),
                    start: usize::from(label.range.start()),
                    end: usize::from(label.range.end()),
                    message: format!("label `{}` is never referenced", label.name),
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

    /// A single-file, no-includes namespace defining `labels` and using `refs`,
    /// optionally rooted.
    fn resolution(labels: &[&str], refs: &[&str], rooted: bool) -> ResolvedLabels {
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
                refs.iter().map(SmolStr::new).collect(),
                rooted,
            )],
            &graph,
        )
    }

    fn findings(src: &str, resolution: Option<&ResolvedLabels>) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse(src).green);
        let model = SemanticModel::build(&root);
        let ctx = RuleContext::new(
            std::path::Path::new(DOC),
            &root,
            &model,
            resolution,
            None,
            None,
        );
        let mut out = Vec::new();
        UnreferencedLabel.check_file(&ctx, &mut out);
        out
    }

    #[test]
    fn flags_label_with_no_reference() {
        let r = resolution(&["sec:intro"], &[], true);
        let out = findings("\\label{sec:intro}\n", Some(&r));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "unreferenced-label");
        assert!(out[0].message.contains("sec:intro"));
        assert!(out[0].fix.is_none());
    }

    #[test]
    fn referenced_label_is_fine() {
        let r = resolution(&["here"], &["here"], true);
        assert!(findings("\\label{here}\\ref{here}\n", Some(&r)).is_empty());
    }

    #[test]
    fn inert_without_resolution() {
        assert!(findings("\\label{orphan}\n", None).is_empty());
    }

    #[test]
    fn rootless_namespace_does_not_fire() {
        // A bare fragment: the reference may live in the (unanalyzed) main document.
        let r = resolution(&["orphan"], &[], false);
        assert!(findings("\\label{orphan}\n", Some(&r)).is_empty());
    }

    #[test]
    fn open_namespace_does_not_fire() {
        // Manually build an open (non-closed) namespace via an unresolved include.
        let graph = IncludeGraph::build(
            &[FileFacts {
                path: PathBuf::from(DOC),
                include_edges: vec![crate::project::include::IncludeEdgeKey {
                    kind: crate::project::IncludeKind::Input,
                    target: crate::project::IncludeTarget::Dynamic,
                }],
            }],
            None,
        );
        let r = ResolvedLabels::build(
            &[(
                PathBuf::from(DOC),
                vec![SmolStr::new("orphan")],
                Vec::new(),
                true,
            )],
            &graph,
        );
        assert!(!r.is_closed(std::path::Path::new(DOC)));
        assert!(findings("\\input{x}\\label{orphan}\n", Some(&r)).is_empty());
    }

    #[test]
    fn flags_only_the_unreferenced_label() {
        let r = resolution(&["used", "dead"], &["used"], true);
        let out = findings("\\label{used}\\ref{used}\\label{dead}\n", Some(&r));
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("dead"));
    }
}
