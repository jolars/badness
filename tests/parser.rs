//! Phase 1 parser tests: tree-shape snapshots over representative inputs, plus
//! targeted assertions on error-recovery behaviour. Every case also re-checks
//! the losslessness invariant. Regenerate snapshots with `task snapshots`.

use badness::parser::parse;
use badness::syntax::SyntaxNode;
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
    let parsed = parse("\\begin{a}\\code{$ x}\\end{a}");
    assert_eq!(parsed.syntax().to_string(), "\\begin{a}\\code{$ x}\\end{a}");
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
