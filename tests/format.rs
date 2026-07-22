//! Phase 2 formatter tests. The first real rule is whitespace normalization, so
//! the output is no longer identical to the input. Behavior is pinned by
//! `tests/fixtures/formatter/<name>/{input,expected}.tex` pairs (a conventional
//! fixture layout). The `AGENTS.md` invariants — idempotence and
//! losslessness of the formatted text — are asserted on the formatted output for
//! every case.

use std::fs;
use std::path::{Path, PathBuf};

use std::collections::BTreeMap;

use badness::formatter::{
    FormatStyle, SentenceOptions, WrapMode, format, format_node_range_with_signatures,
    format_with_style, format_with_style_flavored, format_with_style_flavored_sentence,
};
use badness::parser::{LatexFlavor, LexConfig, parse, parse_with_flavor, reconstruct};
use badness::semantic::SignatureDb;

/// Assert the formatter invariants for a single clean-parsing input. Inputs the
/// parser rejects are out of scope for the formatter (it refuses them), so the
/// caller filters those out.
fn assert_format_invariants(input: &str) {
    let formatted = format(input).expect("clean input should format");

    // Idempotence: fmt(fmt(x)) == fmt(x).
    let twice = format(&formatted).expect("formatted output should re-format");
    assert_eq!(twice, formatted, "format is not idempotent for {input:?}");

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
    // Structured math: scripts, a strippable braced script, a kept multi-char
    // braced script, a group base, and display math — the new lowering must keep
    // all invariants (idempotent, clean, lossless).
    r"$x^{2} + a_i^{n+1} + {a+b}^2$",
    // Operators glued into a `WORD` are split into atoms and spaced (`a+2*1^5` ->
    // `a + 2 * 1^5`), unary signs stay tight (`-x`, `x=-b`), and the split must
    // stay idempotent, clean, and lossless.
    r"$a+2*1^5$ and $x=-b$ and $-x+1$ and $2*-1$ and $a<=b$",
    r"\[ x ^ 2 \quad y_\alpha \]",
    // `\left … \right` matched pairs: nested, scripted, and a control-word
    // delimiter — the new lowering must stay idempotent, clean, and lossless.
    r"$\left[ \left( a \right) \right]^2 + \left\langle x \right\rangle$",
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
    // Argument-taking verbatim environment: args structured, body opaque.
    "\\begin{minted}[frame=single]{python}\nprint(\"$x$\")  # raw\n\\end{minted}\n",
    // Verbatim-argument commands: brace and delimiter forms, a leading-arg
    // command, and — crucially — a brace argument that spans a line break, which
    // must be emitted whole (not truncated at its newline).
    r"see \url{http://x.com/a_b} and \code{$x_y$} inline",
    r"\lstinline|a_$b$_c| then \mintinline{python}{x = $1}",
    "given by \\code{\nmulti-line $verbatim$ body with a_b} and more text here\n",
    // A comment-only line inside an alignment is kept as a passthrough line between
    // the grid rows (not a cell, not counted toward column widths); the invariants
    // (idempotent, clean, lossless) must still hold.
    "\\begin{aligned}\n & a & & b \\\\\n % & long commented-out row & & y \\\\\n & c & & d \\\\\n\\end{aligned}\n",
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
    // A `%` that trails `\begin{…}` on the same source line (the space-suppression
    // idiom) rides the `\begin` header instead of dropping to its own indented
    // line; a `%` the author put on its own line is left there.
    ("environment_begin_trailing_comment", WrapMode::Reflow, 80),
    // A `%` run on its *own* line(s) immediately before a command or environment
    // binds *leading* into that construct (the parser's leading comment-bind) and
    // is rendered on its own line above `\section` / `\begin`, at the construct's
    // indentation — not lifted onto the header line the way a same-line `%` is.
    ("comment_binds_leading_to_construct", WrapMode::Reflow, 80),
    // A class-defined verbatim environment (jss's `Code`) has its body preserved
    // byte-for-byte — never reindented or reflowed.
    ("verbatim_jss_code_environment", WrapMode::Preserve, 80),
    // Arity from a *scanned* definition (not the built-in DB): the document's own
    // `\newenvironment`/`\NewDocumentEnvironment` arg is glued onto the `\begin`.
    ("environment_user_defined_glued", WrapMode::Preserve, 80),
    ("environment_xparse_glued", WrapMode::Preserve, 80),
    ("verbatim_in_environment", WrapMode::Preserve, 80),
    // An argument-taking verbatim environment: the `[options]` are kept verbatim on
    // the (indented) `\begin` line, while the opaque body is emitted byte-for-byte.
    ("verbatim_argument_environment", WrapMode::Preserve, 80),
    // Group / argument indentation.
    ("group_indents_body", WrapMode::Preserve, 80),
    ("optional_indents_body", WrapMode::Preserve, 80),
    ("nested_groups", WrapMode::Preserve, 80),
    ("group_single_line_stays_inline", WrapMode::Preserve, 80),
    ("group_reindents", WrapMode::Preserve, 80),
    // A `%` glued to the open delimiter (`{%`, no newline between) rides on the
    // open-delimiter line instead of dropping to its own indented line: otherwise
    // the newline the formatter inserts after `{` becomes real whitespace inside
    // the group, turning `\textt{%\n}` (empty group) into `\textt{ }`.
    ("group_comment_rides_open_brace", WrapMode::Preserve, 80),
    // Paragraph reflow (the new rule).
    ("reflow_join_short", WrapMode::Reflow, 80),
    ("reflow_wrap_to_width", WrapMode::Reflow, 40),
    ("reflow_tie_no_break", WrapMode::Reflow, 12),
    ("reflow_forced_break", WrapMode::Reflow, 80),
    ("reflow_forced_break_with_optarg", WrapMode::Reflow, 80),
    ("reflow_comment_ends_line", WrapMode::Reflow, 80),
    ("reflow_comment_own_line", WrapMode::Reflow, 80),
    ("reflow_in_environment", WrapMode::Reflow, 20),
    // A physical line that is solely command(s) — `\usepackage{…}` lines, a
    // `\section{…}` header — stays on its own line; the prose around it still
    // reflows.
    ("reflow_command_lines_preserved", WrapMode::Reflow, 80),
    // List environments (`itemize`/`enumerate`/`description`): each `\item` on
    // its own line, the body reflowed with continuation lines hanging-indented at
    // the control word's width (`\item `). A `description` `[label]` trails on the
    // first line but does *not* widen the hang, so the body keeps one left edge
    // regardless of label width (a nested list and a blank line between items are
    // both reproduced).
    ("reflow_list_hanging_indent", WrapMode::Reflow, 72),
    ("reflow_list_item_label", WrapMode::Reflow, 60),
    ("reflow_list_nested", WrapMode::Reflow, 50),
    ("reflow_list_blank_between_items", WrapMode::Reflow, 80),
    // Prose-argument reflow: a signature-marked prose argument reflows like a
    // paragraph — joined when short, wrapped when long — while non-prose groups
    // (`\newcommand` body, `\label`) are left exactly as authored. An `inline`-
    // flagged prose command (`\footnote`, `\emph`, …) flattens into the surrounding
    // text so its body wraps as running prose with `{`/`}` glued to the adjacent
    // words; a block-level prose command (`\section`, `\caption`) block-breaks its
    // braces onto their own lines instead.
    ("reflow_prose_arg_wraps", WrapMode::Reflow, 40),
    ("reflow_prose_arg_joins_short", WrapMode::Reflow, 80),
    ("reflow_prose_arg_optional_omitted", WrapMode::Reflow, 30),
    ("reflow_non_prose_preserved", WrapMode::Reflow, 40),
    // A multi-line brace-group body (a `\newcommand` definition body) is laid out as
    // code-like *statements*: an over-long line wraps to the width — breaking before
    // a trailing `{…}` atom — instead of forcing the printer to detonate the
    // innermost nested prose group (`\textbf`'s argument), the only soft break a
    // rigid body would expose. The continuation is flush (idempotent: it re-parses as
    // a line already at the body indent).
    ("reflow_brace_body_wraps", WrapMode::Reflow, 80),
    // Statement reflow preserves the author's statement-per-line structure: two
    // `\draw …;` lines (each carrying words, so *not* command-only) stay on their own
    // lines rather than rejoining into one fill the way prose reflow would.
    (
        "reflow_brace_body_statements_preserved",
        WrapMode::Reflow,
        80,
    ),
    // A class redefines `\section` via `\renewcommand{\section}{\secdef …}` (jss's
    // idiom). The static scanner reads that body as arity 0, but the trust gate
    // (`semantic::define`) refuses to let a delegating redefinition downgrade the
    // curated built-in, so `\section` keeps its `prose` title: padding collapses and
    // an over-width heading still hangs and reflows.
    ("reflow_secdef_redef_keeps_prose", WrapMode::Reflow, 40),
    ("reflow_prose_arg_blank_line", WrapMode::Reflow, 40),
    ("reflow_prose_arg_nested_in_paragraph", WrapMode::Reflow, 50),
    ("reflow_inline_prose_in_paragraph", WrapMode::Reflow, 50),
    ("reflow_caption_block", WrapMode::Reflow, 40),
    // A signature-marked collapsible token list (`\citep` and the cite family, via
    // the DB's `collapse` arg flag): a key list written across lines folds to one
    // line, and the `inline`-flagged command flows into the paragraph fill as an
    // atom instead of being kept on its own line — so the multi-line and one-line
    // authored forms format identically (determinism). The interior is collapsed,
    // never reflowed (the keys stay together). A `%` comment inside the list is not
    // safely collapsible, so it keeps the indented block form.
    ("reflow_cite_collapses_and_flows", WrapMode::Reflow, 80),
    ("reflow_cite_comment_keeps_block", WrapMode::Reflow, 80),
    // The cross-reference family (`\ref`, `\eqref`, `\cref`, `\nameref`, …) is
    // flagged `inline` but *not* `collapse` (a ref key is a single token where
    // interior spaces can matter). A ref isolated on its own source line flows
    // into the paragraph fill as an atom instead of being kept as a command-only
    // line, with its `{key}` left exactly as authored.
    ("reflow_ref_flows", WrapMode::Reflow, 80),
    // Math formatting (Stage A): aggressive intra-math spacing — collapse runs,
    // trim just inside the delimiters, tight `^`/`_` scripts, and strip redundant
    // braces around a single-token script argument (only where the following
    // token would not glue onto it). A comment inside math forces a line break so
    // it cannot swallow the closing delimiter.
    ("math_collapse_spaces", WrapMode::Preserve, 80),
    ("math_trim_delims", WrapMode::Preserve, 80),
    ("math_tight_scripts", WrapMode::Preserve, 80),
    // A single space is placed around every binary/relation operator (the parser
    // splits a `WORD` glued around `+ - * / = < >` into atoms; command operators
    // like `\cdot` join them via the role model). A unary `+`/`-` with no left
    // operand stays glued (`-x`, `x=-b`, `2^{-5}`), scripts stay tight, and group
    // bodies are normalized too (`x^{a+b}` -> `x^{a + b}`). Scientific notation
    // (`1e-5`) is deliberately not special-cased.
    ("math_op_spacing", WrapMode::Preserve, 80),
    ("math_strip_single_token_braces", WrapMode::Preserve, 80),
    ("math_keep_multichar_braces", WrapMode::Preserve, 80),
    ("math_comment_breaks", WrapMode::Preserve, 80),
    // Display math (`\[…\]`, `$$…$$`) is a block: the delimiters land on their own
    // lines with the body collapsed and indented one level, so `\[ F \]` never
    // stays cramped on a single line the way inline `$ x $` does.
    ("math_display_block", WrapMode::Preserve, 80),
    ("math_display_dollars", WrapMode::Preserve, 80),
    // A display equation too wide for the line breaks before its top-level
    // binary/relation operators (amsmath style): the first relation stays on the
    // opening line and anchors a hanging indent, and each `+` term starts a fresh
    // continuation line aligned under the first term after `=`. Whatever fits
    // still stays on one line.
    ("math_display_break_operators", WrapMode::Preserve, 80),
    // A chain of relations aligns in a column: the second `=` starts a fresh
    // continuation line under the first `=`, not under the first right-hand-side
    // term (the two-level rule — relations align, binaries hang one relation-width
    // deeper).
    ("math_display_break_relations", WrapMode::Preserve, 80),
    // A break before a top-level binary operator does not gain a spurious space at
    // a tight command boundary (`\gamma)`, `}.` stay tight, role-aware like the
    // inline seq path), and an operator nested in parentheses is not a top-level
    // break point (the `-` of `(1 - \gamma)` must not split across lines).
    ("math_display_break_paren_tight", WrapMode::Preserve, 80),
    // `\left … \right` matched pairs: lowered tight to their delimiters (the body
    // trimmed just inside), with nesting and scripts on the whole pair. A
    // control-word delimiter (`\langle`) keeps one space so the body cannot glue
    // onto it (`\left\langlex` would re-lex as one control word).
    ("math_left_right", WrapMode::Preserve, 80),
    ("math_left_right_control_word_delim", WrapMode::Preserve, 80),
    ("math_left_right_nested_scripted", WrapMode::Preserve, 80),
    // Alignment-aware formatting: an `align`/matrix-family environment lays its `&`
    // columns into a grid (left-aligned, single space around `&`, last cell never
    // padded), preserving the row break (with its `[len]`). A lone interior newline
    // in a cell is a continuation line and joins onto its aligned row. A nested
    // block environment (`aligned`, `cases`, a matrix) in the *last* cell of a row
    // keeps the grid: the cell renders multi-line, its later lines hanging at the
    // nested `\begin{…}` column (so the `\end{…}` sits directly under it), and
    // takes no part in column widths. A cell that still
    // cannot sit on the grid (a nested block before a `&`, or a blank line inside
    // the cell) falls back to the plain indented body — while a nested alignment
    // environment is still aligned in its own right.
    ("align_columns_basic", WrapMode::Preserve, 80),
    ("align_columns_uneven_rows", WrapMode::Preserve, 80),
    ("align_columns_linebreak_optional", WrapMode::Preserve, 80),
    ("align_continuation_join", WrapMode::Preserve, 80),
    ("pmatrix_columns", WrapMode::Preserve, 80),
    ("align_nested_block_cell", WrapMode::Preserve, 80),
    ("align_nested_aligned_cell", WrapMode::Preserve, 80),
    // The block-cell layout recurses (a grid inside a grid inside a grid) and
    // survives a wrapper around the nested environment (`\left…\right`, a group):
    // the hang anchors at the first node of the cell that cannot stay flat, and
    // the wrapper's own body alignment keeps the nested `\end{…}` under its
    // `\begin{…}` (one column inside the opening delimiter).
    ("align_nested_recursive", WrapMode::Preserve, 80),
    ("align_nested_left_right_cell", WrapMode::Preserve, 80),
    (
        "align_nested_block_mid_row_fallback",
        WrapMode::Preserve,
        80,
    ),
    ("align_blank_line_in_cell_fallback", WrapMode::Preserve, 80),
    // Comments and rule lines in an alignment grid: a comment-only line is kept as
    // a passthrough between rows (not counted toward column widths); an end-of-line
    // comment trails its row after the `\\`; a mid-row comment (more cells follow)
    // would comment them out, so it falls back to the plain indented body. With the
    // table environments now flagged `align`, `tabular`/`array` grid-align their
    // cells with `\hline`/booktabs rules preserved as passthrough lines.
    ("align_comment_only_line", WrapMode::Preserve, 80),
    ("align_trailing_comment", WrapMode::Preserve, 80),
    ("align_comment_mid_row_fallback", WrapMode::Preserve, 80),
    ("tabular_hline", WrapMode::Preserve, 80),
    ("tabular_booktabs", WrapMode::Preserve, 80),
    // A rule command (`\toprule`) on its own line whose next line opens with a
    // braced cell (`{Scenario}`): the greedy parser glues the `{…}` onto the rule
    // as a bogus argument, but arity refinement peels it back so the rule stays a
    // passthrough line and the cell rejoins its row and grid-aligns.
    ("align_rule_overattached_cell", WrapMode::Preserve, 80),
    ("array_columns", WrapMode::Preserve, 80),
    // Column-spec-aware L/C/R alignment: cells align per the `{lcr}` spec, a
    // right-aligned numeric column pads on the left (no trailing whitespace), a
    // `\multicolumn` spans its columns, `p{…}` reads as left, `\cmidrule(lr){2-3}`
    // and same-line `\\ \hline` stay passthrough lines, and an unknown spec falls
    // back to all-left.
    ("tabular_align_lcr", WrapMode::Preserve, 80),
    ("tabular_align_right_numeric", WrapMode::Preserve, 80),
    ("tabular_multicolumn", WrapMode::Preserve, 80),
    ("tabular_cmidrule_trim", WrapMode::Preserve, 80),
    ("tabular_rule_same_line", WrapMode::Preserve, 80),
    ("tabular_pmb_left", WrapMode::Preserve, 80),
    ("tabular_unknown_spec_fallback", WrapMode::Preserve, 80),
    // Named math environments parse in math mode (their body is a `MATH` node), so
    // they format math-aware like `\[…\]`: a single-formula `equation` breaks at its
    // top-level relations (the relation column aligns the continuation lines); a
    // `gather` stacks its `\\` rows; an `align` grid lays its `&` columns with
    // role-aware cell spacing (`x&=a+b` normalizes to `x & = a + b`).
    ("math_env_equation", WrapMode::Preserve, 80),
    ("math_env_gather", WrapMode::Preserve, 80),
    ("math_env_align_spacing", WrapMode::Preserve, 80),
    // expl3 code formatting in a `.tex` document. A `~` is the catcode-10 literal
    // space and breaks like an ordinary (breakable) space when a line overflows,
    // staying at the line end. An inline `\ExplSyntaxOn … \ExplSyntaxOff` island
    // amid running prose is split out and laid out as code, the surrounding prose
    // still reflowing.
    ("reflow_expl_tilde_breaks", WrapMode::Reflow, 40),
    ("reflow_expl_straddle", WrapMode::Reflow, 80),
    // Sentence wrap (`WrapMode::Sentence`): one sentence per line, line width
    // ignored. Boundary detection is the English abbreviation profile
    // (`formatter::sentence`): a `.`/`!`/`?` ends a sentence unless the word is a
    // known abbreviation (`e.g.`, `Dr.`, `Fig.~`), an ellipsis (`...`/`…`), or a
    // contextual abbreviation whose following word signals the sentence continues
    // (`U.S. Government` stays; `u.s. However` splits). Inline math ending in `.`
    // (`$x$.`) breaks; a `.` *inside* math (`$a.b$`) does not. `sentence` reaches
    // every prose context reflow does — list items keep their hanging indent, a
    // `\caption{…}` prose argument sentence-wraps inside its block. Width is
    // ignored even at width 20 (`sentence_long_no_width_break`).
    ("sentence_basic", WrapMode::Sentence, 80),
    ("sentence_abbreviations", WrapMode::Sentence, 80),
    ("sentence_ellipsis", WrapMode::Sentence, 80),
    ("sentence_contextual_abbrev", WrapMode::Sentence, 80),
    ("sentence_inline_math", WrapMode::Sentence, 80),
    ("sentence_list_items", WrapMode::Sentence, 80),
    ("sentence_caption", WrapMode::Sentence, 80),
    ("sentence_long_no_width_break", WrapMode::Sentence, 20),
    // Semantic wrap (`WrapMode::Semantic`, sembr): the sentence breaks above *plus*
    // preserving the author's own soft line breaks. An authored break after a comma
    // clause survives (`semantic_preserve_authored_break`), and a run-on sentence on
    // one source line is still sentence-split (`semantic_adds_sentence_break`).
    ("semantic_preserve_authored_break", WrapMode::Semantic, 80),
    ("semantic_adds_sentence_break", WrapMode::Semantic, 80),
];

