//! End-to-end tests for the lint driver (`linter::lint_document`): the public
//! entry both the CLI and the language server call. Exercises rule collection,
//! cross-rule ordering, and `% badness-ignore` suppression over realistic
//! multi-line documents — complementing the focused per-rule unit tests in
//! `src/linter/`.

use std::path::{Path, PathBuf};

use badness::linter::{Severity, lint_document};
use badness::parser::parse;
use badness::project::labels::{document_label_names, is_document_root};
use badness::project::{FileFacts, IncludeGraph, ResolvedLabels, collect_include_edge_keys};
use badness::semantic::SemanticModel;
use badness::syntax::SyntaxNode;

/// Lint `src` through the public driver, as the CLI does.
fn lint(src: &str) -> Vec<(&'static str, Severity)> {
    let root = SyntaxNode::new_root(parse(src).green);
    let model = SemanticModel::build(&root);
    lint_document(Path::new("doc.tex"), &root, &model, None)
        .into_iter()
        .map(|d| (d.rule, d.severity))
        .collect()
}

/// Lint a whole `(path, source)` project through the driver exactly as the CLI's
/// `run_lint` does: build every model first, resolve labels across the include
/// graph, then lint each file with the shared resolution. Returns
/// `(path, rule, message)` for every finding.
fn lint_project(files: &[(&str, &str)]) -> Vec<(String, &'static str, String)> {
    let parsed: Vec<(PathBuf, SyntaxNode, SemanticModel)> = files
        .iter()
        .map(|(path, src)| {
            let root = SyntaxNode::new_root(parse(src).green);
            let model = SemanticModel::build(&root);
            (PathBuf::from(path), root, model)
        })
        .collect();

    let facts: Vec<FileFacts> = parsed
        .iter()
        .map(|(path, root, _)| FileFacts {
            path: path.clone(),
            include_edges: collect_include_edge_keys(root, path.parent()),
        })
        .collect();
    let label_inputs: Vec<_> = parsed
        .iter()
        .map(|(path, root, model)| {
            (
                path.clone(),
                document_label_names(model),
                is_document_root(root),
            )
        })
        .collect();
    let resolved = ResolvedLabels::build(&label_inputs, &IncludeGraph::build(&facts, None));

    let mut out = Vec::new();
    for (path, root, model) in &parsed {
        for d in lint_document(path, root, model, Some(&resolved)) {
            out.push((path.display().to_string(), d.rule, d.message));
        }
    }
    out
}

fn rules_only(findings: &[(String, &'static str, String)]) -> Vec<&'static str> {
    findings.iter().map(|(_, rule, _)| *rule).collect()
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

#[test]
fn stylistic_rules_collected_in_document_order() {
    // An obsolete environment, a `$$` display, and a reversed `\left`/`\right`
    // pair — all surface, sorted by position.
    let src = "\
\\begin{eqnarray}a&=&b\\end{eqnarray}
$$x = y$$
$\\left) a \\right| $
";
    assert_eq!(
        lint(src),
        vec![
            ("obsolete-environment", Severity::Warning),
            ("dollar-display-math", Severity::Warning),
            ("mismatched-delimiter", Severity::Warning),
        ]
    );
}

#[test]
fn modern_constructs_have_no_findings() {
    let src = "\
\\begin{align}a &= b\\end{align}
\\[x = y\\]
$\\left( a \\right] $
";
    assert!(lint(src).is_empty(), "got: {:?}", lint(src));
}

#[test]
fn node_ignore_silences_a_stylistic_rule() {
    let src = "\
% badness-ignore dollar-display-math: legacy snippet
$$x = y$$
";
    assert!(lint(src).is_empty(), "got: {:?}", lint(src));
}

// --- Cross-file lints (driver + resolver) -------------------------------------

#[test]
fn well_formed_project_has_no_cross_file_findings() {
    // main declares the document and references a label defined in the chapter
    // it `\input`s — everything resolves, nothing fires.
    let findings = lint_project(&[
        (
            "main.tex",
            "\\documentclass{article}\n\\input{chap}\n\\ref{a}\n",
        ),
        ("chap.tex", "\\label{a}\n"),
    ]);
    assert!(
        findings.is_empty(),
        "expected clean project, got: {findings:?}"
    );
}

#[test]
fn cross_file_duplicate_label_is_reported_in_both_files() {
    // The same key defined in two files of one document is a cross-file dupe;
    // each file's definition is flagged, naming the other.
    let findings = lint_project(&[
        (
            "main.tex",
            "\\documentclass{article}\n\\input{chap}\n\\label{dup}\n",
        ),
        ("chap.tex", "\\label{dup}\n"),
    ]);
    assert_eq!(
        rules_only(&findings),
        vec!["duplicate-label", "duplicate-label"]
    );
    assert!(
        findings
            .iter()
            .any(|(p, _, m)| p == "main.tex" && m.contains("`chap.tex`"))
    );
    assert!(
        findings
            .iter()
            .any(|(p, _, m)| p == "chap.tex" && m.contains("`main.tex`"))
    );
}

#[test]
fn undefined_ref_fires_in_a_closed_rooted_document() {
    let findings = lint_project(&[(
        "main.tex",
        "\\documentclass{article}\n\\label{a}\\ref{a}\\ref{ghost}\n",
    )]);
    assert_eq!(rules_only(&findings), vec!["undefined-ref"]);
    assert!(findings[0].2.contains("ghost"));
}

#[test]
fn undefined_ref_is_silent_for_a_bare_fragment() {
    // No `\documentclass`: the label may live in an unanalyzed main document, so
    // the ref is not flagged.
    let findings = lint_project(&[("chap.tex", "\\ref{elsewhere}\n")]);
    assert!(findings.is_empty(), "expected silence, got: {findings:?}");
}

#[test]
fn independent_documents_do_not_cross_contaminate() {
    // Two standalone documents, each defining `\label{intro}`: separate include
    // components, so neither is a cross-file duplicate and each ref resolves
    // within its own document.
    let findings = lint_project(&[
        (
            "one.tex",
            "\\documentclass{article}\n\\label{intro}\\ref{intro}\n",
        ),
        (
            "two.tex",
            "\\documentclass{article}\n\\label{intro}\\ref{intro}\n",
        ),
    ]);
    assert!(
        findings.is_empty(),
        "expected no collisions, got: {findings:?}"
    );
}
