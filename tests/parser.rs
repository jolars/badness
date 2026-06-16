//! Phase 1 parser tests: tree-shape snapshots over representative inputs, plus
//! targeted assertions on error-recovery behaviour. Every case also re-checks
//! the losslessness invariant. Regenerate snapshots with `task snapshots`.

use badness::parser::parse;
use badness::syntax::{SyntaxKind, SyntaxNode};
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

#[test]
fn command_with_required_and_optional_args() {
    insta::assert_snapshot!(tree(r"\cmd[opt]{req}"));
}

#[test]
fn nested_groups() {
    insta::assert_snapshot!(tree(r"{a {b} c}"));
}

#[test]
fn environment_with_body() {
    insta::assert_snapshot!(tree("\\begin{itemize}\n\\item x\n\\end{itemize}"));
}

#[test]
fn inline_and_display_math() {
    insta::assert_snapshot!(tree(r"$x^2$ and \[ y_i \]"));
}

#[test]
fn display_math_dollars() {
    insta::assert_snapshot!(tree(r"$$a + b$$"));
}

#[test]
fn math_scripts_bind_to_base() {
    // Sub/superscripts in either order, a bare-group base, a command script
    // argument, and a nested script inside a `{…}` group. Atoms are separated by
    // `\,` (a control symbol that takes no arguments, so it does not greedily
    // swallow the following group the way a control word would).
    insta::assert_snapshot!(tree(
        r"$x^{n+1} \, a_i^2 \, x^2_i \, {a+b}^2 \, x^\alpha \, x^{a_b}$"
    ));
}

#[test]
fn left_right_pair() {
    // `\left( … \right)`: the `\left`/`\right` and their delimiter tokens are
    // direct children, the enclosed atoms wrapped in a `MATH` body.
    insta::assert_snapshot!(tree(r"$\left( x + y \right)$"));
}

#[test]
fn left_right_nested_and_scripted() {
    // Nested pairs recurse, and a script after `\right)` binds to the whole pair
    // (the `SCRIPTED` wraps the `LEFT_RIGHT`). The inner `\left[`/`\right]` is a
    // separate pair.
    insta::assert_snapshot!(tree(r"$\left[ \left( a \right) \right]^2$"));
}

#[test]
fn left_right_control_word_delimiters() {
    // A control-word delimiter (`\langle`/`\rangle`) is the delimiter token; a
    // control-symbol one (`\|`) likewise.
    insta::assert_snapshot!(tree(r"$\left\langle x \right\rangle$"));
}

#[test]
fn unclosed_left_recovers() {
    // `\left(` with no `\right` before the closing `$`: an unclosed-`\left` error,
    // the `$` handed back to close the math, and nothing corrupted.
    let parsed = parse(r"$\left( x $");
    assert_eq!(parsed.syntax().to_string(), r"$\left( x $");
    let messages: Vec<&str> = parsed.errors.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(messages, ["unclosed `\\left`"]);
}

#[test]
fn stray_right_reports() {
    // A `\right)` with no open `\left`: reported, consumed with its delimiter,
    // still lossless.
    let parsed = parse(r"$x \right) y$");
    assert_eq!(parsed.syntax().to_string(), r"$x \right) y$");
    let messages: Vec<&str> = parsed.errors.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(messages, ["`\\right` without matching `\\left`"]);
}

#[test]
fn left_missing_delimiter_recovers() {
    // `\left` immediately followed by `\right` (no delimiters): one error per
    // missing delimiter, lossless.
    let parsed = parse(r"$\left \right$");
    assert_eq!(parsed.syntax().to_string(), r"$\left \right$");
    let messages: Vec<&str> = parsed.errors.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "missing delimiter after `\\left`",
            "missing delimiter after `\\right`"
        ]
    );
}

#[test]
fn math_script_with_no_base() {
    // A leading `^` has no base atom: the `^` is consumed as a bare token and `2`
    // as the next atom — no SCRIPTED wrapper (the `^` has nothing to bind to).
    insta::assert_snapshot!(tree(r"$^2$"));
}

#[test]
fn math_script_missing_argument_recovers() {
    // `^` with no argument before the closing `$`: one recovery error, lossless.
    let parsed = parse(r"$x^$");
    assert_eq!(parsed.syntax().to_string(), r"$x^$");
    let messages: Vec<&str> = parsed.errors.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(messages, ["missing argument after `^`/`_`"]);
}

#[test]
fn math_script_missing_argument_at_eof_recovers() {
    // `^` at end of input inside unclosed math: a missing-argument error and an
    // unclosed-math error, and nothing is corrupted.
    let parsed = parse(r"$x^");
    assert_eq!(parsed.syntax().to_string(), r"$x^");
    assert!(
        parsed
            .errors
            .iter()
            .any(|e| e.message == "missing argument after `^`/`_`"),
        "missing-argument is reported"
    );
    assert!(
        parsed.errors.iter().any(|e| e.message == "unclosed `$`"),
        "unclosed math is reported"
    );
}

