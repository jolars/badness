//! The incremental reparse oracle: depth.
//!
//! The invariant, the alphabet's rationale, and the checker live in
//! [`reparse_harness`], because the corpus sweep (`reparse_corpus_sweep.rs`) has to
//! generate the same edits and check the same thing. This file is the *depth* half:
//! hand-written snippets, one per construct whose recognition depends on text a
//! keystroke can reach, plus the workload drivers a real editor produces. The sweep
//! is the breadth half, over the pinned gate corpora.
//!
//! # Status
//!
//! The token and protected-body tiers are live, so the drivers below splice and the
//! assertions bite. Each driver carries a **splice-rate floor**, because a guard that
//! narrows a tier to nothing would otherwise leave every assertion above it vacuously
//! green — panache lost two thirds of its fuzz coverage exactly that way. Two of the
//! floors are *targets* rather than tripwires: prose typing and typing inside a
//! protected body are the workloads the tiers exist for, so anything short of every
//! keystroke is a regression worth failing over.
//!
//! [`the_harness_reaches_the_reparse_entry_point`] is the pointwise half of the same
//! idea: it pins that the harness is calling the thing it claims to check.

#[path = "support/reparse_harness.rs"]
mod reparse_harness;

use std::fs;
use std::path::Path;

use badness_parser::declarations::ResolvedDeclarations;
use badness_parser::parser::{
    Edit, ReparseBase, ReparseTier, Reparsed, parse_with_declarations_resolved, reparse,
};

use reparse_harness::{
    Base, Lcg, Tally, assert_result_matches, assert_splice_floor, config, random_edit,
};

/// Hand-written inputs, one per construct whose recognition depends on text a
/// keystroke can reach. These are where the interesting edits live; the corpus
/// sweep is breadth.
const HAZARD_SNIPPETS: &[&str] = &[
    // Plain prose: the case the token tier exists for.
    "Some ordinary prose with a \\emph{command} in it.\n\nA second paragraph.\n",
    // Protected regions.
    "\\begin{verbatim}\n  raw { $ % \\ text\n\\end{verbatim}\n",
    "\\begin{lstlisting}[language=C]\nint main() { return 0; }\n\\end{lstlisting}\n",
    "Inline \\verb|raw $ % {| and after.\n",
    "\\MakeShortVerb{\\|}\nnow |raw| is verbatim\n\\DeleteShortVerb{\\|}\n",
    // The attached `VERB` shapes, braced and delimited. Also the one place a `WORD`
    // edit sits next to a raw capture the lexer decides by a forward scan, which is
    // not a shape the token tier's join probes were written for.
    "See \\url{https://x/a_b} and \\lstinline|x_$y$| here.\n",
    "\\begin{minted}{python}\nif x: pass  # $not math$\n\\end{minted}\n",
    // Unterminated: the body runs to EOF, so its extent is fixed by the file rather
    // than by anything inside the construct.
    "\\begin{verbatim}\n  raw with no closer\n",
    // Math, including the shape-gated delimiters.
    "Text $x^2 + y_1$ and \\[ \\int_0^1 f \\] and \\(a\\).\n",
    "\\begin{align}\n  a &= b \\\\\n  c &= d\n\\end{align}\n",
    "$\\left( \\frac{a}{b} \\right)$\n",
    // Curated positional domains outside explicit math, plus text and unknown
    // overrides inside it.
    "Before \\frac{x_i+1}{n} and \\sqrt[3^n]{y_j}.\n",
    "$\\text{text_i} + \\unknown{opaque_i} + \\mathrm{math_i}$\n",
    // Environments, including one whose closer the gate can lose.
    "\\begin{itemize}\n  \\item one\n  \\item two\n\\end{itemize}\n",
    "{\\begin{itemize}\\item x}\n",
    // Definition bodies, which drive the two-pass scan.
    "\\newcommand{\\foo}[1]{#1}\n\\foo{bar}\n",
    "\\def\\shellcmd#1{\\@makeother\\$#1}\n\\shellcmd{a_$b$}\n",
    "\\newcommand{\\bea}{\\begin{align}}\n\\newcommand{\\eea}{\\end{align}}\n\\bea x \\eea\n",
    // Conditionals.
    "\\ifnum\\x>5 yes \\else no \\fi\n",
    "\\iffalse commented out \\fi after\n",
    "\\newif\\ifdraft\n\\ifdraft draft \\fi\n",
    // expl3.
    "\\ExplSyntaxOn\n\\cs_new:Npn \\my_fn:nn #1#2 { #1 #2 }\n\\ExplSyntaxOff\n",
    "\\ExplSyntaxOn\n\\tl_set:Nn \\l_tmpa_tl { x }\n\\ExplSyntaxOff\n",
    // `.dtx` layers.
    "% \\begin{macro}{\\foo}\n%    \\begin{macrocode}\n\\def\\foo{bar}\n%    \\end{macrocode}\n% \\end{macro}\n",
    "%<*package>\n\\def\\x{1}\n%</package>\n",
    // Picture bodies.
    "\\begin{tikzpicture}\n  \\draw (0,0) -- (1,1);\n  \\node at (2,2) {x};\n\\end{tikzpicture}\n",
    // Comment binding, which is trivia attachment and therefore easy to get wrong.
    "% a doc comment\n% a second line\n\\section{Titled}\n",
    "text % trailing\nmore text\n",
    // Suppression directives, which are a comment grammar.
    "% badness-format off\n\\weird   spacing\n% badness-format on\n",
    // CRLF, in both a protected region and ordinary prose.
    "prose one\r\nprose two\r\n\r\nnew paragraph\r\n",
    "\\begin{verbatim}\r\n  raw\r\n\\end{verbatim}\r\n",
    // Degenerate shapes.
    "",
    "\n",
    "α",
    "\\",
    "{",
];

