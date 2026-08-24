//! Formatter fixtures and invariant tests.
//!
//! Exact output is pinned by `tests/fixtures/formatter/<name>/{input,expected}.tex`.
//! Every case also checks idempotence and preservation of non-trivia content.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use badness_formatter::declarations::{Declarations, ResolvedDeclarations};
use badness_formatter::formatter::{
    FormatError, FormatStyle, LineEnding, MathWrap, SentenceOptions, WrapMode, format,
    format_node_range_with_signatures, format_with_declarations_sentence, format_with_style,
    format_with_style_flavored, format_with_style_flavored_sentence, perturb,
};
use badness_formatter::parser::{LatexFlavor, LexConfig, parse, parse_with_flavor, reconstruct};
use badness_formatter::semantic::SignatureDb;
use badness_formatter::syntax::SyntaxKind;

/// Every `%` comment in `text`, in document order, trailing whitespace trimmed
/// (the printer may drop a comment's trailing spaces along with the line's).
///
/// `DOC_MARGIN` and `GUARD` are deliberately excluded: a `.dtx` margin is
/// re-synthesized per output line by the doc-paragraph reflow, so its *count* is
/// layout, not content. A `COMMENT` never is.
fn comment_texts(text: &str, config: LexConfig) -> Vec<String> {
    parse_with_flavor(text, config)
        .syntax()
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| token.kind() == SyntaxKind::COMMENT)
        .map(|token| token.text().trim_end().to_string())
        .collect()
}

/// Check the formatter invariants for a single clean-parsing input under
/// `style` and `config`, returning a description of the first violation
/// instead of panicking — the corpus sweep aggregates results against the
/// known-failure registry. Inputs the parser rejects are out of scope for the
/// formatter (it refuses them), so the caller filters those out.
fn check_format_invariants(
    input: &str,
    style: FormatStyle,
    config: LexConfig,
) -> Result<(), String> {
    check_format_invariants_with(input, config, |s| {
        format_with_style_flavored(s, style, config)
    })
}

/// [`check_format_invariants`] over an explicit formatting function, so the same
/// oracle can be run on a pipeline the default entry cannot express — today, one
/// parsing under a project's [declarations](badness_parser::declarations).
///
/// The helper *parses* (for the non-trivia, comment, and losslessness oracles)
/// declaration-blind on purpose: every one compares a projection of the input
/// against the same projection of the output, so a reading both sides share is a
/// valid oracle whether or not it is the reading `fmt` used.
fn check_format_invariants_with(
    input: &str,
    config: LexConfig,
    fmt: impl Fn(&str) -> Result<String, FormatError>,
) -> Result<(), String> {
    let formatted = fmt(input).map_err(|e| format!("clean input failed to format: {e}"))?;

    // Whitespace-only: the formatter changes only trivia, never a non-trivia
    // token (tenet 1 — content rewrites are linter autofixes, not layout). The
    // input is compared with a guaranteed final newline, since the formatter's
    // "exactly one trailing newline" rule is a defined trivia normalization (and
    // for the degenerate trailing-`\` input it folds the newline into a
    // `\<newline>` control symbol — the final-newline rule, not a rewrite).
    if perturb::nontrivia_content(&formatted, config)
        != perturb::nontrivia_content(&format!("{input}\n"), config)
    {
        return Err("format changed non-trivia content".to_string());
    }

    // Comments are a *protected region*: the formatter decides which line a `%`
    // lands on, but never drops, duplicates, reorders, or rewrites one. The oracle
    // above cannot see this — a comment is trivia to the CST, so a dropped comment
    // leaves non-trivia content untouched. It is a real failure mode, not a
    // hypothetical: a lowering that walks a node's *expected* children (branches
    // and a closer) silently loses a `DOC_COMMENT` the grammar bound into it.
    // The sequence is compared in document order, since LaTeX comments ride their
    // line and never reorder (unlike the `.bib` side's multiset).
    if comment_texts(&formatted, config) != comment_texts(input, config) {
        return Err(format!(
            "format changed the document's comments:\n--- before ---\n{:?}\n--- after ---\n{:?}",
            comment_texts(input, config),
            comment_texts(&formatted, config)
        ));
    }

    // Idempotence: fmt(fmt(x)) == fmt(x).
    match fmt(&formatted) {
        Err(e) => return Err(format!("formatted output failed to re-format: {e}")),
        Ok(twice) if twice != formatted => {
            return Err(format!(
                "format is not idempotent:\n--- once ---\n{formatted}\n--- twice ---\n{twice}"
            ));
        }
        Ok(_) => {}
    }

    // The formatted output is itself a clean, lossless document.
    if !parse_with_flavor(&formatted, config).errors.is_empty() {
        return Err("formatted output does not parse without diagnostics".to_string());
    }
    let roundtrip = parse_with_flavor(&formatted, config).syntax().to_string();
    if roundtrip != formatted {
        return Err("formatted output does not round-trip losslessly".to_string());
    }

    // Trivia convergence (strictly stronger than idempotence): every
    // TeX-identical newline<->space perturbation must format to a fixed point
    // upholding the invariants. Valid under every wrap mode — Tier-2 modes owe
    // convergence too (`formatter.md` § Trivia-invariant layout).
    match perturb::check_trivia_convergence(
        input,
        config,
        perturb::DEFAULT_SINGLE_FLIP_SAMPLES,
        |s| fmt(s).map_err(|e| e.to_string()),
    ) {
        // A dropped variant means a parser shape gate is newline-sensitive at
        // one of the swapped gaps, silently shrinking oracle coverage.
        Ok(report) if report.dropped_unsafe > 0 => {
            return Err(format!(
                "trivia oracle dropped {} unsafe variant(s) — a parser shape gate is \
                 newline-sensitive",
                report.dropped_unsafe
            ));
        }
        Ok(_) => {}
        Err(perturb::ConvergenceError::Original(e)) => {
            return Err(format!("trivia oracle could not format the original: {e}"));
        }
        Err(perturb::ConvergenceError::Violation(f)) => {
            return Err(format!(
                "trivia perturbation broke an invariant ({}, variant {}):\n--- perturbed input ---\n{}\n--- once ---\n{}\n--- twice ---\n{}",
                f.reason, f.label, f.perturbed_input, f.once, f.twice
            ));
        }
    }

    Ok(())
}

/// Assert the formatter invariants (including the trivia oracle for reflow
/// styles) under an explicit style, for the LaTeX `Document` flavor.
fn assert_format_invariants_with_style(input: &str, style: FormatStyle) {
    if let Err(msg) = check_format_invariants(input, style, LatexFlavor::Document.into()) {
        panic!("{msg}\nfor input: {input:?}");
    }
}

/// [`assert_format_invariants_with_style`] at the default style — what every
/// pre-existing call site uses.
fn assert_format_invariants(input: &str) {
    assert_format_invariants_with_style(input, FormatStyle::default());
}

/// The clean-parsing subset of the roundtrip unit corpus (mirrors
/// `tests/roundtrip.rs`). Cases with parser diagnostics are excluded — the
/// formatter only operates on input the parser accepts.
const CLEAN_CASES: &[&str] = &[
    "",
    "hello world",
    r"\section{Introduction}",
    r"$x^2 + y_i = \frac{1}{2}$",
    // Structured math: scripts, a single-token braced script (kept verbatim), a
    // multi-char braced script, a group base, and display math — the lowering must
    // keep all invariants (idempotent, clean, lossless).
    r"$x^{2} + a_i^{n+1} + {a+b}^2$",
    // Operators inside a coalesced `WORD` are classified as virtual atoms and
    // spaced (`a+2*1^5` -> `a + 2 * 1^5`); unary signs stay tight (`-x`, `x=-b`).
    r"$a+2*1^5$ and $x=-b$ and $-x+1$ and $2*-1$ and $a<=b$",
    r"\[ x ^ 2 \quad y_\alpha \]",
    // `\left … \right` matched pairs: nested, scripted, and a control-word
    // delimiter — the new lowering must stay idempotent, clean, and lossless.
    r"$\left[ \left( a \right) \right]^2 + \left\langle x \right\rangle$",
    "a % comment\nb",
    // An own-line `%` run bound forward into a paired conditional: the comment is a
    // child of the `CONDITIONAL`, not of a branch, so it is exactly the shape a
    // branches-only lowering drops. Here for the *oracle*'s sake — the fixture pins
    // the bytes, this pins that `check_format_invariants` itself catches the loss.
    "% why\n\\ifnum1>0 a \\else b \\fi\n",
    r"\begin{itemize}\item one\end{itemize}",
    // Beamer overlay syntax belongs to the item marker; include the action form,
    // a following label, and a glued comment in the full trivia-convergence oracle.
    "\\begin{itemize}\n\\item<2-| alert@3>[Note]%\nLater.\n\\end{itemize}\n",
    // Own-line `%`s in a list body (issue #48): a multi-line comment run bound
    // leading into the next `\item`, and a floating comment isolated between
    // blank lines — neither may glue onto neighbouring content, and both must
    // stay idempotent.
    "\\begin{itemize}\n\\item a\n% one\n% two\n\\item b\n\\end{itemize}\n",
    "\\begin{itemize}\n\\item a\n\n% c\n\n\\item b\n\\end{itemize}\n",
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
    r"see \href{https://example.test/a%20b}{visible \emph{text}} inline",
    r"see \href[page=2]{file:a%20b.pdf}{page two} inline",
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

/// In-region `BracketPolicy` audit pins beyond the
/// `expl_bracket_attachment` fixture: the abutting-sensitive gates. Inside an
/// expl3 region `\begin`/`\end` are plain commands (the issue-#60 carve-out),
/// so the curated math `\begin`'s `Tight` policy is unreachable in-region and a
/// next-line `[` attaches greedily. Outside a region `Tight` reads flush-ness,
/// which the header layout preserves in both directions. In math a command is
/// lowered as one verbatim atom, so no layout can touch its `[` junction.
#[test]
fn bracket_attachment_stability() {
    // In-region: the demoted `\begin{align}`'s next-line `[a]` attaches under
    // Greedy and must survive relayout.
    assert_format_invariants(
        "\\ExplSyntaxOn\n\\begin{align}\n[a]_1 &= b\n\\end{align}\n\\ExplSyntaxOff\n",
    );
    // Tight, flush-attached: the header keeps `\begin{align}[a]` glued — a gap
    // opened here would detach the optional on the next pass.
    let flush = "\\begin{align}[a]_1 &= b \\\\ c &= d\n\\end{align}\n";
    let out = format(flush).unwrap();
    assert!(
        out.contains("\\begin{align}[a]"),
        "flush Tight optional must stay flush: {out:?}"
    );
    assert_format_invariants(flush);
    // Tight, next-line: the unattached `[a]` must never glue flush onto the
    // header — that would attach it on the next pass.
    let detached = "\\begin{align}\n[a]_1 &= b \\\\ c &= d\n\\end{align}\n";
    let out = format(detached).unwrap();
    assert!(
        !out.contains("\\begin{align}["),
        "unattached bracket must not glue flush: {out:?}"
    );
    assert_format_invariants(detached);
    // In-region math: `\sqrt[3]` stays tight inside its verbatim command atom,
    // and the spaced bare `[ x ]` keeps its authored gap.
    assert_format_invariants(
        "\\ExplSyntaxOn\n\\tl_set:Nn \\l_tmpa_tl { $ \\bE [ x ] + \\sqrt[3]{x} $ }\n\\ExplSyntaxOff\n",
    );
}

/// The widths the corpus invariants sweep runs at. Every layout hybrid is a
/// column-arithmetic accident, so widths multiply detection.
const SWEEP_WIDTHS: &[usize] = &[60, 72, 80, 100, 120];

/// Corpus files known to violate an invariant at one or more sweep widths —
/// the in-repo mirror of the corpus failure inventory. A registered file is
/// asserted to *fail* somewhere in the sweep, so a fix in a later stage forces
/// the entry's removal; an unregistered failure panics with the details. The
/// third field is a substring every observed failure message must contain, so
/// a registration masks only its recorded failure mode — a *new, unrelated*
/// regression in a registered file still panics. Never weaken the oracle to
/// shrink this list.
const KNOWN_INVARIANT_FAILURES: &[(&str, &str, &str)] = &[];

/// Run the invariants sweep over one corpus file, panicking on any
/// unregistered failure, any registered file that no longer fails, and any
/// registered file whose failure does not match its recorded mode.
fn sweep_corpus_file(name: &str, text: &str, config: LexConfig) {
    let registered = KNOWN_INVARIANT_FAILURES.iter().find(|(n, _, _)| *n == name);
    let mut failures: Vec<String> = Vec::new();
    for &width in SWEEP_WIDTHS {
        let style = FormatStyle {
            line_width: width,
            ..FormatStyle::default()
        };
        if let Err(msg) = check_format_invariants(text, style, config) {
            failures.push(format!("width {width}: {msg}"));
        }
    }
    match registered {
        Some((_, why, matches)) => {
            assert!(
                !failures.is_empty(),
                "{name} is registered in KNOWN_INVARIANT_FAILURES ({why}) but passes the whole \
                 sweep — remove its entry"
            );
            let unrelated: Vec<&String> =
                failures.iter().filter(|f| !f.contains(matches)).collect();
            assert!(
                unrelated.is_empty(),
                "{name} is registered in KNOWN_INVARIANT_FAILURES ({why}), but these failures do \
                 not match its recorded mode ({matches:?}) — a new, unrelated regression:\n{}",
                unrelated
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        None => assert!(
            failures.is_empty(),
            "unregistered invariant failure(s) in {name}:\n{}",
            failures.join("\n")
        ),
    }
}

/// Shapes whose layout satisfies the **strict** trivia-invariance contract:
/// `fmt(perturbed) == fmt(original)` for every TeX-identical
/// newline<->space perturbation, i.e. layout that reads none of the unsafe
/// predicate anywhere in the shape.
///
/// This is an **allowlist, not a ratchet** — unlike [`KNOWN_INVARIANT_FAILURES`],
/// which asserts registered files still *fail* and so forces its own pruning,
/// this list can only prove the shapes on it stay invariant. It cannot discover
/// newly invariant shapes, which must be added manually.
///
/// The convergence oracle in [`check_format_invariants`] cannot stand in for
/// this. It accepts deliberate authored-break preservation by construction, so
/// a layout decision keyed on the unsafe predicate is invisible to it — both
/// spellings are self-consistent fixed points. `badness debug format --checks
/// trivia-strict` is the surveying form of the same oracle.
const STRICT_TRIVIA_INVARIANT_SHAPES: &[(&str, &str)] = &[
    ("reflowed-prose", "alpha\nbeta gamma\n"),
    (
        // Every head has derivable arity, so segmentation is structural and a
        // swap anywhere in the stream formats back to identical bytes — the
        // exact gap that was the `SplitAtNewlines` violation.
        "expl3-structural-statements",
        "\\ExplSyntaxOn\n\\tl_new:N \\l_tmpa_tl\n\\tl_new:N \\l_tmpb_tl\n\\ExplSyntaxOff\n",
    ),
    (
        // An `Npn` definition (single-token slot, shape-scanned parameter text,
        // peeled body group) is one structural call unit, so the authored break
        // before its body carries no layout weight.
        "expl3-structural-definition",
        "\\ExplSyntaxOn\n\\cs_new:Npn \\demo_foo:n #1\n  { \\demo_use:n {#1} }\n\
         \\cs_new:Nn \\demo_bar:n { \\demo_use:n { x } }\n\\ExplSyntaxOff\n",
    ),
    (
        // Curated block commands are intercepted as block-level statements
        // (`CommandSig::block`), so the authored break between them carries no
        // layout weight — the exact gap that was the command-only-line rule's
        // violation. The prose tail proves the boundary between a block
        // statement and reflowed prose is invariant too.
        "command-block-lines",
        "\\usepackage{a}\n\\usepackage{b}\n\\title{Short Title}\nalpha\nbeta gamma\n",
    ),
    (
        // Opaque brace groups are width-driven (`lower_opaque_group`), so a
        // gap inside a group carries no layout weight — the exact read that
        // was the `GROUP` arm's `spans_multiple_lines` violation. The prose
        // around the group keeps every line off the command-only residue.
        "opaque-group-inline",
        "alpha {a b} beta gamma\n",
    ),
    (
        // The nested form: an inner group's gap is the inner fill's business
        // and the outer group re-measures identically either way.
        "opaque-group-nested",
        "alpha {a {b c} d} beta\n",
    ),
    (
        // A segmentable optional's gaps (including the entry gap after the
        // comma) are width-driven; the parenthesised context keeps the bulk
        // space-to-newline variant's lines off the command-only residue.
        "optional-collapsed",
        "alpha (\\baz[a, b] x) beta gamma\n",
    ),
];

/// Every registered shape must hold strict trivia invariance at **all** sweep
/// widths, not just the default: a shape proven invariant at 80 columns is not
/// proven at 60, since every hybrid is a column-arithmetic accident.
#[test]
fn strict_trivia_invariant_shapes_stay_invariant() {
    for (name, input) in STRICT_TRIVIA_INVARIANT_SHAPES {
        for &width in SWEEP_WIDTHS {
            let style = FormatStyle {
                line_width: width,
                ..FormatStyle::default()
            };
            let result = perturb::check_trivia_invariance(
                input,
                LatexFlavor::Document,
                perturb::DEFAULT_SINGLE_FLIP_SAMPLES,
                |s| format_with_style(s, style).map_err(|e| e.to_string()),
            );
            match result {
                Ok(report) => assert!(
                    report.variants_checked > 0,
                    "{name} at width {width}: no perturbation was generated, so the shape proves \
                     nothing — give it an eligible newline<->space gap"
                ),
                Err(perturb::TriviaError::Original(msg)) => {
                    panic!("{name} at width {width}: the original failed to format: {msg}")
                }
                Err(perturb::TriviaError::Violation(failure)) => panic!(
                    "{name} at width {width}: variant `{}` formatted differently — a layout \
                     decision reads the lone-newline predicate.\noriginal:\n{}\nperturbed \
                     ({}):\n{}",
                    failure.label,
                    failure.formatted_original,
                    failure.label,
                    failure.formatted_perturbed
                ),
            }
        }
    }
}

#[test]
fn format_invariants_corpus() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../badness-parser/tests/corpus");
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
            let name = path.file_name().unwrap().to_str().unwrap().to_string();
            sweep_corpus_file(&name, &text, LatexFlavor::Document.into());
            count += 1;
        }
    }
    assert!(count > 0, "no clean .tex corpus files found in {dir:?}");
}

