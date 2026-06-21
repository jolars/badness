//! The lint driver: run every rule over one file, drop suppressed findings,
//! stamp paths, and sort. The single entry point both `badness lint` (CLI) and
//! the language server call — the analog of arity's `check_document`.

use std::path::Path;

use crate::parser::{LexConfig, parse_with_flavor};
use crate::project::{ResolvedCitations, ResolvedLabels};
use crate::semantic::SemanticModel;
use crate::syntax::{SyntaxKind, SyntaxNode};

use super::diagnostic::Diagnostic;
use super::rules::{RuleContext, all_rules};
use super::suppression::SuppressionMap;

/// Parse and lint a single file's `text` from scratch, returning its parse
/// diagnostics plus rule findings. The self-contained analog of
/// [`lint_document`] for callers that hold only text — notably the `lint --fix`
/// fixpoint loop, which re-parses after each round. Cross-file rules run with no
/// project view (`resolution: None`); none of them produce fixes, so the fix
/// path loses nothing. Mirrors arity's `check_document`. `config` fixes the
/// lexer's initial catcode regime so a `.sty`/`.cls` parses under the implicit
/// `\makeatletter` ([`Package`](crate::parser::LatexFlavor::Package)) and a
/// `.dtx` runs the docstrip mode; a bare
/// [`LatexFlavor`](crate::parser::LatexFlavor) coerces in.
pub fn check_document(path: &Path, text: &str, config: impl Into<LexConfig>) -> Vec<Diagnostic> {
    let parsed = parse_with_flavor(text, config);
    let mut diagnostics: Vec<Diagnostic> = parsed
        .errors
        .iter()
        .map(|err| Diagnostic::from_parse(path.to_path_buf(), err))
        .collect();
    let root = SyntaxNode::new_root(parsed.green);
    let model = SemanticModel::build(&root);
    diagnostics.extend(lint_document(path, &root, &model, None, None));
    diagnostics
}

/// Run all built-in rules against `root`/`model`, returning the surviving
/// diagnostics (suppressed ones removed, `path` stamped, sorted by position).
///
/// `root` and `model` must describe the same file as `path`. Callers supply
/// them from wherever is cheapest: the CLI parses directly, the LSP reuses its
/// salsa-cached tree and model. `resolution` is the cross-file label model and
/// `citations` the cross-file bibliography model for the project `path` belongs
/// to, each `None` when there is no project view — the cross-file rules
/// (`undefined-ref`, `undefined-citation`, the cross-file branch of
/// `duplicate-label`) are then inert.
pub fn lint_document(
    path: &Path,
    root: &SyntaxNode,
    model: &SemanticModel,
    resolution: Option<&ResolvedLabels>,
    citations: Option<&ResolvedCitations>,
) -> Vec<Diagnostic> {
    let ctx = RuleContext {
        path,
        root,
        model,
        resolution,
        citations,
    };
    let rules = all_rules();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    // Build the node-dispatch table: kind discriminant -> indices of subscribed
    // rules. `SyntaxKind` is a contiguous `#[repr(u16)]`, so a flat Vec indexed by
    // `kind as usize` beats a hash map.
    let mut by_kind: Vec<Vec<usize>> = vec![Vec::new(); SyntaxKind::COUNT];
    let mut any_node_rules = false;
    for (i, rule) in rules.iter().enumerate() {
        for kind in rule.interests() {
            by_kind[*kind as usize].push(i);
            any_node_rules = true;
        }
    }

    // Single shared traversal feeding every node-shape rule. Visits tokens too
    // (`descendants_with_tokens`) so token-level rules can subscribe to e.g.
    // `COMMENT` or `WORD`.
    if any_node_rules {
        for el in root.descendants_with_tokens() {
            for &i in &by_kind[el.kind() as usize] {
                rules[i].check(&el, &ctx, &mut diagnostics);
            }
        }
    }

    // Whole-file pass for model-/resolution-driven rules.
    for rule in &rules {
        rule.check_file(&ctx, &mut diagnostics);
    }

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
        lint_document(Path::new("x.tex"), &root, &model, None, None)
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
