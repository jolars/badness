//! The lint driver: run every rule over one file, drop suppressed findings,
//! stamp paths, and sort. The single entry point both `badness lint` (CLI) and
//! the language server call — the analog of arity's `check_document`.

use std::path::Path;

use crate::semantic::SemanticModel;
use crate::syntax::SyntaxNode;

use super::diagnostic::Diagnostic;
use super::rules::{RuleContext, all_rules};
use super::suppression::SuppressionMap;

/// Run all built-in rules against `root`/`model`, returning the surviving
/// diagnostics (suppressed ones removed, `path` stamped, sorted by position).
///
/// `root` and `model` must describe the same file as `path`. Callers supply
/// them from wherever is cheapest: the CLI parses directly, the LSP reuses its
/// salsa-cached tree and model.
pub fn lint_document(path: &Path, root: &SyntaxNode, model: &SemanticModel) -> Vec<Diagnostic> {
    let ctx = RuleContext { path, root, model };
    let mut diagnostics: Vec<Diagnostic> =
        all_rules().iter().flat_map(|rule| rule.run(&ctx)).collect();

    let suppress = SuppressionMap::build(root);
    diagnostics.retain(|d| !suppress.is_suppressed(d.rule, d.start, d.end));

    for d in &mut diagnostics {
        d.path = path.to_path_buf();
    }
    diagnostics.sort_by_key(|d| (d.start, d.end, d.rule));
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn lint(src: &str) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse(src).green);
        let model = SemanticModel::build(&root);
        lint_document(Path::new("x.tex"), &root, &model)
    }

    fn rules_of(src: &str) -> Vec<&'static str> {
        lint(src).iter().map(|d| d.rule).collect()
    }

    #[test]
    fn collects_both_rule_families() {
        // A duplicate label and a deprecated switch, sorted by position.
        let rules = rules_of("\\label{a}\\label{a}\n{\\bf x}\n");
        assert_eq!(rules, vec!["duplicate-label", "deprecated-command"]);
    }

    #[test]
    fn node_directive_suppresses_following_command() {
        let out = lint("% badness-ignore deprecated-command: legacy\n{\\bf x}\n");
        assert!(out.is_empty(), "expected suppression, got: {out:?}");
    }

    #[test]
    fn node_directive_only_targets_named_rule() {
        // The directive names a different rule, so the `\bf` is still reported.
        let out = lint("% badness-ignore duplicate-label: nope\n{\\bf x}\n");
        assert_eq!(rules_of_diags(&out), vec!["deprecated-command"]);
    }

    #[test]
    fn node_directive_does_not_leak_past_first_target() {
        // Only the first block is suppressed; a later `\it` still fires.
        let out = lint("% badness-ignore deprecated-command: legacy\n{\\bf x}\n\n{\\it y}\n");
        assert_eq!(rules_of_diags(&out), vec!["deprecated-command"]);
    }

    #[test]
    fn file_directive_suppresses_all_occurrences() {
        let out = lint("% badness-ignore-file deprecated-command: legacy\n{\\bf x}\n{\\it y}\n");
        assert!(
            out.is_empty(),
            "expected file-wide suppression, got: {out:?}"
        );
    }

    fn rules_of_diags(diags: &[Diagnostic]) -> Vec<&'static str> {
        diags.iter().map(|d| d.rule).collect()
    }
}
