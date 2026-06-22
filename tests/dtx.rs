//! `.dtx` (docstrip literate) parsing: the two-layer surface model.
//!
//! M1a establishes the documentation-margin model — a line-leading `%` becomes a
//! `DOC_MARGIN` trivia token so the doc layer parses as ordinary LaTeX — and lets
//! `macrocode` pair through the existing environment grammar with its body lexed
//! as real code. M2 adds docstrip guards: a line-leading `%<…>` becomes a `GUARD`
//! trivia leaf (flat, never a block node) in any layer. Every case also re-checks
//! losslessness.

use badness::parser::{LatexFlavor, LexConfig, parse_with_flavor};
use badness::syntax::{SyntaxKind, SyntaxNode};

/// Parse `input` under the docstrip (`.dtx`) config, asserting losslessness.
fn parse_dtx(input: &str) -> SyntaxNode {
    let config = LexConfig {
        flavor: LatexFlavor::Document,
        dtx: true,
    };
    let parsed = parse_with_flavor(input, config);
    assert_eq!(
        parsed.syntax().to_string(),
        input,
        "losslessness violated for {input:?}"
    );
    parsed.syntax()
}

/// Count descendant nodes of a given kind.
fn count(root: &SyntaxNode, kind: SyntaxKind) -> usize {
    root.descendants().filter(|n| n.kind() == kind).count()
}

/// Count tokens (leaves) of a given kind.
fn count_token(root: &SyntaxNode, kind: SyntaxKind) -> usize {
    root.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == kind)
        .count()
}

/// All `(kind, text)` tokens in document order.
fn tokens(root: &SyntaxNode) -> Vec<(SyntaxKind, String)> {
    root.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .map(|t| (t.kind(), t.text().to_string()))
        .collect()
}

#[test]
fn line_leading_percent_is_a_margin_mid_line_percent_is_a_comment() {
    // The first `%` opens a documentation line (a margin); the `%` on the code
    // line is preceded by `word `, so it is an ordinary trailing comment.
    let root = parse_dtx("% doc text\nword % trailing\n");
    let toks = tokens(&root);
    let margins: Vec<_> = toks
        .iter()
        .filter(|(k, _)| *k == SyntaxKind::DOC_MARGIN)
        .collect();
    let comments: Vec<_> = toks
        .iter()
        .filter(|(k, _)| *k == SyntaxKind::COMMENT)
        .collect();
    assert_eq!(margins, vec![&(SyntaxKind::DOC_MARGIN, "%".to_string())]);
    assert_eq!(
        comments,
        vec![&(SyntaxKind::COMMENT, "% trailing".to_string())]
    );
    // The documentation content lexes as real words, not as comment text.
    assert!(toks.contains(&(SyntaxKind::WORD, "doc".to_string())));
}

#[test]
fn block_guards_are_guard_tokens_wrapping_ordinary_code() {
    // `%<*tag>` / `%</tag>` block delimiters lex as `GUARD` tokens (not margins, not
    // comments); the enclosed un-margined driver code parses as ordinary LaTeX.
    let root = parse_dtx("%<*driver>\n\\documentclass{article}\n%</driver>\n");
    let toks = tokens(&root);
    assert_eq!(count_token(&root, SyntaxKind::DOC_MARGIN), 0);
    assert_eq!(count_token(&root, SyntaxKind::GUARD), 2);
    assert!(toks.contains(&(SyntaxKind::GUARD, "%<*driver>".to_string())));
    assert!(toks.contains(&(SyntaxKind::GUARD, "%</driver>".to_string())));
    // No `%<…>` line was mistaken for a comment.
    assert!(!toks.iter().any(|(k, _)| *k == SyntaxKind::COMMENT));
    assert!(count(&root, SyntaxKind::COMMAND) >= 1);
}