#[test]
fn paragraphs_split_on_blank_lines() {
    insta::assert_snapshot!(tree("First line,\nsame paragraph.\n\nSecond paragraph."));
}

#[test]
fn comment_line_does_not_split_paragraph() {
    // `\n %comment \n` is two line-ends around a comment-only line, not a
    // blank line, so it must stay one paragraph (not a `\par` boundary).
    let out = tree("First line.\n% an aside\nSame paragraph.");
    assert_eq!(out.matches("PARAGRAPH@").count(), 1, "{out}");
}

#[test]
fn comment_line_does_not_close_display_math() {
    // A comment line inside `\[ … \]` previously read as a blank line and
    // closed the math early, orphaning the `\]`. It must parse as one block.
    let out = tree("\\[\n  a = b\n  % aligned variant, commented out\n  + c\n\\]");
    assert!(!out.contains("error @"), "unexpected parse error:\n{out}");
    assert_eq!(out.matches("DISPLAY_MATH@").count(), 1, "{out}");
}

#[test]
fn blank_line_before_comment_still_breaks_math() {
    // A genuine blank line preceding a comment line is still a `\par`: the
    // comment-reset must not erase a break already seen.
    let out = tree("\\[\n  a = b\n\n  % stray\n  c\n\\]");
    assert!(out.contains("unclosed `\\["), "{out}");
}

#[test]
fn verbatim_environment_is_opaque() {
    insta::assert_snapshot!(tree(
        "\\begin{verbatim}\n\\notacommand $x$ %literal\n\\end{verbatim}"
    ));
}

#[test]
fn inline_verb_is_a_single_token() {
    insta::assert_snapshot!(tree(r"text \verb|$x$| more"));
}

