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
fn math_script_missing_argument_at_math_end_recovers() {
    // `^` right before the closing `$`: a missing-argument error, and nothing
    // is corrupted. (A bare `$x^` at EOF no longer opens math at all — the
    // `$` shape gate keeps a closer-less dollar plain — so the closing `$` is
    // what routes this through math recovery.)
    let parsed = parse(r"$x^$");
    assert_eq!(parsed.syntax().to_string(), r"$x^$");
    assert!(
        parsed
            .errors
            .iter()
            .any(|e| e.message == "missing argument after `^`/`_`"),
        "missing-argument is reported"
    );
}

#[test]
fn math_open_interval_bracket_stays_plain() {
    // French open-interval notation (#23): the `[` after `\num{0.5}` has no `]`
    // before the math ends, so it is an ordinary math atom, not the start of an
    // optional argument — no errors, and the closing `$` is not swallowed.
    insta::assert_snapshot!(tree(r"$]0;\num{0.5}[$"));
}

#[test]
fn math_optional_attaches_when_closed() {
    // A `[` with its `]` inside the math still attaches as an optional argument
    // (the interval gate must not over-fire).
    insta::assert_snapshot!(tree(r"$\sqrt[3]{x}$"));
}

#[test]
fn math_spaced_bracket_stays_content() {
    // #43: inside math a `[` separated from its command by whitespace is a
    // delimiter or interval, not an optional argument — `\bE [ x + y ]` keeps
    // the brackets as plain math atoms. (Real math optionals are written
    // tight: `\sqrt[3]{x}`.)
    insta::assert_snapshot!(tree(r"$\bE [ x + y ] .$"));
}

#[test]
fn big_delimiter_bracket_is_never_an_optional() {
    // #43: the delimiter-size commands size the delimiter that follows, so
    // even a tight `\Big[ … \Big]` never attaches the bracket as an optional
    // argument.
    insta::assert_snapshot!(tree(r"$\Big[ x + y \Big]$"));
}

#[test]
fn spaced_bracket_in_text_body_nested_in_math_stays_content() {
    // #43: an unknown environment's body parses in text mode, but lexically it
    // still sits inside `\[ … \]` — the math gate (spaced `[` is content)
    // must keep applying there, or `\Big [ … ]` inside a user alignment
    // environment turns into a bogus optional argument.
    insta::assert_snapshot!(tree(
        "\\[ \\begin{myaligned}[t] \\bE \\Big [ x + y \\Big ] \\end{myaligned} \\]"
    ));
}

#[test]
fn optional_never_attaches_across_newline() {
    // #43: a next-line `[` is content, never an argument — `\begin{align}`
    // followed by a newline and `[\partial_\mu V]_1` must not pull the bracket
    // into the BEGIN and orphan the subscript.
    insta::assert_snapshot!(tree("\\begin{align}\n[a]_1 & = b\n\\end{align}"));
}

#[test]
fn text_spaced_optional_still_attaches() {
    // The math gate is math-only: in prose a same-line spaced `[…]` after a
    // command still attaches greedily (`\item [label]` stays an optional).
    insta::assert_snapshot!(tree(r"\item [label] text"));
}

#[test]
fn text_unreachable_bracket_stays_plain() {
    // #60: the text-mode bracket gate — a `[` with no reachable `]` (here EOF)
    // is data, not an argument: it stays an ordinary token, no OPTIONAL and no
    // diagnostic, mirroring the `$` shape gate.
    let parsed = parse(r"\cmd[oops");
    assert_eq!(parsed.syntax().to_string(), r"\cmd[oops");
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    insta::assert_snapshot!(tree(r"\cmd[oops"));
}

#[test]
fn ifnextchar_bracket_stays_plain() {
    // #60: `\@ifnextchar [\@xmpar\@ympar}` inside a `\def` body — the `[` is
    // the character being tested for, and its scan hits the unbalanced `}`
    // before any `]`, so it must not attach and swallow the group closer.
    let src = r"\def\marginpar{\begingroup \@ifnextchar [\@xmpar\@ympar}";
    let parsed = parse(src);
    assert_eq!(parsed.syntax().to_string(), src);
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    insta::assert_snapshot!(tree(src));
}