#[test]
fn format_invariants_dtx_corpus() {
    // The `.dtx` corpus files, checked under their real docstrip lex config and
    // under `Reflow` — the sweep's default wrap and, since every file kind
    // reflows, the production default too. Same wrap as
    // `debug format --checks trivia` pins.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../badness-parser/tests/corpus");
    let config = LexConfig {
        flavor: LatexFlavor::Package,
        dtx: true,
    };
    let mut count = 0;
    for entry in fs::read_dir(&dir).expect("read corpus dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("dtx") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read corpus file");
        if parse_with_flavor(&text, config).errors.is_empty() {
            let name = path.file_name().unwrap().to_str().unwrap().to_string();
            sweep_corpus_file(&name, &text, config);
            count += 1;
        }
    }
    assert!(count > 0, "no clean .dtx corpus files found in {dir:?}");
}

/// Fixture cases under `tests/fixtures/formatter/<name>/`, each an
/// `input.tex` + hand-verified `expected.tex` pair, with the `(wrap, line_width)`
/// each was authored under.
///
/// The whitespace / indentation fixtures isolate rules that predate paragraph
/// reflow, so they run under [`WrapMode::Preserve`] — their `expected.tex` is the
/// pre-reflow output and must stay byte-identical. The `reflow_*` fixtures
/// exercise the new rule, each at a width chosen to make the wrapping legible.
///
/// Two clarifications on what `Preserve` keeps. It governs *line breaks* only:
/// authored breaks survive, but inter-word spacing on a line still collapses to a
/// single space (`preserve_prose_spacing`) exactly as under the wrapping modes —
/// an *opaque* argument body (a `\newcommand` definition) is left byte-for-byte.
/// And list environments hang their `\item` continuation lines under the marker in
/// every mode (issue #82), so `nested_environments` and `list_item_continuation_hang`
/// show that hang even under `Preserve`, never the flat body indent.
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
    ("preserve_prose_spacing", WrapMode::Preserve, 80),
    ("final_newline_added", WrapMode::Preserve, 80),
    // Environment indentation.
    ("environment_indents_body", WrapMode::Preserve, 80),
    ("nested_environments", WrapMode::Preserve, 80),
    ("list_item_continuation_hang", WrapMode::Preserve, 80),
    ("environment_reindents", WrapMode::Preserve, 80),
    ("environment_blank_lines_in_body", WrapMode::Preserve, 80),
    ("environment_begin_arguments", WrapMode::Preserve, 80),
    ("environment_argument_glued", WrapMode::Preserve, 80),
    // Locally defined delimiter aliases inherit the target environment's list
    // and math-grid layout, nest structurally, and stay ordinary commands when
    // no closer proves the pair.
    ("env_alias_list_items", WrapMode::Reflow, 80),
    ("env_alias_math_grid", WrapMode::Reflow, 80),
    ("env_alias_nested_in_environment", WrapMode::Reflow, 80),
    ("env_alias_unpaired_unchanged", WrapMode::Reflow, 80),
    // A `statementBody` environment (the TikZ/pgf picture family) holds
    // `;`-terminated path statements, not prose. Boundaries are *structural*
    // (the parser's `STATEMENT` node, re-derived from the `;` on every parse),
    // so one statement gets one line and every continuation — a width wrap, a
    // post-comment tail, a `{label}` block — hangs one step under its head
    // (`lower_statement`). Issue #114 pinned the boundary half; the hang closes
    // Routing reads the nearest environment ancestor,
    // so an `itemize` inside a `\node` label reflows as prose.
    ("statement_body_picture_env", WrapMode::Reflow, 80),
    // The structural boundaries and the TikZ unit model at work: two statements
    // on one authored line split, one authored across lines joins when it fits,
    // an over-long one wraps with its continuation hung at a *unit* boundary
    // (before a path operator, between segments — never inside `at (…)` or
    // between a coordinate and its operation, `semantic::tikz`), a glued
    // `;\draw` seam splits onto its own line (the statementBody whitespace-
    // safety claim, proven by `tests/typeset/statement_seams.tex`), and a run
    // with no `;` (`\tikzset`) keeps the authored-line fallback.
    ("statement_hang", WrapMode::Reflow, 40),
    // A `%` that trails `\begin{…}` on the same source line (the space-suppression
    // idiom) rides the `\begin` header instead of dropping to its own indented
    // line; a `%` the author put on its own line is left there.
    ("environment_begin_trailing_comment", WrapMode::Reflow, 80),
    // The same `\begin`-line `%` lift for every *specialized* environment layout —
    // the alignment grid (`tabular`), math formula (`equation`), math grid
    // (`align`), list (`itemize`), and display math (`\[`), plus the empty-body
    // shapes — none of which may relocate the comment onto its own body line
    // (issue #38, second report).
    (
        "begin_trailing_comment_special_layouts",
        WrapMode::Reflow,
        80,
    ),
    // The `\begin` header ends at the last element glued to it; content greedy
    // attachment gave `BEGIN` past that is body, and indents and reflows with it
    // rather than stranding at the `\begin` column. `\begin{frame}{Title}` keeps
    // its undeclared argument on the header because the author glued it there —
    // `Glued`-versus-not is the only trivia predicate read, so a lone newline
    // never reaches the decision.
    ("begin_tail_is_body", WrapMode::Reflow, 80),
    // A `%` run on its *own* line(s) immediately before a command or environment
    // binds *leading* into that construct (the parser's leading comment-bind) and
    // is rendered on its own line above `\section` / `\begin`, at the construct's
    // indentation — not lifted onto the header line the way a same-line `%` is.
    ("comment_binds_leading_to_construct", WrapMode::Reflow, 80),
    // A `%` glued directly onto a forced-break block (a doc-commented command, an
    // `\end{…}`) rides the block's last line instead of dropping to its own line:
    // an own-line `%` would bind *leading* into the next command on reparse and
    // cascade one line further per pass (issue #38).
    ("trailing_comment_rides_block", WrapMode::Reflow, 80),
    // The same ride when inline *whitespace* separates the `%` from the block
    // (`\newcommand{…}{…} % note` under a doc comment): the space must not strand
    // the comment on its own line, where it would re-bind as the next command's
    // doc comment on reparse (issue #54, HoTT/book `macros.tex`).
    ("trailing_comment_after_block_space", WrapMode::Reflow, 80),
    // A trailing `%` never becomes a fill atom of its own: even when the line
    // overflows the width, the comment rides the end of its line rather than
    // wrapping, which would leave an own-line `%` that re-binds as the next
    // command's doc comment on reparse and cascades one line further per pass
    // (issue #54, HoTT/book `opt-*.tex`).
    ("reflow_trailing_comment_never_wraps", WrapMode::Reflow, 80),
    // A trailing `%` on a line directly above a command (`\else % note` over
    // `\pgfmath@tokens@make{…}`) rides its line: dropping it to its own line would
    // re-bind it as the following command's doc comment on reparse and split that
    // command's glued `\word@tail` across passes (issue #64, pgf-tikz/pgf
    // `pgfmathparser.code.tex`).
    (
        "trailing_comment_rides_before_command",
        WrapMode::Reflow,
        80,
    ),
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
    // `filecontents` is the verbatim-body environment whose `\begin` line *defines
    // where the body starts*, so that line may never be broken. Its optional is
    // declared `ContentKind::Keyval`, which everywhere else licenses exploding a
    // `[…]` at its commas under width pressure (`\includegraphics`); here the
    // over-long `\begin{filecontents*}[force,noheader]{…}` must stay on one line,
    // because a break would shift the first protected body byte written to the file.
    // Also pins that the `\end` marker's own indentation is *inside* `VERBATIM_BODY`
    // (hence author-preserved, not reindented with the `\begin`).
    ("filecontents_protected_body", WrapMode::Reflow, 80),
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
    // reflows. (The `\usepackage`/`\section` lines are now held by the
    // block-statement rule below; the residual command-line rule covers the
    // same shape for un-signatured commands.)
    ("reflow_command_lines_preserved", WrapMode::Reflow, 80),
    // The residual rule's fixed-point corner: no authored break exists (the
    // input is one source line), but the width-80 fill strands the
    // un-signatured `\zzconfigure{…}` atom alone on a printed line. The next
    // pass re-reads that line as command-only and *hardens* the fill's breaks
    // around it — layout-neutral because the greedy fill is first-fit, so
    // refilling around a hardened break the fill itself chose reproduces the
    // same lines (the written Tier-2 argument on `line_is_command_only`).
    // Idempotence, asserted on every fixture, is what pins it.
    ("reflow_command_stranded_by_width", WrapMode::Reflow, 80),
    // A paragraph-level sectioning command (`\part` … `\subparagraph`, per the
    // signature DB's `sectioning` level) is a paragraph-separated block: one blank
    // line before it and one after it, whatever trivia the author wrote. This is
    // structural rather than an authored-newline preference, so
    // `\subsection{X}\nprose` and `\subsection{X} prose` lay out identically.
    ("sectioning_starts_own_line", WrapMode::Reflow, 80),
    // Comments retain their attachment while section boundaries normalize: a `%`
    // on the heading's own physical line rides it, and a leading own-line `%` stays
    // with the heading, with the blank separator placed before the whole command.
    ("sectioning_blank_line_and_comment", WrapMode::Reflow, 80),
    // A curated block-level command (`CommandSig::block`: `\usepackage`,
    // `\title`, `\setlength`, …) is a block-level statement like a heading: a
    // break before it and after it, whatever trivia the author wrote —
    // `\usepackage{a} \usepackage{b}` and the newline spelling are the same
    // bytes to the next parse, so both must lay out alike (the command-only-line
    // rule's lone-newline read, bypassed for curated commands; the residue it
    // still decides for is sanctioned Tier 2).
    ("block_command_lines", WrapMode::Reflow, 80),
    // Unlike a heading, a block command glued to adjacent non-trivia keeps its
    // authored adjacency (`\ProcessOptions\relax`, `prose\setcounter{…}`):
    // breaking there materializes a space token TeX typesets, where a heading's
    // own `\par` discards the glue. Such shapes fall to the residual rule.
    ("block_command_glued_stays", WrapMode::Reflow, 80),
    // The trivia predicates the rule *may* read still hold around a block
    // command: an authored blank line survives, a trailing `%` rides its line,
    // and an own-line `%` stays own-line (binding forward as the next command's
    // `DOC_COMMENT`, whose forced-break lowering still closes the line after).
    ("block_command_blank_line_and_comment", WrapMode::Reflow, 80),
    // Under `ReflowKind::Statement` (a brace-group body) the block-statement
    // rule does not fire: `\AtBeginDocument{\setcounter{page}{1}}` stays one
    // line. Statement's Tier-2 contract is the authored line.
    ("block_command_in_brace_body_stays", WrapMode::Reflow, 80),
    // `\caption` and `\label` are deliberately not block commands: a glued
    // `\caption{…} \label{…}` pair must stay untouched.
    ("block_command_exclusions", WrapMode::Reflow, 80),
    // A *bare* block-command head — `\newcommand` in `\newcommand\foo{…}`,
    // where the control-word run break leaves every argument unattached — is
    // not intercepted: its adjacency is not pass-stable (glued to a
    // forced-break sibling the ride path strands it), so it falls to the
    // residual rule, which preserves the authored spelling either way.
    ("block_command_bare_head_residual", WrapMode::Reflow, 80),
    // A block command inside a conditional branch does not defeat the
    // all-or-nothing choice: the flat candidate is collapsed from content, so
    // `\ifdefined\x \usepackage{a} \fi` stays flat when it fits.
    ("conditional_flat_with_block_command", WrapMode::Reflow, 80),
    // A paired `\if…\else…\or…\fi` renders all-or-nothing. Flat when the whole
    // construct fits — and the same content spelled across lines rejoins to
    // exactly that, which is the fix: before the `CONDITIONAL` node the two
    // spellings were separate fixed points, so no oracle could see the
    // lone-newline read between them.
    ("conditional_flat_when_it_fits", WrapMode::Reflow, 80),
    ("conditional_authored_breaks_rejoin", WrapMode::Reflow, 80),
    // Too wide to fit: *every* divider opens a line, never just the ones the
    // author already broke. Breaking one and not its sibling is the lopsided form
    // a per-divider rule at this layer cannot avoid.
    ("conditional_breaks_all_dividers", WrapMode::Reflow, 80),
    // `\or` is a divider like `\else` — `\ifcase` bodies would collapse into a
    // single branch otherwise.
    ("conditional_ifcase_or_branches", WrapMode::Reflow, 80),
    // A `%` inside a branch must end its line, so no flat candidate exists and the
    // construct is unconditionally broken. Comment *presence* is a trivia
    // predicate the formatter preserves, so layout may read it.
    ("conditional_comment_forces_break", WrapMode::Reflow, 80),
    // A divider the author glued (`\ifmmode y\else z\fi`) is never broken: the
    // newline would materialize a space token TeX contributes to the horizontal
    // list — a typeset change no CST oracle can see, since whitespace is trivia to
    // them and content to TeX. Any glued divider sends the whole construct down
    // the byte-faithful path.
    ("conditional_glued_divider_stays_flat", WrapMode::Reflow, 80),
    // All-or-nothing composes: the outer construct breaks because it does not fit,
    // while the inner one fits on the line it lands on and stays flat. Neither
    // gets a body indent — the `\if` test's extent is not statically resolvable,
    // so there is no head/body split to hang one off.
    ("conditional_nested", WrapMode::Reflow, 80),
    // The gate's demotions format exactly as they did before the node existed: a
    // `\newif` declaration, a `\fi` assembled inside a definition body, and a
    // brace-argument `\ifthenelse` all stay plain commands.
    ("conditional_demoted_unchanged", WrapMode::Reflow, 80),
    // An own-line `%` run before the opener binds forward and the grammar reparents
    // it *inside* the `CONDITIONAL`, as a sibling of the branches. It must survive:
    // a lowering that walks only the branches and the closer deletes it outright,
    // and no oracle but the comment one can see that (a comment is trivia to the
    // CST, so non-trivia content is unchanged).
    ("conditional_bound_comment", WrapMode::Reflow, 80),
    // `WrapMode::Preserve` promises authored line breaks are untouched, so the
    // all-or-nothing relayout does not run there at all — it would rejoin a
    // conditional the author deliberately spread over lines.
    ("conditional_preserve_keeps_breaks", WrapMode::Preserve, 80),
    // A branch interior is laid out the way its *enclosing context* lays out the
    // same elements. In running text that means the prose reflow: the branch wraps
    // at the line width and its inter-word spacing normalizes, exactly as the same
    // words would outside the construct. No `PARAGRAPH` nests in a branch to carry
    // that lowering, so it is read off the conditional's ancestors.
    ("conditional_prose_reflows", WrapMode::Reflow, 80),
    // The mirror: inside a `\def` body the enclosing `GROUP` emits the byte-faithful
    // stream, so the branches do too and macro code keeps its authored lines.
    // Feeding this to the prose reflow is not merely cosmetic — `\ifx\\#1\\` has a
    // `LINE_BREAK` node in an operand slot, and the "a `\\` ends its line" rule
    // oscillates on it pass over pass (`pagesel.sty`).
    ("conditional_in_definition_body", WrapMode::Reflow, 80),
    // List environments (`itemize`/`enumerate`/`description`): each `\item` on
    // its own line, the body reflowed with continuation lines hanging-indented at
    // the control word's width (`\item `). A `description` `[label]` trails on the
    // first line but does *not* widen the hang, so the body keeps one left edge
    // regardless of label width (a nested list and a blank line between items are
    // both reproduced).
    ("reflow_list_hanging_indent", WrapMode::Reflow, 72),
    ("reflow_list_item_label", WrapMode::Reflow, 60),
    ("list_item_overlay_prefix", WrapMode::Reflow, 52),
    ("reflow_list_nested", WrapMode::Reflow, 50),
    ("reflow_list_blank_between_items", WrapMode::Reflow, 80),
    // An own-line `%` in a list body stays on its own line (at the item body's
    // hanging indent) instead of gluing onto the preceding content or a nested
    // `\end{…}`; a `%` run bound leading into a `\item` (the parser's
    // `DOC_COMMENT`) renders on its own line(s) above the marker at the item
    // indent (issue #48).
    ("reflow_list_comment_own_line", WrapMode::Reflow, 80),
    ("reflow_list_doc_comment_item", WrapMode::Reflow, 80),
    // Prose-argument reflow: a signature-marked prose argument reflows like a
    // paragraph — joined when short, wrapped when long. An `inline`-
    // flagged prose command (`\footnote`, `\emph`, …) flattens into the surrounding
    // text so its body wraps as running prose with `{`/`}` glued to the adjacent
    // words; a block-level prose command (`\section`, `\caption`) block-breaks its
    // braces onto their own lines instead.
    ("reflow_prose_arg_wraps", WrapMode::Reflow, 40),
    ("reflow_prose_arg_joins_short", WrapMode::Reflow, 80),
    ("reflow_prose_arg_optional_omitted", WrapMode::Reflow, 30),
    // Non-prose groups keep their *bytes* when they fit; width decides otherwise.
    // The over-width `\newcommand` body wraps at its authored gaps (the opener
    // stays glued — no break is invented at `{`), and the gapless `\label`
    // argument has no break opportunity at all, so it overflows as authored.
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
    // A `[` inside a *brace* prose argument is content, not a delimiter: splicing
    // an inline prose command matched any closer, so `\emph{a [b] c}` lost its `]`
    // at default settings (whitespace-only invariant, tenet 1).
    ("reflow_bracket_in_prose_argument", WrapMode::Reflow, 80),
    ("reflow_caption_block", WrapMode::Reflow, 40),
    // A `%` at either edge of a prose argument body: glued to the opener it rides
    // the opener's line (relocating it would turn the synthesized newline after
    // `{` into a real space token inside the group), and one the body *ends* with
    // forces the group open so the closing brace takes its own line — flat, the
    // soft group rendered `\caption{%}` and commented the brace out
    // (`latexindent`'s `commands/figureValign-mod*`). Both bite only when the
    // whole body reflows to one line; any second line already forces the group.
    ("reflow_prose_arg_comment_edges", WrapMode::Reflow, 80),
    // A signature-marked token list (`\citep` and the cite family) joins the
    // paragraph fill at its top-level commas: short and author-broken forms
    // collapse identically, while a long list wraps between keys without exploding
    // the command's delimiters onto separate lines. A `%` comment inside the list
    // is not safely segmentable, so it keeps the indented block form.
    ("reflow_cite_collapses_and_flows", WrapMode::Reflow, 80),
    ("reflow_cite_comment_keeps_block", WrapMode::Reflow, 80),
    // The cross-reference family (`\ref`, `\eqref`, `\cref`, `\nameref`, …) is
    // flagged `inline` but *not* `collapse` (a ref key is a single token where
    // interior spaces can matter). A ref isolated on its own source line flows
    // into the paragraph fill as an atom instead of being kept as a command-only
    // line, with its `{key}` left exactly as authored.
    ("reflow_ref_flows", WrapMode::Reflow, 80),
    // Optional-argument layout (issue #47): a multi-line `[…]` collapses to one
    // line when it fits the width (`\foo[a=1,\nb=2]` -> `\foo[a=1, b=2]`, the
    // interior newlines becoming spaces) and keeps the indented block form when
    // it does not.
    ("optional_collapse_fits", WrapMode::Reflow, 30),
    // A `[…]` is a group over its top-level entries: flat when it fits, one key
    // per line when it does not. The three `\wide` calls carry *identical* option
    // content and differ only in where the author broke the line, so they must
    // format identically — the layout no longer reads `spans_multiple_lines`
    // (see the trivia-invariant-layout section of `formatter.md`).
    ("optional_expands_to_width", WrapMode::Reflow, 60),
    // Splitting a comma the author *glued* needs the signature DB to prove the
    // argument keyval (`ContentKind::Keyval`; `axis` via the CWL `%keyvals` mark).
    // The lexer ends a `WORD` at every control sequence, so `width=\figurewidth`
    // hands the splitter a word that *opens* with the comma closing that entry —
    // it must still break there. A fitting bracket canonicalizes a glued comma
    // to one space, matching the flat spelling of the newline emitted when it
    // breaks; this keeps both spellings on the same width decision.
    ("optional_keyval_splits_glued", WrapMode::Reflow, 80),
    // The mirror: a *textual* optional never gains a space, at any width. Compiling
    // both spellings shows `\item[red,green]`, a `\newcommand` default, and a
    // `\caption` short entry all typeset the inserted space, so these overflow
    // rather than split.
    ("optional_textual_keeps_glued", WrapMode::Reflow, 60),
    // With no split point at all a `[…]` stays inline and overflows; a breakable
    // group would push `[!htb]`-shaped brackets onto three lines to no gain.
    ("optional_unsplittable_overflows", WrapMode::Reflow, 80),
    // The mandatory mirror: a `{…}` the curated DB proves keyval (`\pgfkeys`,
    // `\tikzset`, `\setlist`) segments at its top-level commas instead of reflowing
    // as prose, which wrapped mid-key. The two `\pgfkeys` spellings — one line and
    // one entry per line — converge byte for byte, so the choice is width and
    // content only. A nested `.style={…}` value keeps its own commas sealed inside
    // the child `GROUP`, and a list that fits stays exactly as authored.
    ("keyval_group_splits_entries", WrapMode::Reflow, 80),
    // A keyval `{…}` declines to the block form on the same preserved predicates as
    // the bracket: a `%` (which must end its line) and a blank-line `\par`, reached
    // through `segment_delimited_body`'s bail. The block form breaks after the `{`
    // even where the author glued it, which for any *other* brace group would
    // materialize a space token (`lower_bracketed`'s `open_glued`) — the keyval
    // proof is what lifts that guard, and without it the group glued its opener
    // while its closer took its own line. The blank-line half is well-formed
    // syntax that nonetheless does not run: the blank line is a `\par` token, and
    // `keyval`'s `\kv@processor@default` is not `\long`, so hyperref reports
    // "Paragraph ended before \kv@processor@default was complete" on exactly this
    // input and compiles clean without the blank line. Kept because the formatter
    // owes it losslessness and a deterministic bail either way — but it pins the
    // bail, not a shape to emulate.
    ("keyval_group_declines_on_comment", WrapMode::Reflow, 80),
    // A dropped trailing separator (`[a, ]` / `[a,\n]`) stood for authored
    // whitespace, and an optional is textual — the space token survives as
    // trailing padding in both spellings, never deleted.
    (
        "optional_trailing_separator_keeps_padding",
        WrapMode::Reflow,
        80,
    ),
    // A bracket that declines segmentation (here: a nested group whose comment
    // forces a break) takes the indented block form unconditionally. Delimiter
    // junctions still preserve gluedness: `d}]` cannot converge with `d}\n]`,
    // because the latter carries a trailing space token inside the optional.
    ("optional_block_decline_deterministic", WrapMode::Reflow, 80),
    // Opaque brace groups under `Reflow` are width-driven (`lower_opaque_group`):
    // block-vs-inline reads width, content, and preserved predicates, never the
    // lone-newline predicate. An incidental newline erases to a space, so both
    // spellings lower identically.
    ("group_erases_incidental_newline", WrapMode::Reflow, 80),
    // The three `\newcommand` bodies carry identical content and differ only in
    // where the author broke the line — they must format identically, wrapping
    // at the width with the opener hugged (no break is invented at a glued `{`)
    // and continuations at one indent step.
    ("group_expands_to_width", WrapMode::Reflow, 40),
    // A *padded* group that exceeds the width detonates its delimiters (the
    // padding vanishes broken — the delimiter's own newline supplies the space
    // token); the single-line and multi-line spellings converge.
    ("group_padded_expands_allman", WrapMode::Reflow, 30),
    // The decline set: a blank line or a direct comment (both preserved
    // predicates) keeps today's indented block form even under `Reflow`.
    ("group_blank_line_keeps_block", WrapMode::Reflow, 80),
    ("group_comment_keeps_block", WrapMode::Reflow, 80),
    // Forced-break siblings still obey the glued-divider rule. Each group's `%`
    // forces its own block layout, but the absent gap between `}` and `{` means
    // the formatter must not insert a line break—and therefore a TeX space token—
    // at their shared boundary.
    (
        "reflow_forced_break_keeps_glued_siblings",
        WrapMode::Reflow,
        80,
    ),
    // An empty group's padding survives flat in both spellings (`{ }` ≡ `{\n}`):
    // collapsing `{\n}` to `{}` would delete a space token TeX typesets.
    ("group_empty_keeps_space", WrapMode::Reflow, 80),
    // Group-altitude analogue of `reflow_command_stranded_by_width`: a width
    // break inside the group's fill that strands a command alone on a printed
    // line re-reads to the same layout on the next pass.
    ("group_command_stranded_by_width", WrapMode::Reflow, 40),
    // A `\\` inside a group is a soft atom: rows the author spread over source
    // lines join when they fit (newline ↔ space, typeset-identical — the `\\`
    // still breaks the typeset line), and a glued `\\` never gains a space.
    ("group_linebreak_rows", WrapMode::Reflow, 80),
    // A multi-line group in a tabular cell no longer carries a forced break, so
    // the grid aligns instead of falling back to the preserved layout.
    ("grid_cell_group_joins", WrapMode::Reflow, 80),
    // A blank run at a group's *edge* erases to padding rather than declining:
    // the block form trims an edge blank away, so declining on it would key on
    // a predicate the emitter destroys (the latexindent
    // `poly-switch-blank-line` non-fixed-point family).
    ("group_edge_blank_erases", WrapMode::Reflow, 80),
    // An edge gap joins the vanish-when-broken protocol only when its flat
    // spelling is a single space; a multi-space edge (`{0    }`) rides verbatim
    // and never breaks — vanishing it would hand pass 2 a `" "` gap where
    // pass 1 measured four spaces (pgf's coil tables oscillated).
    ("group_multispace_edge_glues", WrapMode::Reflow, 40),
    // Inside a signature-proven prose argument body (`ReflowKind::ProseArg`)
    // the command-only-line residue does not fire: width alone owns the
    // layout, so an authored command-only line refills. Preserving it minted a
    // forced break only pass 2 could see, and the bit leaked upward through
    // `contains_forced_break` readers, flipping the enclosing group between
    // its inline and block forms (pgf's `\emph{… \href{…} …}` header).
    ("prose_arg_in_group_refills", WrapMode::Reflow, 80),
    // Math formatting (Stage A): aggressive intra-math spacing — collapse runs,
    // trim just inside the delimiters, tight `^`/`_` scripts. Braces are kept
    // verbatim (dropping redundant single-token script braces is a *content*
    // rewrite, so it lives in the `redundant-script-braces` lint autofix, not the
    // layout engine). A comment inside math forces a line break so it cannot
    // swallow the closing delimiter.
    ("math_collapse_spaces", WrapMode::Preserve, 80),
    ("math_trim_delims", WrapMode::Preserve, 80),
    ("math_tight_scripts", WrapMode::Preserve, 80),
    // A single space is placed around every top-level binary/relation virtual atom,
    // including generated Unicode and command classes. A unary `+`/`-` with no
    // left operand stays glued (`-x`, `x=-b`, `2^{-5}`). A fully glued `/` stays
    // tight, while a gap on either side is made symmetric. Script-size punctuation
    // stays tight throughout, including nested known-math command arguments.
    // Control-word operators retain readable spacing (`a \in A`), while function
    // application glues to its opener (`\Gamma(x)`). An operator-adjacent glued
    // slash reaches its symmetric layout in the same pass (issue #143).
    // Scientific notation (`1e-5`) is deliberately not special-cased.
    ("math_op_spacing", WrapMode::Preserve, 80),
    // Curated argument domains are positional: only known `Math` slots recurse
    // through math spacing, while `Text`, `Unknown`, and slots shadowed by a
    // scanned redefinition remain exactly as authored.
    ("math_argument_domains", WrapMode::Preserve, 80),
    ("math_argument_redefinition", WrapMode::Preserve, 80),
    // The layout engine keeps single-token script braces verbatim; it never
    // strips them (that is the `redundant-script-braces` lint autofix's job).
    ("math_keep_single_token_braces", WrapMode::Preserve, 80),
    // Braces around a script argument are likewise kept when an operator follows
    // (`a_{p}/a_{p - 2}`, `\mathcal{A}_{+}/…`): the braces stay, since a raw
    // strip here would re-glue (`a_p/a_q` re-lexes as `_{p/a}`), which is why even
    // the lint autofix withholds it.
    ("math_keep_braces_before_operator", WrapMode::Preserve, 80),
    ("math_keep_multichar_braces", WrapMode::Preserve, 80),
    ("math_comment_breaks", WrapMode::Preserve, 80),
    // Multiline inline math keeps its fitting opening fragment beside prose,
    // while consecutive own-line comments retain their line association and
    // distinct protected-comment boundaries.
    ("issue_132_math_consecutive_comments", WrapMode::Reflow, 80),
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
    // An explicit top-level `\\` is a mandatory row divider. The operator
    // breaker declines this shape so the shared math sequencer preserves the
    // command and emits a hard break before the next row.
    ("math_display_explicit_line_break", WrapMode::Preserve, 80),
    // A chain of relations aligns in a column: the second `=` starts a fresh
    // continuation line under the first `=`, not under the first right-hand-side
    // term (the two-level rule — relations align, binaries hang one relation-width
    // deeper). When an LHS-derived column would make a continuation overflow,
    // both relations fall back to the display body's base indent.
    ("math_display_break_relations", WrapMode::Preserve, 80),
    // Breaking at a relation does not drag the segment's binary operators along:
    // each segment's right-hand side is its own group, so a segment that fits on
    // its line stays flat (`… = \ilink ( \eta_i) - y_i` keeps `- y_i`) even when
    // the body as a whole had to break at the second `=`.
    ("math_display_break_segment_fits", WrapMode::Preserve, 80),
    // A break before a top-level binary operator does not gain a spurious space at
    // a tight command boundary (`\gamma)`, `}.` stay tight, role-aware like the
    // inline seq path), and an operator nested in parentheses is not a top-level
    // break point (the `-` of `(1 - \gamma)` must not split across lines).
    ("math_display_break_paren_tight", WrapMode::Preserve, 80),
    // The colon-relation family (`\coloneq` and friends) anchors the relation
    // column like `=` (issue #42: unrecognized, it let an interior relation
    // anchor instead, producing a bizarre deep alignment column).
    ("math_display_break_coloneq", WrapMode::Preserve, 80),
    // Escaped-brace and named delimiters count toward bracket depth, so a
    // relation or operator inside a set-builder `\{ … \}` is interior: no anchor,
    // no break point. A body whose only break opportunities sit inside delimiters
    // overflows its line instead of breaking mid-set (issue #42). The `\}` of
    // `\Big \}^{1/2}` rides inside a `SCRIPTED` atom and still closes the depth.
    ("math_display_brace_delims_tight", WrapMode::Preserve, 80),
    // A multi-line left-hand side (a nested matrix environment) has no meaningful
    // flat width, so the relations anchor at the base indent and the first
    // relation breaks onto its own line instead of continuing the `\end{…}` line
    // (issue #39: the LHS's joined flat width used to become the relation column,
    // pushing the matrix bodies dozens of columns right).
    ("math_display_multiline_lhs", WrapMode::Preserve, 80),
    // A leading `\label{…}` is equation bookkeeping, not part of the formula, so it
    // lands on its own line and the math starts below it (under every wrap policy;
    // `MATH_WRAP_FIXTURES` covers preserve). The split is scoped to a *leading*
    // `\label`: a trailing one stays glued to its line.
    ("math_display_leading_label", WrapMode::Preserve, 80),
    // `\left … \right` matched pairs: lowered tight to their delimiters (the body
    // trimmed just inside), with nesting and scripts on the whole pair. A
    // control-word delimiter (`\langle`) keeps one space so the body cannot glue
    // onto it (`\left\langlex` would re-lex as one control word).
    ("math_left_right", WrapMode::Preserve, 80),
    ("math_left_right_control_word_delim", WrapMode::Preserve, 80),
    ("math_left_right_nested_scripted", WrapMode::Preserve, 80),
    // Brackets in math are content unless they read as an optional argument
    // (issue #43): a spaced `\Big [ … \Big ]` never becomes an OPTIONAL group
    // (no break after `[` / before `]`), a math environment's next-line
    // `[\partial_\mu V]_1` stays a row cell with its subscript, and only
    // `multlined`'s directly-abutting `[t]` attaches to its `\begin`.
    ("math_bracket_not_optional", WrapMode::Preserve, 80),
    // Alignment-aware formatting: an `align`/matrix-family environment lays its `&`
    // columns into a grid (left-aligned, single space around `&`, terminated rows
    // padded so their `\\` markers align), preserving the row break (with its
    // `[len]`). An unterminated row remains unpadded. A lone interior newline in a
    // cell is a continuation line and joins onto its aligned row. A nested
    // block environment (`aligned`, `cases`, a matrix) in the *last* cell of a row
    // keeps the grid: the cell renders multi-line, its later lines hanging at the
    // nested `\begin{…}` column (so the `\end{…}` sits directly under it), and
    // takes no part in column widths. A cell that still
    // cannot sit on the grid (a nested block before a `&`, or a blank line inside
    // the cell) falls back to the plain indented body — while a nested alignment
    // environment is still aligned in its own right.
    ("align_columns_basic", WrapMode::Preserve, 80),
    // A user-defined (unclassified) environment with a top-level `&` grid-aligns
    // like a curated alignment env: `&` at catcode 4 is a column tab, a static
    // CST-shape fact (issue #84, `\begin{myaligned}`). Uneven columns pad, and a
    // body with only `\\` and no `&` stays generic (never gridded) — the gate keys
    // on `&`, not `\\`, so an arbitrary line-broken environment is left alone.
    ("align_user_env_ampersand", WrapMode::Preserve, 80),
    ("align_user_env_uneven", WrapMode::Preserve, 80),
    ("align_user_env_linebreak_only", WrapMode::Preserve, 80),
    ("align_columns_uneven_rows", WrapMode::Preserve, 80),
    ("align_columns_linebreak_optional", WrapMode::Preserve, 80),
    ("align_row_terminators", WrapMode::Preserve, 80),
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
    // A final row whose trailing comment is followed by comment-only lines keeps
    // the grid: the first comment trails the row, the rest are passthrough lines.
    // Falling back instead diverged across passes under `Reflow` — the fallback
    // merged the comment lines into one, which the *second* pass then laid out as
    // a grid, spacing the `&` (issue #54, HoTT/book `equivalences.tex`).
    ("align_trailing_comment_lines", WrapMode::Reflow, 80),
    ("tabular_hline", WrapMode::Preserve, 80),
    ("tabular_booktabs", WrapMode::Preserve, 80),
    // A comment-only line directly above a rule command binds into it as a
    // `DOC_COMMENT` (issue #49): the passthrough then spans physical lines,
    // each of which keeps the grid indent.
    ("tabular_rule_doc_comment", WrapMode::Preserve, 80),
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
    // A leading brace group separated from `\begin{equation}` is greedy BEGIN
    // tail content, not a header argument. The specialized math lowering must
    // retain it alongside the following `MATH` node (issue #120).
    ("math_env_begin_tail", WrapMode::Preserve, 80),
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
    // An expl3 conditional in a `.tex` document body, whose `N` slot is the relation
    // `=` (issue #106). The relation is a `WORD` sibling, so greedy attachment never
    // reaches the branch groups and they sit at the *stream* level rather than on the
    // command — the sibling-attached shape.
    ("expl_relation_slot_statement", WrapMode::Reflow, 80),
];