fn fixture_path(name: &str, file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formatter")
        .join(name)
        .join(file)
}

/// Package/class fixtures under `tests/fixtures/formatter/<name>/`, each an
/// `input.<ext>` + `expected.<ext>` pair where `<ext>` is `sty` or `cls`. They are
/// parsed and formatted under the [`LatexFlavor::Package`] flavor (`@` is a letter
/// throughout, the implicit `\makeatletter`) and default to [`WrapMode::Preserve`]
/// (package bodies are code, not prose), exactly as the CLI/LSP resolve a
/// `.sty`/`.cls` file.
const PACKAGE_FIXTURES: &[(&str, &str)] = &[
    ("package_at_letter_command", "sty"),
    ("class_provides_preserve", "cls"),
    // expl3 code formatting under the package flavor's default `Preserve` wrap:
    // inside an expl3 region (catcode-9 whitespace / catcode-10 `~`) the formatter
    // owns layout regardless of wrap mode — messy indentation is normalized, a
    // function body becomes an indented block, short brace arguments stay inline
    // (`{ #1 }`), and `#1#2` parameters glue tight.
    ("expl_function_def", "sty"),
    ("expl_inline_vs_block_groups", "sty"),
];

#[test]
fn package_fixtures_match_expected() {
    for &(name, ext) in PACKAGE_FIXTURES {
        let style = FormatStyle {
            wrap: WrapMode::Preserve,
            ..FormatStyle::default()
        };
        let input = fs::read_to_string(fixture_path(name, &format!("input.{ext}")))
            .unwrap_or_else(|e| panic!("read {name}/input.{ext}: {e}"));
        let expected = fs::read_to_string(fixture_path(name, &format!("expected.{ext}")))
            .unwrap_or_else(|e| panic!("read {name}/expected.{ext}: {e}"));

        // Under the package flavor the input must parse cleanly (in particular, no
        // spurious diagnostics from `@`-bearing control words mis-lexing).
        assert!(
            parse_with_flavor(&input, LatexFlavor::Package)
                .errors
                .is_empty(),
            "fixture {name} input must parse cleanly under the package flavor"
        );

        let formatted = format_with_style_flavored(&input, style, LatexFlavor::Package)
            .unwrap_or_else(|e| panic!("format {name}: {e}"));
        assert_eq!(formatted, expected, "fixture {name} output mismatch");

        // Idempotent (same flavor + style), clean, and lossless.
        assert_eq!(
            format_with_style_flavored(&formatted, style, LatexFlavor::Package).expect("reformat"),
            formatted,
            "fixture {name} is not idempotent"
        );
        let reparsed = parse_with_flavor(&formatted, LatexFlavor::Package);
        assert!(
            reparsed.errors.is_empty(),
            "fixture {name} formatted output must parse cleanly"
        );
        assert_eq!(
            reparsed.syntax().to_string(),
            formatted,
            "fixture {name} formatted output must round-trip losslessly"
        );
    }
}

