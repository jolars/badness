//! `obsolete-environment`: math environments the LaTeX community has superseded,
//! reported with their modern replacement.
//!
//! The canonical case is `eqnarray`, which `amsmath` replaced with `align`
//! decades ago (it mis-spaces relations and is a perennial l2tabu/chktex
//! warning). As with [`deprecated_command`](super::deprecated_command), the
//! replacement is a near-mechanical swap, so the message names the modern form —
//! the seed of a later autofix (this slice reports only; see Tenet 5).
//!
//! The table lives here, not in `data/signatures.json`: "this environment is
//! obsolete" is a lint judgment, not the structural arity/math fact the signature
//! DB carries (AGENTS.md core decision #2).

use std::path::PathBuf;

use crate::ast::environment_name;
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use crate::linter::diagnostic::{Diagnostic, Severity};

use super::{Rule, RuleContext};

/// Obsolete environment name → its modern replacement.
const OBSOLETE: &[(&str, &str)] = &[("eqnarray", "align"), ("eqnarray*", "align*")];

pub struct ObsoleteEnvironment;

impl Rule for ObsoleteEnvironment {
    fn id(&self) -> &'static str {
        "obsolete-environment"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::ENVIRONMENT]
    }

    fn check(&self, el: &SyntaxElement, _ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(env) = el.as_node() else {
            return;
        };
        let Some(begin) = env.children().find(|c| c.kind() == SyntaxKind::BEGIN) else {
            return;
        };
        let Some(name) = environment_name(&begin) else {
            return;
        };
        let Some((_, replacement)) = OBSOLETE.iter().find(|(obs, _)| *obs == name) else {
            return;
        };
        // Underline the name inside `\begin{…}`, not the whole environment.
        let range = name_group_range(&begin).unwrap_or_else(|| begin.text_range());
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start: usize::from(range.start()),
            end: usize::from(range.end()),
            message: format!("`{name}` is obsolete; use `{replacement}`"),
            fix: None,
        });
    }
}

/// The range of a `BEGIN`/`END` node's `NAME_GROUP` child (the `{name}`).
fn name_group_range(begin: &SyntaxNode) -> Option<rowan::TextRange> {
    begin
        .children()
        .find(|c| c.kind() == SyntaxKind::NAME_GROUP)
        .map(|g| g.text_range())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::semantic::SemanticModel;

    fn findings(src: &str) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse(src).green);
        let model = SemanticModel::build(&root);
        let ctx = RuleContext {
            path: std::path::Path::new("x.tex"),
            root: &root,
            model: &model,
            resolution: None,
            citations: None,
        };
        let mut out = Vec::new();
        for el in root.descendants_with_tokens() {
            if ObsoleteEnvironment.interests().contains(&el.kind()) {
                ObsoleteEnvironment.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_eqnarray() {
        let out = findings("\\begin{eqnarray}\na &=& b\n\\end{eqnarray}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "obsolete-environment");
        assert!(out[0].message.contains("align"), "got: {}", out[0].message);
        // Caret covers `{eqnarray}` of the `\begin`, not the whole environment.
        assert_eq!((out[0].start, out[0].end), (6, 16));
    }

    #[test]
    fn flags_starred_variant() {
        let out = findings("\\begin{eqnarray*}\na\n\\end{eqnarray*}\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("align*"));
    }

    #[test]
    fn align_is_fine() {
        assert!(findings("\\begin{align}\na &= b\n\\end{align}\n").is_empty());
    }
}
