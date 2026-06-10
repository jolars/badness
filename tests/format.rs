//! Phase 2 formatter tests. The first real rule is whitespace normalization, so
//! the output is no longer identical to the input. Behavior is pinned by
//! `tests/fixtures/formatter/<name>/{input,expected}.tex` pairs (mirroring
//! ravel's fixture layout). The `AGENTS.md` invariants — idempotence, parse
//! stability, and losslessness of the formatted text — are asserted on the
//! formatted output for every case.

use std::fs;
use std::path::{Path, PathBuf};

use badness::formatter::format;
use badness::parser::{parse, reconstruct};
use badness::syntax::{SyntaxKind, SyntaxNode};
use rowan::NodeOrToken;

/// A structural signature of a parse: the node/token *kinds* nested by depth,
/// ignoring byte ranges, token text, and trivia tokens. The formatter rewrites
/// `WHITESPACE`/`NEWLINE` trivia by design, so stability is defined over the
/// meaningful tree shape: `parse(fmt(x))` must match `parse(x)` once trivia is
/// elided.
fn structure(input: &str) -> String {
    let mut out = String::new();
    render_kinds(&parse(input).syntax(), 0, &mut out);
    out
}

fn render_kinds(node: &SyntaxNode, depth: usize, out: &mut String) {
    out.push_str(&format!(
        "{:indent$}{:?}\n",
        "",
        node.kind(),
        indent = depth * 2
    ));
    for child in node.children_with_tokens() {
        match child {
            NodeOrToken::Node(n) => render_kinds(&n, depth + 1, out),
            NodeOrToken::Token(t) => {
                // Trivia is intentionally normalized away; ignore it here.
                if matches!(t.kind(), SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE) {
                    continue;
                }
                out.push_str(&format!(
                    "{:indent$}{:?}\n",
                    "",
                    t.kind(),
                    indent = (depth + 1) * 2
                ));
            }
        }
    }
}

/// Assert the formatter invariants for a single clean-parsing input. Inputs the
/// parser rejects are out of scope for the formatter (it refuses them), so the
/// caller filters those out.
fn assert_format_invariants(input: &str) {
    let formatted = format(input).expect("clean input should format");

    // Idempotence: fmt(fmt(x)) == fmt(x).
    let twice = format(&formatted).expect("formatted output should re-format");
    assert_eq!(twice, formatted, "format is not idempotent for {input:?}");

    // Stability: parse(fmt(x)) is structurally equivalent to parse(x).
    assert_eq!(
        structure(&formatted),
        structure(input),
        "format is not parse-stable for {input:?}"
    );

    // The formatted output is itself a clean, lossless document.
    assert!(
        parse(&formatted).errors.is_empty(),
        "formatted output should parse without diagnostics for {input:?}"
    );
    assert_eq!(
        reconstruct(&formatted),
        formatted,
        "formatted output should round-trip losslessly for {input:?}"
    );
}

/// The clean-parsing subset of the roundtrip unit corpus (mirrors
/// `tests/roundtrip.rs`). Cases with parser diagnostics are excluded — the
/// formatter only operates on input the parser accepts.
const CLEAN_CASES: &[&str] = &[
    "",
    "hello world",
    r"\section{Introduction}",
    r"$x^2 + y_i = \frac{1}{2}$",
    "a % comment\nb",
    r"\begin{itemize}\item one\end{itemize}",
    "unicode: café — naïve ∑∫ 𝕏",
    r"\\ \{ \} \% \, \;",
    "trailing backslash \\",
    "[opt] {req} & # ~ ^_",
    "no final newline",
    "para one\n\npara two\n",
];

#[test]
fn format_invariants_units() {
    for case in CLEAN_CASES {
        // Guard: every listed case must parse cleanly, else it does not belong.
        assert!(
            parse(case).errors.is_empty(),
            "CLEAN_CASES must parse without diagnostics: {case:?}"
        );
        assert_format_invariants(case);
    }
}

#[test]
fn format_invariants_corpus() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut count = 0;
    for entry in fs::read_dir(&dir).expect("read corpus dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("tex") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read corpus file");
        // The corpus may contain inputs that exercise recovery; only the
        // clean-parsing ones are in scope for the formatter.
        if parse(&text).errors.is_empty() {
            assert_format_invariants(&text);
            count += 1;
        }
    }
    assert!(count > 0, "no clean .tex corpus files found in {dir:?}");
}

/// Fixture cases under `tests/fixtures/formatter/<name>/`, each an
/// `input.tex` + hand-verified `expected.tex` pair.
const FIXTURES: &[&str] = &[
    // Whitespace normalization.
    "whitespace_trailing_and_blank_lines",
    "trailing_whitespace_only",
    "collapse_blank_lines",
    "protected_comment_trailing_space",
    "protected_verbatim",
    "final_newline_added",
    // Environment indentation.
    "environment_indents_body",
    "nested_environments",
    "environment_reindents",
    "environment_blank_lines_in_body",
    "verbatim_in_environment",
    // Group / argument indentation.
    "group_indents_body",
    "optional_indents_body",
    "nested_groups",
    "group_single_line_stays_inline",
    "group_reindents",
];

fn fixture_path(name: &str, file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formatter")
        .join(name)
        .join(file)
}

#[test]
fn formatter_fixtures_match_expected() {
    for name in FIXTURES {
        let input = fs::read_to_string(fixture_path(name, "input.tex"))
            .unwrap_or_else(|e| panic!("read {name}/input.tex: {e}"));
        let expected = fs::read_to_string(fixture_path(name, "expected.tex"))
            .unwrap_or_else(|e| panic!("read {name}/expected.tex: {e}"));

        // The input must parse cleanly (the formatter only handles clean parses).
        assert!(
            parse(&input).errors.is_empty(),
            "fixture {name} input must parse without diagnostics"
        );

        let formatted = format(&input).unwrap_or_else(|e| panic!("format {name}: {e}"));
        assert_eq!(formatted, expected, "fixture {name} output mismatch");

        // The formatted output is idempotent, clean, and lossless.
        assert_eq!(
            format(&formatted).expect("reformat"),
            formatted,
            "fixture {name} is not idempotent"
        );
        assert!(
            parse(&formatted).errors.is_empty(),
            "fixture {name} formatted output must parse cleanly"
        );
        assert_eq!(
            reconstruct(&formatted),
            formatted,
            "fixture {name} formatted output must round-trip"
        );
    }
}

#[test]
fn format_rejects_unparseable_input() {
    // A stray closing brace yields a parser diagnostic; the formatter refuses it
    // rather than reshaping around an error.
    let input = "}";
    assert!(!parse(input).errors.is_empty(), "expected a parse error");
    assert!(
        format(input).is_err(),
        "formatter should refuse error input"
    );
}

#[test]
fn format_output_snapshot() {
    // A deliberately messy document — trailing whitespace, runs of blank lines,
    // and no final newline — snapshotted so future rule changes surface as a
    // visible diff.
    let input = "\\section{Intro}   \n\n\n\nSome text with trailing space   \nmore text.";
    insta::assert_snapshot!(format(input).expect("formats"));
}