/// `.dtx` (docstrip) fixtures under `tests/fixtures/formatter/<name>/`, each an
/// `input.dtx` + `expected.dtx` pair. They are parsed and formatted under the
/// docstrip [`LexConfig`] (`dtx: true`, `Document` flavor) and default to
/// [`WrapMode::Preserve`], exactly as the CLI/LSP resolve a `.dtx` file. The
/// two-layer rules are pinned here: documentation margins (`%`) and docstrip
/// guards (`%<…>`) stay byte-for-byte at column 0, a `macrocode` body formats as
/// code at a column-0 base, and a documentation-layer environment's frames are
/// never reindented or split.
const DTX_FIXTURES: &[&str] = &[
    "dtx_macrocode_basic",
    "dtx_macrocode_nested_groups",
    "dtx_prose_itemize",
    "dtx_guards",
    "dtx_driver",
    "dtx_margin_blank_line",
];

/// The docstrip config a `.dtx` file resolves to (`FileKind::Dtx`).
fn dtx_config() -> LexConfig {
    LexConfig {
        flavor: LatexFlavor::Document,
        dtx: true,
    }
}

#[test]
fn dtx_fixtures_match_expected() {
    for &name in DTX_FIXTURES {
        let style = FormatStyle {
            wrap: WrapMode::Preserve,
            ..FormatStyle::default()
        };
        let input = fs::read_to_string(fixture_path(name, "input.dtx"))
            .unwrap_or_else(|e| panic!("read {name}/input.dtx: {e}"));
        let expected = fs::read_to_string(fixture_path(name, "expected.dtx"))
            .unwrap_or_else(|e| panic!("read {name}/expected.dtx: {e}"));

        // Under the docstrip config the input must parse cleanly.
        assert!(
            parse_with_flavor(&input, dtx_config()).errors.is_empty(),
            "fixture {name} input must parse cleanly under the dtx config"
        );

        let formatted = format_with_style_flavored(&input, style, dtx_config())
            .unwrap_or_else(|e| panic!("format {name}: {e}"));
        assert_eq!(formatted, expected, "fixture {name} output mismatch");

        // Idempotent (same config + style), clean, and lossless.
        assert_eq!(
            format_with_style_flavored(&formatted, style, dtx_config()).expect("reformat"),
            formatted,
            "fixture {name} is not idempotent"
        );
        let reparsed = parse_with_flavor(&formatted, dtx_config());
        assert!(
            reparsed.errors.is_empty(),
            "fixture {name} formatted output must parse cleanly"
        );
        assert_eq!(
            reparsed.syntax().to_string(),
            formatted,
            "fixture {name} formatted output must round-trip losslessly"
        );
    }
}