#[test]
fn brace_verbatim_command_is_opaque() {
    // `\code`'s brace argument is verbatim (jss `\@makeother\$`): the `$` is a
    // literal, not math, so no "unclosed `$`" and the body is one VERB token.
    let out = tree(r"\code{$ pip install x_y}");
    assert!(!out.contains("error @"), "{out}");
    assert!(
        !out.contains("DOLLAR@") && !out.contains("INLINE_MATH@"),
        "{out}"
    );
    assert!(out.contains(r#"VERB@5..24 "{$ pip install x_y}""#), "{out}");
}

#[test]
fn delimiter_verbatim_command_is_opaque() {
    insta::assert_snapshot!(tree(r"\lstinline|x_$y$|"));
}

#[test]
fn verbatim_command_skips_leading_args() {
    // `\mintinline{lang}{code}`: the language is an ordinary group, only the
    // trailing argument is verbatim.
    let out = tree(r"\mintinline{python}{x = $1}");
    assert!(!out.contains("error @"), "{out}");
    assert!(out.contains(r#"VERB@19..27 "{x = $1}""#), "{out}");
}

#[test]
fn lstlisting_optional_arg_then_opaque_body() {
    insta::assert_snapshot!(tree(
        "\\begin{lstlisting}[language=Python]\nif x: pass  # $not math$\n\\end{lstlisting}"
    ));
}

#[test]
fn minted_required_arg_then_opaque_body() {
    insta::assert_snapshot!(tree(
        "\\begin{minted}{python}\nprint(\"%not a comment\")\n\\end{minted}"
    ));
}

#[test]
fn minted_optional_and_required_args() {
    insta::assert_snapshot!(tree(
        "\\begin{minted}[frame=single]{python}\ncode\n\\end{minted}"
    ));
}

#[test]
fn verbatim_capital_optional_arg() {
    insta::assert_snapshot!(tree(
        "\\begin{Verbatim}[fontsize=\\small]\nraw  text\n\\end{Verbatim}"
    ));
}

/// An option-free `lstlisting` whose body's first line *is* a bracketed list: the
/// signature has one optional arg, but it sits on the next line, so the `[1,2,3]`
/// belongs to the opaque body, not to an `OPTIONAL` argument node.
#[test]
fn lstlisting_body_starting_with_bracket_is_not_an_argument() {
    insta::assert_snapshot!(tree("\\begin{lstlisting}\n[1,2,3]\n\\end{lstlisting}"));
}

#[test]
fn makeatletter_control_word_with_at() {
    insta::assert_snapshot!(tree(r"\makeatletter\foo@bar\makeatother"));
}

#[test]
fn line_break_groups_star_and_optional_length() {
    // `\\`, `\\*`, `\\[2ex]`, and `\\*[2ex]` each parse to one `LINE_BREAK` node
    // with the `*` / `[len]` bound in; a plain `\\` (here at the end) stays bare.
    insta::assert_snapshot!(tree(r"a \\ b \\* c \\[2ex] d \\*[2ex] e \\"));
}

#[test]
fn line_break_does_not_cross_trivia_for_its_optional() {
    // A `\\` followed by whitespace then `[x]` does NOT absorb the bracket — the
    // modifiers bind only when they directly abut, so a `\\` ending a line stays
    // bare and nothing is pulled across the break.
    insta::assert_snapshot!(tree("row \\\\\n[x] next"));
}

// --- error recovery ------------------------------------------------------

#[test]
fn environment_mismatch_recovers() {
    insta::assert_snapshot!(tree(r"\begin{a}\begin{b}\end{a}"));
}

#[test]
fn unmatched_closing_brace() {
    insta::assert_snapshot!(tree("a } b"));
}

#[test]
fn unclosed_environment_at_eof() {
    insta::assert_snapshot!(tree(r"\begin{proof} text"));
}

#[test]
fn stray_end_at_top_level() {
    let parsed = parse(r"\end{itemize}");
    assert_eq!(parsed.errors.len(), 1);
    assert!(parsed.errors[0].message.contains("without matching"));
    assert_eq!(parsed.syntax().to_string(), r"\end{itemize}");
}

#[test]
fn unclosed_dollar_math_in_group_does_not_escape() {
    // `$`-math cannot span the enclosing group's `}`: the brace closes the
    // group, the math reports a single "unclosed `$`", and nothing downstream
    // is corrupted (no spurious "unmatched `}`" / "unclosed environment").
    // `\foo` is an ordinary (non-verbatim) command, so its argument is real
    // math — contrast `\code`, whose argument is captured verbatim.
    let parsed = parse("\\begin{a}\\foo{$ x}\\end{a}");
    assert_eq!(parsed.syntax().to_string(), "\\begin{a}\\foo{$ x}\\end{a}");
    let messages: Vec<&str> = parsed.errors.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(messages, ["unclosed `$`"], "only the open math is reported");
}

#[test]
fn nested_mismatch_unwinds_to_two_errors() {
    // `b` is closed by the mismatch, `a` matches: exactly one "unclosed" error.
    let parsed = parse(r"\begin{a}\begin{b}\end{a}");
    let unclosed = parsed
        .errors
        .iter()
        .filter(|e| e.message.contains("unclosed environment"))
        .count();
    assert_eq!(unclosed, 1, "only `b` is unclosed; `a` matches");
}

// --- block-vs-inline paragraph wrapping --------------------------------------

/// The kinds of the root's direct child *nodes* (trivia tokens are skipped, as
/// `SyntaxNode::children` yields only nodes). Used to assert whether a run was
/// wrapped in a `PARAGRAPH` or left as a bare block.
fn root_node_kinds(input: &str) -> Vec<SyntaxKind> {
    // Losslessness must hold for every input.
    let parsed = parse(input);
    assert_eq!(
        parsed.syntax().to_string(),
        input,
        "losslessness violated for {input:?}"
    );
    parsed.syntax().children().map(|n| n.kind()).collect()
}

#[test]
fn lone_block_environment_is_not_wrapped() {
    // A `figure` is a block env (signature DB), so it sits bare under ROOT —
    // no redundant PARAGRAPH. Surrounding single newlines ride as direct
    // children, preserving losslessness.
    insta::assert_snapshot!(tree("\\begin{figure}\nx\n\\end{figure}"));
    assert_eq!(
        root_node_kinds("\\begin{figure}\nx\n\\end{figure}"),
        [SyntaxKind::ENVIRONMENT]
    );
}

#[test]
fn block_environment_with_trailing_text_stays_wrapped() {
    // Not a *lone* env: trailing text makes the run ordinary prose, so the
    // PARAGRAPH wrapper is retained.
    assert_eq!(
        root_node_kinds(r"\begin{center}x\end{center} y"),
        [SyntaxKind::PARAGRAPH]
    );
}

#[test]
fn text_before_block_environment_stays_wrapped() {
    assert_eq!(
        root_node_kinds(r"see \begin{center}x\end{center}"),
        [SyntaxKind::PARAGRAPH]
    );
}

#[test]
fn nested_lone_block_env_drops_inner_paragraph() {
    // The figure body's lone `center` is also left unwrapped.
    insta::assert_snapshot!(tree(
        "\\begin{figure}\n\\begin{center}\nx\n\\end{center}\n\\end{figure}"
    ));
}

#[test]
fn lone_unknown_environment_stays_wrapped() {
    // User/unknown environments are not in the built-in DB, so they are never
    // treated as block: the conservative PARAGRAPH wrapper is kept.
    assert_eq!(
        root_node_kinds("\\begin{myenv}\nx\n\\end{myenv}"),
        [SyntaxKind::PARAGRAPH]
    );
}
