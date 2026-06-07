//! Phase 2 formatter tests. The MVP lowering is identity, so the headline
//! oracle is `format(x) == x`. The idempotence and stability invariants from
//! `AGENTS.md` are asserted too: they are trivially satisfied today but guard
//! the rules that will replace the identity lowering.

use std::fs;
use std::path::Path;

use badness::formatter::format;
use badness::parser::parse;
use badness::syntax::SyntaxNode;
use rowan::NodeOrToken;

/// A structural signature of a parse: the node/token *kinds* nested by depth,
/// ignoring byte ranges and token text. Two inputs with the same signature
/// parse to structurally-equivalent trees (the `parse(fmt(x)) ≅ parse(x)`
/// invariant, which must hold even once the formatter rewrites whitespace).
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

/// Assert the three invariants for a single clean-parsing input. Inputs the
/// parser rejects are out of scope for the formatter (it refuses them), so the
/// caller filters those out.
fn assert_format_invariants(input: &str) {
    let formatted = format(input).expect("clean input should format");

    // Identity (MVP milestone): the lowering reproduces the input byte-for-byte.
    assert_eq!(formatted, input, "format is not identity for {input:?}");

    // Idempotence: fmt(fmt(x)) == fmt(x).
    let twice = format(&formatted).expect("formatted output should re-format");
    assert_eq!(twice, formatted, "format is not idempotent for {input:?}");

    // Stability: parse(fmt(x)) is structurally equivalent to parse(x).
    assert_eq!(
        structure(&formatted),
        structure(input),
        "format is not parse-stable for {input:?}"
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
    // A representative document, snapshotted so future rule changes surface as a
    // visible diff. Under the identity lowering this equals the input.
    let input = "\\section{Intro}\n\nSome text with $x^2$ and a % trailing comment\nmore text.\n";
    insta::assert_snapshot!(format(input).expect("formats"));
}