#[test]
fn an_inline_guard_prefixes_parsed_code() {
    // `%<tag>code` is a `GUARD` prefix; the code after the closing `>` lexes as
    // ordinary LaTeX (it is not swallowed into the guard token).
    let root = parse_dtx("%<plain>\\RequirePackage{xcolor}\n");
    let toks = tokens(&root);
    assert!(toks.contains(&(SyntaxKind::GUARD, "%<plain>".to_string())));
    assert!(toks.contains(&(SyntaxKind::CONTROL_WORD, "\\RequirePackage".to_string())));
    assert!(toks.contains(&(SyntaxKind::WORD, "xcolor".to_string())));
    assert_eq!(count(&root, SyntaxKind::COMMAND), 1);
}

#[test]
fn guards_punctuate_macrocode_bodies() {
    // Guards are recognized in any layer, `macrocode` body included; a plain `%`
    // comment line inside the body still lexes as a code comment, not a guard.
    let input =
        "%    \\begin{macrocode}\n%<*foo>\n\\def\\x{y}\n% note\n%</foo>\n%    \\end{macrocode}\n";
    let root = parse_dtx(input);
    let toks = tokens(&root);
    assert!(toks.contains(&(SyntaxKind::GUARD, "%<*foo>".to_string())));
    assert!(toks.contains(&(SyntaxKind::GUARD, "%</foo>".to_string())));
    // The `\def` parses as code under the package regime.
    assert!(toks.contains(&(SyntaxKind::CONTROL_WORD, "\\def".to_string())));
    // The non-guard `%` line stays a code comment.
    assert!(toks.contains(&(SyntaxKind::COMMENT, "% note".to_string())));
}

#[test]
fn a_malformed_guard_falls_back_to_a_comment() {
    // A `%<` with no closing `>` before the line ends is not a guard.
    let root = parse_dtx("%<unterminated\nword\n");
    let toks = tokens(&root);
    assert_eq!(count_token(&root, SyntaxKind::GUARD), 0);
    assert!(toks.contains(&(SyntaxKind::COMMENT, "%<unterminated".to_string())));
}

#[test]
fn blank_margin_line_breaks_the_paragraph() {
    // `%\n` is the doc-layer blank line: the two surrounding `NEWLINE`s, with the
    // floating margins, form a `\par` boundary.
    let root = parse_dtx("% first paragraph\n%\n% second paragraph\n");
    assert_eq!(count(&root, SyntaxKind::PARAGRAPH), 2);
}

#[test]
fn continuation_margins_keep_one_paragraph() {
    // Consecutive content doc lines are a single paragraph; the margins float
    // inside it like whitespace.
    let root = parse_dtx("% one two three\n% four five six\n");
    assert_eq!(count(&root, SyntaxKind::PARAGRAPH), 1);
}

#[test]
fn macrocode_is_an_environment_whose_body_is_real_code() {
    let input = "%    \\begin{macrocode}\n\\def\\foo{\\bar}\n%    \\end{macrocode}\n";
    let root = parse_dtx(input);
    // The framing lines pair through the ordinary environment grammar.
    assert_eq!(count(&root, SyntaxKind::ENVIRONMENT), 1);
    // The body is parsed code (a `\def` command), not an opaque verbatim blob.
    assert!(count(&root, SyntaxKind::COMMAND) >= 1);
    assert_eq!(count_token(&root, SyntaxKind::VERBATIM_BODY), 0);
    // Both framing lines kept their margins.
    assert_eq!(count_token(&root, SyntaxKind::DOC_MARGIN), 2);
}

#[test]
fn macrocode_body_lexes_under_the_package_regime() {
    // Inside `macrocode`, `@` is a letter (`\bar@baz` is one control word) — the
    // package internals regime. In the documentation layer it is not.
    let inside = parse_dtx("%    \\begin{macrocode}\n\\bar@baz\n%    \\end{macrocode}\n");
    assert!(tokens(&inside).contains(&(SyntaxKind::CONTROL_WORD, "\\bar@baz".to_string())));

    let doc = parse_dtx("% \\bar@baz\n");
    let dtoks = tokens(&doc);
    assert!(dtoks.contains(&(SyntaxKind::CONTROL_WORD, "\\bar".to_string())));
    assert!(
        !dtoks
            .iter()
            .any(|(k, t)| *k == SyntaxKind::CONTROL_WORD && t == "\\bar@baz")
    );
}

