//! `unreferenced-label`: a label definition unused by a reference command
//! or a recognized equation group or range in its document's label namespace.
//!
//! The mirror image of [`undefined-ref`](super::undefined_ref): that rule flags a
//! *reference* with no definition, this one flags a *definition* with no
//! reference. Both consult the cross-file [`ResolvedLabels`] and share the exact
//! same soundness gate, because both are only trustworthy over a **complete**
//! namespace:
//!
//! - **closed** — every include resolves to an analyzed member, so no opaque file
//!   could hold the missing `\ref` (a label referenced only from an
//!   un-analyzed `\input` would otherwise be a false positive).
//! - **rooted** — the namespace contains a document root. A bare chapter fragment
//!   opened on its own, whose labels are referenced from the main document, is
//!   never flagged.
//!
//! Inert when no [`ResolvedLabels`] is available (stdin, or the language server
//! today). Report-only: two resolutions are always valid (delete the dead
//! definition, or add the missing `\ref`), so there is no single correct-by-
//! construction rewrite — no autofix (see [`crate::linter`] tenet 1).
//! `Severity::Warning` keeps a stray package-defined reference target
//! conservative.
//!
//! [`ResolvedLabels`]: crate::project::ResolvedLabels

use std::collections::HashSet;
use std::path::PathBuf;

use smol_str::SmolStr;

use crate::ast::{command_name, environment_name};
use crate::linter::diagnostic::{Diagnostic, Severity};
use crate::semantic::{LabelDef, RefCommand};
use crate::syntax::{SyntaxKind, SyntaxNode};

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
        "Flag a label definition unused by a `\\ref`-family command anywhere in the \
         document. A `\\eqref{A}--\\eqref{D}` range also uses labels between A and D \
         when they occur in consecutive `equation` environments in the same \
         file without manual numbering changes. Referencing a `subequations` \
         group label also uses the labels in its enclosed math environments. \
         Other equation layouts keep their warnings. The mirror of `undefined-ref`, \
         and sound only when the label \
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

        let ranged = ranged_equation_labels(ctx);
        let grouped = referenced_subequation_labels(ctx);

        sink.extend(
            ctx.model
                .labels()
                .iter()
                .filter(|label| {
                    !resolution.is_referenced(ctx.path, &label.name)
                        && !ranged.contains(&label.name)
                        && !grouped.contains(&usize::from(label.range.start()))
                })
                .map(|label| Diagnostic {
                    rule: self.id(),
                    severity: self.default_severity(),
                    path: PathBuf::new(),
                    start: usize::from(label.range.start()),
                    end: usize::from(label.range.end()),
                    message: format!("label `{}` is never referenced", label.name),
                    fix: None,
                    related: Vec::new(),
                }),
        );
    }
}

/// A reference to a `subequations` group names the numbered equations inside
/// it. Only a label outside the nested environments can identify that group.
fn referenced_subequation_labels(ctx: &RuleContext<'_>) -> HashSet<usize> {
    let Some(resolution) = ctx.resolution else {
        return HashSet::new();
    };
    let mut grouped = HashSet::new();
    for group in ctx.root.descendants().filter(|node| {
        node.kind() == SyntaxKind::ENVIRONMENT
            && node
                .children()
                .find(|child| child.kind() == SyntaxKind::BEGIN)
                .and_then(|begin| environment_name(&begin))
                .as_deref()
                == Some("subequations")
    }) {
        if !group.children().any(|child| {
            child.kind() == SyntaxKind::END
                && environment_name(&child).as_deref() == Some("subequations")
        }) {
            continue;
        }
        let nested: Vec<_> = group
            .descendants()
            .filter(|node| node.kind() == SyntaxKind::ENVIRONMENT && *node != group)
            .collect();
        let parent_referenced = ctx.model.labels().iter().any(|label| {
            group.text_range().contains_range(label.range)
                && !nested
                    .iter()
                    .any(|environment| environment.text_range().contains_range(label.range))
                && resolution.is_referenced(ctx.path, &label.name)
                && resolution.definers(ctx.path, &label.name) == [ctx.path]
                && ctx
                    .model
                    .labels()
                    .iter()
                    .filter(|other| other.name == label.name)
                    .count()
                    == 1
        });
        if !parent_referenced {
            continue;
        }
        for environment in nested {
            let Some(math) = environment
                .children()
                .find(|child| child.kind() == SyntaxKind::MATH)
            else {
                continue;
            };
            for label in ctx
                .model
                .labels()
                .iter()
                .filter(|label| math.text_range().contains_range(label.range))
            {
                grouped.insert(usize::from(label.range.start()));
            }
        }
    }
    grouped
}

