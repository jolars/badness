//! Tree-shape snapshots over representative inputs, plus
//! targeted assertions on error-recovery behaviour. Every case also re-checks
//! the losslessness invariant. Regenerate snapshots with `task snapshots`.

use badness_parser::declarations::{Declarations, ResolvedDeclarations};
use badness_parser::parser::{
    LatexFlavor, LexConfig, parse, parse_with_declarations, parse_with_flavor,
};
use badness_parser::syntax::{SyntaxKind, SyntaxNode};
use rowan::{NodeOrToken, TextSize};

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

/// An environment defined by a package verbatim-definer (`\lstnewenvironment`) has a
/// raw body: environment tokens inside it (`\begin{tabular}`) are literal listing
/// content collected into a `VERBATIM_BODY`, never parsed as real structure.
#[test]
fn lstnewenvironment_body_is_verbatim() {
    insta::assert_snapshot!(tree(
        "\\lstnewenvironment{demo}{}{}\n\\begin{demo}\n\\begin{tabular}{S}\n\\end{demo}\n"
    ));
}

#[test]
fn display_math_dollars() {
    insta::assert_snapshot!(tree(r"$$a + b$$"));
}

#[test]
fn def_parameter_dollar_is_not_math() {
    insta::assert_snapshot!(tree(
        "\\def\\take#1] ${%\n% comment\n  body\n}%\n\\def\\next#1${next}\n",
    ));
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
fn unclosed_left_demotes_without_diagnostic() {
    // `\left(` with no reachable `\right` before the closing `$`: shape-gated like
    // `\[` (issue #77), the `\left` stays an ordinary command and the `(` an
    // ordinary atom — no `LEFT_RIGHT` node, **no diagnostic** (a likely typo is
    // linter territory), and nothing corrupted.
    let parsed = parse(r"$\left( x $");
    assert_eq!(parsed.syntax().to_string(), r"$\left( x $");
    assert!(
        !tree(r"$\left( x $").contains("LEFT_RIGHT"),
        "unclosed `\\left` must not open a LEFT_RIGHT node"
    );
    let messages: Vec<&str> = parsed.errors.iter().map(|e| e.message.as_str()).collect();
    assert!(messages.is_empty(), "unexpected diagnostics: {messages:?}");
}

#[test]
fn left_right_pairs_through_nested_environments() {
    // The shape gate must reach the `\right` across a nested `\begin`/`\end`
    // (`matrix` holding `pmatrix` cells), so a well-formed pair still opens a
    // `LEFT_RIGHT` and its `\right` is never orphaned (issue #77 regression: a
    // gate that miscounted nested environments demoted this valid pair).
    let src = r"$\left\{\begin{matrix}\begin{pmatrix}1\end{pmatrix}\end{matrix}\right\}$";
    let parsed = parse(src);
    assert_eq!(parsed.syntax().to_string(), src);
    assert!(
        tree(src).contains("LEFT_RIGHT"),
        "a balanced `\\left…\\right` around nested environments must pair"
    );
    assert!(
        parsed.errors.is_empty(),
        "unexpected diagnostics: {:?}",
        parsed
            .errors
            .iter()
            .map(|e| e.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn left_right_pairs_inside_array_cell() {
    // `array` is a math-only environment (the math-mode analog of `tabular`), so
    // its body parses in math mode and a `\left…\right` inside a cell pairs into
    // a `LEFT_RIGHT` — even nested in a display. Regression: `array` was modeled
    // as a plain block environment, so its body stayed text mode, the `\left`
    // never paired, and the linter's `unclosed-math-delimiter` fired a false
    // positive (dalcde/cam-notes, an `\[\begin{array}…\left(\frac…\right)…\]`).
    let src = "\\[\n\\begin{array}{c}\n\\left( a \\right) & b\n\\end{array}\n\\]\n";
    let parsed = parse(src);
    assert_eq!(parsed.syntax().to_string(), src);
    assert!(
        tree(src).contains("LEFT_RIGHT"),
        "a balanced `\\left…\\right` inside an `array` cell must pair"
    );
    assert!(
        parsed.errors.is_empty(),
        "unexpected diagnostics: {:?}",
        parsed
            .errors
            .iter()
            .map(|e| e.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn left_right_pairs_inside_macro_code() {
    // A `\left…\right` in math inside macro code (a `\def` body, and the same for
    // a `.dtx` `macrocode` chunk, both of which set `in_def_body`) must still pair
    // into a `LEFT_RIGHT`. The pair is catcode-neutral math structure that pairs by
    // count regardless of macro meaning, so the shape gate may not skip `\left`/
    // `\right` in macro code the way it skips `\begin`/`\end`. Regression (issue
    // #95): the gate did skip them, so package math like ltmath.dtx's
    // `\bordermatrix` (`$…\left(…\right)$`) and delarray.dtx's `$\left#2\right#4$`
    // never paired and the closer reported a spurious `\right` without matching
    // `\left`, a parse error that blocked the whole file for the formatter.
    let src = r"\def\x{$\kern\wd\@ne\left(\vcenter{a}\,\right)$}";
    let parsed = parse(src);
    assert_eq!(parsed.syntax().to_string(), src);
    assert!(
        tree(src).contains("LEFT_RIGHT"),
        "a balanced `\\left…\\right` in a def body must pair"
    );
    assert!(
        parsed.errors.is_empty(),
        "unexpected diagnostics: {:?}",
        parsed
            .errors
            .iter()
            .map(|e| e.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn left_right_pairs_across_a_math_opener() {
    // The gate's math anchor is the *closing* side (`MathAnchor::Closing`): a
    // `\left` already sits inside a math body, so what ends it is the delimiter
    // that ends that body. A `\[` in the way is ordinary content — it opens no
    // math here (`delim_math_closes` refuses it: no `\]` in reach) — so the pair
    // still closes at its `\right`.
    let src = r"$\left( \[ x \right)$";
    let parsed = parse(src);
    assert_eq!(parsed.syntax().to_string(), src);
    assert!(
        tree(src).contains("LEFT_RIGHT"),
        "a math *opener* in the way must not refuse the pair"
    );
}

#[test]
fn left_right_refuses_across_a_math_closer() {
    // The mirror: `\]` ends the display the `\left` lives in, so the `\right`
    // beyond it is unreachable and the `\left` stays an ordinary command — the
    // same anchor `left_right`'s own walk stops at, with no diagnostic.
    let src = "\\[ \\left( x \\]\n";
    let parsed = parse(src);
    assert_eq!(parsed.syntax().to_string(), src);
    assert!(
        !tree(src).contains("LEFT_RIGHT"),
        "a `\\left` whose `\\right` sits past the math's end must not pair"
    );
    assert!(
        parsed.errors.is_empty(),
        "a gated `\\left` draws no diagnostic: {:?}",
        parsed
            .errors
            .iter()
            .map(|e| e.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn an_end_inside_a_nested_left_refuses_the_whole_scan() {
    // `\left`/`\right` frames and environment frames share one stack
    // (`Nesting::Interleaved`), so an `\end` that finds a `\left` frame innermost
    // is a mismatch — and the same mismatch every *outer* `\left` sees, since
    // that frame is innermost for them too. Both openers demote; neither may pair
    // with the `\right]`/`\right)` beyond the `\end`.
    let src = r"$\left( \begin{matrix} a \left[ \end{matrix} \right] \right)$";
    let parsed = parse(src);
    assert_eq!(parsed.syntax().to_string(), src);
    assert!(
        !tree(src).contains("LEFT_RIGHT"),
        "an `\\end` reached inside a nested `\\left` must refuse every open pair"
    );
}

#[test]
fn a_nested_left_shields_the_outer_pair_from_a_paragraph_break() {
    // The other half of the interleaved model, and its one asymmetry: a *mismatch*
    // is shared, but an *absence* of frames is not. The blank-line anchor asks
    // whether the frame stack is empty, which is true only for the innermost
    // `\left`, so the inner one demotes and the outer keeps scanning to its
    // `\right)`.
    //
    // The gate is looser than the walk here — `left_right` bails at the paragraph
    // break itself and reports the outer `\left` unclosed. Pre-existing, and
    // preserved verbatim by the batch migration (`TODO.md`, container stack
    // C2.4); the shape is only reachable inside a math *environment*, since the
    // `$`/`\[` gates refuse a blank line themselves.
    let src = "\\begin{equation}\n  \\left( \\left[ y\n\n  \\right] \\right)\n\\end{equation}\n";
    let parsed = parse(src);
    assert_eq!(parsed.syntax().to_string(), src);
    assert!(
        tree(src).contains("LEFT_RIGHT"),
        "the outer pair is shielded from the break by the inner `\\left`"
    );
}

#[test]
fn left_right_pairs_inside_tikzcd_cell() {
    // `tikzcd` (tikz-cd commutative diagrams) typesets its cells in math mode, so
    // a `\left…\right` in a cell pairs into a `LEFT_RIGHT`. Same regression class
    // as `array` (dalcde/cam-notes: `\begin{tikzcd} … \left(\frac…\right) \ar[…]`).
    let src = "\\begin{tikzcd}\nH\\left(\\frac{X}{A}\\right) \\ar[r] & Y\n\\end{tikzcd}\n";
    let parsed = parse(src);
    assert_eq!(parsed.syntax().to_string(), src);
    assert!(
        tree(src).contains("LEFT_RIGHT"),
        "a balanced `\\left…\\right` inside a `tikzcd` cell must pair"
    );
    assert!(
        parsed.errors.is_empty(),
        "unexpected diagnostics: {:?}",
        parsed
            .errors
            .iter()
            .map(|e| e.message.as_str())
            .collect::<Vec<_>>()
    );
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
fn math_optional_with_balanced_dollar_attaches() {
    // A balanced inline `$…$` inside a command-abutting `[…]` in display math
    // must not abort bracket attachment: mathpartir's `\inferrule*[right=$\Pi$]`
    // sets its label in text mode, so the pair is real inline math (wrapped in
    // an INLINE_MATH under the OPTIONAL), not two unclosed dollars.
    insta::assert_snapshot!(tree(r"\[ \foo[$x$] \]"));
}

#[test]
fn math_optional_with_unclosed_dollar_stays_plain() {
    // The mirror guard: an *unclosed* `$` inside the bracket leaves no reachable
    // `]`, so the `[` stays a plain math atom (no OPTIONAL, no over-attachment).
    insta::assert_snapshot!(tree(r"\[ \foo[$x] \]"));
}

#[test]
fn math_stray_bracket_in_dollar_stays_plain_despite_later_bracket() {
    // A `[` inside `$…$` with no `]` before the closing `$` is a stray math atom
    // (a missing `]` typo: `$\mathcal{N}[\mathcal{S}$`, stacks-project #99). The
    // closing `$` bounds the search — a `]` in a *later* `$…$` region cannot be
    // this bracket's, so the `[` stays plain and the first math closes cleanly
    // instead of the optional swallowing everything to the second `]`.
    insta::assert_snapshot!(tree(
        r"$\mathcal{N}[\mathcal{S}$ and $\mathcal{N}[\mathcal{S}]$"
    ));
}

#[test]
fn starred_command_folds_star_and_attaches_arguments() {
    // A starred variant carries its `*` before its arguments; the `*` folds into
    // the invocation so the following `[…]`/`{…}` still attach — here the
    // mathpartir shape whose optional label holds inline math.
    insta::assert_snapshot!(tree(r"\[ \inferrule*[right=$\Pi$-eq]{A}{B} \]"));
}

#[test]
fn star_as_math_operator_is_not_folded() {
    // A `*` with no argument after it is a binary operator, not a variant
    // marker: `\pi*r` keeps the star a sibling atom, never a command child.
    insta::assert_snapshot!(tree(r"$\pi*r$"));
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
fn braced_verbatim_command_argument_may_start_on_next_line() {
    let out = tree("\\code\n{$ pip install x_y}");
    assert!(!out.contains("error @"), "{out}");
    assert!(out.contains(r#"VERB@6..25 "{$ pip install x_y}""#), "{out}");
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
fn redefined_braced_verbatim_command_lexes_as_group() {
    // A document that *redefines* a built-in braced-verbatim command (`\code`, jss) to
    // an ordinary macro shadows the built-in: `\code{x_y}` must lex as an ordinary group
    // with token children, not an opaque `VERB` (follow-up to issue #53). The second
    // parse pass records the non-verbatim redefinition as a suppression.
    let out = tree("\\newcommand{\\code}{\\ensuremath{\\mathsf{code}}}\n\\code{x_y}\n");
    assert!(!out.contains("error @"), "{out}");
    assert!(!out.contains("VERB@"), "{out}");
    // The argument is a real group: its `_` lexes as an ordinary token, not swallowed.
    assert!(out.contains("UNDERSCORE@"), "{out}");
}

#[test]
fn redefined_delimited_verbatim_command_lexes_normally() {
    // The suppression covers `verbatimDelimited` built-ins too: a redefined `\url`
    // captures neither its braced nor its `\verb`-style delimiter form.
    let out = tree("\\newcommand{\\url}[1]{\\texttt{#1}}\n\\url{a_b} and \\url|a_b|\n");
    assert!(!out.contains("error @"), "{out}");
    assert!(!out.contains("VERB@"), "{out}");
    assert!(out.contains("UNDERSCORE@"), "{out}");
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

/// pgfmanual's `codeexample` is a curated verbatim-body env: the leading `[…]`
/// option is parsed, but the executed-yet-verbatim body (with `$`, `%`, `...`)
/// collapses to one opaque `VERBATIM_BODY`, so prose rules never see its tokens.
#[test]
fn codeexample_optional_arg_then_opaque_body() {
    insta::assert_snapshot!(tree(
        "\\begin{codeexample}[]\n\\immediate\\write\\w{...} $x$ %literal\n\\end{codeexample}"
    ));
}

/// The kernel's `filecontents` writes its body to a file byte-for-byte: it
/// `\@makeother`s `\dospecials`, which includes `%`. So a `%` inside a field value
/// is data, not a comment, and the `}` it would otherwise swallow still closes its
/// group. Curating the env verbatim is the only way a static reader gets this right
/// (smoke-test issue #98, `plk/biblatex` `doc/latex/biblatex/examples/96-dates.tex`).
#[test]
fn filecontents_optional_and_required_args_then_opaque_body() {
    insta::assert_snapshot!(tree(
        "\\begin{filecontents}[force]{\\jobname.bib}\n@misc{a,\n  date = {1723%},\n}\n\\end{filecontents}\n"
    ));
}

/// ltxdockit's `ltxexample` is `\lstnewenvironment{ltxexample}[1][]`: one optional
/// argument, opaque body. It is defined in an external class, so no in-file scan can
/// learn it — the curated database is the only place the fact can live, as for
/// `codeexample` and `oldcomments`. Its bodies quote whole documents, `\begin` /
/// `\end` pairs and all (smoke-test issue #98, `plk/biblatex` `doc/latex/biblatex/biblatex.tex`).
#[test]
fn ltxexample_optional_arg_then_opaque_body() {
    insta::assert_snapshot!(tree(
        "\\begin{ltxexample}[style=latex]\n\\begin{document}\n\\newrefcontext}[labelprefix=B]\n\\end{ltxexample}\n"
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

// --- environments never escape their brace group (issue #71) -----------------

#[test]
fn environment_does_not_escape_its_enclosing_group() {
    // array.sty's `\newcolumntype`: the `\begin` sits in one sibling group and
    // its `\end` in another, so neither is document structure. Each `}` closes
    // the group it belongs to and nothing cascades into unmatched-brace noise.
    let parsed = parse(r"\newcolumntype{w}[2]{>{\begin{lrbox}\b}c<{\end{lrbox}}}");
    assert_eq!(parsed.errors, vec![]);
}

#[test]
fn begin_in_a_message_argument_is_a_plain_command() {
    // amstex.sty: `\begin{split}` is prose inside an error message, never
    // executed as structure, and its group closes before any `\end`.
    let parsed = parse(r"\def\s{\PackageError{a}{\begin{split} is not allowed}}");
    assert_eq!(parsed.errors, vec![]);
}

#[test]
fn end_inside_a_group_is_not_stray() {
    // ltxdoc's `\StopEventually{\end{document}}`: this `\end`'s `\begin` is
    // outside the group, so it is macro code, not a stray.
    let parsed = parse(r"\StopEventually{\end{document}}");
    assert_eq!(parsed.errors, vec![]);
}

#[test]
fn unbraced_definee_body_still_gets_the_group_gate() {
    // rotex.tex's `\newcommand\BeginExample{…\begin{VerbatimOut}…}` pairs with
    // a separate `\EndExample`. The body group is not attached to
    // `\newcommand` (the definee is a bare control word), so the definition-body
    // flag never reaches it and only the group gate keeps the parse clean.
    let parsed = parse(r"\newcommand\BeginExample{\begin{VerbatimOut}}");
    assert_eq!(parsed.errors, vec![]);
}

#[test]
fn def_body_begin_is_a_plain_command() {
    // The `\def` family is not in `is_definition_body_command`, so before the
    // group gate this swallowed the closing brace.
    let parsed = parse(r"\def\x{\begin{y}}");
    assert_eq!(parsed.errors, vec![]);
}

#[test]
fn an_end_orphaned_by_the_gate_is_demoted_in_step() {
    // amsldoc.tex (issue #71): `\lowercase{…}` smuggles a literal `}` into text,
    // so the `\begin{error}` sits inside a group its `\end` is outside of and the
    // gate demotes it. Left as a stray `\end`, its partner then unwound every
    // enclosing environment on the way to the root — un-closing the whole
    // `document` and stranding `\end{document}` as a second stray.
    let parsed = parse(concat!(
        r"\begin{document}\lowercase{\begin{error}{Missing @ inserted}}",
        "\nx\n",
        r"\end{error}",
        "\n",
        r"\end{document}",
    ));
    assert_eq!(parsed.errors, vec![]);
}

#[test]
fn an_end_the_gate_never_touched_is_still_stray() {
    // The mirror is scoped to names the gate actually demoted: a plain typo has
    // no demoted partner, so it still reports.
    let parsed = parse(r"egin{itemize}\item a\end{itemiz}");
    assert!(!parsed.errors.is_empty());
}

#[test]
fn a_reachable_end_inside_a_group_still_nests() {
    // The gate only fires when the `\end` is unreachable before the enclosing
    // `}`: a properly paired environment inside a group still builds a real
    // ENVIRONMENT node.
    let parsed = parse(r"{\begin{center}hi\end{center}}");
    assert_eq!(parsed.errors, vec![]);
    let kinds: Vec<SyntaxKind> = parsed.syntax().descendants().map(|n| n.kind()).collect();
    assert!(
        kinds.contains(&SyntaxKind::ENVIRONMENT),
        "a reachable `\\end` still nests as an environment"
    );
}

#[test]
fn unclosed_environment_outside_a_group_still_diagnoses() {
    // The gate is scoped to a *group* boundary. A `\begin` that merely runs out
    // of file has no competing brace, so a genuinely forgotten `\end` in prose
    // keeps its diagnostic.
    let parsed = parse(r"\begin{itemize}\item x");
    assert_eq!(parsed.errors.len(), 1);
    assert!(parsed.errors[0].message.contains("unclosed environment"));
}

// --- conditionals pair only when their `\fi` is reachable --------------------

/// The `CONDITIONAL` nodes in `input`, as `(branch count, has closer)` pairs in
/// preorder. Also re-checks losslessness, as `tree` does.
fn conditionals(input: &str) -> Vec<(usize, bool)> {
    let parsed = parse(input);
    assert_eq!(
        parsed.syntax().to_string(),
        input,
        "losslessness violated for {input:?}"
    );
    parsed
        .syntax()
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::CONDITIONAL)
        .map(|n| {
            let branches = n
                .children()
                .filter(|c| c.kind() == SyntaxKind::CONDITIONAL_BRANCH)
                .count();
            let closer = n
                .last_child()
                .is_some_and(|c| c.kind() == SyntaxKind::COMMAND);
            (branches, closer)
        })
        .collect()
}

#[test]
fn a_paired_conditional_builds_one_branch_per_divider() {
    // The canonical shape: opener + test + then-body in the first branch, the
    // `\else` opening the second, `\fi` the last child.
    insta::assert_snapshot!(tree(r"\ifnum1<2 b \else c \fi"));
}

#[test]
fn ifcase_or_branches_each_open_their_own() {
    // `\or` is a divider like `\else` — excluding it would collapse an `\ifcase`
    // body into a single branch.
    assert_eq!(
        conditionals(r"\ifcase#1\relax a\or b\or c\else d\fi"),
        [(4, true)]
    );
}

#[test]
fn a_conditional_with_no_reachable_fi_is_a_plain_command() {
    // `\fi` is routinely assembled elsewhere (`\def\stopit{\fi}`,
    // `\expandafter\fi`). Running out of file demotes silently — unlike the
    // environment gate, there is no diagnostic here to preserve.
    assert_eq!(conditionals(r"\ifnum1<2 b"), []);
    assert_eq!(parse(r"\ifnum1<2 b").errors, vec![]);
}

#[test]
fn a_conditional_does_not_escape_its_enclosing_group() {
    // The `}` closing a group opened before the `\if` always wins: braces are
    // catcode structure, `\if`/`\fi` are only macros. `\def\stopit{\fi}` is the
    // idiom this protects.
    assert_eq!(conditionals(r"\def\x{\ifnum1<2 }\fi"), []);
    assert_eq!(parse(r"\def\x{\ifnum1<2 }\fi").errors, vec![]);
}

#[test]
fn a_conditional_inside_a_group_still_pairs_when_its_fi_is_reachable() {
    // The gate's mirror direction: a self-contained conditional inside a group
    // is untouched. A gate stricter than the parse would drop the node and
    // refuse the whole file to the formatter.
    assert_eq!(conditionals(r"{\ifnum1<2 a\else b\fi}"), [(2, true)]);
}

#[test]
fn a_conditional_does_not_span_an_unowed_end() {
    // Mirrors the `$`/`\[` anchor: an `\end` not owed to an intervening `\begin`
    // ends the construct, so the `\if` is macro code.
    assert_eq!(
        conditionals("\\begin{center}\\ifnum1<2 a\\end{center}\\fi\n"),
        []
    );
}

#[test]
fn a_conditional_may_contain_a_whole_environment() {
    // An environment *owed* to a `\begin` inside the conditional nests normally.
    assert_eq!(
        conditionals(r"\ifnum1<2 \begin{center}a\end{center}\else b\fi"),
        [(2, true)]
    );
}

#[test]
fn a_blank_line_ends_the_conditional() {
    // Decision: a paragraph break anchors the gate exactly as it does for `$`,
    // which keeps `CONDITIONAL` a within-paragraph construct — it can never
    // straddle a `PARAGRAPH` boundary, so no paragraph nests inside one.
    assert_eq!(conditionals("\\ifcmh\n\na\n\n\\else\n\nb\n\\fi\n"), []);
}

#[test]
fn a_blank_line_inside_a_nested_group_does_not_anchor() {
    // The break blocks only at the construct's own level.
    assert_eq!(
        conditionals("\\ifnum1<2 {a\n\nb}\\else c\\fi\n"),
        [(2, true)]
    );
}

#[test]
fn a_refuted_nested_opener_still_consumes_a_fi() {
    // The gate counts nested openers by name and never un-counts one: the
    // unowed `\end{center}` demotes `\ifdim`, but `\ifdim`'s slot still
    // consumes the lone `\fi`, so the outer `\ifnum` runs out of closers and
    // demotes too. The batched scan must settle a refuted entry *without*
    // removing it from the pending stack (`Parser::gate_batch`);
    // popping it would hand the `\fi` to `\ifnum` — a `CONDITIONAL` the
    // per-opener scan never built. (The two-`\fi` sibling of this shape is
    // `conditional_walk_may_close_before_the_located_fi`.)
    assert_eq!(
        conditionals(r"\ifnum1<2 \begin{center}\ifdim1pt<2pt \end{center}x\fi"),
        []
    );
}

#[test]
fn a_blank_line_inside_a_nested_environment_demotes_only_the_inner_opener() {
    // The paragraph break sits at the inner conditional's own level (inside
    // `center`), so it refutes `\ifcmh` alone; the outer `\ifnum`, one
    // environment up, pairs across it — the batched scan settles exactly the
    // level-matching suffix of its live entries.
    assert_eq!(
        conditionals("\\ifnum1<2 \\begin{center}\\ifcmh a\n\nb\\fi\\end{center}\\fi\n"),
        [(1, true)]
    );
}

#[test]
fn a_fi_inside_an_environment_the_conditional_opened_is_not_its_closer() {
    // The `\fi` is consumed by the environment's body, so the walk cannot
    // reach it: the closer must sit at the opener's own environment level.
    assert_eq!(
        conditionals(r"\ifnum1<2 \begin{center}x\fi\end{center}"),
        []
    );
}

#[test]
fn a_macrocode_frame_refutes_every_pending_opener() {
    // The chunk boundary is hard in both directions; the outer opener and the
    // nested one it counted demote together, in one batch.
    assert_eq!(
        conditionals(
            "\\ifnum1<2 \\ifdim1pt<2pt a\\begin{macrocode}\nb\\fi\\fi\n\\end{macrocode}\n"
        ),
        []
    );
}

#[test]
fn newif_declares_a_flag_without_opening_a_conditional() {
    // 574 corpus occurrences. The `\if@foo` after `\newif` is the flag being
    // declared, not an opener; the *use* below it is the real conditional.
    assert_eq!(
        conditionals("\\newif\\if@foo\n\\if@foo a\\else b\\fi"),
        [(2, true)]
    );
}

#[test]
fn ifx_operands_are_data_even_when_if_named() {
    // `\ifx\ifpdf\iftrue` compares two `if*`-named tokens without running them,
    // so exactly one conditional opens.
    assert_eq!(conditionals(r"\ifx\ifpdf\iftrue x\fi"), [(1, true)]);
}

#[test]
fn brace_argument_if_macros_open_nothing() {
    // `\ifthenelse` and the etoolbox test family take `{true}{false}` and are
    // never `\fi`-terminated.
    assert_eq!(conditionals(r"\ifthenelse{\a}{b}{c}"), []);
}

#[test]
fn a_brace_argument_if_macro_does_not_steal_an_enclosing_fi() {
    // latexindent's `test-cases/ifelsefi/issue-250.tex`. Subtracting the
    // `\ifthenelse` family is load-bearing rather than cosmetic: shape alone
    // does not merely fail here, it *mis-pairs* — a trusted `\ifnumgreater`
    // would take the `\ifluatex`'s `\fi` and unnest the whole file.
    let src = "\\ifxetex\n\tfoo\n\\else\n\t\\ifluatex\n\t\t\\ifnumgreater{2}{1}{\n\t\t\tbar\n\t\t}{}\n\t\\else\n\t\\fi\n\\fi\n";
    assert_eq!(conditionals(src), [(2, true), (2, true)]);
}

#[test]
fn a_divider_takes_no_arguments() {
    // Inside a conditional an `\else`/`\or`/`\fi` is a structural delimiter
    // parsed like `\end`, so a following group is the next branch's first
    // element rather than the divider's argument.
    let parsed = parse(r"\ifx\a\b \else{foo}\fi");
    let else_cmd = parsed
        .syntax()
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::COMMAND)
        .find(|n| n.text().to_string().starts_with("\\else"))
        .expect("an `\\else` command");
    assert_eq!(
        else_cmd.children().count(),
        0,
        "`\\else` attached {:?}",
        else_cmd.text().to_string()
    );
}

#[test]
fn expl3_regions_grow_no_conditionals() {
    // In-region layout is the formatter's, owned through the expl3 statement
    // segmentation; a `CONDITIONAL` there would contend with it. The exclusion
    // also keeps the `\else:`/`\fi:` spellings out of scope.
    let src = "\\ExplSyntaxOn\n\\if_int_compare:w 1 < 2 \\else: b \\fi:\n\\ExplSyntaxOff\n";
    assert_eq!(conditionals(src), []);
}

#[test]
fn a_conditional_body_still_binds_a_leading_comment() {
    // An own-line `%` run before a construct inside a branch binds into it as a
    // `DOC_COMMENT`, exactly as it does at paragraph level.
    let src = "\\ifnum1<2\n% doc\n\\foo\n\\fi\n";
    let parsed = parse(src);
    assert_eq!(parsed.syntax().to_string(), src);
    let kinds: Vec<SyntaxKind> = parsed.syntax().descendants().map(|n| n.kind()).collect();
    assert!(
        kinds.contains(&SyntaxKind::DOC_COMMENT),
        "leading comment run should still bind inside a branch: {kinds:?}"
    );
}

#[test]
fn a_conditional_whose_fi_hides_behind_math_demotes() {
    // `ltboxes.dtx`'s shape, minimized: every `\fi` sits inside a `$…$` the
    // conditional opened, so none is a closer the walk can reach. Counting one
    // would promise a pairing the walk cannot honor and carry the construct off
    // the end of its `macrocode` chunk. The gate refuses instead, and the whole
    // run stays plain commands.
    assert_eq!(conditionals(r"\if@pboxsw $\vcenter \fi\fi$"), []);
    // The mirror: math *after* the closer is nobody's business, so the same
    // opener with a reachable `\fi` still pairs.
    assert_eq!(conditionals(r"\if@pboxsw a\fi $x$"), [(1, true)]);
}

#[test]
fn conditional_walk_may_close_before_the_located_fi() {
    // The gate's token scan counts `\ifB` as a nested opener and so picks the
    // *second* `\fi`; the walk re-gates `\ifB`, demotes it (its own scan meets an
    // `\end` it does not own), and closes at the *first*. The scan's index bounds
    // the walk but does not predict it — so the pairing may undershoot, and the
    // leftover `\fi` is a plain command rather than a second construct.
    //
    // Pinned because the one-directional guarantee is what
    // `ast::Conditional::closer` being fallible rests on: the dangerous direction
    // (the walk running *past* the located closer) is the one that must stay
    // impossible.
    let src = r"\ifA \begin{center} \ifB \end{center} \fi \fi";
    assert_eq!(conditionals(src), [(1, true)]);
    let parsed = parse(src);
    let conditional = parsed
        .syntax()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::CONDITIONAL)
        .expect("the outer conditional pairs");
    // It closed at the first `\fi`, so the second is left outside the node.
    assert!(
        conditional.text_range().end() < TextSize::of(src),
        "the construct should stop at the first `\\fi`, leaving the second stray"
    );
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

// --- the batched bracket family (container stack C2.5) -----------------------

/// Whether each `[` in `src`, in source order, attaches as an `OPTIONAL`.
fn bracket_attachments(src: &str) -> Vec<bool> {
    bracket_attachments_with(src, LatexFlavor::Document.into())
}

/// [`bracket_attachments`] under the `.dtx` docstrip lexer mode, where a
/// line-leading `%` is a `DOC_MARGIN`, a line-leading `%<…>` a `GUARD`, and a
/// `macrocode` body lexes as ordinary code. Required by every case whose shape
/// is one of those three — under the default config they are all just comments.
fn bracket_attachments_dtx(src: &str) -> Vec<bool> {
    bracket_attachments_with(
        src,
        LexConfig {
            flavor: LatexFlavor::Document,
            dtx: true,
        },
    )
}

fn bracket_attachments_with(src: &str, config: LexConfig) -> Vec<bool> {
    let parsed = parse_with_flavor(src, config);
    assert_eq!(parsed.syntax().to_string(), src, "losslessness");
    parsed
        .syntax()
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SyntaxKind::L_BRACKET)
        .map(|t| t.parent().is_some_and(|p| p.kind() == SyntaxKind::OPTIONAL))
        .collect()
}

#[test]
fn nested_bracket_claims_settle_innermost_first() {
    // The claim countdown of issue #55, seen from the batch that now computes
    // it: one scan settles every command-abutting `[` in the frame, and closer
    // matching is LIFO, so the lone `]` belongs to the innermost opener and the
    // two around it stay ordinary tokens. The pre-batch code asked each opener
    // in turn and had to arrive at the same three verdicts.
    assert_eq!(
        bracket_attachments("\\a[x\n\\b[y\n\\c[z\n]\n"),
        [false, false, true]
    );
}

#[test]
fn a_bracket_refuses_an_anchor_inside_a_group() {
    // Both the environment anchor and the paragraph break are depth-*blind* for
    // this family (`ParagraphAnchor::AnyDepth`, `ANCHORS_AT_ANY_DEPTH`), because
    // the `optional` walk they guard is: it bails wherever the cursor stands, so
    // a gate that read either only at the bracket's own brace level would attach
    // an optional the walk then reports unclosed.
    assert_eq!(
        bracket_attachments("\\cmd[{\\begin{center}a\\end{center}} x]"),
        [false]
    );
    assert_eq!(bracket_attachments("\\cmd[{ x\n\ny} ]\n"), [false]);
}

#[test]
fn a_math_bracket_anchors_on_an_environment_inside_macro_code() {
    // In a definition body `\begin`/`\end` are plain commands (issues #45/#60),
    // so the text-mode gate ignores them and the bracket attaches.
    assert_eq!(
        bracket_attachments(r"\newcommand{\x}{\cmd[a \begin{center} b]}"),
        [true]
    );
    // The same body inside math refuses: the in-math gate's environment anchor
    // carries no `in_macro_code` filter (`ENV_ANCHOR_IN_MACRO_CODE`), so it is
    // stricter there than the `optional` bail it mirrors. Preserved from the
    // pre-batch scan, in the direction that only ever declines to attach.
    assert_eq!(
        bracket_attachments(r"\newcommand{\x}{$\cmd[a \begin{center} b]$}"),
        [false]
    );
}

#[test]
fn a_guard_line_does_not_part_a_macrocode_optional() {
    // `rotating.dtx`: `\ProvidesPackage`'s date optional runs over several
    // docstrip guard lines inside one `macrocode` chunk. Docstrip *deletes* a
    // guard-only line when it strips the file, so it does not part what
    // surrounds it (issue #71): the guard breaks the newline run without being
    // a newline, so the two newlines around it are not a blank line.
    let src = concat!(
        "%    \\begin{macrocode}\n",
        "\\ProvidesPackage{rot}%\n",
        "    [2026 v1\n",
        "%<*dtx>\n",
        "  more%\n",
        "%</dtx>\n",
        "        ]\n",
        "%    \\end{macrocode}\n",
    );
    assert_eq!(bracket_attachments_dtx(src), [true]);
    // A real blank line in the same place still refuses, so the run is read,
    // not ignored.
    let src = concat!(
        "%    \\begin{macrocode}\n",
        "\\ProvidesPackage{rot}%\n",
        "    [2026 v1\n",
        "\n",
        "  more%\n",
        "        ]\n",
        "%    \\end{macrocode}\n",
    );
    assert_eq!(bracket_attachments_dtx(src), [false]);
}

#[test]
fn a_guard_line_parts_no_construct_for_the_text_bracket_gate_either() {
    // The reading above is the driver's, not one gate's: a `GUARD` breaks the
    // paragraph run for every gate that has one, so the same shape in a `.dtx`
    // *documentation* line — where `TextBracketGate` decides, not
    // `MacrocodeBracketGate` — keeps its optional too.
    assert_eq!(
        bracket_attachments_dtx("% \\cmd[a\n%<*dtx>\n% b]\n"),
        [true]
    );
    // The two controls that say the run is read rather than ignored. A real
    // blank line still parts it...
    assert_eq!(bracket_attachments_dtx("% \\cmd[a\n\n% b]\n"), [false]);
    // ...and so does a margin-only line, which *is* the blank line of the
    // documentation layer: a `DOC_MARGIN` floats like whitespace, so its two
    // surrounding newlines still count (`TriviaScan::saw_blank_line`).
    assert_eq!(bracket_attachments_dtx("% \\cmd[a\n%\n% b]\n"), [false]);
}

#[test]
fn a_guard_line_parts_no_math_either() {
    // Same for a gate whose paragraph anchor fires at its own brace level
    // rather than at any depth (`DollarGate`, `ParagraphAnchor::OwnLevel`):
    // it settles entries where the bracket family breaks outright, so the two
    // arms of the anchor are pinned separately.
    assert!(has_dtx_math("% $x\n%<*dtx>\n% y$\n"));
    assert!(!has_dtx_math("% $x\n\n% y$\n"));
    assert!(!has_dtx_math("% $x\n%\n% y$\n"));
}

/// Whether `src`, parsed in `.dtx` mode, contains an `INLINE_MATH` node — the
/// `$` shape gate's verdict.
fn has_dtx_math(src: &str) -> bool {
    let parsed = parse_with_flavor(
        src,
        LexConfig {
            flavor: LatexFlavor::Document,
            dtx: true,
        },
    );
    assert_eq!(parsed.syntax().to_string(), src, "losslessness");
    parsed
        .syntax()
        .descendants()
        .any(|n| n.kind() == SyntaxKind::INLINE_MATH)
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
fn demoted_display_dollar_regates_its_second_dollar_as_inline() {
    // `$$ a $`: no `$$` closer, so the display opener is demoted and its
    // *second* `$` re-enters the gate — where it does pair, as inline math.
    // The two queries land on the same token index under the same walk state
    // but ask different questions (`display: true` then `false`), which is why
    // the `$` gate runs unmemoized (container stack C2.3): a batch slot keyed
    // on the walk state alone would answer the second from the first.
    insta::assert_snapshot!(tree("$$ a $"));
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
fn delim_math_pairs_across_a_blank_line_inside_a_nested_environment() {
    // Smoke-test issue #70: a display equation laid out from `tikzpicture`
    // cells has blank lines inside the pictures. Those belong to the nested
    // environment — `delim_math` only anchors on a paragraph break between
    // top-level atoms — so the gate must not read them as ending the math.
    // Reading them as blockers dropped the math node and left the real `\]`
    // reported as unmatched.
    insta::assert_snapshot!(tree(
        "\\[\n\\begin{tikzpicture}\na\n\nb\n\\end{tikzpicture}\n\\]"
    ));
}

#[test]
fn delim_math_pairs_across_a_blank_line_inside_a_group() {
    // Same boundary via `{…}`: `math_group` consumes a blank line as ordinary
    // body trivia, so the gate must too.
    insta::assert_snapshot!(tree("\\(\n\\text{a\n\nb}\n\\)"));
}

#[test]
fn unclosed_delim_math_before_paragraph_break_stays_plain() {
    // The other side of that boundary: at the math body's own level a blank
    // line is still a `\par`, so `\[` finds no reachable closer and stays a
    // plain token (the orphaned `\]` carries the diagnostic).
    insta::assert_snapshot!(tree("\\[\n  a\n\n  b\n\\]"));
}

#[test]
fn dollar_math_pairs_across_a_blank_line_inside_a_nested_environment() {
    // The `$$` twin of issue #70: `dollar_closes` mirrors the same anchors.
    insta::assert_snapshot!(tree(
        "$$\n\\begin{tikzpicture}\na\n\nb\n\\end{tikzpicture}\n$$"
    ));
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
    // `\char`$` must not open math, and in running text `\char`}` is the
    // close-group *character*, not a group closer. The backtick and its
    // character lex as one plain `WORD` token.
    insta::assert_snapshot!(tree("\\item[\\char`$ or z] and \\char`} too"));
}

#[test]
fn char_constant_backtick_never_hides_a_brace_inside_a_group() {
    // Inside a group the brace wins: whichever balanced-text scan opened it — a
    // `\def` body here — counts brace *tokens* long before `\char` could run,
    // so `` \def\v{\char`} `` closes at that `}` (longtable.dtx), and the
    // `` \ifnum`}=0\fi `` / `` \ifnum`{=\z@\fi `` balance idiom keeps its braces
    // structural instead of stranding the enclosing group (issue #71). The
    // escaped form `` `\} `` is a control symbol, never a delimiter, so it
    // stays data at any depth.
    insta::assert_snapshot!(tree(
        "\\def\\v{\\char`}\n\\def\\w{\\noalign{\\ifnum`}=0\\fi x}\n\\def\\z{\\char`\\}}"
    ));
}

#[test]
fn char_constant_escaped_form_lexes_benignly() {
    // The escaped single-character form is captured as one plain `WORD` too
    // (backtick, backslash, and the escaped character): `\catcode`\%=12`.
    insta::assert_snapshot!(tree("\\catcode`\\%=12"));
}

#[test]
fn char_constant_escaped_bracket_is_not_a_math_delimiter() {
    // A numeric-context primitive followed by an escaped-bracket char constant
    // (`\number`\[`, `\number`\]`) is data, not display math: encguide.tex's
    // char-code table pairs `\relax[ … ]` across table rows, and the `\[`/`\]`
    // must not open/close a `\[…\]` display that swallows the row's `]`
    // (smoke-test issue #71).
    insta::assert_snapshot!(tree("\\relax[ & \\number`\\[ \\\\\n] & \\number`\\]"));
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

// --- environment aliases (issue #109) ---------------------------------------

/// The issue-#109 shape: a `\begin{X}`/`\end{X}` pair defined with `\newcommand`.
const ALIAS_DEFS: &str =
    "\\newcommand{\\bea}{\\begin{eqnarray}}\n\\newcommand{\\eea}{\\end{eqnarray}}\n";

/// How many `ENVIRONMENT` nodes `input` produces, and whether it parsed clean.
fn environments(input: &str) -> (usize, bool) {
    let parsed = parse(input);
    assert_eq!(
        parsed.syntax().to_string(),
        input,
        "losslessness violated for {input:?}"
    );
    let count = parsed
        .syntax()
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::ENVIRONMENT)
        .count();
    (count, parsed.errors.is_empty())
}

#[test]
fn command_alias_pairs_as_an_environment() {
    // `\bea … \eea` becomes the same `ENVIRONMENT > BEGIN … END` shape a
    // spelled-out `\begin{eqnarray} … \end{eqnarray}` does, so every downstream
    // consumer works unchanged.
    insta::assert_snapshot!(tree(&format!("{ALIAS_DEFS}\\bea a&=&b \\eea\n")));
}

#[test]
fn command_alias_body_routes_into_math() {
    // The target's curated `math` flag decides the body, exactly as for the
    // spelled-out environment: scripts become `SCRIPTED`, operators split.
    let parsed = parse(&format!("{ALIAS_DEFS}\\bea x^2 \\eea\n"));
    let root = parsed.syntax();
    assert!(
        root.descendants().any(|n| n.kind() == SyntaxKind::MATH),
        "an eqnarray alias body must be math"
    );
    assert!(root.descendants().any(|n| n.kind() == SyntaxKind::SCRIPTED));
}

#[test]
fn def_alias_definee_is_not_an_opener() {
    // The regression that motivates the definition-keyword filter. `command()`
    // sets `in_def_body` after a `\def` head only when the definee is a
    // CONTROL_SYMBOL, so in `\def\bea{…}` the definee reaches `element` as an
    // ordinary sibling command at brace depth 0. Unfiltered, the dispatch fires
    // on it, the scan finds `\def\eea`'s definee at the same depth, and the two
    // *definition lines* pair into an ENVIRONMENT — lossless and silent, but
    // layout is destroyed.
    let defs = "\\def\\bea{\\begin{eqnarray}}\n\\def\\eea{\\end{eqnarray}}\n";
    assert_eq!(environments(defs), (0, true));
    // The genuine use still pairs — exactly one environment, not three.
    assert_eq!(environments(&format!("{defs}\\bea a \\eea\n")), (1, true));
}

#[test]
fn command_alias_without_a_closer_stays_a_command() {
    // Positive gate: no reachable closer, no pairing — and, like every other
    // gated construct, no diagnostic (a `\fi`-style closer assembled elsewhere
    // is routine macro code, not an error).
    assert_eq!(environments(&format!("{ALIAS_DEFS}\\bea a\n")), (0, true));
}

#[test]
fn orphan_alias_closer_stays_a_command() {
    // Only openers dispatch; a tail with no opener is just a macro call.
    assert_eq!(environments(&format!("{ALIAS_DEFS}a \\eea\n")), (0, true));
}

#[test]
fn command_alias_escaping_a_brace_group_demotes() {
    // An environment can never outlive the brace group its opener sits in:
    // braces are catcode structure, an alias is only a macro (issue #71's rule,
    // transcribed). Silent, as there.
    assert_eq!(
        environments(&format!("{ALIAS_DEFS}\\foo{{\\bea a}} \\eea\n")),
        (0, true)
    );
}

#[test]
fn command_alias_crossing_an_environment_demotes() {
    // A closer inside an environment the alias opened is consumed by that
    // environment's body, so the walk can never reach it (`envs == 0`).
    let src = format!("{ALIAS_DEFS}\\bea \\begin{{itemize}} \\eea \\end{{itemize}}\n");
    let (envs, _) = environments(&src);
    assert_eq!(envs, 1, "only the itemize pairs; the alias demotes");
}

#[test]
fn command_alias_behind_math_demotes() {
    // The scan does not model the `$`/`\[`/`\(` shape gates, so rather than
    // re-derive them it declines behind one — a conservative false negative.
    assert_eq!(
        environments(&format!("{ALIAS_DEFS}$x$ \\bea a \\eea\n")),
        (1, true),
        "math before the opener is fine"
    );
    assert_eq!(
        environments(&format!("{ALIAS_DEFS}\\bea $x$ a \\eea\n")),
        (0, true),
        "a closer behind a math delimiter is refused"
    );
}

#[test]
fn command_aliases_nest() {
    let defs = "\\newcommand{\\bc}{\\begin{center}}\n\\newcommand{\\ec}{\\end{center}}\n";
    assert_eq!(
        environments(&format!("{defs}\\bc \\bc x \\ec \\ec\n")),
        (2, true)
    );
}

#[test]
fn crossing_alias_pairs_refuse() {
    // `\bea \bc \eea \ec` crosses. Refusing outright is what keeps an inner
    // walk from running past the outer bound.
    let defs = format!(
        "{ALIAS_DEFS}\\newcommand{{\\bc}}{{\\begin{{center}}}}\n\\newcommand{{\\ec}}{{\\end{{center}}}}\n"
    );
    assert_eq!(
        environments(&format!("{defs}\\bea \\bc a \\eea \\ec\n")),
        (0, true)
    );
}

#[test]
fn lone_alias_environment_is_not_wrapped_in_a_paragraph() {
    // A block environment stands bare; the aliased spelling must format like the
    // spelled-out one, so it has to reach the same decision.
    let defs = "\\newcommand{\\bc}{\\begin{center}}\n\\newcommand{\\ec}{\\end{center}}\n";
    let parsed = parse(&format!("{defs}\n\\bc x \\ec\n"));
    let env = parsed
        .syntax()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::ENVIRONMENT)
        .expect("the alias pairs");
    assert_ne!(
        env.parent().map(|p| p.kind()),
        Some(SyntaxKind::PARAGRAPH),
        "a lone block environment is left bare"
    );
}

#[test]
fn comment_before_an_alias_closer_does_not_bind_into_the_body() {
    // `binding_run` classifies the next construct after an own-line `%` run. An
    // alias closer terminates the body just as `\end` does, so it is not
    // bindable — otherwise the loop opens a DOC_COMMENT and then consumes the
    // closer *inside* the body, and the environment never closes.
    let src = format!("{ALIAS_DEFS}\\bea\n  x = y\n  % note\n\\eea\n");
    assert_eq!(environments(&src), (1, true));
    let parsed = parse(&src);
    let env = parsed
        .syntax()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::ENVIRONMENT)
        .expect("the alias pairs");
    assert!(
        env.children().any(|c| c.kind() == SyntaxKind::END),
        "the environment must still be closed"
    );
}

#[test]
fn alias_body_spans_a_blank_line() {
    // Deliberately no paragraph-break anchor in the gate: unlike a conditional,
    // an alias environment is *supposed* to straddle PARAGRAPH boundaries.
    let defs = "\\newcommand{\\bc}{\\begin{center}}\n\\newcommand{\\ec}{\\end{center}}\n";
    assert_eq!(
        environments(&format!("{defs}\\bc a\n\nb \\ec\n")),
        (1, true)
    );
}

#[test]
fn an_alias_inside_math_pairs_as_its_literal_spelling_does() {
    // `math_atom` dispatches alias openers since issue #117, and it had to: the
    // environments aliases get written for (`split`, `matrix`) are math-only, so
    // a text-mode-only feature could not see the shape that issue reports at
    // all. The arm is gated exactly as the text one is, and sits beside the
    // `environment()` call that already pairs the literal spelling right here.
    let defs = "\\newcommand{\\bm}{\\begin{matrix}}\n\\newcommand{\\em}{\\end{matrix}}\n";
    assert_eq!(environments(&format!("{defs}$\\bm a \\em$\n")), (1, true));
    assert_eq!(
        environments("$\\begin{matrix} a \\end{matrix}$\n"),
        (1, true),
        "the alias must land where the spelling it stands in for does"
    );
    // Still gated: no reachable closer, so the opener stays a plain command.
    assert_eq!(environments(&format!("{defs}$\\bm a$\n")), (0, true));
}

#[test]
fn uncurated_and_verbatim_alias_targets_never_pair() {
    // Admission rules from the scan, observed through the parser.
    let unknown = "\\newcommand{\\bq}{\\begin{notreal}}\n\\newcommand{\\eq}{\\end{notreal}}\n";
    assert_eq!(environments(&format!("{unknown}\\bq a \\eq\n")), (0, true));
    // `\newcommand{\bv}{\begin{verbatim}}` does not work in TeX at all.
    let verb = "\\newcommand{\\bv}{\\begin{verbatim}}\n\\newcommand{\\ev}{\\end{verbatim}}\n";
    assert_eq!(environments(&format!("{verb}\\bv a \\ev\n")), (0, true));
}

#[test]
fn a_let_source_operand_is_not_an_opener() {
    // `\let` binds *two* names: the definee and the meaning it is given. Both are
    // mentions, not calls, so the definee filter counts slots rather than testing
    // the single word after the keyword. With only one slot skipped, the `\bc` in
    // `\let\oldbc\bc` reads as a live opener, pairs with the next stray `\ec`, and
    // wraps the prose in between in an environment nobody wrote — lossless and
    // silent, but layout is destroyed.
    let defs = "\\newcommand{\\bc}{\\begin{center}}\n\\newcommand{\\ec}{\\end{center}}\n";
    assert_eq!(
        environments(&format!(
            "{defs}\\bc x \\ec\n\\let\\oldbc\\bc\nprose\n\\ec\n"
        )),
        (1, true),
        "only the genuine pair; the `\\let` operand opens nothing"
    );
    // The closer side mirrors it, and a `\let`ted definition keyword is the
    // operand it looks like rather than a fresh countdown (`\let\a\def`).
    assert_eq!(
        environments(&format!("{defs}\\bc x \\let\\olde\\ec\ny\n\\ec\n")),
        (1, true)
    );
    assert_eq!(
        environments(&format!("{defs}\\let\\a\\def\\bc x \\ec\n")),
        (0, true),
        "a keyword consumed as an operand does not re-arm, but its slot still ran"
    );
}

#[test]
fn a_literal_begin_of_an_alias_name_is_an_ordinary_environment() {
    // `\begin{bc}` is a `bc` environment that happens to spell the alias's name.
    // The parser pairs it on its own terms (a real `\begin`/`\end`), and it must
    // not become a `center` — the signature side of that is
    // `Signatures::environment_at`, keyed on the node rather than the name.
    let defs = "\\newcommand{\\bc}{\\begin{center}}\n\\newcommand{\\ec}{\\end{center}}\n";
    assert_eq!(
        environments(&format!("{defs}\\begin{{bc}} x \\end{{bc}}\n")),
        (1, true)
    );
}

// --- one-sided aliases (issue #117) ------------------------------------------

/// The reported file's own shape: only the opener is defined, and the author
/// writes the closer out. `\bsplit` *expands to* `\begin{split}`, so `\end{split}`
/// is what closes it — nothing about that needs a partner command to exist.
const BSPLIT_DEF: &str = "\\def\\bsplit{\\begin{split}}\n";

#[test]
fn a_lone_opener_alias_pairs_with_the_literal_end() {
    assert_eq!(
        environments(&format!("{BSPLIT_DEF}\\bsplit a&=b \\end{{split}}\n")),
        (1, true)
    );
}

/// The reported document, whole: `split` is math-only, so the shape that
/// actually ships has the alias nested inside a display environment. This is
/// the case a text-mode-only feature would miss.
#[test]
fn a_lone_opener_alias_pairs_inside_a_math_environment() {
    let src = format!(
        "{BSPLIT_DEF}\\begin{{equation}}\n\\bsplit\na&=b,\\\\\nc&=d.\n\\end{{split}}\n\\end{{equation}}\n"
    );
    assert_eq!(environments(&src), (2, true));
}

/// The `END` a literal closer produces is byte-for-byte the ordinary one — a
/// `\end` plus its `NAME_GROUP`, not the bare control word an alias closer
/// leaves. Every downstream reader of `Environment::end` depends on that.
#[test]
fn a_literally_closed_alias_end_carries_its_name_group() {
    let parsed = parse(&format!("{BSPLIT_DEF}\\bsplit a \\end{{split}}\n"));
    let end = parsed
        .syntax()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::END)
        .expect("the alias pairs");
    assert!(
        end.children().any(|c| c.kind() == SyntaxKind::NAME_GROUP),
        "a literal `\\end{{split}}` closer keeps its name group"
    );
}

/// The mirror: only the *closer* is defined, and the opener is written out.
#[test]
fn a_lone_closer_alias_closes_a_literal_begin() {
    let defs = "\\def\\eeq{\\end{equation}}\n";
    assert_eq!(
        environments(&format!("{defs}\\begin{{equation}} a \\eeq\n")),
        (1, true)
    );
    let parsed = parse(&format!("{defs}\\begin{{equation}} a \\eeq\n"));
    let end = parsed
        .syntax()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::END)
        .expect("the environment closes");
    assert!(
        !end.children().any(|c| c.kind() == SyntaxKind::NAME_GROUP),
        "an alias closer is the bare control word"
    );
}

/// An alias closer naming some *other* environment is a plain command, exactly
/// as a mismatched `\end{…}` is left for the caller to unwind. Only the
/// innermost open environment's own closer terminates a body.
#[test]
fn an_alias_closer_for_another_environment_does_not_close_this_one() {
    let defs = "\\def\\eeq{\\end{equation}}\n";
    let src = format!("{defs}\\begin{{center}} a \\eeq b \\end{{center}}\n");
    assert_eq!(environments(&src), (1, true));
}

/// Both one-sided directions still run every shape gate — a declaration or an
/// inference names a *spelling*, never a pairing.
#[test]
fn one_sided_aliases_are_still_gated() {
    // No `\end{split}` anywhere: the opener stays a plain command, silently.
    assert_eq!(
        environments(&format!("{BSPLIT_DEF}\\bsplit a\n")),
        (0, true)
    );
    // The closer is stranded outside the brace group the opener sits in.
    assert_eq!(
        environments(&format!("{BSPLIT_DEF}\\foo{{\\bsplit a}} \\end{{split}}\n")),
        (0, true)
    );
    // …and behind a math delimiter the scan does not model.
    assert_eq!(
        environments(&format!("{BSPLIT_DEF}\\bsplit $x$ \\end{{split}}\n")),
        (0, true)
    );
}

/// A literal `\end{X}` whose `X` no alias opens is untouched — the index only
/// admits the names some opener alias actually targets, so an ordinary stray
/// `\end` still reports.
#[test]
fn an_unrelated_literal_end_is_not_an_alias_closer() {
    assert_eq!(
        environments(&format!("{BSPLIT_DEF}\\bsplit a \\end{{center}}\n")),
        (0, false),
        "the alias does not pair, and the stray `\\end{{center}}` still reports"
    );
}

/// The whole point of keeping the two closer maps separate: a file with both
/// spellings live must pair each with the shape it actually is.
#[test]
fn both_closer_spellings_coexist() {
    let defs = "\\newcommand{\\bea}{\\begin{eqnarray}}\n\\newcommand{\\eea}{\\end{eqnarray}}\n";
    assert_eq!(
        environments(&format!(
            "{defs}\\bea a \\eea\n\n\\bea b \\end{{eqnarray}}\n\n\\begin{{eqnarray}} c \\eea\n"
        )),
        (3, true)
    );
}

// --- declared environments (`badness.toml`; AGENTS.md decision #12) ----------

/// Resolve a declaration block, written here as JSON: the TOML surface is the
/// CLI's and is pinned in `config.rs`, since `toml` is a dependency of the root
/// crate only. The shapes are the same either way.
fn declared(json: &str) -> ResolvedDeclarations {
    serde_json::from_str::<Declarations>(json)
        .expect("declarations deserialize")
        .resolve()
        .expect("declarations resolve")
}

/// [`tree`], parsed under a declaration block — the snapshot counterpart of
/// [`declared_environments`], so the *shape* a declaration produces is pinned
/// and not just a node count.
fn declared_tree(input: &str, json: &str) -> String {
    let parsed = parse_with_declarations(input, LatexFlavor::Document, &declared(json));
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

/// [`environments`], parsed under a declaration block.
fn declared_environments(input: &str, json: &str) -> (usize, bool) {
    let parsed = parse_with_declarations(input, LatexFlavor::Document, &declared(json));
    assert_eq!(
        parsed.syntax().to_string(),
        input,
        "losslessness violated for {input:?}"
    );
    let count = parsed
        .syntax()
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::ENVIRONMENT)
        .count();
    (count, parsed.errors.is_empty())
}

/// The complex case the inferred scan cannot reach: the pair is defined
/// somewhere this file cannot see (a sibling `.sty`, or machinery no scan
/// follows), so the document carries no definition at all.
const EQNARRAY_DECL: &str =
    r#"{"environments": {"eqnarray": {"begin": ["\\bea"], "end": ["\\eea"]}}}"#;

/// The whole shape at once: the pair becomes an `ENVIRONMENT` whose delimiters
/// are the declared commands, and whose body is routed as math by the *target's*
/// curated entry. The blind reading of the same bytes is the snapshot below it.
#[test]
fn declared_alias_tree() {
    insta::assert_snapshot!(declared_tree("\\bea a^2 &= b \\eea\n", EQNARRAY_DECL));
}

/// The control: the same input with no declaration is two plain commands, so
/// the snapshot above cannot be read as something the parser did anyway.
#[test]
fn undeclared_alias_tree() {
    insta::assert_snapshot!(tree("\\bea a^2 &= b \\eea\n"));
}

#[test]
fn a_declared_alias_pairs_with_no_definition_in_the_file() {
    assert_eq!(
        declared_environments("\\bea a&=&b \\eea\n", EQNARRAY_DECL),
        (1, true)
    );
}

#[test]
fn a_declared_alias_routes_its_body_by_the_target() {
    // Behavior still comes from the curated entry for `eqnarray`, exactly as for
    // an inferred alias: the declaration supplied only the spelling.
    let parsed = parse_with_declarations(
        "\\bea x^2 \\eea\n",
        LatexFlavor::Document,
        &declared(EQNARRAY_DECL),
    );
    let root = parsed.syntax();
    assert!(root.descendants().any(|n| n.kind() == SyntaxKind::MATH));
    assert!(root.descendants().any(|n| n.kind() == SyntaxKind::SCRIPTED));
}

#[test]
fn declaring_nothing_parses_exactly_as_before() {
    let input = "\\newcommand{\\bea}{\\begin{eqnarray}}\n\\bea a\n";
    let plain = parse(input);
    let empty = parse_with_declarations(
        input,
        LatexFlavor::Document,
        &ResolvedDeclarations::default(),
    );
    assert_eq!(plain.syntax().to_string(), empty.syntax().to_string());
    assert_eq!(plain.errors.len(), empty.errors.len());
}

// The safety property that makes config admissible at all: a declaration names a
// *spelling*, never a pairing, so every shape gate still runs. These mirror the
// inferred cases above one for one — a declared alias is refused in exactly the
// situations an inferred one is.

#[test]
fn a_declared_alias_without_a_closer_still_demotes() {
    assert_eq!(declared_environments("\\bea a\n", EQNARRAY_DECL), (0, true));
}

#[test]
fn a_declared_alias_escaping_a_brace_group_still_demotes() {
    // An environment can never outlive the brace group its opener sits in, and a
    // declaration cannot buy an exception: braces are catcode structure, while a
    // declared spelling is still only a macro.
    assert_eq!(
        declared_environments("{\\bea a} \\eea\n", EQNARRAY_DECL),
        (0, true)
    );
}

#[test]
fn a_declared_alias_inside_math_pairs_too() {
    assert_eq!(
        declared_environments("$\\bea a \\eea$\n", EQNARRAY_DECL),
        (1, true)
    );
    // And is still gated there: no reachable closer, no environment.
    assert_eq!(
        declared_environments("$\\bea a$\n", EQNARRAY_DECL),
        (0, true)
    );
}

/// The issue-#117 config surface: half an entry is a whole declaration, because
/// the literal delimiter is a spelling of the other side. This is the block the
/// reporter needed and could not write — theirs was rejected for having no
/// `end`, and `end = ['\end{split}']` is not a control word.
#[test]
fn a_declared_alias_may_name_one_side_only() {
    const OPENER_ONLY: &str = r#"{"environments": {"split": {"begin": ["\\bsplit"]}}}"#;
    assert_eq!(
        declared_environments("\\bsplit a&=b \\end{split}\n", OPENER_ONLY),
        (1, true)
    );
    // Gated like any other: no closer, no environment, no diagnostic.
    assert_eq!(
        declared_environments("\\bsplit a\n", OPENER_ONLY),
        (0, true)
    );

    const CLOSER_ONLY: &str = r#"{"environments": {"eqnarray": {"end": ["\\eea"]}}}"#;
    assert_eq!(
        declared_environments("\\begin{eqnarray} a \\eea\n", CLOSER_ONLY),
        (1, true)
    );
}

#[test]
fn a_literal_begin_of_a_declared_alias_name_is_an_ordinary_environment() {
    // The node-keyed rule holds for declared aliases too: `\begin{bea}` is a
    // `bea` environment that happens to spell the alias, and inherits nothing.
    assert_eq!(
        declared_environments("\\begin{bea} x \\end{bea}\n", EQNARRAY_DECL),
        (1, true)
    );
}

#[test]
fn a_declared_verbatim_environment_captures_its_body() {
    // The parked `codeexample` knob: an environment badness cannot name, whose
    // body must not be reflowed or linted as prose.
    let parsed = parse_with_declarations(
        "\\begin{mycode}\n  not $math$ %not a comment\n\\end{mycode}\n",
        LatexFlavor::Document,
        &declared(r#"{"environments": {"mycode": {"like": "lstlisting"}}}"#),
    );
    let root = parsed.syntax();
    assert!(
        root.descendants_with_tokens()
            .any(|n| n.kind() == SyntaxKind::VERBATIM_BODY),
        "a declared verbatim environment must capture its body as one token"
    );
    assert!(root.descendants().all(|n| n.kind() != SyntaxKind::MATH));
}

#[test]
fn a_declared_alias_pairs_for_a_declared_target() {
    // `\startmyenv … \endmyenv` around an environment with no built-in
    // counterpart. The pairing is the parser's; the *behavior* of a declared
    // target reaches body routing separately.
    assert_eq!(
        declared_environments(
            "\\startmyenv a \\endmyenv\n",
            r#"{"environments": {"myenv": {
                 "like": "center", "begin": ["\\startmyenv"], "end": ["\\endmyenv"]
               }}}"#
        ),
        (1, true)
    );
}

#[test]
fn a_declaration_beats_a_definition_the_file_scan_found() {
    // Declared wins: the user is explicitly correcting the inference. The file
    // defines `\bea` as a `center` alias; the declaration says `eqnarray`, so the
    // body is math.
    let parsed = parse_with_declarations(
        "\\newcommand{\\bea}{\\begin{center}}\n\\newcommand{\\eea}{\\end{center}}\n\\bea x^2 \\eea\n",
        LatexFlavor::Document,
        &declared(EQNARRAY_DECL),
    );
    let root = parsed.syntax();
    assert_eq!(
        root.descendants()
            .filter(|n| n.kind() == SyntaxKind::ENVIRONMENT)
            .count(),
        1
    );
    assert!(
        root.descendants().any(|n| n.kind() == SyntaxKind::MATH),
        "the declared `eqnarray` target must win over the scanned `center` one"
    );
}

// Body routing reads a declared signature the way it reads a curated one: the
// declaration *is* curated data, since `like` copies a built-in entry and
// resolves against nothing else.

#[test]
fn a_declared_environment_routes_its_body_into_math() {
    // `myenv` has no built-in counterpart at all, so this is the case
    // `is_math_environment` could not answer before it consulted the context.
    let parsed = parse_with_declarations(
        "\\begin{myenv} x^2 \\end{myenv}\n",
        LatexFlavor::Document,
        &declared(r#"{"environments": {"myenv": {"like": "align"}}}"#),
    );
    let root = parsed.syntax();
    assert!(root.descendants().any(|n| n.kind() == SyntaxKind::MATH));
    assert!(root.descendants().any(|n| n.kind() == SyntaxKind::SCRIPTED));
}

#[test]
fn a_declared_alias_of_a_declared_environment_routes_its_body_too() {
    // Both halves of the `\startmyenv … \endmyenv` shape at once: the spelling
    // comes from the declaration and the behavior from the target it names.
    let parsed = parse_with_declarations(
        "\\startmyenv x^2 \\endmyenv\n",
        LatexFlavor::Document,
        &declared(
            r#"{"environments": {"myenv": {
                 "like": "align", "begin": ["\\startmyenv"], "end": ["\\endmyenv"]
               }}}"#,
        ),
    );
    let root = parsed.syntax();
    assert!(root.descendants().any(|n| n.kind() == SyntaxKind::MATH));
    assert!(root.descendants().any(|n| n.kind() == SyntaxKind::SCRIPTED));
}

#[test]
fn a_lone_declared_block_environment_is_not_wrapped_in_a_paragraph() {
    let parsed = parse_with_declarations(
        "\n\\begin{myenv} x \\end{myenv}\n",
        LatexFlavor::Document,
        &declared(r#"{"environments": {"myenv": {"like": "center"}}}"#),
    );
    let env = parsed
        .syntax()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::ENVIRONMENT)
        .expect("the environment parses");
    assert_ne!(
        env.parent().map(|p| p.kind()),
        Some(SyntaxKind::PARAGRAPH),
        "a declared block environment is left bare, like the entry it copies"
    );
}

#[test]
fn a_declaration_is_authoritative_for_the_name_it_covers() {
    // Not merged with what the scan found: the file defines `myenv` as verbatim,
    // the declaration says it behaves like `align`, and the declaration wins —
    // the body is math, not one opaque `VERBATIM_BODY` token.
    let input = "\\lstnewenvironment{myenv}{}{}\n\\begin{myenv} x^2 \\end{myenv}\n";
    let verbatim_body = |root: &SyntaxNode| {
        root.descendants_with_tokens()
            .any(|n| n.kind() == SyntaxKind::VERBATIM_BODY)
    };
    // Control, so the assertion below cannot pass vacuously: undeclared, the
    // scan does find the definition and the body *is* captured.
    assert!(verbatim_body(&parse(input).syntax()));

    let parsed = parse_with_declarations(
        input,
        LatexFlavor::Document,
        &declared(r#"{"environments": {"myenv": {"like": "align"}}}"#),
    );
    let root = parsed.syntax();
    assert!(
        !verbatim_body(&root),
        "the declaration overrides the scanned verbatim definition"
    );
    assert!(root.descendants().any(|n| n.kind() == SyntaxKind::MATH));
}

// --- statements wrap up to a top-level `;` in statementBody bodies -----------

/// The `STATEMENT` nodes in `input`, as their source text in preorder. Also
/// re-checks losslessness, as `tree` does.
fn statements(input: &str) -> Vec<String> {
    let parsed = parse(input);
    assert_eq!(
        parsed.syntax().to_string(),
        input,
        "losslessness violated for {input:?}"
    );
    assert_eq!(parsed.errors, vec![], "unexpected errors for {input:?}");
    parsed
        .syntax()
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::STATEMENT)
        .map(|n| n.text().to_string())
        .collect()
}

#[test]
fn a_picture_body_wraps_each_semicolon_run_in_a_statement() {
    // The canonical shape: one STATEMENT per `;`-terminated path statement,
    // each a child of the body's PARAGRAPH.
    insta::assert_snapshot!(tree(
        "\\begin{tikzpicture}\n  \\draw (0,0) -- (1,1);\n  \\node at (0,0) {A};\n\\end{tikzpicture}\n"
    ));
}

#[test]
fn statements_are_paragraph_children() {
    let parsed =
        parse("\\begin{tikzpicture}\n  \\draw (0,0);\n  \\draw (1,1);\n\\end{tikzpicture}\n");
    let stmts: Vec<_> = parsed
        .syntax()
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::STATEMENT)
        .collect();
    assert_eq!(stmts.len(), 2);
    for stmt in stmts {
        assert_eq!(
            stmt.parent().map(|p| p.kind()),
            Some(SyntaxKind::PARAGRAPH),
            "PARAGRAPH ⊃ STATEMENT, so paragraph structure is untouched"
        );
    }
}

#[test]
fn a_detached_label_group_and_its_lone_semicolon_join_the_statement() {
    // `{…}` after a WORD is a sibling group and `;` after `}` is its own WORD;
    // the statement owns head, groups, and terminator alike.
    assert_eq!(
        statements("\\begin{tikzpicture}\n\\node[o] at (2,3)\n{label}\n;\n\\end{tikzpicture}\n"),
        ["\\node[o] at (2,3)\n{label}\n;"]
    );
}

#[test]
fn a_semicolon_inside_a_group_optional_math_or_comment_does_not_terminate() {
    // Only a *top-level* `;` ends a statement: one nested in an argument, a
    // label, math, or a comment is content. The statement ends at the real
    // terminator instead.
    assert_eq!(
        statements(
            "\\begin{tikzpicture}\n\\node[a;b] at (0,0) {x;y} $u;v$ % c;d\n(1,1);\n\\end{tikzpicture}\n"
        ),
        ["\\node[a;b] at (0,0) {x;y} $u;v$ % c;d\n(1,1);"]
    );
}

#[test]
fn a_run_with_no_reachable_semicolon_stays_plain_paragraph_content() {
    // Recognition degrades silently, like every gated construct: a `\tikzset`
    // line, a `\foreach` header, or an in-progress edit is left unwrapped for
    // the formatter's authored-line fallback.
    assert_eq!(
        statements("\\begin{tikzpicture}\n\\tikzset{x=1cm}\n\\end{tikzpicture}\n"),
        Vec::<String>::new()
    );
}

#[test]
fn a_statement_never_crosses_a_blank_line() {
    // A blank line is the paragraph boundary; a `;` after one cannot rescue the
    // run before it. The second paragraph's run wraps on its own.
    assert_eq!(
        statements("\\begin{tikzpicture}\n\\draw (0,0)\n\n-- (1,1);\n\\end{tikzpicture}\n"),
        ["-- (1,1);"]
    );
}

#[test]
fn an_unterminated_tail_after_a_statement_stays_unwrapped() {
    assert_eq!(
        statements("\\begin{tikzpicture}\n\\draw (0,0);\n\\draw (1,1)\n\\end{tikzpicture}\n"),
        ["\\draw (0,0);"]
    );
}

#[test]
fn a_nested_environment_is_a_statement_boundary_not_statement_content() {
    // A genuine `\begin` at statement level is a sibling: the pending run is
    // abandoned (stays unwrapped) and recognition restarts after the `\end`.
    // The nested `scope` is itself a statementBody environment, so its own
    // body wraps.
    assert_eq!(
        statements(
            "\\begin{tikzpicture}\n\\draw (0,0)\n\\begin{scope}\n\\draw (2,2);\n\\end{scope}\n\\draw (1,1);\n\\end{tikzpicture}\n"
        ),
        ["\\draw (2,2);", "\\draw (1,1);"]
    );
}

#[test]
fn a_non_statement_environment_body_does_not_wrap() {
    // The flag is per environment and never inherited: an `itemize` inside a
    // `\node` label (or anywhere else) keeps prose parsing even though the
    // picture around it is a statement body.
    assert_eq!(
        statements("\\begin{itemize}\n\\item a; b\n\\end{itemize}\n"),
        Vec::<String>::new()
    );
    assert_eq!(
        statements(
            "\\begin{tikzpicture}\n\\node at (0,0) {\\begin{itemize}\\item a; b\\end{itemize}};\n\\end{tikzpicture}\n"
        ),
        ["\\node at (0,0) {\\begin{itemize}\\item a; b\\end{itemize}};"]
    );
}

#[test]
fn a_foreach_and_its_terminated_body_are_one_statement() {
    // pgffor's `\foreach … {…}` iterates a body whose trailing `;` terminates
    // the whole loop statement — the structure the authored-line rule could
    // never see.
    assert_eq!(
        statements(
            "\\begin{tikzpicture}\n\\foreach \\x in {0,1,2}\n\\draw (\\x,0) -- (\\x,1);\n\\end{tikzpicture}\n"
        ),
        ["\\foreach \\x in {0,1,2}\n\\draw (\\x,0) -- (\\x,1);"]
    );
}

#[test]
fn a_bound_comment_run_lands_inside_its_statement() {
    // An own-line `%` run binds forward into the next construct (decision #9);
    // the statement owns the construct, so it owns the bound run too.
    let input = "\\begin{tikzpicture}\n% the anchor\n\\node at (0,0) {A};\n\\end{tikzpicture}\n";
    assert_eq!(statements(input), ["% the anchor\n\\node at (0,0) {A};"]);
    let parsed = parse(input);
    let stmt = parsed
        .syntax()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::STATEMENT)
        .expect("statement");
    assert!(
        stmt.descendants()
            .any(|n| n.kind() == SyntaxKind::DOC_COMMENT),
        "the bound run is still a DOC_COMMENT"
    );
}

#[test]
fn a_demoted_begin_stays_statement_content() {
    // A `\begin` whose `\end` is unreachable before the group closes is a plain
    // command (issue #71) — in a statement body it is statement content, not a
    // boundary, mirroring the element dispatch exactly.
    let input = "\\begin{tikzpicture}\n{\\draw \\begin{pgfonlayer} (0,0);}\n\\end{tikzpicture}\n";
    let parsed = parse(input);
    assert_eq!(parsed.syntax().to_string(), input);
    // No statement forms (the run has no *top-level* `;` — it is inside the
    // group), and the demoted `\begin{pgfonlayer}` opens no environment: the
    // only ENVIRONMENT is the picture itself.
    let root = parsed.syntax();
    assert!(
        !root
            .descendants()
            .any(|n| n.kind() == SyntaxKind::STATEMENT)
    );
    assert_eq!(
        root.descendants()
            .filter(|n| n.kind() == SyntaxKind::ENVIRONMENT)
            .count(),
        1
    );
}

#[test]
fn a_declared_statement_environment_wraps_like_the_entry_it_copies() {
    // `like = "tikzpicture"` copies the curated entry wholesale, statementBody
    // included, so a declared picture wraps with no code change (decision #12).
    let input = "\\begin{mypic}\n\\draw (0,0);\n\\end{mypic}\n";
    let count = |parsed: &badness_parser::parser::Parse| {
        parsed
            .syntax()
            .descendants()
            .filter(|n| n.kind() == SyntaxKind::STATEMENT)
            .count()
    };
    assert_eq!(count(&parse(input)), 0, "undeclared, the body is prose");
    let parsed = parse_with_declarations(
        input,
        LatexFlavor::Document,
        &declared(r#"{"environments": {"mypic": {"like": "tikzpicture"}}}"#),
    );
    assert_eq!(parsed.syntax().to_string(), input);
    assert_eq!(count(&parsed), 1);
}

// --- arity-directed expl3 attachment ----------------------------------------
//
// AGENTS.md decision #8's sanctioned deviation, landed through the staged
// migration TODO.md recorded: in-region colon-suffixed heads attach by
// argspec arity; `w`/`D`/colonless and the `\::n` drivers stay greedy.

/// The text of every `COMMAND` node in document order, after asserting a
/// clean, lossless parse.
fn expl3_commands(input: &str) -> Vec<String> {
    let parsed = parse(input);
    assert_eq!(parsed.syntax().to_string(), input);
    assert!(
        parsed.errors.is_empty(),
        "expected a clean parse: {:?}",
        parsed.errors
    );
    parsed
        .syntax()
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::COMMAND)
        .map(|n| n.text().to_string().trim_end().to_string())
        .collect()
}

const EXPL3_ARITY_SAMPLE: &str = "\\ExplSyntaxOn\n\\tl_set:Nn \\l_tmpa_tl { x }\n\\int_compare:nNnTF { 1 } = { 2 } { y } { n }\n\\scan_stop: { data }\n\\ExplSyntaxOff\n";

#[test]
fn expl3_arity_attaches_call_units() {
    insta::assert_snapshot!(tree(EXPL3_ARITY_SAMPLE));
}

#[test]
fn expl3_arity_head_owns_its_single_token_and_group_slots() {
    let cmds = expl3_commands("\\ExplSyntaxOn\n\\tl_set:Nn \\l_tmpa_tl { x }\n");
    assert!(cmds.contains(&"\\tl_set:Nn \\l_tmpa_tl { x }".to_string()));
    // The N argument stays a name-keyed COMMAND node of its own.
    assert!(cmds.contains(&"\\l_tmpa_tl".to_string()));
}

#[test]
fn expl3_arity_zero_arity_head_attaches_nothing() {
    let cmds = expl3_commands("\\ExplSyntaxOn\n\\scan_stop: { data }\n");
    assert!(cmds.contains(&"\\scan_stop:".to_string()));
    assert!(!cmds.iter().any(|c| c.contains("data")));
}

#[test]
fn expl3_arity_blank_line_at_paragraph_level_stays_greedy() {
    // A blank line is a paragraph separator, so the walk's element stream ends
    // there — the semantic scan reads that as running out of stream, and the
    // grammar scan mirrors it: the head falls back to greed, which likewise
    // attaches nothing across a paragraph break.
    let cmds = expl3_commands("\\ExplSyntaxOn\n\\tl_set:Nn \\l_tmpa_tl\n\n{ x }\n");
    assert!(cmds.contains(&"\\tl_set:Nn".to_string()));
    assert!(cmds.contains(&"\\l_tmpa_tl".to_string()));
}

#[test]
fn expl3_arity_blank_line_in_a_group_commits_the_prefix() {
    // Inside a brace group the element loop runs to the `}` regardless of
    // blank lines, so the unit commits its consumed prefix (the sanctioned
    // partial commit) and the rest parses as ordinary siblings.
    let cmds =
        expl3_commands("\\ExplSyntaxOn\n{ \\tl_set:Nn \\l_tmpa_tl\n\n{ x } }\n\\ExplSyntaxOff\n");
    assert!(cmds.contains(&"\\tl_set:Nn \\l_tmpa_tl".to_string()));
}

#[test]
fn expl3_arity_head_inside_math_stays_greedy() {
    // The scan refuses in-math heads outright: an N slot facing the enclosing
    // math's closer would swallow it into the head and leave the math
    // unclosed (`xo-grid.dtx`'s `\cs_set_nopar:Npn \]{…}` inside the `\[…\]`
    // the previous definition opened). `expl3_commands` asserting a clean
    // parse is the real pin — the swallowed closer surfaced as an error.
    let cmds = expl3_commands(
        "\\ExplSyntaxOn\n\\cs_set_nopar:Npn \\[{\\begin{displaymath}}\n\\cs_set_nopar:Npn \\]{\\end{displaymath}}\n\\ExplSyntaxOff\n",
    );
    assert!(cmds.contains(&"\\cs_set_nopar:Npn".to_string()));
}

#[test]
fn expl3_arity_underivable_head_stays_greedy() {
    // `w` has no derivable call-site shape; greedy attachment never consumes a
    // control word, so the head attaches nothing.
    let cmds = expl3_commands("\\ExplSyntaxOn\n\\exp_after:wN \\l_tmpa_tl\n");
    assert!(cmds.contains(&"\\exp_after:wN".to_string()));
}

#[test]
fn expl3_arity_corpus_file_roundtrips_cleanly() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/expl3_arity.tex");
    let text = std::fs::read_to_string(&path).expect("corpus file");
    let parsed = parse_with_flavor(&text, LatexFlavor::Document);
    assert_eq!(parsed.syntax().to_string(), text, "losslessness violated");
    assert!(parsed.errors.is_empty(), "clean: {:?}", parsed.errors);
}