/// `.dtx` reflow fixtures: `(name, line_width)`. Formatted under the docstrip
/// [`LexConfig`] like [`DTX_FIXTURES`] but with [`WrapMode::Reflow`] and a narrow
/// width, so the documentation *prose* layer rewraps while a canonical `% ` margin
/// is re-emitted on every wrapped line. Structured content (margin-framed lists,
/// `macrocode` frames) and the `%`-only paragraph separator must round-trip
/// byte-for-byte; only running prose reflows.
const DTX_REFLOW_FIXTURES: &[(&str, usize)] = &[
    // A single long doc line wrapped onto several `% ` lines.
    ("dtx_reflow_prose_wrap", 50),
    // Short lines join; a `%no-space` margin normalizes to `% `.
    ("dtx_reflow_prose_joins", 80),
    // The `%`-only separator round-trips; the two paragraphs rewrap independently
    // (the second one's leading margin floats out of its paragraph).
    ("dtx_reflow_margin_blank_line", 80),
    // A margin-framed `itemize` stays byte-identical (no item-line reflow).
    ("dtx_reflow_itemize", 50),
];

#[test]
fn dtx_reflow_fixtures_match_expected() {
    for &(name, line_width) in DTX_REFLOW_FIXTURES {
        let style = FormatStyle {
            wrap: WrapMode::Reflow,
            line_width,
            ..FormatStyle::default()
        };
        let input = fs::read_to_string(fixture_path(name, "input.dtx"))
            .unwrap_or_else(|e| panic!("read {name}/input.dtx: {e}"));
        let expected = fs::read_to_string(fixture_path(name, "expected.dtx"))
            .unwrap_or_else(|e| panic!("read {name}/expected.dtx: {e}"));

        // Under the docstrip config the input must parse cleanly.
        assert!(
            parse_with_flavor(&input, dtx_config()).errors.is_empty(),
            "fixture {name} input must parse cleanly under the dtx config"
        );

        let formatted = format_with_style_flavored(&input, style, dtx_config())
            .unwrap_or_else(|e| panic!("format {name}: {e}"));
        assert_eq!(formatted, expected, "fixture {name} output mismatch");

        // No reflowed line exceeds the width (a fill never overflows except an
        // unbreakable atom wider than the line, which these fixtures avoid).
        for line in formatted.lines() {
            assert!(
                line.chars().count() <= line_width,
                "fixture {name} line exceeds width {line_width}: {line:?}"
            );
        }

        // Idempotent (same config + style), clean, and lossless.
        assert_eq!(
            format_with_style_flavored(&formatted, style, dtx_config()).expect("reformat"),
            formatted,
            "fixture {name} is not idempotent"
        );
        let reparsed = parse_with_flavor(&formatted, dtx_config());
        assert!(
            reparsed.errors.is_empty(),
            "fixture {name} formatted output must parse cleanly"
        );
        assert_eq!(
            reparsed.syntax().to_string(),
            formatted,
            "fixture {name} formatted output must round-trip losslessly"
        );
    }
}

