//! End-to-end tests for the lint driver (`linter::lint_document`): the public
//! entry both the CLI and the language server call. Exercises rule collection,
//! cross-rule ordering, and `% badness-ignore` suppression over realistic
//! multi-line documents — complementing the focused per-rule unit tests in
//! `src/linter/`.

use std::path::Path;

use badness::linter::{Severity, lint_document};
use badness::parser::parse;
use badness::semantic::SemanticModel;
use badness::syntax::SyntaxNode;

/// Lint `src` through the public driver, as the CLI does.
fn lint(src: &str) -> Vec<(&'static str, Severity)> {
    let root = SyntaxNode::new_root(parse(src).green);
    let model = SemanticModel::build(&root);
    lint_document(Path::new("doc.tex"), &root, &model)
        .into_iter()
        .map(|d| (d.rule, d.severity))
        .collect()
}

#[test]
fn reports_both_rules_in_document_order() {
    let src = "\\section{Intro}\n\\label{a}\n{\\bf bold}\n\\label{a}\n";
    assert_eq!(
        lint(src),
        vec![
            ("deprecated-command", Severity::Warning),
            ("duplicate-label", Severity::Warning),
        ]
    );
}

#[test]
fn clean_document_has_no_findings() {
    let src = "\\section{Intro}\n\\label{a}\\ref{a}\n\\textbf{ok}\n";
    assert!(lint(src).is_empty());
}

#[test]
fn node_ignore_suppresses_only_the_next_block() {
    let src = "\
% badness-ignore deprecated-command: legacy macro
{\\bf one}

{\\it two}
";
    // The first switch is suppressed; the second still fires.
    assert_eq!(lint(src), vec![("deprecated-command", Severity::Warning)]);
}

#[test]
fn file_ignore_silences_a_rule_everywhere() {
    let src = "\
% badness-ignore-file deprecated-command: legacy file
{\\bf one}
{\\it two}
\\label{a}\\label{a}
";
    // Every deprecated switch is gone; the duplicate label still reports.
    assert_eq!(lint(src), vec![("duplicate-label", Severity::Warning)]);
}

#[test]
fn file_ignore_all_silences_everything() {
    let src = "\
% badness-ignore-file: vendored
{\\bf one}
\\label{a}\\label{a}
";
    assert!(lint(src).is_empty());
}