#[test]
fn paragraphs_split_on_blank_lines() {
    insta::assert_snapshot!(tree("First line,\nsame paragraph.\n\nSecond paragraph."));
}

#[test]
fn blank_line_in_dollar_math_keeps_the_dollars_plain() {
    // #35: a blank line inside would-be `$…$` in a tabular cell. A blank line
    // bounds the `$` shape gate's closer scan (as it bounds math in TeX), so
    // neither dollar opens math: both stay ordinary tokens, the cell and the
    // tabular parse intact, and nothing downstream is corrupted.
    let text = "\\begin{tabular}{c}\n  $a =\n\n  b$ \\\\\n\\end{tabular}\n";
    let parsed = parse(text);
    assert_eq!(parsed.syntax().to_string(), text);
    assert_eq!(
        parsed.errors,
        [],
        "gated dollars are plain tokens, not errors"
    );
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
    // comment-reset must not erase a break already seen, so the `\]` past it
    // is unreachable — the gated `\[` stays a plain token (no math, no
    // diagnostic) and the orphan `\]` diagnoses instead.
    let out = tree("\\[\n  a = b\n\n  % stray\n  c\n\\]");
    assert!(out.contains("unmatched `\\]`"), "{out}");
    assert!(!out.contains("DISPLAY_MATH"), "{out}");
}

// --- leading comment-bind (AGENTS.md #9) ---------------------------------

#[test]
fn comment_binds_leading_into_command() {
    // An own-line `%` run immediately before a command attaches *leading* into
    // the `COMMAND` node, before the control word.
    insta::assert_snapshot!(tree("% c\n\\section{A}"));
}

#[test]
fn comment_binds_leading_into_environment() {
    // The bound comment sits inside `ENVIRONMENT`, before `BEGIN`; a lone block
    // environment stays bare (no `PARAGRAPH` wrapper).
    insta::assert_snapshot!(tree("% caption\n\\begin{figure}\nx\n\\end{figure}"));
}

#[test]
fn comment_run_binds_as_a_whole() {
    // A contiguous run of own-line comments all bind into the construct.
    insta::assert_snapshot!(tree("% a\n% b\n\\foo"));
}

#[test]
fn trailing_same_line_comment_does_not_bind() {
    // `% x` shares `\foo`'s line (no newline before it), so it is a trailing
    // comment and floats — it does not bind into the following `\bar`.
    let out = tree("\\foo % x\n\\bar");
    // The comment is a direct child of the paragraph, not inside the second
    // COMMAND: there are two sibling COMMAND nodes and the COMMENT floats.
    assert_eq!(out.matches("COMMAND@").count(), 2, "{out}");
    insta::assert_snapshot!(out);
}

#[test]
fn blank_line_breaks_the_leading_bind() {
    // A blank line between the comment and the construct breaks the bind: the
    // comment floats at the enclosing level, the `\foo` starts a fresh paragraph.
    insta::assert_snapshot!(tree("% a\n\n\\foo"));
}

#[test]
fn comment_after_blank_line_still_binds() {
    // `%a` floats (blank line before `\foo`), but `%b` — with no blank line
    // between it and `\foo` — binds. The bind is the maximal blank-line-free
    // suffix.
    //
    // This is the deliberate divergence from RA's `n_attached_trivias`: RA would
    // peek past the blank line and attach `%a` too (it treats a trailing outer doc
    // comment `///`/`//!` as continuing across the gap). LaTeX's single catcode-14
    // `%` has no such intent marker, and attaching `%a` would wrongly glue a
    // license/copyright header into the construct, so we stop at the blank line.
    // See AGENTS.md #9 and `Parser::binding_run`.
    insta::assert_snapshot!(tree("%a\n\n%b\n\\foo"));
}

