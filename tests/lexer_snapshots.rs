//! Snapshot the token stream for a few representative inputs (insta demo /
//! Phase 0 scaffolding). Regenerate with `INSTA_UPDATE=always cargo test` or
//! `task snapshots`.

use badness::parser::lex;

fn dump(input: &str) -> String {
    lex(input)
        .iter()
        .map(|t| format!("{:?} {:?}", t.kind, t.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn lex_command_with_args() {
    insta::assert_snapshot!(dump(r"\section{Hello}[opt]"));
}

#[test]
fn lex_inline_math() {
    insta::assert_snapshot!(dump(r"$x^2_i = \alpha$"));
}

#[test]
fn lex_comment_and_paragraph() {
    insta::assert_snapshot!(dump("a % note\n\nb"));
}

#[test]
fn lex_environment() {
    insta::assert_snapshot!(dump("\\begin{eq}\n  a &= b \\\\\n\\end{eq}"));
}
