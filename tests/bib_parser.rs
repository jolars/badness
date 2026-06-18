//! BibTeX parser tests: tree-shape snapshots over representative inputs, plus
//! targeted assertions on error-recovery behaviour. Every case also re-checks the
//! losslessness invariant. Regenerate snapshots with `task snapshots`.

use badness::bib::parse;
use badness::bib::syntax::SyntaxNode;
use rowan::NodeOrToken;

/// Render a CST as an indented `KIND@range` tree, with token text, followed by
/// any syntax errors. Stable and snapshot-friendly.
fn tree(input: &str) -> String {
    let parsed = parse(input);
    // Losslessness must hold for every input the parser sees.
    assert_eq!(
        parsed.syntax().to_string(),
        input,
        "losslessness violated for {input:?}"
    );

    let mut out = String::new();
    render(&parsed.syntax(), 0, &mut out);
    for err in &parsed.errors {
        out.push_str(&format!(
            "error @{}..{}: {}\n",
            err.start, err.end, err.message
        ));
    }
    out
}

fn render(node: &SyntaxNode, depth: usize, out: &mut String) {
    out.push_str(&format!(
        "{:indent$}{:?}@{:?}\n",
        "",
        node.kind(),
        node.text_range(),
        indent = depth * 2
    ));
    for child in node.children_with_tokens() {
        match child {
            NodeOrToken::Node(n) => render(&n, depth + 1, out),
            NodeOrToken::Token(t) => out.push_str(&format!(
                "{:indent$}{:?}@{:?} {:?}\n",
                "",
                t.kind(),
                t.text_range(),
                t.text(),
                indent = (depth + 1) * 2
            )),
        }
    }
}

// --- well-formed inputs ----------------------------------------------------

#[test]
fn regular_entry() {
    insta::assert_snapshot!(tree(
        "@article{knuth1984,\n  author = {Donald Knuth},\n  year = 2020,\n}"
    ));
}

#[test]
fn paren_delimited_entry() {
    insta::assert_snapshot!(tree("@book(key, title = {T})"));
}

#[test]
fn trailing_comma() {
    insta::assert_snapshot!(tree("@misc{k, note = {n},}"));
}

#[test]
fn key_only_entry() {
    insta::assert_snapshot!(tree("@misc{lonelykey}"));
}

#[test]
fn string_entry() {
    insta::assert_snapshot!(tree(r#"@string{jan = "January"}"#));
}

#[test]
fn preamble_entry() {
    insta::assert_snapshot!(tree(r#"@preamble{ "\newcommand{\noop}[1]{}" }"#));
}

#[test]
fn comment_entry() {
    insta::assert_snapshot!(tree("@comment{ jabref-meta: databaseType:bibtex; }"));
}

#[test]
fn quoted_and_braced_values() {
    insta::assert_snapshot!(tree(r#"@article{k, a = "quoted", b = {braced}, c = 1999}"#));
}

#[test]
fn concatenation() {
    insta::assert_snapshot!(tree(r#"@string{x = "a" # foo # {b}}"#));
}

#[test]
fn nested_braces() {
    insta::assert_snapshot!(tree("@misc{k, title = {a {B{c}} d}}"));
}

#[test]
fn braces_protect_quote_in_quoted_value() {
    insta::assert_snapshot!(tree(r#"@misc{k, title = "a {\"} b"}"#));
}

#[test]
fn junk_between_entries() {
    insta::assert_snapshot!(tree("leading junk\n@misc{a}\n\nsome notes\n@misc{b}\n"));
}

// --- error recovery --------------------------------------------------------

#[test]
fn unterminated_brace() {
    insta::assert_snapshot!(tree("@misc{k, title = {unclosed"));
}

#[test]
fn unterminated_quote() {
    insta::assert_snapshot!(tree(r#"@misc{k, title = "unclosed}"#));
}

#[test]
fn missing_equals() {
    insta::assert_snapshot!(tree("@misc{k, title {v}}"));
}

#[test]
fn stray_at_starts_new_entry() {
    insta::assert_snapshot!(tree("@misc{a, title = {x}\n@misc{b}"));
}

#[test]
fn missing_entry_type() {
    insta::assert_snapshot!(tree("@ {oops}"));
}