fn fixture_path(name: &str, file: &str) -> PathBuf {
    fixture_root().join(name).join(file)
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/formatter")
}

#[test]
fn every_formatter_fixture_is_registered_once() {
    let mut registered: BTreeMap<&'static str, &'static str> = BTreeMap::new();
    let mut register = |name, family| {
        if let Some(previous) = registered.insert(name, family) {
            panic!("fixture {name} is registered in both {previous} and {family}");
        }
    };

    for &(name, _, _) in FIXTURES {
        register(name, "FIXTURES");
    }
    for &(name, _) in PACKAGE_FIXTURES {
        register(name, "PACKAGE_FIXTURES");
    }
    for &name in DTX_FIXTURES {
        register(name, "DTX_FIXTURES");
    }
    for &(name, _, _) in DTX_REFLOW_FIXTURES {
        register(name, "DTX_REFLOW_FIXTURES");
    }
    for &name in INS_FIXTURES {
        register(name, "INS_FIXTURES");
    }
    for &(name, _, _, _) in MATH_WRAP_FIXTURES {
        register(name, "MATH_WRAP_FIXTURES");
    }

    let mut on_disk = BTreeSet::new();
    for entry in fs::read_dir(fixture_root()).expect("read formatter fixture directory") {
        let entry = entry.expect("read formatter fixture entry");
        assert!(
            entry.file_type().expect("read fixture entry type").is_dir(),
            "unexpected non-directory in formatter fixtures: {}",
            entry.path().display()
        );
        // Git cannot carry an empty directory. Ignore local rename residue so
        // the guard describes the fixture corpus that can actually be committed.
        if fs::read_dir(entry.path())
            .expect("read formatter fixture")
            .next()
            .is_none()
        {
            continue;
        }
        on_disk.insert(
            entry
                .file_name()
                .into_string()
                .expect("formatter fixture name must be UTF-8"),
        );
    }

    for name in registered.keys() {
        assert!(
            on_disk.contains(*name),
            "registered formatter fixture {name} does not exist on disk"
        );
    }
    for name in &on_disk {
        assert!(
            registered.contains_key(name.as_str()),
            "formatter fixture {name} is not registered"
        );
    }
}

