//! Snapshot the BibTeX token stream for a few representative inputs. Regenerate
//! with `INSTA_UPDATE=always cargo test` or `task snapshots`.

use badness_parser::bib::lex;

fn dump(input: &str) -> String {
    lex(input)
        .iter()
        .map(|t| format!("{:?} {:?}", t.kind, t.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn lex_entry() {
    insta::assert_snapshot!(dump("@article{key, year = 2020}"));
}

#[test]
fn lex_quoted_and_concat() {
    insta::assert_snapshot!(dump(r#"title = "a" # foo # {b}"#));
}

#[test]
fn lex_word_vs_number() {
    insta::assert_snapshot!(dump("jan 2020 key123 123abc"));
}

#[test]
fn lex_percent_is_a_bare_special() {
    // The lexer never decides comment-ness — `%` is one token wherever it appears
    // and the grammar wraps only the legal ones in a `COMMENT` node.
    insta::assert_snapshot!(dump("{50% off} % note"));
}

#[test]
fn lex_newlines_and_whitespace() {
    insta::assert_snapshot!(dump("a\r\nb\n\n  c"));
}
