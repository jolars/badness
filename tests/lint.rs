//! End-to-end tests for the lint driver (`linter::lint_document`): the public
//! entry both the CLI and the language server call. Exercises rule collection,
//! cross-rule ordering, and `% badness-ignore` suppression over realistic
//! multi-line documents — complementing the focused per-rule unit tests in
//! `src/linter/`.

use std::path::{Path, PathBuf};

use badness::linter::{Severity, lint_document};
use badness::parser::{parse, reconstruct};
use badness::project::labels::{document_label_names, is_document_root};
use badness::project::{FileFacts, IncludeGraph, ResolvedLabels, collect_include_edge_keys};
use badness::semantic::SemanticModel;
use badness::syntax::SyntaxNode;

/// Lint `src` through the public driver, as the CLI does.
fn lint(src: &str) -> Vec<(&'static str, Severity)> {
    let root = SyntaxNode::new_root(parse(src).green);
    let model = SemanticModel::build(&root);
    lint_document(Path::new("doc.tex"), &root, &model, None, None)
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
        for d in lint_document(path, root, model, Some(&resolved), None) {
            out.push((path.display().to_string(), d.rule, d.message));
        }
    }
    out
}

fn rules_only(findings: &[(String, &'static str, String)]) -> Vec<&'static str> {
    findings.iter().map(|(_, rule, _)| *rule).collect()
}

/// Lint a `.tex` source against a set of `(bib_path, bib_source)` bibliographies,
/// exactly as the CLI's `run_lint` assembles cross-file citation resolution.
/// Returns the rule ids of every finding for the `.tex` file (`doc.tex`).
fn lint_with_bib(tex: &str, bibs: &[(&str, &str)]) -> Vec<&'static str> {
    use badness::project::{CiteFileFacts, ResolvedCitations, collect_bib_resource_targets};
    use smol_str::SmolStr;
    use std::collections::HashMap;

    let tex_path = PathBuf::from("doc.tex");
    let root = SyntaxNode::new_root(parse(tex).green);
    let model = SemanticModel::build(&root);

    let bib_keys: HashMap<PathBuf, Vec<SmolStr>> = bibs
        .iter()
        .map(|(path, src)| {
            let bib_model =
                badness::bib::semantic::Model::build(&badness::bib::parse(src).syntax());
            (
                PathBuf::from(path),
                bib_model.entries().iter().map(|e| e.key.clone()).collect(),
            )
        })
        .collect();

    let facts = vec![FileFacts {
        path: tex_path.clone(),
        include_edges: collect_include_edge_keys(&root, tex_path.parent()),
    }];
    let graph = IncludeGraph::build(&facts, None);
    let cite_facts = vec![CiteFileFacts {
        path: tex_path.clone(),
        bib_targets: collect_bib_resource_targets(&root, tex_path.parent()),
        nocite_all: model.has_wildcard_nocite(),
        is_document_root: is_document_root(&root),
    }];
    let citations = ResolvedCitations::build(&cite_facts, &graph, &bib_keys);

    lint_document(&tex_path, &root, &model, None, Some(&citations))
        .into_iter()
        .map(|d| d.rule)
        .collect()
}

#[test]
fn cross_file_undefined_citation_is_flagged() {
    let tex = "\\documentclass{article}\n\\addbibresource{refs.bib}\n\\begin{document}\n\\cite{missing}\n\\end{document}\n";
    let bib = "@article{present, title = {T}}\n";
    let rules = lint_with_bib(tex, &[("refs.bib", bib)]);
    assert!(rules.contains(&"undefined-citation"), "{rules:?}");
}

#[test]
fn cross_file_resolved_citation_is_silent() {
    let tex = "\\documentclass{article}\n\\addbibresource{refs.bib}\n\\begin{document}\n\\cite{present}\n\\end{document}\n";
    let bib = "@article{present, title = {T}}\n";
    let rules = lint_with_bib(tex, &[("refs.bib", bib)]);
    assert!(!rules.contains(&"undefined-citation"), "{rules:?}");
}

#[test]
fn citation_gating_holds_for_fragment_and_wildcard() {
    let bib = "@article{present, title = {T}}\n";
    // No \documentclass → rootless fragment → not flagged even if the key is absent.
    let fragment = "\\addbibresource{refs.bib}\n\\cite{missing}\n";
    assert!(!lint_with_bib(fragment, &[("refs.bib", bib)]).contains(&"undefined-citation"));

    // \nocite{*} pulls in every entry → nothing is undefined.
    let wildcard = "\\documentclass{article}\n\\addbibresource{refs.bib}\n\\nocite{*}\n\\begin{document}\n\\cite{missing}\n\\end{document}\n";
    assert!(!lint_with_bib(wildcard, &[("refs.bib", bib)]).contains(&"undefined-citation"));
}

#[test]
fn bibliography_command_resolves_keys() {
    // The legacy `\bibliography{refs}` form (default `.bib`) resolves too.
    let tex = "\\documentclass{article}\n\\begin{document}\n\\cite{present}\n\\bibliography{refs}\n\\end{document}\n";
    let bib = "@article{present, title = {T}}\n";
    let rules = lint_with_bib(tex, &[("refs.bib", bib)]);
    assert!(!rules.contains(&"undefined-citation"), "{rules:?}");
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
fn hard_coded_reference_fires_end_to_end() {
    // A hard-coded `Figure 3` and a tied `Table~1` both surface; the genuine
    // `\ref` and a spelled-out number stay silent. Report-only: no fix.
    let src = "See Figure 3 and Table~1, but Section~\\ref{s} and Figure three are fine.\n";
    assert_eq!(
        lint(src),
        vec![
            ("hard-coded-reference", Severity::Warning),
            ("hard-coded-reference", Severity::Warning),
        ]
    );
}

#[test]
fn node_ignore_silences_a_stylistic_rule() {
    let src = "\
% badness-ignore dollar-display-math: legacy snippet
$$x = y$$
";
    assert!(lint(src).is_empty(), "got: {:?}", lint(src));
}

#[test]
fn dash_length_fires_end_to_end_and_its_fix_is_correct() {
    // A hyphenated number range trips the rule; the compound `well-known` and the
    // ISO date do not.
    let src = "See pages 5-10 of the well-known text dated 2020-01-15.\n";
    assert_eq!(lint(src), vec![("dash-length", Severity::Warning)]);
    // The unsafe en-dash fix stays lossless and parses (tenet 1).
    assert_fix_is_correct(src);
    assert_eq!(
        fix_to_fixpoint(src),
        "See pages 5--10 of the well-known text dated 2020-01-15.\n"
    );
}

#[test]
fn abbreviation_spacing_fires_end_to_end_and_its_fix_is_correct() {
    // The lowercase abbreviation `e.g.` (before a lowercase word) and the acronym
    // `USA.` (ending a sentence, before a capital) both trip the rule; the trailing
    // `home.` does not.
    let src = "see e.g. foo and the USA. Then go home.\n";
    assert_eq!(
        lint(src),
        vec![
            ("abbreviation-spacing", Severity::Warning),
            ("abbreviation-spacing", Severity::Warning),
        ]
    );
    // The unsafe spacing fixes stay lossless and parse (tenet 1).
    assert_fix_is_correct(src);
    assert_eq!(
        fix_to_fixpoint(src),
        "see e.g.\\ foo and the USA\\@. Then go home.\n"
    );
}

#[test]
fn space_before_command_fires_end_to_end_and_its_fix_is_correct() {
    // A space before `\footnote` and before `\label` trip the rule; the tight
    // `\emph` does not.
    let src = "See \\emph{this} word \\footnote{n} and here \\label{s}.\n";
    assert_eq!(
        lint(src),
        vec![
            ("space-before-command", Severity::Warning),
            ("space-before-command", Severity::Warning),
        ]
    );
    // The unsafe delete fix stays lossless and parses (tenet 1).
    assert_fix_is_correct(src);
    assert_eq!(
        fix_to_fixpoint(src),
        "See \\emph{this} word\\footnote{n} and here\\label{s}.\n"
    );
}

#[test]
fn times_variable_fires_end_to_end_and_its_fix_is_correct() {
    // A `digits x digits` product trips the rule; `matrix` and the hex mask do not.
    let src = "A 640x200 matrix with mask 0xFF.\n";
    assert_eq!(lint(src), vec![("times-variable", Severity::Warning)]);
    // The unsafe fix wraps the cross in inline math; it stays lossless and parses.
    assert_fix_is_correct(src);
    assert_eq!(
        fix_to_fixpoint(src),
        "A 640$\\times$200 matrix with mask 0xFF.\n"
    );
}

#[test]
fn math_operator_name_fires_end_to_end_and_its_fix_is_correct() {
    // Bare `sin`/`cos` in math trip the rule; `\tan` (already a command) and the
    // subscript label `x_{max}` do not.
    let src = "$sin x + \\tan y$ with $x_{max}$ and bare $cos z$.\n";
    assert_eq!(
        lint(src),
        vec![
            ("math-operator-name", Severity::Warning),
            ("math-operator-name", Severity::Warning),
        ]
    );
    // The unsafe fix inserts the backslash; it stays lossless and parses.
    assert_fix_is_correct(src);
    assert_eq!(
        fix_to_fixpoint(src),
        "$\\sin x + \\tan y$ with $x_{max}$ and bare $\\cos z$.\n"
    );
}

#[test]
fn primitive_command_reports_and_swaps_end_to_end() {
    // `\over` restructures its operands, so it is report-only (no fix); the
    // plain-TeX subscript alias `\sb` carries a safe 1:1 swap to `_`.
    let src = "$a \\over b$ and $x\\sb2$.\n";
    assert_eq!(
        lint(src),
        vec![
            ("primitive-command", Severity::Warning),
            ("primitive-command", Severity::Warning),
        ]
    );
    // Only the `\sb` swap fires as a safe fix; `\over` is left untouched.
    assert_fix_is_correct(src);
    assert_eq!(fix_to_fixpoint(src), "$a \\over b$ and $x_2$.\n");
}

#[test]
fn swallowed_space_fires_end_to_end_and_its_fix_is_correct() {
    // `\LaTeX is` glues to "LaTeXis"; the already-braced `\TeX{}` does not fire.
    let src = "We use \\LaTeX is nice and \\TeX{} too.\n";
    assert_eq!(lint(src), vec![("swallowed-space", Severity::Warning)]);
    // The unsafe `{}` insertion stays lossless and parses, and clears the finding.
    assert_fix_is_correct(src);
    assert_eq!(
        fix_to_fixpoint(src),
        "We use \\LaTeX{} is nice and \\TeX{} too.\n"
    );
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

// ---------------------------------------------------------------------------
// Autofixes (`lint --fix`). The engine and the `dollar-display-math` swap.
// ---------------------------------------------------------------------------

use badness::formatter::{FormatStyle, format_with_style};
use badness::linter::{apply_fixes, check_document};
use badness::parser::LatexFlavor;

/// Apply every available fix (including unsafe) to `text` at a fixpoint, exactly
/// as the CLI's `fix_file` does, and return the rewritten text.
fn fix_to_fixpoint(text: &str) -> String {
    let path = Path::new("doc.tex");
    let mut content = text.to_owned();
    for _ in 0..10 {
        let fixes: Vec<_> = check_document(path, &content, LatexFlavor::Document)
            .into_iter()
            .filter_map(|d| d.fix)
            .collect();
        if fixes.is_empty() {
            break;
        }
        let out = apply_fixes(&content, &fixes, true);
        if out.applied == 0 {
            break;
        }
        content = out.output;
    }
    content
}

/// Tenet 1: a fix is a textual edit judged on correctness, not formatting.
/// Applying every fix to fixpoint must leave a tree that still parses cleanly
/// and is still lossless. A fix does *not* owe line-width or format-idempotence
/// (layout is the formatter's job; the pipeline is fix-then-format).
fn assert_fix_is_correct(input: &str) {
    let style = FormatStyle::default();
    let clean = format_with_style(input, style).expect("input should format");
    let fixed = fix_to_fixpoint(&clean);

    assert!(
        parse(&fixed).errors.is_empty(),
        "fixed output must parse cleanly:\n{fixed:?}"
    );
    assert_eq!(
        reconstruct(&fixed),
        fixed,
        "fix broke losslessness (tenet 1).\nfrom:\n{clean}\n--- after fixes ---\n{fixed}"
    );
}

#[test]
fn dollar_display_fix_rewrites_to_bracket_form() {
    assert_eq!(fix_to_fixpoint("$$x = y$$\n"), "\\[x = y\\]\n");
}

#[test]
fn dollar_display_fix_clears_the_finding() {
    // After the swap, re-linting the rewritten document is clean.
    let fixed = fix_to_fixpoint("$$a + b$$\n\n$$c$$\n");
    assert_eq!(fixed, "\\[a + b\\]\n\n\\[c\\]\n");
    let remaining: Vec<_> = check_document(Path::new("doc.tex"), &fixed, LatexFlavor::Document)
        .into_iter()
        .filter(|d| d.rule == "dollar-display-math")
        .collect();
    assert!(
        remaining.is_empty(),
        "expected a clean re-lint, got: {remaining:?}"
    );
}

#[test]
fn dollar_display_fix_is_correct() {
    for case in ["$$x = y$$\n", "$$\n  a + b\n$$\n", "\\[x = y\\]\n", "$x$\n"] {
        assert_fix_is_correct(case);
    }
}

#[test]
fn makeat_macro_flags_at_names_outside_regions_only() {
    // An `@`-in-name macro in the body splits into a control word + `@`-word and is
    // flagged; wrapping it in `\makeatletter`…`\makeatother` lexes it as one control
    // word, so it stays quiet.
    let body: Vec<_> = lint("\\my@command\n")
        .into_iter()
        .filter(|(rule, _)| *rule == "makeat-macro")
        .collect();
    assert_eq!(body.len(), 1);

    let in_region: Vec<_> = lint("\\makeatletter\\my@command\\makeatother\n")
        .into_iter()
        .filter(|(rule, _)| *rule == "makeat-macro")
        .collect();
    assert!(in_region.is_empty(), "in-region use must not flag");
}

#[test]
fn missing_nbsp_fix_is_correct() {
    // The tie fix is `Unsafe` (it alters line-breaking); `fix_to_fixpoint`
    // applies unsafe fixes, so this exercises parse-clean + losslessness on it.
    for case in ["Figure \\ref{x}\n", "see \\cite{a}\n", "Eq. \\eqref{z}\n"] {
        assert_fix_is_correct(case);
    }
}

#[test]
fn missing_nbsp_fix_clears_the_finding() {
    let fixed = fix_to_fixpoint("Figure \\ref{x}\n");
    assert_eq!(fixed, "Figure~\\ref{x}\n");
    let remaining: Vec<_> = check_document(Path::new("doc.tex"), &fixed, LatexFlavor::Document)
        .into_iter()
        .filter(|d| d.rule == "missing-nonbreaking-space")
        .collect();
    assert!(
        remaining.is_empty(),
        "expected a clean re-lint, got: {remaining:?}"
    );
}

#[test]
fn missing_nbsp_skipped_without_unsafe_opt_in() {
    // The CLI's plain `--fix` (no `--unsafe-fixes`) must not insert the tie.
    let src = "Figure \\ref{x}\n";
    let fixes: Vec<_> = check_document(Path::new("doc.tex"), src, LatexFlavor::Document)
        .into_iter()
        .filter_map(|d| d.fix)
        .collect();
    let out = apply_fixes(src, &fixes, false);
    assert_eq!(out.output, src, "unsafe tie fix must be skipped");
}

#[test]
fn ellipsis_flags_text_and_math() {
    let out = lint("An ellipsis... and $a + ... + b$.\n");
    let hits: Vec<_> = out.iter().filter(|(r, _)| *r == "ellipsis").collect();
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|(_, sev)| *sev == Severity::Warning));
}

#[test]
fn ellipsis_text_fix_rewrites_to_dots() {
    // The text fix is Safe, so plain `--fix` (unsafe = false) applies it.
    assert_eq!(fix_to_fixpoint("done...\n"), "done\\dots\n");
}

#[test]
fn ellipsis_fix_is_correct() {
    for case in [
        "foo...bar\n",
        "one, two, ...\n",
        "$a + ... + b$\n",
        "$a_1,...,a_n$\n",
    ] {
        assert_fix_is_correct(case);
    }
}

#[test]
fn straight_quotes_flags_open_and_close() {
    let out = lint("He said \"hello\" today.\n");
    let hits: Vec<_> = out
        .iter()
        .filter(|(r, _)| *r == "straight-quotes")
        .collect();
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|(_, sev)| *sev == Severity::Warning));
}

#[test]
fn straight_quotes_fix_is_unsafe_and_correct() {
    // The direction-inferring fix is Unsafe, so `--fix` (unsafe = false) is a
    // no-op; `--unsafe-fixes` rewrites to the ligatures.
    assert_eq!(fix_to_fixpoint("say \"hi\"\n"), "say ``hi''\n");
    for case in [
        "He said \"hello world\" today.\n",
        "(\"quoted\")\n",
        "\"Start.\n",
    ] {
        assert_fix_is_correct(case);
    }
}

#[test]
fn sectioning_level_jump_flags_skipped_level() {
    let out = lint("\\section{Intro}\n\\subsubsection{Deep}\n");
    let hits: Vec<_> = out
        .iter()
        .filter(|(r, _)| *r == "sectioning-level-jump")
        .collect();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].1, Severity::Warning);
    // A well-formed outline draws no finding.
    assert!(
        lint("\\section{A}\n\\subsection{B}\n\\subsubsection{C}\n")
            .iter()
            .all(|(r, _)| *r != "sectioning-level-jump")
    );
}