/// Package/class fixtures under `tests/fixtures/formatter/<name>/`, each an
/// `input.<ext>` + `expected.<ext>` pair where `<ext>` is `sty` or `cls`. They are
/// parsed and formatted under the [`LatexFlavor::Package`] flavor (`@` is a letter
/// throughout, the implicit `\makeatletter`) and under [`WrapMode::Reflow`],
/// exactly as the CLI/LSP resolve a `.sty`/`.cls` file — every file kind reflows
/// unless the caller overrides `wrap`. Package code barely reaches the prose fill:
/// expl3 regions own their own layout regardless of wrap mode, and a command-only
/// physical line keeps its own line (`line_is_command_only`).
const PACKAGE_FIXTURES: &[(&str, &str)] = &[
    ("package_at_letter_command", "sty"),
    ("class_provides_preserve", "cls"),
    // A trailing comment on the explicit region-ending `\ExplSyntaxOff` remains
    // on that line instead of binding forward into the following definition.
    ("issue_132_expl_off_comment_definition", "sty"),
    // A glued keyval comma must lower to the same flat separator that its broken
    // rendering reparses as. Otherwise pass one can expand the bracket, while
    // pass two sees the inserted newline as a space and collapses it (issue #121).
    ("issue_121_keyval_glued_fixed_point", "sty"),
    // expl3 code formatting, which the wrap mode never reaches:
    // inside an expl3 region (catcode-9 whitespace / catcode-10 `~`) the formatter
    // owns layout regardless of wrap mode — messy indentation is normalized, a
    // function body becomes an indented block, short brace arguments stay inline
    // (`{ value }`), and parameter runs glue tight (`{#1}`, `#1#2`).
    ("expl_function_def", "sty"),
    ("expl_inline_vs_block_groups", "sty"),
    // A statement-leading expl3 conditional explodes structurally (R4/R5): the
    // l3styleguide gold example, head on its own line then each `nTF` branch on its
    // own line at +6 (a short true branch stays `{ … }` inline, the multi-line
    // false branch nests +8) — reproduced regardless of whether it would fit on one
    // line, keyed on the `:…TF` name suffix (`expl_conditional_branches`).
    ("expl_conditional_gold", "sty"),
    // A one-sided (`:nT`) conditional explodes its single branch onto its own line
    // too — the trailing `T`/`F` run is the branch count, so `:nT`/`:nF` is one
    // branch.
    ("expl_conditional_oneside", "sty"),
    // A comment among the branches does not cost the exploded shape (issue #101):
    // one trailing a branch rides that branch's line, an own-line one keeps its own
    // line between branches, and a comment after the whole call (not a child of the
    // command) leaves a fitting conditional flat.
    ("expl_conditional_annotated_branches", "sty"),
    // Issue #101 in full: an annotated branch beside a multi-line one. Each brace
    // argument breaks on its own body — `{ > }` and `{ 1 }` stay inline rather than
    // detonating in sympathy with the sibling the comment forced open.
    ("expl_conditional_comment_siblings", "sty"),
    // The explosion does not depend on where greedy attachment put the branches. A
    // single-token (`N`/`V`) slot breaks attachment, so the branch groups end up on a
    // later sibling — all three peeled off `\l_mypkg_seq`, two off `\l_tmpa_tl`, or at
    // the stream level once a `WORD` relation intervenes — and every one of these fits
    // 80 columns, so the width path would have collapsed it onto one line. Resolved
    // from the call unit's `T`/`F` slots (`expl3_unit`), not from the head node's
    // children, so all four shapes here lay out identically.
    ("expl_conditional_sibling_branches", "sty"),
    // The *mid-line* mirror, pinning that the unit rescan stays out of it. A
    // conditional consumed as another call's `N` slot is a token being passed, not a
    // call: `{ undef }` is `\mypkg_patch:NNnn`'s third argument, so resolving a unit
    // headed at `\cs_if_exist:NTF` would claim it as a true branch and explode the
    // outer call's arguments. Neither line may move — the trailing arm reads only the
    // conditional node's own greedily attached children, which is empty here.
    ("expl_conditional_sibling_trailing", "sty"),
    // A recognized conditional renders all-or-nothing even inside a *fallback*
    // statement, where both position-keyed paths are gated off: a fitting call stays
    // flat, an overflowing one explodes wholly rather than hanging only its last
    // branch and splitting the branch list across two indents.
    ("expl_conditional_in_fallback", "sty"),
    // R3 outside the brace: an expl3 function's argument written flush against its
    // head is respaced (`\clist_count:n{#1}` -> `\clist_count:n {#1}`), while an
    // embedded 2e-named command keeps its authored gap (`\eqref{#1}`,
    // `\ProvidesExplPackage{demo}{…}`). The parameter-run exception is inner-only,
    // so `{#1}` stays tight *and* gains the leading space.
    ("expl_arg_leading_space", "sty"),
    // The l3styleguide's *simple run of parameter* exception: `{#1}`, `{#1#2}`,
    // `{##1}` stay tight (and a padded `{ #1 }` normalizes to tight), while a
    // multi-parameter group with interior spaces (`{ #1 #2 }`) or any
    // non-parameter token (`{ X #2 }`) keeps the canonical inner spaces — exactly
    // the discrimination the guide's own worked example draws.
    ("expl_param_run_tight", "sty"),
    // The positional gate (issue #69): a `\ProvidesExplPackage` in `\def`-definee
    // position (`\protected\def\ProvidesExplPackage`) is tokenized, never executed,
    // so it opens no formatter-owned region — the loader body is left to generic
    // layout, not relaid as expl3 code (which would rewrite real space tokens).
    ("expl_region_midline_open", "sty"),
    // The positional gate, stored-toggle case (issue #69): an `\ExplSyntaxOn` stored
    // inside a definition body is never executed at load, so it opens no region; the
    // following top-level code with `{ x }`/`[ y ]` shapes is left untouched.
    ("expl_region_false_positive", "sty"),
    // Comments in expl3 code: a *trailing* comment rides its statement line
    // zero-width (rustfmt-style — the line may overflow, but prose length
    // never re-breaks code and the comment is never relocated), and an
    // *own-line* comment that binds leading into the next command
    // renders as comment lines continuing the statement —
    // never an opaque block, which would strand a blank line and split the
    // statement head (issue #61, l3bigint.dtx).
    ("expl_doc_comment_statement", "sty"),
    // Structural statement boundaries: a call unit is the head plus the
    // arguments its argspec arity consumes, so authored mid-call newlines join —
    // `\tl_set:Nn` gathers its two arguments across lines, and an `Npn`/`Nn`
    // definition (parameter text shape-scanned, over-attached body peeled back)
    // is one statement regardless of where its body group was authored.
    ("expl_stmt_join", "sty"),
    // The mirror: statement boundaries are formatter-owned, so several complete
    // calls authored on one line split to one call per line.
    ("expl_stmt_split", "sty"),
    // Fallback interleaving: a `w`-spec head (`\exp_after:wN`) and a colonless
    // 2e head (`\def`) have no derivable arity, so their authored physical lines
    // stay the statements — a recognized call sharing the `\def` line is *not*
    // split out — while the recognized `\tl_set:Nn` between them is structural.
    ("expl_stmt_fallback_mixed", "sty"),
    // A blank line (preserved separator) ends a call unit mid-consumption: the
    // partial `\tl_set:Nn \l_demo_tl` commits as-is before the blank, and the
    // stranded `{ x }` starts a fresh statement-leading hang after it.
    ("expl_stmt_blank_end", "sty"),
    // A command's trailing block argument hugs: short leading arguments stay
    // inline and only the over-long trailing group detonates (smoke-test issue
    // #71, latex-lab-block.dtx). Measuring a later group *flat* while deciding an
    // earlier one charged `{block}`/`{thm}` for width that never lands on their
    // line, and the charge depended on where the trailing group's own body
    // happened to break — so pass 1 and pass 2 disagreed and idempotence failed.
    ("expl_trailing_block_hug", "sty"),
    // A *trailing* expl3 conditional — one used mid-line as a value after head atoms,
    // with only trivia after it in the statement — is width-conditional (issue #96,
    // lthooks.dtx). It stays flat on the line when the whole statement fits (the short
    // `\tl_if_empty:nTF {#1} { yes } { no }` line), but when head + conditional
    // overflow, the head drops to its own line and the conditional explodes (R4). Both
    // are committed as one `group(IfBreak { flat, exploded })` measured by the group's
    // flat width, head included, so pass 1 and pass 2 agree: on overflow the flushed
    // head re-parses the conditional as statement-leading and it re-explodes to the
    // identical bytes, where the fill's arbitrary head-break was not pass-stable.
    ("expl_conditional_trailing", "sty"),
    // A trailing brace argument follows its siblings on a *sticky* fill: once a
    // multi-line true-branch detonates onto its own line, the empty (or short)
    // false-branch drops to its own line too, instead of gluing onto the block's
    // short closing `}` line (`} {}`). The greedy fill glued it there, and
    // whether the block's own body broke hard or soft is not pass-invariant, so
    // pass 1 (`} {}`) and pass 2 (`}` / `{}`) disagreed and idempotence failed
    // (smoke-test issue #94, josephwright/siunitx's `\@ifpackageloaded` blocks).
    // The single-statement true-branch is load-bearing: its block breaks only
    // from width — soft on pass 1, hard on the reparse — which is what exposed
    // the drift. A two-statement body would break unconditionally on both passes
    // and never expose it, so do not "tidy" this body. (Under structural
    // boundaries the
    // `\cs_set_protected:Npn \…aux:` head joins — the soft-trailing glue keeps
    // the definiendum on the head line and hangs the body.)
    ("expl_trailing_empty_branch", "sty"),
    // A *trailing* greedily-hung `{body}` — a brace group after head atoms with only
    // trivia following it — whose body is a *multi-command* fill flips K&R->Allman
    // across passes: a body authored on one source line hangs K&R (soft fill) on
    // pass 1, but once that fill wraps the reparse reads the wrapped lines as several
    // statements and detonates it Allman on pass 2 (tagpdf.sty line 1007,
    // latex-lab-testphase-bookmark.sty line 298). A three-candidate all-lines-fit
    // choice (flat / Allman-inline / Allman-broken), keyed on the body's real
    // one-line fit rather than its authored line count, makes both passes agree.
    ("expl_trailing_hang_group", "sty"),
    // A brace group forced open by a comment (or guard, or `.dtx` margin) anywhere
    // inside it lays its body out as a *block*, so the body must be laid out in break
    // mode. When the forced block was a bare concat it inherited the caller's mode
    // instead: dispatched flat, every fill gap in the body rendered flat while the
    // groups hanging off those gaps still decided their own break, producing the K&R
    // hybrid `\int_set:Nn \l_…_int {` with the body wrapped below. The wrapped lines
    // re-parse as separate statements, so the body acquired a forced break and pass 2
    // laid the same group out Allman (smoke-test issue #97, latex3's l3auxdata.dtx).
    // The leading `%` comment is load-bearing — it is what forces the enclosing
    // blocks open and hands the inner body a flat mode; without it the shape is
    // already stable.
    //
    // Under structural boundaries the head joins: the soft-trailing glue keeps `\int_set:Nn
    // \l_@@_groups_int` on one line on *both* passes (the old head/definiendum
    // wart — the statement fill width-splitting the pair on pass 1 while the
    // reparse's forced body head-hugged them on pass 2 — was the last
    // soft-vs-forced dispatch asymmetry; the `\fp_eval:n` body's wrapped `)` is
    // what still flips the body's forced-ness across passes and keeps this
    // fixture load-bearing).
    ("expl_forced_block_body_mode", "sty"),
    // A brace group inside a *fallback* statement must never take the
    // forced-break dispatch: a fallback statement's extent is the authored
    // physical line (Tier 2), so a width wrap inside the group's body mints
    // statement boundaries the reparse reads as hard breaks and the group flips
    // soft->forced on pass 2. The forced arm hard-commits the line, which a
    // plain greedy fill (what a fallback line commits as) has no sticky cascade
    // to reproduce — so the *sibling after the group* glued onto the closing `}`
    // line on pass 1 and dropped to its own line on pass 2.
    //
    // Load-bearing in the `_sibling` fixture (l3kernel's `expl3.sty` backend
    // gate): the `.choices:nn` value is unrecognized, so the line is fallback;
    // the backend list must genuinely wrap at width 80 (that is what mints the
    // pass-2 boundaries); and a *second* group must follow it in the same
    // statement — that trailing `{ \sys_load_backend:n {#1} }` is the gap that
    // flipped. Do not shorten the list.
    ("expl_fallback_forced_group_sibling", "sty"),
    // The same defect where the sibling is a *recognized* head, so
    // `StatementMap::glue_before` owes the gap an unbreakable space
    // (latex2e's `support/lipsum.sty`): the forced arm's `commit_line` emptied
    // `parts`, and `flush_atom` drops a pending separator when `parts` is empty,
    // so the glue was silently discarded and `\tl_put_right:NV` dropped to its
    // own line on pass 2. Load-bearing: `\int_do_until:nNnn`'s unit consumption
    // must fail (that is what makes the line fallback), the multi-line group
    // must be a *sibling* rather than a greedily-attached child, and the whole
    // definition is authored on one physical line.
    //
    // Under arity attachment the expected bytes changed once, deliberately:
    // `\tl_put_right:NV`'s second operand is consumed into the head, and a
    // command node's bare-argument gaps are unbreakable (the house style
    // breaks a call before a braced argument, never between its single-token
    // operands — and a minted newline there is the shape
    // `semantic::expl3::fallback_line` reads as a statement boundary), so the
    // over-width call now overflows instead of wrapping mid-call.
    ("expl_fallback_forced_group_glue", "sty"),
    // The rest of that dispatch, gated the same way: inside a fallback
    // statement *no* arm reads the forced-break predicate, because a fallback
    // line's fill hugs ([`Ir::HugFill`]). A detonating atom is measured by its
    // first line, so it stays on the head's line exactly where the old
    // head-hug arm put it — without that arm's line commit. Load-bearing here
    // (latex2e's `base/ltshipout.dtx`): `\hbox_set_to_wd:Nnn`'s `Nnn` shape
    // does not consume `\l_shipout_box_wd_dim`, so the call is a *fallback*
    // line whose last atom (the greedily-attached `\l_shipout_box_wd_dim
    // {…}`) detonates. Gating the dispatch without the hugging fill splits the
    // pair; no other rule rejoins it.
    ("expl_fallback_hug_head", "sty"),
    // The mirror: an atom the author *abutted* onto a detonating block's
    // closing brace (`}\@ehc`, latex2e's `latex-lab-block.dtx`) stays abutted.
    // The old no-head-to-hug arm committed the line at the block, so the
    // abutting sibling was stranded on a line of its own — a gap the source
    // never had. Load-bearing: the `\@latex@error` body must genuinely wrap at
    // width 80, or the block never detonates and the shape is already stable.
    ("expl_fallback_abutting_sibling", "sty"),
    // In-region `BracketPolicy` audit: bracket re-attachment is
    // stable across passes because the formatter never creates or removes a
    // *flush* junction before a `[` — flush-ness before an attached bracket is
    // a preserved predicate, and space<->lone-newline conversion is invisible
    // to every bracket gate. Four load-bearing shapes: a spaced attached `[…]`
    // that overflows (the fill breaks before the `[`; greedy attachment
    // crosses the newline and re-forms the same unit), a flush `[x]` on an
    // expl3-named head (kept flush — the l3 leading-space respace does not
    // apply to an `OPTIONAL`), the issue-#55 nested shape (the outer `[` stays
    // a plain token only while the flush inner `[` claims the lone `]` in the
    // reachability scan — the inner junction must stay flush), and a bare
    // unattached `[` (its authored gap stands).
    ("expl_bracket_attachment", "sty"),
];

