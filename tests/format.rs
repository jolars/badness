//! Phase 2 formatter tests. The first real rule is whitespace normalization, so
//! the output is no longer identical to the input. Behavior is pinned by
//! `tests/fixtures/formatter/<name>/{input,expected}.tex` pairs (mirroring
//! ravel's fixture layout). The `AGENTS.md` invariants — idempotence, parse
//! stability, and losslessness of the formatted text — are asserted on the
//! formatted output for every case.

use std::fs;
use std::path::{Path, PathBuf};

use badness::formatter::{FormatStyle, WrapMode, format, format_with_style};
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
    // Signature-DB-aware environment headers: a declared argument glued onto the
    // `\begin` line, an already-inline one, an optional argument, and an unknown
    // environment (generic path). Invariants must hold for all.
    "\\begin{tabular}\n{cc}\nx & y\n\\end{tabular}\n",
    "\\begin{tabular}{cc}\nx & y\n\\end{tabular}\n",
    "\\begin{minipage}[t]{4cm}\ntext\n\\end{minipage}\n",
    "\\begin{myenv}\n{cc}\nbody\n\\end{myenv}\n",
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
/// `input.tex` + hand-verified `expected.tex` pair, with the `(wrap, line_width)`
/// each was authored under.
///
/// The whitespace / indentation fixtures isolate rules that predate paragraph
/// reflow, so they run under [`WrapMode::Preserve`] — their `expected.tex` is the
/// pre-reflow output and must stay byte-identical. The `reflow_*` fixtures
/// exercise the new rule, each at a width chosen to make the wrapping legible.
const FIXTURES: &[(&str, WrapMode, usize)] = &[
    // Whitespace normalization.
    (
        "whitespace_trailing_and_blank_lines",
        WrapMode::Preserve,
        80,
    ),
    ("trailing_whitespace_only", WrapMode::Preserve, 80),
    ("collapse_blank_lines", WrapMode::Preserve, 80),
    ("protected_comment_trailing_space", WrapMode::Preserve, 80),
    ("protected_verbatim", WrapMode::Preserve, 80),
    ("final_newline_added", WrapMode::Preserve, 80),
    // Environment indentation.
    ("environment_indents_body", WrapMode::Preserve, 80),
    ("nested_environments", WrapMode::Preserve, 80),
    ("environment_reindents", WrapMode::Preserve, 80),
    ("environment_blank_lines_in_body", WrapMode::Preserve, 80),
    ("environment_begin_arguments", WrapMode::Preserve, 80),
    ("environment_argument_glued", WrapMode::Preserve, 80),
    // Arity from a *scanned* definition (not the built-in DB): the document's own
    // `\newenvironment`/`\NewDocumentEnvironment` arg is glued onto the `\begin`.
    ("environment_user_defined_glued", WrapMode::Preserve, 80),
    ("environment_xparse_glued", WrapMode::Preserve, 80),
    ("verbatim_in_environment", WrapMode::Preserve, 80),
    // Group / argument indentation.
    ("group_indents_body", WrapMode::Preserve, 80),
    ("optional_indents_body", WrapMode::Preserve, 80),
    ("nested_groups", WrapMode::Preserve, 80),
    ("group_single_line_stays_inline", WrapMode::Preserve, 80),
    ("group_reindents", WrapMode::Preserve, 80),
    // Paragraph reflow (the new rule).
    ("reflow_join_short", WrapMode::Reflow, 80),
    ("reflow_wrap_to_width", WrapMode::Reflow, 40),
    ("reflow_tie_no_break", WrapMode::Reflow, 12),
    ("reflow_forced_break", WrapMode::Reflow, 80),
    ("reflow_forced_break_with_optarg", WrapMode::Reflow, 80),
    ("reflow_comment_ends_line", WrapMode::Reflow, 80),
    ("reflow_in_environment", WrapMode::Reflow, 20),
];

fn fixture_path(name: &str, file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formatter")
        .join(name)
        .join(file)
}

#[test]
fn formatter_fixtures_match_expected() {
    for &(name, wrap, line_width) in FIXTURES {
        let style = FormatStyle {
            wrap,
            line_width,
            ..FormatStyle::default()
        };
        let input = fs::read_to_string(fixture_path(name, "input.tex"))
            .unwrap_or_else(|e| panic!("read {name}/input.tex: {e}"));
        let expected = fs::read_to_string(fixture_path(name, "expected.tex"))
            .unwrap_or_else(|e| panic!("read {name}/expected.tex: {e}"));

        // The input must parse cleanly (the formatter only handles clean parses).
        assert!(
            parse(&input).errors.is_empty(),
            "fixture {name} input must parse without diagnostics"
        );

        let formatted =
            format_with_style(&input, style).unwrap_or_else(|e| panic!("format {name}: {e}"));
        assert_eq!(formatted, expected, "fixture {name} output mismatch");

        // The formatted output is idempotent (under the same style), clean, and
        // lossless.
        assert_eq!(
            format_with_style(&formatted, style).expect("reformat"),
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

/// `WrapMode::Preserve` leaves authored intra-paragraph line breaks untouched —
/// the pre-reflow behavior — while the default `Reflow` joins them. This pins the
/// distinction and guards the fallback path the (not-yet-implemented) `Sentence`
/// and `Semantic` modes also take.
#[test]
fn preserve_keeps_author_breaks_while_reflow_joins() {
    let input = "one two\nthree four\n";
    let preserve = FormatStyle {
        wrap: WrapMode::Preserve,
        ..FormatStyle::default()
    };
    assert_eq!(
        format_with_style(input, preserve).expect("preserve formats"),
        "one two\nthree four\n",
        "preserve must keep authored line breaks"
    );
    assert_eq!(
        format(input).expect("reflow formats"),
        "one two three four\n",
        "default reflow must join the lines"
    );
}

/// The `\begin` argument glue is driven by the scanned signature, not the name: the
/// *same* `\begin{thm}\n{x}` glues only when the document defines `thm`'s arity.
/// Without the definition `thm` is unknown to both the document and the built-in DB,
/// so it stays on the generic path and the argument is pushed to its own line.
#[test]
fn user_definition_drives_begin_argument_glue() {
    let style = FormatStyle {
        wrap: WrapMode::Preserve,
        ..FormatStyle::default()
    };
    let undefined = "\\begin{thm}\n{x}\nbody\n\\end{thm}\n";
    assert_eq!(
        format_with_style(undefined, style).expect("formats"),
        "\\begin{thm}\n{x}\n  body\n\\end{thm}\n",
        "an undefined environment must not glue its argument"
    );

    let defined = format!("\\newenvironment{{thm}}[1]{{a}}{{b}}\n{undefined}");
    assert_eq!(
        format_with_style(&defined, style).expect("formats"),
        "\\newenvironment{thm}[1]{a}{b}\n\\begin{thm}{x}\n  body\n\\end{thm}\n",
        "defining thm's arity must glue the argument onto \\begin"
    );
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
    // visible diff. Under the default `Reflow`, the two short prose lines also
    // join into one.
    let input = "\\section{Intro}   \n\n\n\nSome text with trailing space   \nmore text.";
    insta::assert_snapshot!(format(input).expect("formats"));
}