struct Equation<'a> {
    node: SyntaxNode,
    label: Option<&'a LabelDef>,
}

/// A textual range is evidence for the labels inside it only when its endpoints
/// bound consecutive, plainly numbered equations in this file. This does not
/// alter the reference model: navigation still points to the two explicit keys.
fn ranged_equation_labels(ctx: &RuleContext<'_>) -> HashSet<SmolStr> {
    let Some(resolution) = ctx.resolution else {
        return HashSet::new();
    };
    let mut refs: Vec<_> = ctx
        .model
        .refs()
        .iter()
        .filter(|reference| reference.command == RefCommand::EqRef)
        .collect();
    if refs.len() < 2 {
        return HashSet::new();
    }
    refs.sort_by_key(|reference| reference.range.start());
    let source = ctx.root.text().to_string();
    let equations: Vec<_> = ctx
        .root
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::ENVIRONMENT)
        .filter(|node| {
            node.children()
                .find(|child| child.kind() == SyntaxKind::BEGIN)
                .and_then(|begin| environment_name(&begin))
                .as_deref()
                == Some("equation")
        })
        .map(|node| {
            let labels: Vec<_> = ctx
                .model
                .labels()
                .iter()
                .filter(|label| node.text_range().contains_range(label.range))
                .collect();
            let label = (labels.len() == 1
                && ctx
                    .model
                    .labels()
                    .iter()
                    .filter(|other| other.name == labels[0].name)
                    .count()
                    == 1
                && node.children().any(|child| {
                    child.kind() == SyntaxKind::END
                        && environment_name(&child).as_deref() == Some("equation")
                })
                && !node.descendants().any(|child| {
                    child.kind() == SyntaxKind::COMMAND
                        && command_name(&child)
                            .as_deref()
                            .is_some_and(changes_equation_number)
                }))
            .then_some(labels.first().copied())
            .flatten();
            Equation { node, label }
        })
        .collect();

    let mut ranged = HashSet::new();
    for pair in refs.windows(2) {
        let [first, last] = pair else { continue };
        let gap = usize::from(first.range.end())..usize::from(last.range.start());
        if source.get(gap).is_none_or(|gap| gap.trim() != "--") {
            continue;
        }
        let Some(start) = equations.iter().position(|equation| {
            equation.label.is_some_and(|label| label.name == first.name)
                && resolution.definers(ctx.path, &first.name) == [ctx.path]
        }) else {
            continue;
        };
        let Some(end) = equations.iter().position(|equation| {
            equation.label.is_some_and(|label| label.name == last.name)
                && resolution.definers(ctx.path, &last.name) == [ctx.path]
        }) else {
            continue;
        };
        if end <= start + 1 || !equations[start..=end].iter().all(|eq| eq.label.is_some()) {
            continue;
        }
        if !equations[start..=end].windows(2).all(|pair| {
            let gap = usize::from(pair[0].node.text_range().end())
                ..usize::from(pair[1].node.text_range().start());
            source.get(gap).is_some_and(|gap| gap.trim().is_empty())
        }) {
            continue;
        }
        for equation in &equations[start + 1..end] {
            let label = equation.label.expect("checked above");
            if resolution.definers(ctx.path, &label.name) == [ctx.path] {
                ranged.insert(label.name.clone());
            }
        }
    }
    ranged
}

fn changes_equation_number(name: &str) -> bool {
    matches!(
        name,
        "tag"
            | "notag"
            | "nonumber"
            | "setcounter"
            | "addtocounter"
            | "stepcounter"
            | "refstepcounter"
            | "counterwithin"
            | "numberwithin"
    )
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