/// `.ins` (docstrip installation script) fixtures under
/// `tests/fixtures/formatter/<name>/`, each an `input.ins` + `expected.ins` pair.
/// A `.ins` is a driver TeX runs directly, so — unlike a `.dtx` — it is parsed as
/// plain `Document`-flavored LaTeX with the docstrip mode *off* (`dtx: false`):
/// a leading `%` stays an ordinary comment (never a `DOC_MARGIN`), so commented-out
/// driver lines are protected. It defaults to [`WrapMode::Preserve`] (it is code),
/// exactly as the CLI/LSP resolve a `.ins` file (`FileKind::Ins`).
const INS_FIXTURES: &[&str] = &["ins_driver"];

/// The config a `.ins` file resolves to (`FileKind::Ins`): plain `Document`
/// flavor, no docstrip mode.
fn ins_config() -> LexConfig {
    LexConfig::from(LatexFlavor::Document)
}

#[test]
fn ins_fixtures_match_expected() {
    for &name in INS_FIXTURES {
        let style = FormatStyle {
            wrap: WrapMode::Preserve,
            ..FormatStyle::default()
        };
        let input = fs::read_to_string(fixture_path(name, "input.ins"))
            .unwrap_or_else(|e| panic!("read {name}/input.ins: {e}"));
        let expected = fs::read_to_string(fixture_path(name, "expected.ins"))
            .unwrap_or_else(|e| panic!("read {name}/expected.ins: {e}"));

        assert!(
            parse_with_flavor(&input, ins_config()).errors.is_empty(),
            "fixture {name} input must parse cleanly under the ins config"
        );

        let formatted = format_with_style_flavored(&input, style, ins_config())
            .unwrap_or_else(|e| panic!("format {name}: {e}"));
        assert_eq!(formatted, expected, "fixture {name} output mismatch");

        // Idempotent (same config + style), clean, and lossless.
        assert_eq!(
            format_with_style_flavored(&formatted, style, ins_config()).expect("reformat"),
            formatted,
            "fixture {name} is not idempotent"
        );
        let reparsed = parse_with_flavor(&formatted, ins_config());
        assert!(
            reparsed.errors.is_empty(),
            "fixture {name} formatted output must parse cleanly"
        );
        assert_eq!(
            reparsed.syntax().to_string(),
            formatted,
            "fixture {name} formatted output must round-trip losslessly"
        );
    }
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

/// The `sentence`/`semantic` language profile is config-driven: the German profile
/// keeps `bzw.` from ending a sentence, while the default English profile does not
/// know it and splits there. User `no-break-abbreviations` merge on top of the
/// built-in list. Exercises the [`SentenceOptions`] plumbing the fixture table
/// (English default) cannot reach.
#[test]
fn sentence_wrap_language_and_user_abbreviations() {
    let style = FormatStyle {
        wrap: WrapMode::Sentence,
        line_width: 80,
        ..FormatStyle::default()
    };
    let input = "Das ist eins bzw. zwei. Und drei.\n";

    // English (the default) does not know `bzw.`, so it ends a sentence there.
    let english = format_with_style_flavored_sentence(
        input,
        style,
        LatexFlavor::Document,
        SentenceOptions::default(),
    )
    .expect("format");
    assert_eq!(english, "Das ist eins bzw.\nzwei.\nUnd drei.\n");

    // The German profile suppresses the break after `bzw.`.
    let de = SentenceOptions::from_lang(Some("de"));
    let german = format_with_style_flavored_sentence(input, style, LatexFlavor::Document, de)
        .expect("format");
    assert_eq!(german, "Das ist eins bzw. zwei.\nUnd drei.\n");
    // Idempotent under the same options.
    assert_eq!(
        format_with_style_flavored_sentence(&german, style, LatexFlavor::Document, de)
            .expect("reformat"),
        german,
    );

    // A user `no-break-abbreviations` entry (the `default` bucket) suppresses a
    // break after an otherwise-unknown abbreviation (`foo.`).
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    map.insert("default".to_string(), vec!["foo.".to_string()]);
    let mut scratch = Vec::new();
    let opts = SentenceOptions::resolve(None, &map, &mut scratch);
    let user = format_with_style_flavored_sentence(
        "See foo. Then more here. Done.\n",
        style,
        LatexFlavor::Document,
        opts,
    )
    .expect("format");
    assert_eq!(user, "See foo. Then more here.\nDone.\n");
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

#[test]
fn stable_preserves_an_equilibrium_break_that_reflow_removes() {
    // The soft target is `line_width - 15` (see `FormatStyle::stable_wrap_target`),
    // so a width of 40 targets column 25.
    let input = "Alpha beta gamma delta epsilon.\nZeta eta theta iota kappa lambda.\n";
    let stable = FormatStyle {
        line_width: 40,
        wrap: WrapMode::Stable,
        ..FormatStyle::default()
    };
    assert_eq!(
        format_with_style(input, stable).expect("stable formats"),
        input,
        "an authored break at the soft target must remain stable"
    );

    let reflow = FormatStyle {
        line_width: 40,
        wrap: WrapMode::Reflow,
        ..FormatStyle::default()
    };
    assert_ne!(
        format_with_style(input, reflow).expect("reflow formats"),
        input,
        "canonical reflow should not prefer the authored boundary"
    );
}

#[test]
fn stable_repairs_overflow_locally_and_is_idempotent() {
    // Width 60 targets column 45 (`line_width - 15`).
    let input = "This stable opening line reaches the target today.\n\
This edited middle line is now much too long for the configured hard width here.\n\
This following boundary reaches the target safely today.\n";
    let style = FormatStyle {
        line_width: 60,
        wrap: WrapMode::Stable,
        ..FormatStyle::default()
    };
    let formatted = format_with_style(input, style).expect("stable formats");
    assert!(
        formatted.starts_with("This stable opening line reaches the target today.\n"),
        "the preceding equilibrium boundary should remain fixed: {formatted:?}"
    );
    assert!(
        formatted.lines().all(|line| line.chars().count() <= 60),
        "stable wrapping must honor the hard width: {formatted:?}"
    );
    assert_eq!(
        format_with_style(&formatted, style).expect("reformat"),
        formatted,
        "stable wrapping must be idempotent"
    );
}

#[test]
fn stable_rebalances_only_unequilibrated_regions() {
    let input = "The opening line is already safely within the accepted range today.\n\
This edited middle line has become too long for the configured width here.\n\
while this following line can donate some nearby space today.\n\
Finally this boundary should remain exactly where it is.\n\n\
This second opening line is also an acceptable stable anchor today.\n\
A shortened line now needs a few more nearby words.\n\
from this following line which has enough content to share with it today.\n\
The final short line remains a valid paragraph ending.\n";
    let expected = "The opening line is already safely within the accepted range today.\n\
This edited middle line has become too long for the configured width\n\
here. while this following line can donate some nearby space today.\n\
Finally this boundary should remain exactly where it is.\n\n\
This second opening line is also an acceptable stable anchor today.\n\
A shortened line now needs a few more nearby words. from\n\
this following line which has enough content to share with it today.\n\
The final short line remains a valid paragraph ending.\n";
    // Width 70 targets column 55 (`line_width - 15`).
    let style = FormatStyle {
        line_width: 70,
        wrap: WrapMode::Stable,
        ..FormatStyle::default()
    };
    let formatted = format_with_style(input, style).expect("stable formats");
    assert_eq!(formatted, expected);
    assert_eq!(
        format_with_style(&formatted, style).expect("reformat"),
        formatted
    );
}

/// Stable wrapping claims idempotence. The cost model is idempotent by
/// construction (a solver output `b` is the unique global lex-min once fed back
/// as the `preferred` set), so the only residual risk is parse-stability: that
/// the reformatted text re-lexes to the same atoms and run segmentation. This
/// fuzzes that empirically over many pseudo-random prose paragraphs, widths, and
/// authored-break placements, asserting `fmt(fmt(x)) == fmt(x)`, the hard-width
/// bound (modulo unbreakable long words), and losslessness of the output.
#[test]
fn stable_wrapping_is_idempotent_over_random_prose() {
    // A tiny deterministic LCG (Numerical Recipes constants) — no dev-dep on a
    // PRNG crate, and reproducible across platforms/runs.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 16
        }
        fn below(&mut self, bound: usize) -> usize {
            (self.next() as usize) % bound.max(1)
        }
    }

    // Build a paragraph of random words separated by a space or a newline, with a
    // sprinkling of blank-line paragraph breaks. Words are lowercase-letter runs so
    // they never re-lex into something exotic (no commands, math, or comments).
    fn random_prose(rng: &mut Lcg) -> String {
        let mut out = String::new();
        let paragraphs = 1 + rng.below(3);
        for p in 0..paragraphs {
            if p > 0 {
                out.push_str("\n\n");
            }
            let words = 4 + rng.below(40);
            for w in 0..words {
                if w > 0 {
                    // Roughly one in three gaps is an authored newline.
                    out.push(if rng.below(3) == 0 { '\n' } else { ' ' });
                }
                let len = 1 + rng.below(12);
                for _ in 0..len {
                    out.push((b'a' + rng.below(26) as u8) as char);
                }
            }
        }
        out.push('\n');
        out
    }

    let mut rng = Lcg(0x1234_5678_9abc_def0);
    for case in 0..400 {
        let input = random_prose(&mut rng);
        // Vary the hard width; the soft target rides along at `width - 10`.
        for &line_width in &[24usize, 40, 60, 72, 90] {
            let style = FormatStyle {
                line_width,
                wrap: WrapMode::Stable,
                ..FormatStyle::default()
            };
            let once = format_with_style(&input, style)
                .unwrap_or_else(|e| panic!("case {case} @ {line_width}: format failed: {e:?}"));
            let twice = format_with_style(&once, style)
                .unwrap_or_else(|e| panic!("case {case} @ {line_width}: reformat failed: {e:?}"));
            assert_eq!(
                twice, once,
                "stable wrap not idempotent (case {case} @ width {line_width})\ninput:  {input:?}\nonce:   {once:?}\ntwice:  {twice:?}"
            );

            // Hard width holds except where a single unbreakable word exceeds it.
            for line in once.lines() {
                let cols = line.chars().count();
                let widest_word = line
                    .split_whitespace()
                    .map(|w| w.chars().count())
                    .max()
                    .unwrap_or(0);
                assert!(
                    cols <= line_width || widest_word > line_width,
                    "line over hard width with a breakable layout (case {case} @ {line_width}): {line:?}"
                );
            }

            // The output is a clean, lossless document.
            assert!(
                parse(&once).errors.is_empty(),
                "stable output should parse cleanly (case {case} @ {line_width}): {once:?}"
            );
            assert_eq!(
                reconstruct(&once),
                once,
                "stable output should round-trip losslessly (case {case} @ {line_width})"
            );
        }
    }
}