#[test]
fn package_fixtures_match_expected() {
    for &(name, ext) in PACKAGE_FIXTURES {
        let style = FormatStyle {
            wrap: WrapMode::Reflow,
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

        // The full invariant set — whitespace-only, idempotent, clean, lossless,
        // and the trivia-convergence oracle. The expl3 `.sty` fixtures are
        // exactly the K&R<->Allman family the oracle exists to catch, so they
        // must run under it in CI, not only via the `.dtx` corpus and the
        // manual external gate.
        if let Err(msg) = check_format_invariants(&input, style, LatexFlavor::Package.into()) {
            panic!("fixture {name}: {msg}");
        }
    }
}

/// `.dtx` (docstrip) fixtures under `tests/fixtures/formatter/<name>/`, each an
/// `input.dtx` + `expected.dtx` pair. They are parsed and formatted under the
/// docstrip [`LexConfig`] (`dtx: true`, `Document` flavor) and under
/// [`WrapMode::Reflow`], exactly as the CLI/LSP resolve a `.dtx` file. Reflow is
/// safe here because the formatter declines it wherever the `%` margin would be
/// escaped (see `lower_dtx_doc_paragraph`), in every wrap mode. The
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
    // A virtual documentation region owns its leading `%`; generic trivia
    // consumption must not retain it and let the region wrapper add a second one.
    "issue_125_dtx_empty_documentation",
    // The mathtools smoke-test reproducer: command-only doc lines must remain a
    // fixed point after their `%  ` margins normalize (issue #126).
    "issue_126_dtx_command_lines",
    // A virtual environment at the start of a doc paragraph owns the floated
    // leading margin even when ordinary prose follows it in the same paragraph.
    "issue_126_dtx_environment_then_prose",
    // Inline math in a virtual doc environment must treat physical margins as
    // framing; a literal `%` would comment out the rest of the formula.
    "issue_126_dtx_inline_math_margin",
    // Every docstrip guard owns its physical line. Reflow must not join adjacent
    // guarded commands and turn the later guard into a `%` comment.
    "issue_132_dtx_adjacent_guards",
    // Physical margins inside a virtual documentation environment are framing,
    // including at an optional argument's closing edge.
    "issue_132_dtx_optional_argument",
    // A prose-layer tabular environment must choose the same generic/grid path
    // before and after its physical documentation margins are normalized.
    "issue_132_dtx_tabular",
    // A documentation-layer environment whose `\begin` shares its line with
    // body content is not a frame. Splitting after the header would move that
    // content off the `%` margin (smoke-test issue #127, mathtools).
    "issue_127_dtx_inline_environment_body",
    // A math grid inside virtual `.dtx` documentation owns physical `%` margins
    // only as framing. They must not survive as grid-cell text and become a
    // second `%` that comments out each row (smoke-test issue #138, xcolor).
    "issue_138_dtx_doc_math_grid",
    "dtx_expl3_chunks",
    // A statement-leading expl3 conditional inside a `macrocode` body lays out
    // byte-identically to the same code under the `.sty`/`.tex` flavor
    // (`expl_conditional_gold`): the margin frame's column-0 base composes with the
    // in-region `hang_group`/branch-explode so there is no path divergence — the
    // R4/R5 conditional break is flavor-independent.
    "dtx_expl3_conditional",
    // An unmargined doc-part line (a stray `␣%` between chunks, issue #58) is
    // still documentation: an open expl3 region owns only `macrocode` bodies,
    // so the following `% \subsection` keeps its column-0 margin.
    "dtx_expl3_unmargined_doc_line",
    // Doc-prose display math (`% \[…\]`) holding a matrix/array: the nested
    // environment must keep its `%` margins verbatim rather than being re-broken
    // off column 0 (issue #61, l3backend-draw.dtx / l3color.dtx). Covers both a
    // margin-framed `bmatrix` and a `\left\{\begin{array}` opened mid-line.
    "dtx_doc_margin_math",
    // An expl3 code group whose body carries a docstrip guard (`%<…>`) or margin
    // must never flatten inline (issue #61, l3ldb.dtx): a guard off line-start
    // re-lexes as an ordinary `%` comment that swallows the closing brace,
    // unbalancing the enclosing group on the next parse. Forcing the broken form
    // keeps the guard column-0 pinned and the layout a fixed point.
    "dtx_expl3_guarded_group",
    // Docstrip guards *between the arguments* of an expl3 command (issue #78,
    // l3backend-basics.dtx's per-backend `.def` list). The command lays out its
    // attached arguments as a width fill (`Statements::Ignore`, source newlines
    // collapsed), so without pinning the guard would pack onto the previous line
    // as a trailing `%<…>` comment — losing its docstrip meaning and re-lexing on
    // the next pass as a comment that swallows the following `{…}` (a shape drift
    // that never reaches a fixed point). Each guard must open its own line at
    // column 0, its guarded argument following on the same line.
    "dtx_expl3_guarded_arguments",
    // A *fully*-guarded expl3 chunk (a docstrip release block, every line led by
    // `%<latexrelease>`) is preserved verbatim, not block-relaid (issue #72,
    // latex2e `ltcmdhooks.dtx`): the guards pin every line to column 0, so
    // reflowing would strand a delimiter or wrapped token onto an unguarded line
    // — a docstrip meaning change that also never reaches a fixed point. The
    // surrounding *unguarded* expl3 code still formats (its `\cs_new` body
    // collapses and gains l3 brace spacing), proving the region subtraction is
    // surgical.
    "dtx_expl3_guarded_release_block",
    // Adjacent release blocks where a fully guarded expl3 definition has a
    // multi-line parameter text. The parser attaches the continuation guards
    // inside the command, while the trailing single-line commands keep their
    // guards as paragraph siblings. Both shapes must remain byte-faithful.
    "dtx_expl3_adjacent_release_blocks",
    // An *indented* `macrocode` begin frame (smoke-test issue #71, multicol.dtx /
    // latex-lab-block.dtx): `\DocInput` runs the documentation part under
    // `\MakePercentIgnore`, so a `%` there is catcode 9 at any column and the
    // frame opens a chunk exactly like the column-0 spelling. The formatter owns
    // the margin, so it re-pins the frame at column 0 — a trivia-only change.
    "dtx_indented_macrocode_frame",
    // A fallback line's hugging fill and the *early line commits* that take its
    // atoms away from `commit_line` must build the same fill: the trailing
    // command arm hands a head off as one fill, and if that head were a plain
    // [`Ir::Fill`] the very atoms that hugged mid-line would break instead
    // (latex3's `xo-place.dtx`). Load-bearing: the `\int_compare:nNnT` line must
    // be fallback (the `nNnT` shape does not consume `\c_one`), its
    // greedily-attached `{…}` must detonate on a *guard* rather than width, and
    // the statement must end in a trailing command carrying a block
    // (`\bool_if:NT \g_xor_trial_failed_bool {…}`) — that is the arm that
    // commits early. The mid-chunk `\cs_new_nopar:Npn … {` left open across the
    // prose is the file's own shape, and what puts the closing `}` at the end.
    "dtx_expl3_fallback_head_fill",
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
            wrap: WrapMode::Reflow,
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

        assert_eq!(
            perturb::nontrivia_content(&formatted, dtx_config()),
            perturb::nontrivia_content(&format!("{input}\n"), dtx_config()),
            "fixture {name} changed non-trivia content"
        );
        assert_eq!(
            comment_texts(&formatted, dtx_config()),
            comment_texts(&input, dtx_config()),
            "fixture {name} changed protected comments"
        );

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

#[test]
fn dtx_document_environment_is_structural_in_every_wrap_mode() {
    let input = fs::read_to_string(fixture_path(
        "issue_127_dtx_inline_environment_body",
        "input.dtx",
    ))
    .expect("read issue #127 input");
    for wrap in [
        WrapMode::Reflow,
        WrapMode::Stable,
        WrapMode::Sentence,
        WrapMode::Semantic,
        WrapMode::Preserve,
    ] {
        let style = FormatStyle {
            wrap,
            ..FormatStyle::default()
        };
        let formatted = format_with_style_flavored(&input, style, dtx_config()).expect("format");
        assert!(
            formatted.contains("%   \\begin{picture}(90,30)(-30,40)"),
            "picture header split under {wrap:?}:\n{formatted}"
        );
        assert!(
            formatted.lines().all(|line| line.starts_with('%')),
            "documentation line lost its margin under {wrap:?}:\n{formatted}"
        );
        assert_eq!(
            format_with_style_flavored(&formatted, style, dtx_config()).expect("reformat"),
            formatted,
            "document environment is not idempotent under {wrap:?}"
        );
    }
}

/// Whether every line of a `.dtx` reflow fixture's output has to fit the width.
///
/// The width guarantee covers what the formatter *lays out*; a construct it
/// declines to break on a structural gate is out of its scope and legitimately
/// overflows. Opting out is per fixture and has to name the gate, so a new
/// overflow cannot appear silently.
#[derive(Clone, Copy)]
enum WidthBound {
    /// Every output line fits `line_width`.
    Enforced,
    /// The fixture pins a line the formatter deliberately leaves over-width.
    DeclinedBreak,
}

/// `.dtx` reflow fixtures: `(name, line_width, width_bound)`. The same mode as
/// [`DTX_FIXTURES`] under the same docstrip [`LexConfig`], but at a narrow width,
/// so the documentation *prose* layer rewraps while a canonical `% ` margin
/// is re-emitted on every wrapped line. Structured content (margin-framed lists,
/// `macrocode` frames) and the `%`-only paragraph separator must round-trip
/// byte-for-byte; only running prose reflows.
const DTX_REFLOW_FIXTURES: &[(&str, usize, WidthBound)] = &[
    // A single long doc line wrapped onto several `% ` lines.
    ("dtx_reflow_prose_wrap", 50, WidthBound::Enforced),
    // Short lines join; a `%no-space` margin normalizes to `% `.
    ("dtx_reflow_prose_joins", 80, WidthBound::Enforced),
    // The `%`-only separator round-trips; the two paragraphs rewrap independently
    // (the second one's leading margin floats out of its paragraph).
    ("dtx_reflow_margin_blank_line", 80, WidthBound::Enforced),
    // A margin-framed `itemize` stays byte-identical (no item-line reflow).
    ("dtx_reflow_itemize", 50, WidthBound::Enforced),
    // A `\title{^^A…}` block whose argument carries `%` margins, and a guarded
    // `%<package>` line, both stay byte-identical — the block through the raw
    // margined-block path, the guard through its column-0 line segment — while
    // the plain prose paragraph between them still rewraps.
    ("dtx_reflow_margin_escape", 50, WidthBound::Enforced),
    // A margin-framed `\changes{…}` block amid long prose: the prose on both
    // sides rewraps under `% `, the block's interior lines stay byte-identical,
    // and its non-canonical `%   ` first-line margin normalizes to `% `.
    ("dtx_reflow_block_amid_prose", 50, WidthBound::Enforced),
    // A `%<package>` guard line inside a doc paragraph: the guard line keeps its
    // column-0 pin byte-identically while the prose on both sides rewraps.
    ("dtx_reflow_guard_mid_paragraph", 50, WidthBound::Enforced),
    // The residual margin-escape gate: a forced-break block with an *unmargined*
    // interior line cannot ride the `% ` margin, so the whole paragraph stays
    // byte-identical on the preserve path.
    ("dtx_reflow_block_escape_residual", 50, WidthBound::Enforced),
    // A glued macro-like documentation atom has no safe or useful prose break.
    // Keep it intact—even over width—instead of splitting a nested `\textit`
    // argument and synthesizing margins that change pass 2's lowering.
    ("issue_128_dtx_nested_group", 80, WidthBound::DeclinedBreak),
    // A root-level paragraph sharing doc prose with two `macrocode` chunks
    // (an out-of-region expl3 run): the prose rewraps under `% ` while each
    // chunk commits raw behind its byte-exact `%    ` frame lead.
    ("dtx_reflow_expl3_doc_run", 50, WidthBound::Enforced),
    // Fully margin-owned virtual environments compose with prose on either side;
    // each environment owns its generated margins while the surrounding prose
    // resumes the ordinary `DtxProse` fill.
    ("dtx_reflow_virtual_regions", 50, WidthBound::Enforced),
    // A `function` environment's xparse `v` name argument is a same-line VERB
    // capture, so its preceding optional must not break. The ordinary `axis`
    // optional can expand with synthesized doc margins, and `\documentclass`
    // inside the `%<*driver>` region expands as ordinary code.
    (
        "dtx_reflow_optional_on_doc_line",
        50,
        WidthBound::DeclinedBreak,
    ),
];

#[test]
fn dtx_reflow_fixtures_match_expected() {
    for &(name, line_width, width_bound) in DTX_REFLOW_FIXTURES {
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
        check_format_invariants_with(&input, dtx_config(), |text| {
            format_with_style_flavored(text, style, dtx_config())
        })
        .unwrap_or_else(|error| panic!("fixture {name} broke an invariant: {error}"));

        // No reflowed line exceeds the width (a fill never overflows except an
        // unbreakable atom wider than the line, which these fixtures avoid) —
        // unless the fixture exists to pin a break the formatter declines.
        if let WidthBound::Enforced = width_bound {
            for line in formatted.lines() {
                assert!(
                    line.chars().count() <= line_width,
                    "fixture {name} line exceeds width {line_width}: {line:?}"
                );
            }
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
            wrap: WrapMode::Reflow,
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
            // This table predates `math-wrap`; pin the breaker so its many
            // Preserve-wrap math fixtures keep testing it (`Auto` would flip
            // them to math preserve). [`MATH_WRAP_FIXTURES`] exercises the knob.
            math_wrap: MathWrap::Break,
            ..FormatStyle::default()
        };
        assert_fixture(name, style);
    }
}

/// Fixtures for the `math-wrap` knob (display-math break policy) and its `auto`
/// derivation from the wrap mode. Scope: `\[…\]`, `$$…$$`, and non-grid math
/// environments; grids and inline math are untouched by the knob.
const MATH_WRAP_FIXTURES: &[(&str, WrapMode, MathWrap, usize)] = &[
    // `Auto` under a Preserve wrap resolves to math preserve: authored line
    // breaks inside the display body survive as hard breaks at the body indent
    // (width ignored), while in-line content still normalizes (operator
    // spacing, brace stripping). Issue #42's motivating equation: the
    // `\qtextq{for all} m \in \bN^*` qualifier stays on its authored line
    // instead of chaining into the relation column.
    (
        "math_wrap_preserve_authored",
        WrapMode::Preserve,
        MathWrap::Auto,
        80,
    ),
    // The same policy through the non-grid `equation` environment route.
    (
        "math_wrap_preserve_equation_env",
        WrapMode::Preserve,
        MathWrap::Auto,
        80,
    ),
    // `single-line` never inserts breaks: a long body joins onto one line and
    // overflows the width, matching inline math's behavior.
    (
        "math_wrap_single_line",
        WrapMode::Preserve,
        MathWrap::SingleLine,
        80,
    ),
    // An explicit `break` decouples from the Preserve wrap: prose keeps
    // authored breaks while display math still re-breaks at its operators.
    (
        "math_wrap_break_under_preserve",
        WrapMode::Preserve,
        MathWrap::Break,
        80,
    ),
    // The leading-`\label` split runs under every policy, not just the breaker:
    // under math preserve the label still lands on its own line while the authored
    // break inside the formula survives.
    (
        "math_wrap_label_splits_under_preserve",
        WrapMode::Preserve,
        MathWrap::Auto,
        80,
    ),
    // Issue #141: a final `\\<newline>` is one control-symbol token whose
    // embedded newline already separates the body from its closer. Math grids
    // may absorb that break into their closing frame and still align; generic
    // environments and display math must not add a second, blank line.
    (
        "issue_141_trailing_control_newline",
        WrapMode::Preserve,
        MathWrap::Preserve,
        80,
    ),
];

#[test]
fn math_wrap_fixtures_match_expected() {
    for &(name, wrap, math_wrap, line_width) in MATH_WRAP_FIXTURES {
        let style = FormatStyle {
            wrap,
            math_wrap,
            line_width,
            ..FormatStyle::default()
        };
        assert_fixture(name, style);
    }
}

/// Run one `input.tex`/`expected.tex` fixture pair under `style`, asserting the
/// output matches and holds the formatter invariants (idempotence, clean parse,
/// losslessness).
fn assert_fixture(name: &str, style: FormatStyle) {
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
                // Mostly short words, but ~1 in 16 is a long unbreakable run that
                // exceeds even the narrowest tested width (24), so the hard-width
                // assertion's `widest_word > line_width` escape hatch is actually
                // exercised rather than dead for this corpus.
                let len = if rng.below(16) == 0 {
                    25 + rng.below(20)
                } else {
                    1 + rng.below(12)
                };
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
        // Vary the hard width; the soft target rides along at `width - 15`.
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

/// Stable wrapping over a `.dtx` documentation-prose block: the `% ` margin
/// runs through the [`Ir::margin_prefix`] path (`continuation_col` accounts for
/// the prefix), which the plain-prose stable tests never exercise. Every emitted
/// line must honor the hard width including the margin, the output must round-trip
/// losslessly under the docstrip config, and stable wrapping must be idempotent.
#[test]
fn stable_wraps_dtx_doc_prose_within_the_margin() {
    let input = "% This documentation paragraph is authored on one long line that clearly overflows.\n\
% A second authored line that also runs well past the configured hard width here.\n";
    let style = FormatStyle {
        line_width: 50,
        wrap: WrapMode::Stable,
        ..FormatStyle::default()
    };
    let formatted = format_with_style_flavored(input, style, dtx_config()).expect("stable dtx");

    // The margin counts toward the width: every wrapped `% ` line stays within 50.
    assert!(
        formatted.lines().all(|line| line.chars().count() <= 50),
        "stable dtx wrapping must honor the hard width including the margin: {formatted:?}"
    );
    // Every line keeps its documentation margin (trivia-only change).
    assert!(
        formatted.lines().all(|line| line.starts_with('%')),
        "stable dtx wrapping must preserve the `%` margin on every line: {formatted:?}"
    );
    // Clean, lossless round-trip under the docstrip config.
    let reparsed = parse_with_flavor(&formatted, dtx_config());
    assert!(
        reparsed.errors.is_empty(),
        "stable dtx output must parse cleanly: {formatted:?}"
    );
    assert_eq!(
        reparsed.syntax().to_string(),
        formatted,
        "stable dtx output must round-trip losslessly"
    );
    // Idempotent under the same config + style.
    assert_eq!(
        format_with_style_flavored(&formatted, style, dtx_config()).expect("reformat"),
        formatted,
        "stable dtx wrapping must be idempotent"
    );
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

    let long_spaced = "See \\citep{anderson2020longitudinal, bernstein2021comparative, chen2022replication} for details.\n";
    let long_glued = "See \\citep{anderson2020longitudinal,bernstein2021comparative,chen2022replication} for details.\n";
    let from_spaced = format(long_spaced).expect("spaced long list formats");
    let from_glued = format(long_glued).expect("glued long list formats");
    assert_eq!(
        from_spaced, from_glued,
        "insignificant whitespace at cite separators must not steer wrapping"
    );
    assert!(
        from_spaced
            .lines()
            .all(|line| line.chars().count() <= FormatStyle::default().line_width),
        "a segmentable cite list must not overflow the configured width"
    );
    assert_format_invariants(long_spaced);
    assert_format_invariants(long_glued);

    let stable = FormatStyle {
        wrap: WrapMode::Stable,
        ..FormatStyle::default()
    };
    let stable_out = format_with_style(long_glued, stable).expect("stable long list formats");
    assert!(
        stable_out
            .lines()
            .all(|line| line.chars().count() <= stable.line_width),
        "stable wrapping must also repair a segmentable cite overflow"
    );
    assert_format_invariants_with_style(long_glued, stable);
}

/// A multi-line brace group whose opener is glued to its first body token
/// (`{\aaa`, no source whitespace) keeps that token on the opener's line rather
/// than Allman-breaking after `{`. In normal catcodes an end-of-line after the
/// non-control-word `{` reads as a space token (TeX reading state M), so the
/// old unconditional break silently injected a space the author never wrote — a
/// meaning change in horizontal mode (TODO's issue #57 review item). A whitespace
/// boundary after `{` is TeX-identical to a newline, so it still breaks. Only the
/// first line rides the opener; the interior still indents one step. The same
/// safety rule keeps a source-glued closing brace on the body's final line.
#[test]
fn glued_brace_opener_keeps_first_body_token_on_its_line() {
    // `Preserve` keeps `\def\x{` on one line so the assertion isolates the
    // opener boundary from paragraph reflow (which would split `\def` and `\x`).
    let style = FormatStyle {
        wrap: WrapMode::Preserve,
        ..FormatStyle::default()
    };

    let glued = "\\def\\x{\\aaa\\bbb\n\\ccc}\n";
    assert_eq!(
        format_with_style_flavored(glued, style, LatexFlavor::Package).expect("formats"),
        "\\def\\x{\\aaa\\bbb\n  \\ccc}\n",
        "a glued opener must not gain a break (and space token) after `{{`"
    );

    // Whitespace already after the opener is TeX-identical to a newline, so the
    // Allman break stands.
    let spaced = "\\def\\x{ \\aaa\\bbb\n\\ccc}\n";
    assert_eq!(
        format_with_style_flavored(spaced, style, LatexFlavor::Package).expect("formats"),
        "\\def\\x{\n  \\aaa\\bbb\n  \\ccc}\n",
        "whitespace after the opener keeps the Allman break"
    );
}

/// The `\begin` argument glue is driven by the scanned signature, not the name: the
/// *same* `\begin{thm}\n{x}` glues only when the document defines `thm`'s arity.
/// Without the definition `thm` is unknown to both the document and the built-in DB,
/// so nothing claims `{x}` as an argument and it is body — indented with the body,
/// not stranded at the `\begin` column.
#[test]
fn user_definition_drives_begin_argument_glue() {
    let style = FormatStyle {
        wrap: WrapMode::Preserve,
        ..FormatStyle::default()
    };
    let undefined = "\\begin{thm}\n{x}\nbody\n\\end{thm}\n";
    assert_eq!(
        format_with_style(undefined, style).expect("formats"),
        "\\begin{thm}\n  {x}\n  body\n\\end{thm}\n",
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
        "\\begin{appendix}\n\\section{Proofs}\n\nText.\n\\end{appendix}\n",
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

/// The canonical `document` environment is a transparent range-formatting
/// container: its body sits at root indentation, so a direct body block can be
/// lowered without also emitting sibling blocks from the document body.
#[test]
fn range_format_document_body_block_excludes_siblings() {
    let style = FormatStyle::default();
    let input = concat!(
        "\\begin{document}\n\n",
        "\\begin{frame}\nUnselected    text.\n\\end{frame}\n\n",
        "Selected    paragraph.\n\n",
        "\\end{document}\n",
    );
    let root = parse(input).syntax();
    let selected = root
        .descendants()
        .find(|node| {
            node.kind() == SyntaxKind::PARAGRAPH && node.text().to_string().contains("Selected")
        })
        .expect("the selected paragraph");

    let fragment = format_node_range_with_signatures(
        &root,
        style,
        &SignatureDb::default(),
        selected.text_range(),
    )
    .expect("formats");

    assert_eq!(fragment, "Selected paragraph.");

    let frame = root
        .descendants()
        .find(|node| {
            node.kind() == SyntaxKind::ENVIRONMENT
                && node.text().to_string().starts_with("\\begin{frame}")
        })
        .expect("the sibling frame");
    let fragment = format_node_range_with_signatures(
        &root,
        style,
        &SignatureDb::default(),
        frame.text_range(),
    )
    .expect("formats");
    assert_eq!(fragment, "\\begin{frame}\n  Unselected text.\n\\end{frame}");
}

// --- Line endings -----------------------------------------------------------
//
// The formatter's printer always builds output with `\n`; `LineEnding` decides
// how those breaks — and any carried through from a protected region — are
// spelled. `Auto` (the default) keeps what the source used, so formatting never
// rewrites a file's line endings behind the author's back.

/// A document with no bare LF outside a CRLF pair.
fn assert_all_crlf(text: &str) {
    assert!(text.contains("\r\n"), "expected CRLF output, got {text:?}");
    assert!(
        !text.replace("\r\n", "").contains('\n'),
        "expected no bare LF, got {text:?}"
    );
}

#[test]
fn crlf_input_keeps_crlf_under_auto() {
    let input = "\\section{One}\r\n\r\nsome    text\r\n";
    let out = format(input).expect("formats");
    assert_all_crlf(&out);
    assert_eq!(out, "\\section{One}\r\n\r\nsome text\r\n");
}

#[test]
fn lf_input_stays_lf_under_auto() {
    let out = format("\\section{One}\n\nsome    text\n").expect("formats");
    assert!(!out.contains('\r'), "auto must not invent a CR: {out:?}");
}

#[test]
fn line_ending_lf_normalizes_crlf() {
    let style = FormatStyle {
        line_ending: LineEnding::Lf,
        ..FormatStyle::default()
    };
    let out = format_with_style("a\r\n\r\nb\r\n", style).expect("formats");
    assert_eq!(out, "a\n\nb\n");
}

#[test]
fn line_ending_crlf_converts_lf() {
    let style = FormatStyle {
        line_ending: LineEnding::Crlf,
        ..FormatStyle::default()
    };
    let out = format_with_style("a\n\nb\n", style).expect("formats");
    assert_eq!(out, "a\r\n\r\nb\r\n");
    assert_all_crlf(&out);
}

/// A verbatim body is emitted from source token text, so before `LineEnding`
/// existed a CRLF document came out with CRLF inside the protected region and LF
/// everywhere else. The conversion is document-wide precisely to close that gap.
#[test]
fn protected_regions_follow_the_document_ending() {
    let input = "text\r\n\\begin{verbatim}\r\n  raw   line\r\n\\end{verbatim}\r\n";

    let auto = format(input).expect("formats");
    assert_all_crlf(&auto);
    assert!(auto.contains("  raw   line"), "verbatim body is preserved");

    let lf = format_with_style(
        input,
        FormatStyle {
            line_ending: LineEnding::Lf,
            ..FormatStyle::default()
        },
    )
    .expect("formats");
    assert!(
        !lf.contains('\r'),
        "lf must reach into verbatim too: {lf:?}"
    );
    assert!(lf.contains("  raw   line"), "verbatim body is preserved");
}

#[test]
fn crlf_output_is_idempotent() {
    for style in [
        FormatStyle::default(),
        FormatStyle {
            line_ending: LineEnding::Crlf,
            ..FormatStyle::default()
        },
        FormatStyle {
            line_ending: LineEnding::Lf,
            ..FormatStyle::default()
        },
    ] {
        let input = "\\begin{itemize}\r\n\\item one\r\n\\item two\r\n\\end{itemize}\r\n";
        let once = format_with_style(input, style).expect("formats");
        let twice = format_with_style(&once, style).expect("re-formats");
        assert_eq!(once, twice, "not idempotent for {:?}", style.line_ending);
    }
}

/// A range fragment is spliced into the surrounding document, so it must carry
/// the document's endings — including when the selected block holds no line
/// break of its own and could not answer on its own.
#[test]
fn range_format_fragment_follows_the_document_ending() {
    let style = FormatStyle::default();
    let input = "first    paragraph.\r\n\r\nsecond    paragraph.\r\n";
    let root = parse(input).syntax();
    let second = root.children().nth(1).expect("a second top-level block");

    let fragment = format_node_range_with_signatures(
        &root,
        style,
        &SignatureDb::default(),
        second.text_range(),
    )
    .expect("formats");
    assert_eq!(fragment, "second paragraph.");

    let env = "\\begin{itemize}\r\n\\item one\r\n\\end{itemize}\r\n";
    let root = parse(env).syntax();
    let block = root.children().next().expect("the environment block");
    let fragment = format_node_range_with_signatures(
        &root,
        style,
        &SignatureDb::default(),
        block.text_range(),
    )
    .expect("formats");
    assert_all_crlf(&fragment);
}

#[test]
fn native_line_ending_matches_the_platform() {
    let style = FormatStyle {
        line_ending: LineEnding::Native,
        ..FormatStyle::default()
    };
    let out = format_with_style("a\n\nb\n", style).expect("formats");
    if cfg!(windows) {
        assert_all_crlf(&out);
    } else {
        assert!(!out.contains('\r'));
    }
}

#[test]
fn detect_reads_the_first_line_break() {
    assert_eq!(LineEnding::detect("a\r\nb\n"), LineEnding::Crlf);
    assert_eq!(LineEnding::detect("a\nb\r\n"), LineEnding::Lf);
    assert_eq!(LineEnding::detect("no break at all"), LineEnding::Lf);
    assert_eq!(LineEnding::detect("\nleading"), LineEnding::Lf);
}

// --- environment aliases (issue #109) ---------------------------------------

/// The issue-#109 shape, with the definitions the alias is inferred from.
const ALIAS_DOC: &str = concat!(
    "\\newcommand{\\bea}{\\begin{eqnarray}}\n",
    "\\newcommand{\\eea}{\\end{eqnarray}}\n",
    "\\bea a&=&b \\\\ &=&c \\eea\n"
);

#[test]
fn env_alias_formats_like_the_spelled_out_environment() {
    // The whole point of the feature: `\bea … \eea` must lay out exactly as
    // `\begin{eqnarray} … \end{eqnarray}` does, delimiters aside.
    let aliased = format(ALIAS_DOC).expect("formats");
    let spelled = format("\\begin{eqnarray} a&=&b \\\\ &=&c \\end{eqnarray}\n").expect("formats");
    let body = |s: &str| {
        s.lines()
            .filter(|l| !l.starts_with("\\newcommand") && !l.trim().is_empty())
            .skip(1)
            .take_while(|l| !l.starts_with("\\eea") && !l.starts_with("\\end{"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(body(&aliased), body(&spelled));
    assert!(body(&aliased).contains("a & = & b"), "math spacing applies");
}

#[test]
fn env_alias_survives_formatting() {
    // The pass-1/pass-2 fixed point. The detector reads the CST rather than the
    // body's source text precisely so a reformat cannot change what is detected;
    // if it could, the second `format` would see no alias and undo the layout.
    let once = format(ALIAS_DOC).expect("formats");
    let twice = format(&once).expect("formats");
    assert_eq!(
        once, twice,
        "alias detection must be stable across a reformat"
    );
    // And a re-spaced definition body still detects.
    let respaced = format("\\newcommand{\\bea}{ \\begin{eqnarray} }\n\\newcommand{\\eea}{\\end{eqnarray}}\n\\bea a&=&b \\eea\n")
        .expect("formats");
    assert!(
        respaced.contains("a & = & b"),
        "trivia in the body must not matter"
    );
}

#[test]
fn trailing_control_newline_reuses_environment_frame_under_reflow() {
    for (input, expected) in [
        (
            "\\begin{center}\ntext\\\n\\end{center}\n",
            "\\begin{center}\n  text\\\n\\end{center}\n",
        ),
        (
            "\\begin{center}\r\ntext\\\r\n\\end{center}\r\n",
            "\\begin{center}\r\n  text\\\r\n\\end{center}\r\n",
        ),
    ] {
        let once = format(input).expect("formats");
        assert_eq!(once, expected);
        assert_eq!(format(&once).expect("reformats"), once);
    }
}

#[test]
fn trailing_control_crlf_remains_aligned() {
    let input = concat!(
        "\\def\\beaa{\\begin{eqnarray*}}\r\n",
        "\\def\\eeaa{\\end{eqnarray*}}\r\n",
        "\\beaa\r\n",
        "a&=&b\\\\\r\n",
        "&&c\\\r\n",
        "\\eeaa\r\n",
    );
    let expected = concat!(
        "\\def\\beaa{\\begin{eqnarray*}}\r\n",
        "\\def\\eeaa{\\end{eqnarray*}}\r\n",
        "\\beaa\r\n",
        "  a & = & b  \\\\\r\n",
        "    &   & c\\\r\n",
        "\\eeaa\r\n",
    );

    let once = format(input).expect("formats");
    assert_eq!(once, expected);
    assert_eq!(format(&once).expect("reformats"), once);
}

#[test]
fn a_literal_begin_does_not_inherit_the_alias_target() {
    // An alias names a *command*. A literal `\begin{bi}` in a file that also
    // defines `\bi` as an alias for `itemize` is an unrelated environment that
    // happens to spell the same word, so it must lay out like any environment the
    // signature DB cannot name — one paragraph, `\item`s left where they are.
    let defs = "\\newcommand{\\bi}{\\begin{itemize}}\n\\newcommand{\\ei}{\\end{itemize}}\n";
    let literal = format(&format!(
        "{defs}\\begin{{bi}}\n\\item aaa\n\\item bbb\n\\end{{bi}}\n"
    ))
    .expect("formats");
    assert!(
        literal.contains("\\item aaa \\item bbb"),
        "a literal `\\begin{{bi}}` must not pick up itemize's list layout, got:\n{literal}"
    );
    // While the alias delimiters themselves still do.
    let aliased = format(&format!("{defs}\\bi\n\\item aaa\n\\item bbb\n\\ei\n")).expect("formats");
    assert!(
        aliased.contains("  \\item aaa\n  \\item bbb"),
        "the alias itself still lays out as itemize, got:\n{aliased}"
    );
}

#[test]
fn env_alias_resolves_with_no_external_signatures() {
    // `format` passes an empty external `SignatureDb`, which is the dprint/wasm
    // plugin's path. The alias must resolve from the document's own scan alone,
    // or the plugin would become a second sanctioned divergence from `badness
    // format` (AGENTS.md allows exactly one).
    let out = format(ALIAS_DOC).expect("formats");
    assert!(out.contains("a & = & b"));
}

// ---------------------------------------------------------------------------
// Comment directives (`% badness-format …`, `% badness …`)
// ---------------------------------------------------------------------------

/// A table the formatter demonstrably rewrites when left to itself, so every
/// test below distinguishes "suppressed" from "happened to already be canonical".
const HAND_ALIGNED: &str =
    "\\begin{tabular}{ll}\n  a   &   b \\\\\n  ccc &   d \\\\\n\\end{tabular}\n";

#[test]
fn baseline_reformats_the_hand_aligned_table() {
    let out = format(HAND_ALIGNED).expect("formats");
    assert_ne!(
        out, HAND_ALIGNED,
        "the fixture must be one the formatter actually rewrites"
    );
}

#[test]
fn format_skip_preserves_the_documented_construct() {
    let src = format!("% badness-format skip: hand-aligned\n{HAND_ALIGNED}");
    let out = format(&src).expect("formats");
    assert_eq!(out, src, "a skipped construct is reproduced byte for byte");
    assert_format_invariants(&src);
}

#[test]
fn format_off_on_preserves_every_block_between_them() {
    let src = format!(
        "% badness-format off\n{HAND_ALIGNED}\n{HAND_ALIGNED}% badness-format on\n{HAND_ALIGNED}"
    );
    let out = format(&src).expect("formats");
    let (suppressed, formatted) = out
        .split_once("% badness-format on\n")
        .expect("the closer survives");
    assert_eq!(
        suppressed,
        &src[..src.find("% badness-format on").expect("has closer")],
        "both blocks inside the region, and the seam between them, stay byte-exact"
    );
    assert_ne!(
        formatted, HAND_ALIGNED,
        "the block after `on` is formatted again"
    );
    assert_format_invariants(&src);
}

#[test]
fn format_skip_file_preserves_the_whole_document() {
    let src = format!("% badness-format skip-file: generated\n{HAND_ALIGNED}");
    assert_eq!(format(&src).expect("formats"), src);
    assert_format_invariants(&src);
}

/// Suppression must not leak past the region: content *outside* it is laid out
/// exactly as if the directives were not there at all.
#[test]
fn suppression_does_not_leak_to_surrounding_content() {
    let prose = "Some     prose    with     collapsible     spacing.\n";
    let src =
        format!("{prose}\n% badness-format off\n{HAND_ALIGNED}% badness-format on\n\n{prose}");
    let out = format(&src).expect("formats");
    assert_eq!(
        out.matches("Some prose with collapsible spacing.").count(),
        2,
        "prose either side of the region still normalizes, got:\n{out}"
    );
}

/// The bare `% badness` family suppresses layout too — it is the combined axis,
/// not a lint-only one.
#[test]
fn combined_family_suppresses_layout() {
    let src = format!("% badness skip: leave it alone\n{HAND_ALIGNED}");
    assert_eq!(format(&src).expect("formats"), src);
    assert_format_invariants(&src);
}

/// An unrecognized directive must format as the ordinary comment it is, never
/// silently suppress — and the lint axis, including its retired spellings, must
/// stay inert here in particular. Those two families still resolve; they simply
/// resolve on the other axis, and nothing about that may reach layout.
#[test]
fn unrecognized_and_lint_directives_do_not_suppress_layout() {
    for lead in [
        "% badness-lint skip deprecated-command: legacy",
        "% badness-lint off",
        "% badness-ignore deprecated-command: legacy",
        "% badness-ignore-file: noisy",
        "% badness-format nonsense",
        "% badness",
        "% an ordinary comment",
    ] {
        let src = format!("{lead}\n{HAND_ALIGNED}");
        let out = format(&src).expect("formats");
        assert_ne!(out, src, "{lead:?} must not suppress the formatter");
    }
}

/// An `off` with no `on` runs to end of file, as it does in every other
/// formatter carrying the directive.
#[test]
fn unclosed_off_runs_to_end_of_file() {
    let src = format!("% badness-format off\n{HAND_ALIGNED}\n{HAND_ALIGNED}");
    assert_eq!(format(&src).expect("formats"), src);
    assert_format_invariants(&src);
}

/// A suppressed region is a preservation-only construct, so it upholds the
/// trivia-perturbation oracle (which [`assert_format_invariants`] runs) by
/// construction: every perturbed variant is reproduced verbatim and is
/// therefore its own fixed point. Asserted on an input with real prose either
/// side, so the perturber has gaps both inside and outside the region to flip.
#[test]
fn suppressed_regions_converge_under_trivia_perturbation() {
    assert_format_invariants(&format!(
        "Prose before, long enough to wrap somewhere along its length.\n\n\
         % badness-format off\n{HAND_ALIGNED}% badness-format on\n\n\
         Prose after, also long enough that the formatter has a decision to make.\n"
    ));
}

// --- declared environments (`badness.toml`) ----------------------------------

/// Format `input` under a project's declarations, through the same engine entry
/// the CLI's stdin path and the language server's fallback take — not a mirror
/// of it, so the oracles below cannot pass on a pipeline nothing ships.
fn format_declared(input: &str, decls: &ResolvedDeclarations) -> Result<String, FormatError> {
    format_with_declarations_sentence(
        input,
        FormatStyle::default(),
        LatexFlavor::Document,
        SentenceOptions::default(),
        decls,
    )
}

/// Resolve a declaration block, written as JSON — the TOML surface belongs to
/// the CLI crate, and the shapes are the same either way.
fn declarations(json: &str) -> ResolvedDeclarations {
    serde_json::from_str::<Declarations>(json)
        .expect("declarations deserialize")
        .resolve()
        .expect("declarations resolve")
}

/// The full invariant suite — whitespace-only, comments, idempotence,
/// losslessness, and the trivia-perturbation oracle — under a declaring config.
///
/// Worth running separately rather than trusting the blind sweep: a declaration
/// changes the tree's *shape*, so it reaches lowerings (grid alignment, math
/// spacing, body indent) that the same bytes never reach without it. An
/// idempotency bug there would be invisible to every other test in this file.
fn assert_declared_format_invariants(input: &str, json: &str) {
    let decls = declarations(json);
    if let Err(msg) = check_format_invariants_with(input, LatexFlavor::Document.into(), |s| {
        format_declared(s, &decls)
    }) {
        panic!("{msg}\nfor declared input: {input:?}");
    }
}

const EQNARRAY_DECL: &str =
    r#"{"environments": {"eqnarray": {"begin": ["\\bea"], "end": ["\\eea"]}}}"#;
const ALIGN_LIKE_DECL: &str = r#"{"environments": {"myenv": {"like": "align"}}}"#;

/// A declared `like` routes the body the way its target's own body is routed:
/// into math, and into the grid layout. `myenv` has no built-in counterpart at
/// all, so nothing but the declaration can produce either.
#[test]
fn a_declared_align_like_environment_lays_out_like_its_target() {
    let decls = declarations(ALIGN_LIKE_DECL);
    let as_env = |name: &str, body: &str| format!("\\begin{{{name}}}\n{body}\\end{{{name}}}\n");

    // Math routing, which is the declaration's own contribution: an operator
    // glued into a `WORD` is split into atoms only inside math.
    let math_body = "a+b = c\n";
    let declared = format_declared(&as_env("myenv", math_body), &decls).expect("formats");
    assert_eq!(declared, "\\begin{myenv}\n  a + b = c\n\\end{myenv}\n");
    let blind = format(&as_env("myenv", math_body)).expect("formats");
    assert!(
        blind.contains("a+b"),
        "undeclared, the body is prose and nothing splits the operator, got:\n{blind}"
    );

    // The grid, on a `&`-carrying body. Alignment alone would not prove the
    // declaration did anything — the generic top-level-`&` arm aligns an
    // unknown environment too — so the claim is the stronger one: the declared
    // environment lays out byte-for-byte as its target does.
    for body in [math_body, "a^2+b &= c \\\\ &= d\n"] {
        let declared = format_declared(&as_env("myenv", body), &decls).expect("formats");
        let spelled = format(&as_env("align", body)).expect("formats");
        assert_eq!(
            declared.replace("myenv", "align"),
            spelled,
            "declared `myenv` must lay out as `align` does, for body {body:?}"
        );
    }
}

#[test]
fn a_declared_alias_lays_out_like_the_environment_it_names() {
    let aliased = format_declared(
        "\\bea a&=&b \\\\ &=&c \\eea\n",
        &declarations(EQNARRAY_DECL),
    )
    .expect("formats");
    assert_eq!(
        aliased, "\\bea\n  a & = & b \\\\\n    & = & c\n\\eea\n",
        "got:\n{aliased}"
    );
}

#[test]
fn declared_alias_layout_upholds_every_invariant() {
    assert_declared_format_invariants("\\bea a&=&b \\\\ &=&c \\eea\n", EQNARRAY_DECL);
    // With prose either side, so the perturber has gaps outside the environment
    // as well as inside it.
    assert_declared_format_invariants(
        "Prose before, long enough that the reflow has a decision to make.\n\n\
         \\bea\n  a &= b \\\\\n  c &= d\n\\eea\n\n\
         Prose after, also long enough to wrap somewhere along its length.\n",
        EQNARRAY_DECL,
    );
}

#[test]
fn declared_environment_layout_upholds_every_invariant() {
    assert_declared_format_invariants(
        "\\begin{myenv}\na&=&b \\\\ &=&c\n\\end{myenv}\n",
        ALIGN_LIKE_DECL,
    );
    // A declared *verbatim* environment: the body is a protected region the
    // formatter may not touch, and the oracle checks it comes back byte-exact.
    assert_declared_format_invariants(
        "\\begin{mycode}\n  keep    this   spacing\n     and this\n\\end{mycode}\n",
        r#"{"environments": {"mycode": {"like": "lstlisting"}}}"#,
    );
}