#[test]
fn comment_does_not_bind_into_non_documentable() {
    // Math, words, and other non-command/-environment tokens are not
    // documentable: the comment floats.
    let math = tree("% c\n$x$");
    assert!(
        !math.contains("COMMAND@") && math.contains("INLINE_MATH@"),
        "{math}"
    );
    let word = tree("% c\nword");
    assert!(!word.contains("COMMAND@"), "{word}");
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
fn braced_only_verbatim_command_without_brace_lexes_normally() {
    // The delimiter form is opt-in per signature. `\code` is braced-only (jss),
    // so a non-brace follower means this `\code` is some unrelated user macro —
    // here the HoTT book's math operator (issue #53). Capturing `:A+B\to\type$ …`
    // as a `:`-delimited verb run would swallow the closing `$` and cascade into
    // unbalanced-delimiter diagnostics.
    let out = tree(r"a family $\code:A+B\to\type$ and $\code(\inl(a))$ here");
    assert!(!out.contains("error @"), "{out}");
    assert!(!out.contains("VERB@"), "{out}");

    // TikZ's `\path (0,0) …` collides with the url package's braced-only `\path`
    // the same way.
    let out = tree(r"\path (0,0) -- (1,1);");
    assert!(!out.contains("error @"), "{out}");
    assert!(!out.contains("VERB@"), "{out}");
}

#[test]
fn delimiter_verbatim_command_is_opaque() {
    // The `VERB` body attaches *into* the command (a child, like any greedy
    // argument — decision #8), not as a stranded sibling.
    insta::assert_snapshot!(tree(r"\lstinline|x_$y$|"));
}

#[test]
fn brace_verbatim_command_argument_is_a_child() {
    // `\url{…}`: the brace-delimited verbatim body is the command's argument, so
    // it nests under the `COMMAND` node rather than floating beside it.
    insta::assert_snapshot!(tree(r"\url{a_$b$}"));
}

#[test]
fn verbatim_command_skips_leading_args() {
    // `\mintinline{lang}{code}`: the language is an ordinary group, only the
    // trailing argument is verbatim. Both the group and the `VERB` body nest
    // under the command, which therefore spans the whole construct.
    let out = tree(r"\mintinline{python}{x = $1}");
    assert!(!out.contains("error @"), "{out}");
    assert!(out.contains(r#"VERB@19..27 "{x = $1}""#), "{out}");
    assert!(out.contains("COMMAND@0..27"), "{out}");
}

#[test]
fn user_defined_verbatim_command_argument_is_opaque() {
    // A document that *defines* a catcode-othering command (`\@makeother\$`) makes
    // its call-site argument verbatim via the second parse pass. `\shellcmd` is not a
    // built-in verbatim command, so the VERB capture proves the definition scan, not
    // the built-in DB, did the work: `$`/`_` inside `{a_$b$}` stay literal.
    let out = tree("\\newcommand\\shellcmd[1]{\\@makeother\\$#1}\n\\shellcmd{a_$b$}\n");
    assert!(!out.contains("error @"), "{out}");
    assert!(
        !out.contains("DOLLAR@") && !out.contains("INLINE_MATH@"),
        "{out}"
    );
    assert!(
        out.contains("VERB@") && out.contains(r#""{a_$b$}""#),
        "{out}"
    );
}

#[test]
fn def_defined_verbatim_command_argument_is_opaque() {
    // The same two-pass protection extends to `\def`-defined commands: the parameter
    // text (`#1`) is scanned for arity and the body for the catcode signal, so the
    // call-site argument of `\shellcmd` is captured verbatim and `$`/`_` stay literal.
    let out = tree("\\def\\shellcmd#1{\\@makeother\\$#1}\n\\shellcmd{a_$b$}\n");
    assert!(!out.contains("error @"), "{out}");
    assert!(
        !out.contains("DOLLAR@") && !out.contains("INLINE_MATH@"),
        "{out}"
    );
    assert!(
        out.contains("VERB@") && out.contains(r#""{a_$b$}""#),
        "{out}"
    );
}

#[test]
fn user_defined_verbatim_environment_body_is_opaque() {
    // A document that defines a catcode-othering *environment* (`\@makeother\$` in its
    // begin-code) makes its `\begin…\end` body verbatim via the second parse pass.
    // `shellenv` is not a built-in verbatim environment, so the VERBATIM_BODY capture
    // proves the definition scan did the work: `$`/`_`/`%` inside stay literal.
    let out = tree(concat!(
        "\\newenvironment{shellenv}{\\@makeother\\$}{}\n",
        "\\begin{shellenv}\na_$b$ % literal\n\\end{shellenv}\n",
    ));
    assert!(!out.contains("error @"), "{out}");
    assert!(
        !out.contains("DOLLAR@") && !out.contains("INLINE_MATH@") && !out.contains("COMMENT@"),
        "{out}"
    );
    assert!(out.contains("VERBATIM_BODY@"), "{out}");
}

#[test]
fn undefined_command_argument_is_not_verbatim() {
    // The fast path: with no catcode-othering definition, the same call site stays
    // ordinary — a single parse pass, and `$b$` lexes as inline math. Guards against
    // the two-pass ever firing (and changing tokenization) when it should not.
    let out = tree("\\shellcmd{a_$b$}\n");
    assert!(out.contains("INLINE_MATH@"), "{out}");
    assert!(!out.contains(r#""{a_$b$}""#), "{out}");
}

#[test]
fn standalone_verb_after_command_is_not_captured() {
    // A self-contained `\verb…` token (text begins with `\`) following another
    // command must stay a sibling — it is no one's argument. Only a verbatim
    // *argument* `VERB` (`{…}` / delimiter run, never `\`-prefixed) is attached.
    insta::assert_snapshot!(tree(r"\foo \verb|x|"));
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
fn expl_syntax_control_word_with_underscore_and_colon() {
    insta::assert_snapshot!(tree(r"\ExplSyntaxOn\seq_new:N\ExplSyntaxOff\seq_new:N"));
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
    // `$`-math cannot span the enclosing group's `}`, so the shape gate never
    // opens math here: the `$` stays a plain token inside the argument group
    // and nothing downstream is corrupted (no spurious "unmatched `}`" /
    // "unclosed environment"). `\foo` is an ordinary (non-verbatim) command,
    // so its argument group is really parsed — contrast `\code`, whose
    // argument is captured verbatim.
    let parsed = parse("\\begin{a}\\foo{$ x}\\end{a}");
    assert_eq!(parsed.syntax().to_string(), "\\begin{a}\\foo{$ x}\\end{a}");
    assert_eq!(parsed.errors, [], "a gated dollar is not an error");
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

// --- environment-definition bodies (issue #45) -------------------------------

#[test]
fn newenvironment_split_begin_end_bodies() {
    // The begin-code opens `center` and the end-code closes it: valid LaTeX
    // (the two need not balance within one group), so `\begin`/`\end` parse as
    // plain commands inside the definition bodies and no errors are reported.
    insta::assert_snapshot!(tree(r"\newenvironment{wrap}{\begin{center}}{\end{center}}"));
}

#[test]
fn xparse_environment_split_bodies_parse_clean() {
    let parsed = parse(r"\NewDocumentEnvironment{wrap}{O{x}}{\begin{center}}{\end{center}}");
    assert_eq!(parsed.errors, vec![]);
}

#[test]
fn newenvironment_split_bodies_in_optional_default() {
    let parsed = parse(r"\newenvironment{w}[1][\begin{center}]{a}{b}");
    assert_eq!(parsed.errors, vec![]);
}

#[test]
fn env_def_body_flag_does_not_leak_to_siblings() {
    // The environment *after* the definition still parses as a real
    // ENVIRONMENT: the definition-body treatment ends with the attached
    // arguments.
    let parsed = parse(
        "\\newenvironment{w}{\\begin{center}}{\\end{center}}\n\\begin{itemize}x\\end{itemize}",
    );
    assert_eq!(parsed.errors, vec![]);
    let kinds: Vec<SyntaxKind> = parsed.syntax().descendants().map(|n| n.kind()).collect();
    assert!(
        kinds.contains(&SyntaxKind::ENVIRONMENT),
        "itemize is a real environment"
    );
}

// --- hook and command-definition bodies (issue #55) --------------------------

#[test]
fn hook_bodies_split_begin_end() {
    // `\AtBeginDocument` opens an environment that `\AtEndDocument` closes:
    // the code arguments run at different points in the document, so
    // `\begin`/`\end` need not balance within either group.
    let parsed = parse("\\AtBeginDocument{\\begin{page}}\n\\AtEndDocument{\\end{page}}");
    assert_eq!(parsed.errors, vec![]);
}

#[test]
fn newcommand_body_splits_end_begin() {
    // A page-break macro closes the current environment and reopens it
    // (dalcde/cam-notes headers): plain commands, no errors.
    insta::assert_snapshot!(tree(r"\newcommand{\newpg}{\end{page}\begin{page}}"));
}

// --- nested command-abutting brackets in math (issue #55) --------------------

#[test]
fn math_bracket_claimed_by_inner_command_stays_atom() {
    // The lone `]` is claimed by `\gamma[`, so the outer `\P[` must not
    // attach as an optional argument (it would end up unclosed): it stays an
    // ordinary math atom and the parse is clean.
    insta::assert_snapshot!(tree(
        r"\[0 < \P[\gamma[0, \infty) \cap A = \emptyset] < 1\]"
    ));
}

#[test]
fn math_bracket_with_own_closer_still_attaches_past_interval() {
    // A `[` not abutting a command (the interval `[0, 1)`) claims no `]`, so
    // the outer bracket still reads as an argument and attaches.
    let parsed = parse(r"$\E[[0, 1) \cap A]$");
    assert_eq!(parsed.errors, vec![]);
    let kinds: Vec<SyntaxKind> = parsed.syntax().descendants().map(|n| n.kind()).collect();
    assert!(
        kinds.contains(&SyntaxKind::OPTIONAL),
        "the outer bracket attaches to `\\E`"
    );
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

#[test]
fn dollar_without_reachable_closer_stays_plain() {
    // The `$` shape gate (smoke-test issue #60): a dollar whose closer is not
    // reachable before the enclosing group closes is macro-code data — the
    // tabular preamble `>{$}` injects math per cell — not a math delimiter.
    // It parses as an ordinary token: no math node, no diagnostic.
    insta::assert_snapshot!(tree(r"\begin{tabular}{>{$}c<{$}} a \end{tabular}"));
}

#[test]
fn lone_dollar_in_group_stays_plain() {
    // An expl3 token list holding a literal dollar (l3htoks: `{ $ }`).
    insta::assert_snapshot!(tree(r"\tl_put:Nn \l_tmpa_tl { $ }"));
}

#[test]
fn unclosed_dollar_before_paragraph_break_stays_plain() {
    // A paragraph break bounds the closer scan: the dollar cannot pair with
    // one in a later paragraph, so it stays plain rather than swallowing the
    // rest of its paragraph as math.
    insta::assert_snapshot!(tree("a $ b\n\nc $ d\n\ne"));
}

#[test]
fn dollar_display_without_closer_gates_each_dollar() {
    // `{ $$ }`: no `$$` closer is reachable, so the display opener is not
    // math; each `$` re-enters the gate independently and both stay plain.
    insta::assert_snapshot!(tree(r"{ $$ }"));
}

#[test]
fn dollar_math_still_pairs_across_groups_and_environments() {
    // The gate must not regress legit math: a closer past balanced `{…}`
    // nesting and a balanced `\begin…\end` still opens math.
    insta::assert_snapshot!(tree(
        r"${a}^2$ and $\begin{smallmatrix} a \end{smallmatrix}$"
    ));
}

#[test]
fn def_control_symbol_name_is_plain() {
    // `\def`-family name isolation (smoke-test issue #65): the control symbol
    // after `\def` is the sequence being defined, not syntax — `\[` is no
    // math opener — and the attached body is a macro-code body, so the
    // trivlist that opens in `\def\[`'s body and closes in `\def\]`'s draws
    // no diagnostics.
    insta::assert_snapshot!(tree(
        "\\def\\[{\\begin{trivlist}\\item[]$\\displaystyle}%\n\\def\\]{$\\end{trivlist}}"
    ));
}

#[test]
fn display_math_without_reachable_closer_stays_plain() {
    // The `\[` shape gate (smoke-test issue #65), mirroring the `$` gate:
    // macro code passes `\[` around as a data token
    // (`\expandafter\@tempa\[\@nil`), so an opener with no reachable closer
    // is an ordinary token: no math node, no diagnostic.
    insta::assert_snapshot!(tree("\\expandafter\\@tempa\\[\\@nil"));
}

#[test]
fn delim_math_still_pairs_across_lines() {
    // The gate must not regress legit display math: a closer on a later line
    // (no blank line between) still opens math.
    insta::assert_snapshot!(tree("\\[\n  x + y\n\\]\nand \\( a \\)"));
}

#[test]
fn braceless_end_is_a_plain_command() {
    // The `\begin`/`\end` shape gate (smoke-test issue #60): macro code uses
    // the bare TeX primitive (`\let\end\@@end`, docstrip's
    // `\errmessage{…}\end`) and the delimiter pattern
    // (`\long\def\@gobble@nv#1\end#2{…}`), so a `\begin`/`\end` with no
    // reachable `{` is a plain command: no environment, no diagnostic.
    insta::assert_snapshot!(tree("\\let\\end\\@@end\n\\def\\g#1\\end#2{x}"));
}

#[test]
fn braceless_end_does_not_terminate_an_environment_body() {
    // A bare `\end` inside an environment body is body content, not the
    // closer: the environment still pairs with its real `\end{name}`.
    insta::assert_snapshot!(tree("\\begin{myenv}\n\\expandafter\\end\n\\end{myenv}"));
}

#[test]
fn end_with_macro_name_group_is_a_plain_command() {
    // A name group holding a parameter or control word is computed macro
    // data (`\edef\reserved@a{\noexpand\end{\reserved@a}}`, xparse's
    // `\begin \end {#3}`), statically unpairable — a plain command with an
    // ordinary argument, no diagnostic.
    insta::assert_snapshot!(tree(r"\def\x{\end{\reserved@a}} \def\y#1{\end{#1}}"));
}

#[test]
fn spaced_name_group_still_reads_as_an_environment() {
    // The name-shape check must not regress `\begin { longtable }`-style
    // spacing (expl3 sources): a word-only name group still pairs.
    insta::assert_snapshot!(tree(r"\begin { myenv } x \end { myenv }"));
}

#[test]
fn char_constant_backtick_keeps_the_next_character_plain() {
    // TeX char-constant notation (smoke-test issue #60): after `\char`/
    // `\catcode`-family primitives, a backtick makes the next character data —
    // `\char`$` must not open math and `\char`}` must not close a group. The
    // backtick and its character lex as one plain `WORD` token.
    insta::assert_snapshot!(tree("\\item[\\char`$ or z] and {\\char`} too}"));
}

#[test]
fn char_constant_escaped_form_lexes_benignly() {
    // The escaped form keeps its ordinary shape: a backtick word, then the
    // `\$` control symbol.
    insta::assert_snapshot!(tree("\\catcode`\\%=12"));
}

#[test]
fn expl3_region_begin_end_is_plain_macro_code() {
    // Inside an expl3 region, token lists pass `\begin`/`\end` around as data
    // (l3prefixes.tex builds a longtable across two token-list bodies, issue
    // #60): both parse as plain commands — no pairing, no diagnostics.
    insta::assert_snapshot!(tree(
        "\\ExplSyntaxOn\n\\tl_set:Nn \\l_tmpa_tl { \\begin { longtable } { ll } }\n\\tl_put_right:Nn \\l_tmpa_tl { \\end { longtable } }\n\\ExplSyntaxOff\n"
    ));
}

#[test]
fn environments_pair_again_after_expl_syntax_off() {
    // The region gate ends at `\ExplSyntaxOff`: document markup after it
    // pairs as usual.
    insta::assert_snapshot!(tree(
        "\\ExplSyntaxOn \\foo:n \\ExplSyntaxOff\n\\begin{itemize}\n\\item x\n\\end{itemize}\n"
    ));
}