/// A collapsible, inline-flagged command (the cite family) formats identically
/// regardless of how the author broke its key list across source lines: the same
/// meaning must yield the same output (determinism). The single-line form is the
/// canonical result both converge on.
#[test]
fn cite_key_list_layout_is_deterministic() {
    let one_line =
        "Something \\citep{koslinski2023comparative, srivastava2025amino} were selected.\n";
    let multi_line = "Something\n\\citep{\n  koslinski2023comparative,\n  srivastava2025amino\n}\nwere selected.\n";

    let from_one = format(one_line).expect("one-line formats");
    let from_multi = format(multi_line).expect("multi-line formats");
    assert_eq!(
        from_one, from_multi,
        "cite key-list layout must not depend on the authored source line breaks"
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

/// A user-defined catcode-othering command (`\@makeother\$`) makes its argument a
/// protected verbatim region: the formatter must leave the body's literal `$`, `_`,
/// and interior spacing exactly as authored, and the result must be idempotent.
#[test]
fn user_verbatim_command_body_is_protected() {
    let input = "\\newcommand\\shellcmd[1]{\\@makeother\\$#1}\n\\shellcmd{a_$b$  c}\n";
    let formatted = format(input).expect("formats");
    assert!(
        formatted.contains("\\shellcmd{a_$b$  c}"),
        "verbatim body must pass through unaltered: {formatted:?}"
    );
    assert_format_invariants(input);
}

/// A user-defined catcode-othering *environment* (`\@makeother\$` in its begin-code)
/// makes its `\begin…\end` body a protected verbatim region: the formatter must leave
/// the body's literal `$`, `_`, comment, and interior spacing exactly as authored, and
/// the result must be idempotent. The environment analog of
/// [`user_verbatim_command_body_is_protected`].
#[test]
fn user_verbatim_environment_body_is_protected() {
    let input = "\\newenvironment{shellenv}{\\@makeother\\$}{}\n\\begin{shellenv}\na_$b$  c % literal\n\\end{shellenv}\n";
    let formatted = format(input).expect("formats");
    assert!(
        formatted.contains("a_$b$  c % literal"),
        "verbatim body must pass through unaltered: {formatted:?}"
    );
    assert_format_invariants(input);
}

/// Environments carrying the `noIndent` signature flag (`document`) keep their body
/// flush against the surrounding indentation, while environments nested inside them
/// still indent normally. This pins the convention that `\begin{document}` content
/// sits at the margin.
#[test]
fn no_indent_environment_keeps_body_flush() {
    let input = "\\begin{document}\nHello.\n\n\\begin{itemize}\n\\item one\n\\end{itemize}\n\\end{document}\n";
    assert_eq!(
        format(input).expect("formats"),
        "\\begin{document}\nHello.\n\n\\begin{itemize}\n  \\item one\n\\end{itemize}\n\\end{document}\n",
        "document body must stay flush while nested itemize indents"
    );
}

/// The appendix-package `appendix` environment shares `document`'s `noIndent`
/// flag: it is a sectioning-level container whose body is whole sections, so it
/// sits flush against the surrounding indentation rather than nesting a level.
/// Sections inside it stay at the margin, while a genuinely nested block still
/// indents normally.
#[test]
fn appendix_environment_keeps_body_flush() {
    let input = "\\begin{appendix}\n\\section{Proofs}\nText.\n\\end{appendix}\n";
    assert_eq!(
        format(input).expect("formats"),
        "\\begin{appendix}\n\\section{Proofs}\nText.\n\\end{appendix}\n",
        "appendix body must stay flush like document"
    );
    assert_format_invariants(input);
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

/// Range formatting lays out a single top-level block at its real (indent-0)
/// context: the formatted fragment equals that block's source formatted
/// standalone, and—being a mid-document fragment—it carries no forced trailing
/// newline.
#[test]
fn range_format_block_equals_standalone_without_trailing_newline() {
    let style = FormatStyle::default();
    let input = "first    paragraph.\n\nsecond    paragraph.\n";
    let root = parse(input).syntax();
    let first = root.children().next().expect("a first top-level block");
    let r = first.text_range();

    let fragment =
        format_node_range_with_signatures(&root, style, &SignatureDb::default(), r).unwrap();

    let slice = &input[usize::from(r.start())..usize::from(r.end())];
    let standalone = format_with_style(slice, style).unwrap();
    assert_eq!(fragment, standalone.trim_end_matches('\n'));
    assert!(
        !fragment.ends_with('\n'),
        "fragment must not force a newline"
    );
    assert_eq!(fragment, "first paragraph.");
}

/// Range formatting a multi-line environment block reindents its body at the
/// real indent-0 context, matching a standalone format of the same source and
/// carrying no forced trailing newline.
#[test]
fn range_format_multiline_environment_block() {
    let style = FormatStyle::default();
    let input =
        "\\begin{itemize}\n\\item one\n\\item two\n\\end{itemize}\n\nsecond    paragraph.\n";
    let root = parse(input).syntax();
    let env = root.children().next().expect("the environment block");
    let r = env.text_range();

    let fragment =
        format_node_range_with_signatures(&root, style, &SignatureDb::default(), r).unwrap();

    let slice = &input[usize::from(r.start())..usize::from(r.end())];
    let standalone = format_with_style(slice, style).unwrap();
    assert_eq!(fragment, standalone.trim_end_matches('\n'));
    assert!(fragment.starts_with("\\begin{itemize}"));
    assert!(fragment.ends_with("\\end{itemize}"));
    assert!(
        fragment.contains('\n'),
        "a multi-line block stays multi-line"
    );
}