/// How many edits each snippet gets. Scaled by `BADNESS_REPARSE_FUZZ_ITERS`, so the
/// gate before a default flip can run the same harness at 10x without a code change.
fn iterations(base: usize) -> usize {
    let scale: usize = std::env::var("BADNESS_REPARSE_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    base * scale.max(1)
}

#[test]
fn single_edits_over_hazard_snippets() {
    let mut tally = Tally::default();

    for (i, snippet) in HAZARD_SNIPPETS.iter().enumerate() {
        let base = Base::new(*snippet);
        for n in 0..iterations(64) {
            let seed = (i as u64) << 32 | n as u64;
            let mut rng = Lcg::new(seed.wrapping_add(0x9E37_79B9_7F4A_7C15));
            let edit = random_edit(&mut rng, snippet);
            base.check(&edit, seed, &format!("snippet #{i}"), &mut tally);
        }
    }

    eprintln!(
        "single edits: {}/{} spliced",
        tally.spliced, tally.attempted
    );
    // A floor, not a target. The hazard alphabet is deliberately most of what a
    // tier must refuse — `\`, `{`, `$`, `%`, `\begin{`, `\ExplSyntaxOn` — so the
    // rate here is low by construction and its only job is to catch a guard that
    // empties the harness rather than narrowing it.
    assert_splice_floor("single edits over hazard snippets", &tally, 5);
}

#[test]
fn chained_edits_over_hazard_snippets() {
    let mut tally = Tally::default();

    for (i, snippet) in HAZARD_SNIPPETS.iter().enumerate() {
        let base = Base::new(*snippet);
        for n in 0..iterations(16) {
            let seed = (i as u64) << 40 | n as u64;
            let mut rng = Lcg::new(seed.wrapping_add(0x1234_5678_9ABC_DEF0));

            // Build a chain of 2-4 edits, each against the text its predecessors
            // produced — the shape a `didChange` batch arrives in.
            let mut text = snippet.to_string();
            let mut chain = Vec::new();
            for _ in 0..(2 + rng.below(3)) {
                let edit = random_edit(&mut rng, &text);
                if !edit.fits(&text) {
                    break;
                }
                text = edit.apply(&text);
                chain.push(edit);
            }
            if chain.is_empty() {
                continue;
            }

            base.check_chain(
                &chain,
                &text,
                seed,
                &format!("chain over snippet #{i}"),
                &mut tally,
            );
        }
    }

    eprintln!(
        "chained edits: {}/{} spliced",
        tally.spliced, tally.attempted
    );
    // Lower than the single-edit floor on purpose: a chain splices only if *every*
    // step does, so the rate is roughly the single-edit rate raised to the chain
    // length.
    assert_splice_floor("chained edits over hazard snippets", &tally, 1);
}

/// Breadth over real documents: the corpus carries constructs no hand-written
/// snippet thought of, and the `.dtx` files carry the doc-margin and guard layers
/// that no `.tex` does.
///
/// This is the in-crate corpus, which runs in `cargo test`. The pinned gate corpora
/// are two orders of magnitude larger and live behind `task reparse-corpora:check`.
#[test]
fn single_edits_over_the_corpus() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut files = 0usize;
    let mut tally = Tally::default();

    for entry in fs::read_dir(&dir).expect("read corpus dir") {
        let path = entry.expect("dir entry").path();
        let ext = path.extension().and_then(|e| e.to_str());
        if !matches!(ext, Some("tex" | "dtx")) {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read corpus file");
        files += 1;

        let label = path.display().to_string();
        let base = Base::new(text.clone());
        for n in 0..iterations(24) {
            let seed = n as u64;
            let mut rng = Lcg::new(seed.wrapping_add(path.as_os_str().len() as u64));
            let edit = random_edit(&mut rng, &text);
            base.check(&edit, seed, &label, &mut tally);
        }
    }

    assert!(files > 0, "no corpus files found in {dir:?}");
    eprintln!(
        "corpus edits: {}/{} spliced across {files} files",
        tally.spliced, tally.attempted
    );
    assert_splice_floor("single edits over the corpus", &tally, 5);
}

/// Typing a word one character at a time, the workload the token tier exists for
/// and the one a tier is most likely to get subtly wrong at a boundary.
#[test]
fn char_by_char_typing_into_prose() {
    let prefix = "A paragraph of prose ";
    let suffix = " and more after it.\n";
    let typed = "incremental";

    let mut text = format!("{prefix}{suffix}");
    let mut tally = Tally::default();
    for (i, ch) in typed.char_indices() {
        let at = prefix.len() + i;
        let edit = Edit {
            range: at..at,
            insert: ch.to_string(),
        };
        Base::new(text.clone()).check(&edit, i as u64, "char-by-char typing", &mut tally);
        text = edit.apply(&text);
    }

    assert_eq!(text, format!("{prefix}{typed}{suffix}"));
    eprintln!(
        "prose typing: {}/{} spliced",
        tally.spliced, tally.attempted
    );
    // The one floor that is a *target* rather than a tripwire. This is the workload
    // the tier exists for, so anything short of every keystroke after the first is a
    // regression worth failing over. The first character is genuinely outside the
    // tier: inserted between two spaces it splits one `WHITESPACE` token into three
    // tokens, which is a change to the kind sequence.
    assert_splice_floor("char-by-char typing into prose", &tally, 90);
}

/// The same, inside a protected body — where newlines are safe but the closing
/// delimiter is what makes the region a region.
#[test]
fn char_by_char_typing_into_a_verbatim_body() {
    let prefix = "\\begin{verbatim}\n";
    let suffix = "\n\\end{verbatim}\n";
    let typed = "raw $ % { text";

    let mut text = format!("{prefix}{suffix}");
    let mut tally = Tally::default();
    for (i, ch) in typed.char_indices() {
        let at = prefix.len() + i;
        let edit = Edit {
            range: at..at,
            insert: ch.to_string(),
        };
        Base::new(text.clone()).check(&edit, i as u64, "typing into verbatim", &mut tally);
        text = edit.apply(&text);
    }

    eprintln!(
        "verbatim typing: {}/{} spliced",
        tally.spliced, tally.attempted
    );
    // A *target*, like the prose driver. Until the protected-body tier landed this
    // driver asserted nothing about splicing at all, so it stayed green while every
    // keystroke here fell back — the shape of hole the floors exist to close. Every
    // character is inside one `VERBATIM_BODY` leaf, so every one should splice.
    assert_splice_floor("char-by-char typing into a verbatim body", &tally, 100);
}

/// Pressing Enter inside a listing, repeatedly.
///
/// The token tier bans a line terminator outright; this is the workload the
/// protected-body tier exists for, and the analogous case was worth 30x in fatou.
#[test]
fn enter_pressed_repeatedly_inside_a_listing() {
    let prefix = "\\begin{lstlisting}[language=C]\nint main() {";
    let suffix = "}\n\\end{lstlisting}\n";

    let mut text = format!("{prefix}{suffix}");
    let mut tally = Tally::default();
    for i in 0..8 {
        let at = prefix.len();
        let edit = Edit {
            range: at..at,
            insert: "\n  return 0;".to_string(),
        };
        Base::new(text.clone()).check(&edit, i, "enter inside a listing", &mut tally);
        text = edit.apply(&text);
    }

    eprintln!(
        "listing newlines: {}/{} spliced",
        tally.spliced, tally.attempted
    );
    assert_splice_floor("enter pressed inside a listing", &tally, 100);
}

/// Typing inside each attached `VERB` shape, where the fragment is the command node
/// rather than the token — the case a token-only relex cannot reach.
#[test]
fn char_by_char_typing_into_a_verb() {
    let mut tally = Tally::default();
    for (prefix, suffix) in [
        ("Inline \\verb|raw ", "| and after.\n"),
        ("A \\lstinline|x_", "| here.\n"),
        ("See \\url{https://x/", "} now.\n"),
    ] {
        let mut text = format!("{prefix}{suffix}");
        for (i, ch) in "abc".char_indices() {
            let at = prefix.len() + i;
            let edit = Edit {
                range: at..at,
                insert: ch.to_string(),
            };
            Base::new(text.clone()).check(&edit, i as u64, "typing into a verb", &mut tally);
            text = edit.apply(&text);
        }
    }

    eprintln!("verb typing: {}/{} spliced", tally.spliced, tally.attempted);
    assert_splice_floor("char-by-char typing into a verb", &tally, 100);
}

/// The harness must be calling the thing it claims to check.
///
/// Every assertion above is vacuously true on a `None`, so without this the suite
/// could pass with `reparse` unwired entirely. It is the pointwise half of the
/// splice-rate floors the drivers carry: a future guard must not be able to
/// silently empty this harness, which is exactly what a window cutoff did to
/// panache's (two thirds of its coverage, every assertion still green).
#[test]
fn the_harness_reaches_the_reparse_entry_point() {
    let text = "Some ordinary prose.\n";
    let declared = ResolvedDeclarations::default();
    let (base_parse, ctx) = parse_with_declarations_resolved(text, config(), &declared);
    let base = ReparseBase::from_parts(
        text,
        &base_parse.green,
        &base_parse.errors,
        &ctx,
        config(),
        &declared,
    );

    let edit = Edit {
        range: 5..5,
        insert: "x".to_string(),
    };
    let result = reparse(&base, &edit, &edit.apply(text))
        .expect("a letter typed into a prose word is the token tier's whole reason to exist");
    assert_eq!(result.tier, ReparseTier::Token);
}

/// The harness's own checker must be able to fail.
///
/// `assert_result_matches` never runs while no tier splices, so nothing else here
/// would notice if it were wrong — a comparison against the *base* instead of the
/// edited text, say, which would pass on every no-op and silently accept every real
/// divergence the day a tier lands. Feeding it a result it must reject is the only
/// thing that pins it.
#[test]
#[should_panic(expected = "tree diverged")]
fn the_harness_rejects_a_wrong_tree() {
    let declared = ResolvedDeclarations::default();
    let wrong = parse_with_declarations_resolved("\\section{Ho}\n", config(), &declared).0;
    let result = Reparsed {
        green: wrong.green,
        errors: wrong.errors,
        tier: ReparseTier::Token,
    };
    assert_result_matches(
        &result,
        "\\section{Hi}\n",
        &declared,
        config(),
        0,
        "self-test",
        &Edit {
            range: 0..0,
            insert: String::new(),
        },
    );
}

/// The other half: a correct tree carrying an error vector a full parse would not.
#[test]
#[should_panic(expected = "errors diverged")]
fn the_harness_rejects_a_perturbed_error_vector() {
    let declared = ResolvedDeclarations::default();
    let text = "\\section{Hi}\n";
    let parse = parse_with_declarations_resolved(text, config(), &declared).0;
    let result = Reparsed {
        green: parse.green,
        errors: vec![badness_parser::parser::SyntaxError {
            message: "invented".to_string(),
            start: 0,
            end: 1,
        }],
        tier: ReparseTier::Token,
    };
    assert_result_matches(
        &result,
        text,
        &declared,
        config(),
        0,
        "self-test",
        &Edit {
            range: 0..0,
            insert: String::new(),
        },
    );
}

/// Errors come out sorted by start offset.
///
/// A precondition, not a nicety: every tier splices the error vector by keeping a
/// prefix, regenerating a middle, and shifting a suffix, which reproduces a full
/// parse's vector only if emission order is positional. badness has one error
/// source (`grammar::parse`), so there is no multi-stream merge to get right — but
/// the ordering has never been asserted anywhere, and a splice is unsound without
/// it.
#[test]
fn parse_errors_are_ordered_by_offset() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut checked = 0usize;

    for entry in fs::read_dir(&dir).expect("read corpus dir") {
        let path = entry.expect("dir entry").path();
        let ext = path.extension().and_then(|e| e.to_str());
        if !matches!(ext, Some("tex" | "dtx")) {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read corpus file");
        let declared = ResolvedDeclarations::default();
        let parse = parse_with_declarations_resolved(&text, config(), &declared).0;

        let mut previous = 0usize;
        for error in &parse.errors {
            assert!(
                error.start >= previous,
                "errors out of order in {}: {} after {previous}\n  {error:?}",
                path.display(),
                error.start,
            );
            previous = error.start;
        }
        checked += 1;
    }

    assert!(checked > 0, "no corpus files found in {dir:?}");
}