#[test]
fn a_stray_percent_line_inside_macrocode_is_a_code_comment() {
    // Within the body, a line-leading `%` that is not the terminator is an
    // ordinary code comment, not a documentation margin.
    let input =
        "%    \\begin{macrocode}\n\\foo\n% an in-code comment\n\\bar\n%    \\end{macrocode}\n";
    let root = parse_dtx(input);
    assert!(tokens(&root).contains(&(SyntaxKind::COMMENT, "% an in-code comment".to_string())));
    // Only the two frame lines carry margins.
    assert_eq!(count_token(&root, SyntaxKind::DOC_MARGIN), 2);
}

#[test]
fn verbatim_command_defined_and_used_inside_macrocode_is_two_pass_stable() {
    // A catcode-othering command defined in `macrocode` (only lexable because `@`
    // is a letter there) is discovered by the definition scan and, on the second
    // pass, captures its call-site argument as one opaque `VERB`. The docstrip
    // mode reproduces identically across both passes, so this round-trips.
    let input = "%    \\begin{macrocode}\n\\newcommand\\shex[1]{\\@makeother\\$#1}\n\\shex{a_$b$}\n%    \\end{macrocode}\n";
    let root = parse_dtx(input);
    assert_eq!(count_token(&root, SyntaxKind::VERB), 1);
}

#[test]
fn unterminated_macrocode_recovers_losslessly() {
    // No closing `%    \end{macrocode}`: the environment grammar recovers at EOF
    // and the bytes still round-trip (losslessness asserted in `parse_dtx`).
    let root = parse_dtx("%    \\begin{macrocode}\n\\foo\n\\bar\n");
    assert_eq!(count(&root, SyntaxKind::ENVIRONMENT), 1);
}

#[test]
fn doc_margins_never_form_a_doc_comment() {
    // The leading-comment bind (decision #9) keys on real `COMMENT` tokens. A
    // `.dtx` documentation margin is a `DOC_MARGIN` trivia leaf, not a comment, so
    // even prose directly above a documentable construct never binds into a
    // `DOC_COMMENT` node — margins float like whitespace.
    let root = parse_dtx(
        "% Some prose about \\foo.\n%    \\begin{macrocode}\n\\foo\n%    \\end{macrocode}\n",
    );
    assert_eq!(count(&root, SyntaxKind::DOC_COMMENT), 0);
}

#[test]
fn macro_env_and_describe_parse_in_the_doc_layer() {
    // The doc/ltxdoc vocabulary lexes as ordinary LaTeX in the documentation
    // layer: the `macro` environment frames its documentation, `\DescribeMacro`
    // is a plain command. Their arities come from the signature DB, not the parse.
    let root =
        parse_dtx("% \\begin{macro}{\\foo}\n% \\DescribeMacro{\\foo} does foo.\n% \\end{macro}\n");
    assert_eq!(count(&root, SyntaxKind::ENVIRONMENT), 1);
    let toks = tokens(&root);
    assert!(toks.contains(&(SyntaxKind::CONTROL_WORD, "\\DescribeMacro".to_string())));
    // No comment bind in the doc layer (margins float).
    assert_eq!(count(&root, SyntaxKind::DOC_COMMENT), 0);
}

#[test]
fn meta_comment_header_parses_as_documentation() {
    // The conventional self-extracting header: every line is a margin doc line
    // carrying `\iffalse … \fi` (left un-evaluated — ordinary commands to us).
    let root = parse_dtx("% \\iffalse meta-comment\n% \\fi\n");
    let toks = tokens(&root);
    assert!(toks.contains(&(SyntaxKind::CONTROL_WORD, "\\iffalse".to_string())));
    assert!(toks.contains(&(SyntaxKind::CONTROL_WORD, "\\fi".to_string())));
    assert_eq!(count_token(&root, SyntaxKind::DOC_MARGIN), 2);
}
