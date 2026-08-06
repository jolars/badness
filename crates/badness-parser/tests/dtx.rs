//! `.dtx` (docstrip literate) parsing: the two-layer surface model.
//!
//! M1a establishes the documentation-margin model — a line-leading `%` becomes a
//! `DOC_MARGIN` trivia token so the doc layer parses as ordinary LaTeX — and lets
//! `macrocode` pair through the existing environment grammar with its body lexed
//! as real code. M2 adds docstrip guards: a line-leading `%<…>` becomes a `GUARD`
//! trivia leaf (flat, never a block node) in any layer. Every case also re-checks
//! losslessness.

use badness_parser::parser::{LatexFlavor, LexConfig, parse_with_flavor};
use badness_parser::semantic::{DocKind, doc_associations};
use badness_parser::syntax::{SyntaxKind, SyntaxNode};

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
fn macrocode_frame_margins_sit_where_the_formatter_expects() {
    // The formatter's two-layer lowering relies on a CST-shape fact: the opening
    // frame's `DOC_MARGIN` immediately precedes `\begin` (reachable by walking back
    // over inline whitespace), and the closing frame's `DOC_MARGIN` is a child token
    // of the ENVIRONMENT sitting *before* the END node (so it can be pulled onto the
    // `\end` line). Pin both so a future grammar change can't silently break the
    // formatter.
    let root = parse_dtx("%    \\begin{macrocode}\n\\def\\foo{x}\n%    \\end{macrocode}\n");
    let env = root
        .descendants()
        .find(|n| n.kind() == SyntaxKind::ENVIRONMENT)
        .expect("environment");

    // Opening frame: the token before `\begin`, skipping inline whitespace, is a
    // DOC_MARGIN.
    let begin = env
        .children()
        .find(|c| c.kind() == SyntaxKind::BEGIN)
        .expect("begin");
    let mut tok = begin.first_token().and_then(|t| t.prev_token());
    while let Some(t) = &tok {
        if t.kind() == SyntaxKind::WHITESPACE {
            tok = t.prev_token();
        } else {
            break;
        }
    }
    assert_eq!(
        tok.map(|t| t.kind()),
        Some(SyntaxKind::DOC_MARGIN),
        "opening `\\begin` must be preceded by a DOC_MARGIN on its line"
    );

    // Closing frame: a DOC_MARGIN child of the ENVIRONMENT appears before the END
    // node in document order.
    let children: Vec<_> = env.children_with_tokens().collect();
    let last_margin = children
        .iter()
        .rposition(|e| e.kind() == SyntaxKind::DOC_MARGIN)
        .expect("a closing-frame DOC_MARGIN");
    let end = children
        .iter()
        .position(|e| e.kind() == SyntaxKind::END)
        .expect("END node");
    assert!(
        last_margin < end,
        "the closing-frame DOC_MARGIN must precede the END node within the environment"
    );
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
fn doc_associations_pair_prose_with_nested_code() {
    // The semantic prose↔code association (TODO M3 follow-up): a realistic `.dtx`
    // slice with a documented macro (code nested in `macrocode`) followed by a
    // `\DescribeMacro`. The parser leaves margins floating; the semantic query ties
    // each documented name to the code it brackets.
    let src = "% \\begin{macro}{\\foo}\n%   The \\foo macro.\n%    \\begin{macrocode}\n\\def\\foo{x}\n%    \\end{macrocode}\n% \\end{macro}\n% \\DescribeMacro\\bar is described.\n";
    let root = parse_dtx(src);
    let assocs = doc_associations(&root);

    assert_eq!(assocs.len(), 2);
    assert_eq!(assocs[0].name, "\\foo");
    assert_eq!(assocs[0].kind, DocKind::Macro);
    assert_eq!(assocs[0].code.len(), 1);
    assert!(src[assocs[0].code[0]].contains("\\def\\foo{x}"));

    assert_eq!(assocs[1].name, "\\bar");
    assert_eq!(assocs[1].kind, DocKind::DescribeMacro);
    assert!(assocs[1].code.is_empty());
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

#[test]
fn full_self_extracting_dtx_needs_no_driver_machinery() {
    // The whole self-extracting shape in one file: a `% \iffalse … % \fi` meta
    // comment wrapping a `%<*driver> … %</driver>` block of real driver code, then
    // a documented `%<*package>` body with a `macrocode` block. None of it gets
    // special handling — `\iffalse`/`\fi` are ordinary commands on margin lines,
    // the guards are flat trivia, and every layer stays lossless (asserted in
    // `parse_dtx`). This pins "the driver block needs no driver-specific model."
    let root = parse_dtx(
        "% \\iffalse meta-comment\n\
         %<*driver>\n\
         \\documentclass{ltxdoc}\n\
         \\DocInput{foo.dtx}\n\
         %</driver>\n\
         % \\fi\n\
         % \\section{The \\foo macro}\n\
         %<*package>\n\
         %    \\begin{macrocode}\n\
         \\newcommand\\foo[1]{\\bar@baz{#1}}\n\
         %    \\end{macrocode}\n\
         %</package>\n",
    );
    let toks = tokens(&root);
    // The `\iffalse`/`\fi` guard ride margin lines as ordinary commands.
    assert!(toks.contains(&(SyntaxKind::CONTROL_WORD, "\\iffalse".to_string())));
    assert!(toks.contains(&(SyntaxKind::CONTROL_WORD, "\\fi".to_string())));
    // The driver block delimiters and the package guards are flat `GUARD` trivia.
    for guard in ["%<*driver>", "%</driver>", "%<*package>", "%</package>"] {
        assert!(toks.contains(&(SyntaxKind::GUARD, guard.to_string())));
    }
    // The un-margined driver body parses as ordinary code.
    assert!(toks.contains(&(SyntaxKind::CONTROL_WORD, "\\documentclass".to_string())));
    // The `macrocode` body lexes as code under the package regime (`@` a letter).
    assert_eq!(count(&root, SyntaxKind::ENVIRONMENT), 1);
    assert!(toks.contains(&(SyntaxKind::CONTROL_WORD, "\\bar@baz".to_string())));
    assert_eq!(count_token(&root, SyntaxKind::VERBATIM_BODY), 0);
}

/// Parse under the docstrip config and return the diagnostics too.
fn parse_dtx_with_errors(input: &str) -> (SyntaxNode, usize) {
    let config = LexConfig {
        flavor: LatexFlavor::Document,
        dtx: true,
    };
    let parsed = parse_with_flavor(input, config);
    let root = parsed.syntax();
    assert_eq!(
        root.to_string(),
        input,
        "losslessness violated for {input:?}"
    );
    (root, parsed.errors.len())
}

#[test]
fn definition_split_across_macrocode_chunks_parses_cleanly() {
    // A `macrocode` chunk is macro code (issue #57): a `\def` regularly opens a
    // brace in one chunk and closes it several chunks later. The chunk-unmatched
    // braces are plain tokens — no `GROUP`, no diagnostics — and every frame
    // still pairs.
    let input = "\
% \\begin{macro}{\\foo}\n\
%    \\begin{macrocode}\n\
\\def\\foo#1{%\n\
%    \\end{macrocode}\n\
% some docs\n\
%    \\begin{macrocode}\n\
  bar}\n\
%    \\end{macrocode}\n\
% \\end{macro}\n";
    let (root, errors) = parse_dtx_with_errors(input);
    assert_eq!(errors, 0, "chunk-split braces must not diagnose");
    // The `macro` doc environment and both `macrocode` chunks all pair.
    assert_eq!(count(&root, SyntaxKind::ENVIRONMENT), 3);
    // The unmatched braces are plain tokens, not `GROUP`s (the only group is
    // the `{\foo}` argument of `\begin{macro}`).
    assert_eq!(count(&root, SyntaxKind::GROUP), 1);
}

#[test]
fn doc_layer_environments_pair_across_a_stranded_brace() {
    // longtable.dtx: a doc line leaves a brace open on purpose (here the
    // `` \char`} `` constant hides the closer). The doc layer is exempt from
    // the group gate (issue #71), so `\begin{macro}` still pairs with its
    // `\end{macro}` instead of being demoted to a plain command — which would
    // unnest every doc environment behind it.
    let input = "\
% \\def\\v{\\char`}\n\
%\n\
% \\begin{macro}{\\foo}\n\
%    \\begin{macrocode}\n\
\\def\\foo{1}\n\
%    \\end{macrocode}\n\
% \\end{macro}\n";
    let (root, _) = parse_dtx_with_errors(input);
    // The stranded brace itself still diagnoses; what matters here is that the
    // doc structure behind it survives — `macro` and `macrocode` both pair.
    assert_eq!(count(&root, SyntaxKind::ENVIRONMENT), 2);
}

#[test]
fn end_frame_with_trailing_comment_still_terminates_macrocode() {
    // doc.sty's terminator is a delimited match on `%    \end{macrocode}`, so a
    // trailing `%` on the frame line (a guard against a stray trailing space —
    // matze/mtheme, issue #62) still ends the chunk. Without this, the body
    // swallows the doc layer's `% \end{macro}` and the `macro` environment
    // diagnoses as unclosed.
    let input = "\
% \\begin{macro}{\\foo}\n\
%    \\begin{macrocode}\n\
\\newcommand{\\foo}{bar}\n\
%    \\end{macrocode}%\n\
% \\end{macro}\n";
    let (root, errors) = parse_dtx_with_errors(input);
    assert_eq!(errors, 0, "trailing `%` on the end frame must not diagnose");
    // The `macro` doc environment and the `macrocode` chunk both pair.
    assert_eq!(count(&root, SyntaxKind::ENVIRONMENT), 2);
    // The tail lexes as an ordinary comment, not part of the frame.
    assert!(tokens(&root).contains(&(SyntaxKind::COMMENT, "%".to_string())));
}

#[test]
fn begin_frame_with_trailing_comment_is_not_a_frame() {
    // A begin frame stays strict: same-line text after `\begin{macrocode}` is
    // captured into the body by `\xmacro@code`, so a `%` tail means the line is
    // not the documented frame shape. The next line then lexes in the doc
    // layer's Document regime (`@` not a letter), not as macrocode body code.
    let root = parse_dtx("%    \\begin{macrocode}%\n\\a@b\n");
    assert!(
        !tokens(&root)
            .iter()
            .any(|(k, t)| *k == SyntaxKind::CONTROL_WORD && t == "\\a@b")
    );
}

#[test]
fn bare_end_primitive_in_macrocode_is_a_plain_command() {
    // Kernel code uses the `\end` TeX primitive inside `macrocode`; only the
    // margin-framed `\end{macrocode}` line terminates the chunk.
    let input = "\
%    \\begin{macrocode}\n\
\\def\\stop@here{\\end}\n\
\\let\\@@end\\end\n\
%    \\end{macrocode}\n";
    let (root, errors) = parse_dtx_with_errors(input);
    assert_eq!(errors, 0, "a bare \\end primitive must not diagnose");
    assert_eq!(count(&root, SyntaxKind::ENVIRONMENT), 1);
}

#[test]
fn macrocode_frame_begin_attaches_no_arguments() {
    // The frame line holds nothing but the name, so the next code line's `{`
    // is body macro code — never an argument of `\begin{macrocode}`.
    let input = "\
%    \\begin{macrocode}\n\
    {\\parindent \\z@ \\raggedright\n\
     \\normalfont\n\
%    \\end{macrocode}\n";
    let (root, errors) = parse_dtx_with_errors(input);
    assert_eq!(errors, 0);
    let begin = root
        .descendants()
        .find(|n| n.kind() == SyntaxKind::BEGIN)
        .expect("begin");
    assert_eq!(
        begin
            .children()
            .filter(|c| c.kind() == SyntaxKind::GROUP)
            .count(),
        0,
        "the frame `\\begin` must not steal the body's brace as an argument"
    );
}

#[test]
fn short_verb_captures_in_the_doc_layer_but_not_in_macrocode() {
    // `.dtx` documentation is typeset under ltxdoc, where `|…|` is a short verb
    // (`\MakeShortVerb{\|}`), so `|$|` is an opaque VERB — while inside a
    // `macrocode` body `|` is an ordinary catcode-12 character.
    let (root, errors) = parse_dtx_with_errors(
        "% Note that |$| just produces a dollar sign.\n\
         %    \\begin{macrocode}\n\
         \\def\\pipe{|}\n\
         %    \\end{macrocode}\n",
    );
    assert_eq!(errors, 0, "the `$` inside a short verb must not open math");
    let toks = tokens(&root);
    assert!(toks.contains(&(SyntaxKind::VERB, "|$|".to_string())));
    // The code-layer `|` stays a plain word token.
    assert!(toks.contains(&(SyntaxKind::WORD, "|".to_string())));
}

#[test]
fn control_symbol_swallowing_its_newline_keeps_the_next_margin() {
    // `… \LaTeX\` at end of line lexes the backslash-newline as one control
    // symbol; the next line still begins in column 0, so its `%` is a margin
    // (not a comment that would swallow the closing braces).
    let (root, errors) = parse_dtx_with_errors(
        "% \\title{Hooks\\thanks{version \\LaTeX\\\n\
         %    Project.}}\n",
    );
    assert_eq!(errors, 0);
    assert_eq!(count_token(&root, SyntaxKind::DOC_MARGIN), 2);
    assert_eq!(count_token(&root, SyntaxKind::COMMENT), 0);
}

#[test]
fn caret_caret_a_is_a_comment_on_doc_lines() {
    // ltxdoc/l3doc set `\catcode`\^^A=14`, and the l3 sources lean on it for
    // editor-balance hacks in doc-margin prose (issue #60): `^^A{` hides a `{`
    // whose visual pair is inside a verb span, and `^^A\end{function}` comments
    // an `\end` out. On a doc line, `^^A` is a comment to end of line.
    let (root, errors) = parse_dtx_with_errors(
        "% namely a ^^A{\n\
         % |}| if standard category codes apply.\n\
         % \\begin{function}{\\foo}\n\
         %   body\n\
         % ^^A\\end{function}\n\
         % \\end{function}\n",
    );
    assert_eq!(errors, 0, "the hidden `{{` and `\\end` must not diagnose");
    let toks = tokens(&root);
    assert!(toks.contains(&(SyntaxKind::COMMENT, "^^A{".to_string())));
    assert!(toks.contains(&(SyntaxKind::COMMENT, "^^A\\end{function}".to_string())));
    // Only the real `\end{function}` closes the environment.
    assert_eq!(count(&root, SyntaxKind::ENVIRONMENT), 1);
}

#[test]
fn caret_caret_a_in_macrocode_is_live_code() {
    // Inside a `macrocode` body `^^A` is live code — `\char_set_catcode:nn
    // { `\^^A } { 14 }` must not have its line (including the real closing
    // braces) swallowed as a comment.
    let (root, errors) = parse_dtx_with_errors(
        "%    \\begin{macrocode}\n\
         \\char_set_catcode:nn { `\\^^A } { 14 }\n\
         %    \\end{macrocode}\n",
    );
    assert_eq!(errors, 0);
    let toks = tokens(&root);
    assert!(
        !toks
            .iter()
            .any(|(k, t)| *k == SyntaxKind::COMMENT && t.contains("^^A")),
        "macrocode `^^A` must stay code, got {toks:?}"
    );
}

#[test]
fn delimited_macro_name_argument_captures_as_verb() {
    // l3doc's `macro`/`function`/`variable` take a `v`-type name argument
    // (`{ O{} +v }`), and upstream uses the delimited form precisely when the
    // name holds unbalanced braces (`\begin{macro}+\@@_compile_{:+`, issue
    // #60). The span captures as one opaque `VERB` attached to the `BEGIN`.
    let (root, errors) = parse_dtx_with_errors(
        "% \\begin{macro}+\\@@_compile_{:+\n\
         %   doc prose\n\
         % \\end{macro}\n\
         % \\begin{macro}+\\@@_compile_}:+\n\
         %   doc prose\n\
         % \\end{macro}\n",
    );
    assert_eq!(
        errors, 0,
        "the unbalanced brace in the name must not diagnose"
    );
    assert_eq!(count(&root, SyntaxKind::ENVIRONMENT), 2);
    let toks = tokens(&root);
    assert!(toks.contains(&(SyntaxKind::VERB, "+\\@@_compile_{:+".to_string())));
    assert!(toks.contains(&(SyntaxKind::VERB, "+\\@@_compile_}:+".to_string())));
    // The captured argument attaches into the `BEGIN` node like any verbatim
    // command argument, so the environment body starts after it.
    let begin = root
        .descendants()
        .find(|n| n.kind() == SyntaxKind::BEGIN)
        .expect("a BEGIN node");
    assert!(
        begin
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == SyntaxKind::VERB),
        "the VERB argument belongs to the BEGIN node"
    );
}

#[test]
fn braced_macro_name_argument_captures_content_as_verb() {
    // The braced form of the `v`-type argument keeps its ordinary `GROUP`
    // shape (the dtx outline reads it off the `BEGIN`), but the content
    // between the real brace tokens is one opaque `VERB` — a v-arg is raw
    // data in either form (issue #60).
    let (root, errors) = parse_dtx_with_errors(
        "% \\begin{macro}{\\foo}\n\
         %   doc prose\n\
         % \\end{macro}\n",
    );
    assert_eq!(errors, 0);
    assert_eq!(count(&root, SyntaxKind::GROUP), 1);
    assert!(tokens(&root).contains(&(SyntaxKind::VERB, "\\foo".to_string())));
}

#[test]
fn braced_v_arg_name_holding_a_closer_is_no_orphan() {
    // xo-grid.dtx documents `\[`/`\]` themselves (issue #60): the braced
    // v-type name is raw data, so the closer draws no orphan diagnostic and
    // both `macro` environments still pair.
    let (root, errors) = parse_dtx_with_errors(
        "% \\begin{macro}{\\[}\n\
         % \\begin{macro}{\\]}\n\
         %   doc prose\n\
         % \\end{macro}\n\
         % \\end{macro}\n",
    );
    assert_eq!(errors, 0);
    let toks = tokens(&root);
    assert!(toks.contains(&(SyntaxKind::VERB, "\\[".to_string())));
    assert!(toks.contains(&(SyntaxKind::VERB, "\\]".to_string())));
    assert_eq!(count(&root, SyntaxKind::ENVIRONMENT), 2);
}

#[test]
fn orphan_closer_in_macrocode_is_plain_data() {
    // `\char_set_catcode_letter:N \)` in a macrocode chunk (l3bigint.dtx,
    // issue #60): in macro code an orphan `\)`/`\]` is data — an ordinary
    // token, no diagnostic.
    let (_root, errors) = parse_dtx_with_errors(
        "%    \\begin{macrocode}\n\
         \\char_set_catcode_letter:N \\)\n\
         %    \\end{macrocode}\n",
    );
    assert_eq!(errors, 0);
}

#[test]
fn doc_margin_envs_still_pair_inside_a_spanning_expl_region() {
    // An expl3 region regularly spans macrocode chunks (`\ExplSyntaxOn` in
    // one chunk, the `Off` chunks later). The doc-layer markup between them
    // — `\begin{macro}` prose and the frames themselves — sits on doc-margin
    // lines and must keep pairing; only in-region *code* is macro code.
    let (root, errors) = parse_dtx_with_errors(
        "%    \\begin{macrocode}\n\
         \\ExplSyntaxOn\n\
         %    \\end{macrocode}\n\
         % \\begin{macro}{\\foo}\n\
         %   doc prose\n\
         % \\end{macro}\n\
         %    \\begin{macrocode}\n\
         \\ExplSyntaxOff\n\
         %    \\end{macrocode}\n",
    );
    assert_eq!(errors, 0);
    // Two macrocode chunks plus the doc-layer `macro` environment.
    assert_eq!(count(&root, SyntaxKind::ENVIRONMENT), 3);
}

#[test]
fn guard_only_line_does_not_part_an_optional_argument() {
    // A `%<*dtx>`/`%</dtx>` block-guard line is content, and one docstrip
    // *deletes* when it strips the file — so it is not the blank line its bare
    // `NEWLINE GUARD NEWLINE` shape resembles. rotating.dtx splits
    // `\ProvidesPackage{rotating}`'s `[…date…]` optional across such guards;
    // reading them as a paragraph break bailed out of the `[` mid-argument
    // (issue #71).
    let (_root, errors) = parse_dtx_with_errors(
        "%    \\begin{macrocode}\n\
         \\ProvidesPackage{rotating}%\n    \
         [2026-05-17 v2.16e\n\
         %<*dtx>\n            \
         rotating package source file%\n\
         %</dtx>\n        \
         ]\n\
         %    \\end{macrocode}\n",
    );
    assert_eq!(errors, 0);
}

#[test]
fn short_verb_char_after_string_is_literal() {
    // `\string|` prints the bar: `\string` takes the next token unexpanded, so
    // the active short-verb `|` is data, not a capture opener. Capturing there
    // ran the span to the *next* `|` and swallowed the intervening braces,
    // stranding `\meta{`/`\texttt{` and unnesting the doc environment around it
    // (lthooks.dtx, issue #71).
    let (root, errors) = parse_dtx_with_errors(
        "% \\begin{quote}\n\
         %   \\verb|x |\\meta{a\\texttt{\\string|}b}\\verb|y|\n\
         % \\end{quote}\n",
    );
    assert_eq!(errors, 0);
    assert_eq!(count(&root, SyntaxKind::ENVIRONMENT), 1);
}

#[test]
fn an_indented_macrocode_begin_frame_opens_a_chunk() {
    // `\DocInput` runs the documentation part under `\MakePercentIgnore`
    // (`\catcode`\%=9`, doc.dtx), so a `%` there is an *ignored* character at any
    // column: an indented `␣␣%␣␣␣␣\begin{macrocode}` opens a chunk exactly like
    // the column-0 spelling (multicol.dtx, latex-lab-block.dtx — issue #71).
    // Lexed as a comment instead, the frame vanished and its `\end{macrocode}`
    // unwound the whole doc layer behind it.
    let (root, errors) = parse_dtx_with_errors(
        "% \\begin{macro}{\\a}\n\
         \x20 %    \\begin{macrocode}\n\
         \\def\\a{1}\n\
         %    \\end{macrocode}\n\
         % \\end{macro}\n",
    );
    assert_eq!(errors, 0);
    // The doc-layer `macro` and the `macrocode` chunk, properly nested.
    assert_eq!(count(&root, SyntaxKind::ENVIRONMENT), 2);
}

#[test]
fn an_indented_macrocode_end_frame_stays_column_zero() {
    // The *end* frame is not symmetric: inside the body `%` is a comment again,
    // and doc.sty terminates the chunk on a delimited match against the literal
    // `%    \end{macrocode}` line. An indented one is body text, so the chunk
    // runs on — and the `\end{macro}` after it is code, not doc structure.
    let (_root, errors) = parse_dtx_with_errors(
        "% \\begin{macro}{\\a}\n\
         %    \\begin{macrocode}\n\
         \\def\\a{1}\n\
         \x20 %    \\end{macrocode}\n\
         % \\end{macro}\n",
    );
    assert!(errors > 0, "an indented end frame must not close the chunk");
}

#[test]
fn a_doc_line_group_gates_a_split_environment_definition() {
    // theorem.dtx (issue #71): the documentation layer writes the same split
    // environment definition the code layer does. The `\begin{list}` sits inside
    // a brace group the doc line itself opened, so the group-boundary gate
    // applies — the `.dtx` doc-margin exemption covers braces the *code* layer
    // stranded, not ones the documentation opened in plain sight.
    let (root, errors) = parse_dtx_with_errors(
        "% \\def\\deflist#1{\\begin{list}{}{\\let\\makelabel\\deflabel}}\n\
         % \\def\\enddeflist{\\end{list}}\n",
    );
    assert_eq!(errors, 0);
    assert_eq!(count(&root, SyntaxKind::ENVIRONMENT), 0);
}

#[test]
fn a_doc_environment_still_pairs_across_macrocode_chunks() {
    // The exemption itself is intact: a doc-layer `\begin{macro}` with no
    // doc-line group around it keeps pairing across the chunks between its
    // halves, even when a chunk leaves a brace open on purpose.
    let (root, errors) = parse_dtx_with_errors(
        "% \\begin{macro}{\\a}\n\
         %    \\begin{macrocode}\n\
         \\def\\a#1{%\n\
         %    \\end{macrocode}\n\
         %    Continued in the next chunk.\n\
         %    \\begin{macrocode}\n\
         \x20 #1}\n\
         %    \\end{macrocode}\n\
         % \\end{macro}\n",
    );
    assert_eq!(errors, 0);
    assert_eq!(
        count(&root, SyntaxKind::ENVIRONMENT),
        3,
        "the doc `macro` plus both `macrocode` chunks"
    );
}

#[test]
fn oldcomments_is_an_opaque_ltxdoc_environment() {
    // `oldcomments` (ltxdoc) wraps historical LaTeX 2.09 sources and typesets
    // them verbatim-ish. It is defined inside a catcode-swapped region, so no
    // static scan can learn it — the curated signature database is the only place
    // the fact can live. Its body's unbalanced braces and prose `\end{document}`
    // are data (ltmiscen.dtx / lttab.dtx / ltfloat.dtx, issue #71).
    let (root, errors) = parse_dtx_with_errors(
        "% \\begin{oldcomments}\n\
         %   \\hbox { \\unhbox\\@currfield  %%} brace matching\n\
         %   \\end{document} will catch a still-open environment.\n\
         % \\end{oldcomments}\n",
    );
    assert_eq!(errors, 0);
    assert_eq!(count(&root, SyntaxKind::ENVIRONMENT), 1);
}

#[test]
fn implicit_expl_module_guard_macrocode_body_is_expl3() {
    // A toggle-less `.dtx` (`ltx-talk-structure.dtx`-style): expl3 is declared in
    // the parent build, and only a `%<@@=mod>` module guard signals it. The
    // macrocode body is therefore expl3 code, so `\seq_new:N` parses as one
    // control word rather than splitting at `_`/`:` (TODO.md).
    let (root, errors) = parse_dtx_with_errors(
        "%<@@=talk>\n\
         %    \\begin{macrocode}\n\
         \\seq_new:N \\l_talk_frames_seq\n\
         %    \\end{macrocode}\n",
    );
    assert_eq!(errors, 0);
    assert!(tokens(&root).contains(&(SyntaxKind::CONTROL_WORD, "\\seq_new:N".to_string())));
}

#[test]
fn implicit_expl_body_begin_as_data_pairs_nothing() {
    // The issue #60 shape, but *implicit*: an expl3 body passes `\begin{longtable}`
    // as data in an n-type argument with no `\end` in the body. Macrocode bodies
    // are already blanket-exempt from environment pairing (`in_def_body`), so this
    // is clean with no grammar change — the implicit-expl lexing only affects
    // `_`/`:` catcodes, never `\begin`/`\end` structure.
    let (root, errors) = parse_dtx_with_errors(
        "%<@@=mod>\n\
         %    \\begin{macrocode}\n\
         \\tl_set:Nn \\l_mod_tmp_tl { \\begin{longtable} }\n\
         %    \\end{macrocode}\n",
    );
    assert_eq!(errors, 0);
    // Only the `macrocode` frame pairs; the inner `\begin{longtable}` is data.
    assert_eq!(count(&root, SyntaxKind::ENVIRONMENT), 1);
    assert!(tokens(&root).contains(&(SyntaxKind::CONTROL_WORD, "\\tl_set:Nn".to_string())));
}

#[test]
fn implicit_expl_provides_expl_flags_body_before_declaration() {
    // `\ProvidesExplPackage` is a whole-file signal, so a `\cs_new:Npn` body split
    // across macrocode chunks — with an unmatched brace, exercising `plain_braces`
    // — lexes as expl3 even when the declaration sits after it.
    let (root, errors) = parse_dtx_with_errors(
        "%    \\begin{macrocode}\n\
         \\cs_new:Npn \\mod_foo:n #1 {\n\
         %    \\end{macrocode}\n\
         % \\ProvidesExplPackage{mod}{2026/01/01}{1.0}{demo}\n\
         %    \\begin{macrocode}\n\
         \\tl_set:Nn #1 }\n\
         %    \\end{macrocode}\n",
    );
    assert_eq!(errors, 0);
    let toks = tokens(&root);
    assert!(toks.contains(&(SyntaxKind::CONTROL_WORD, "\\cs_new:Npn".to_string())));
    assert!(toks.contains(&(SyntaxKind::CONTROL_WORD, "\\tl_set:Nn".to_string())));
}
