//! The incremental reparse oracle.
//!
//! # What is asserted
//!
//! The governing invariant, on every edit this harness generates: whenever
//! [`reparse`] returns `Some`, its green tree and its error vector must be
//! byte-identical to a full parse of the edited text — and the tree must still be
//! lossless, the house oracle every other parser test leans on. A `None` is
//! trivially correct, since the caller full-parses.
//!
//! The in-crate assert (`parser::reparse::assert_matches_full_parse`) already fires
//! on every successful reparse in a debug build, so this harness is a *generator*
//! rather than a second checker: its job is to reach shapes a hand-written test
//! would not. It re-checks anyway, because the in-crate assert is compiled out in
//! release and a `cargo test --release` run must still be worth something.
//!
//! # Why the alphabet is what it is
//!
//! Random ASCII would spend its whole budget on prose. Every entry in
//! [`HAZARD_ALPHABET`] is a character or word that changes how *later* text lexes
//! or parses — the boundary of one of the sanctioned lexer modes. Typing a `%` mid
//! line turns the rest of it into a comment; a `\begin{` opens an environment whose
//! `\end` is now missing; an `\ExplSyntaxOn` re-lexes `_` and `:` as letters for
//! the rest of the file. Those are the edits a tier has to refuse, and an alphabet
//! that cannot spell them proves nothing.
//!
//! CRLF entries are deliberate rather than incidental. Panache lost its entire
//! reparse feature on Windows-authored files to a seam predicate written as a
//! literal `"\n\n"` test — safe, since it simply never spliced, and a total loss.
//! Measuring the gap beats discovering it.
//!
//! # Status
//!
//! Phase 1 implements no tier, so every edit here currently falls back and the
//! assertions are vacuously true. That is expected and it is why
//! [`the_harness_reaches_the_reparse_entry_point`] exists: it pins that the harness
//! is actually calling the thing it claims to check, so the suite cannot quietly
//! become a no-op. The splice-rate floor that replaces it lands with the first tier.

use std::fs;
use std::path::Path;

use badness_parser::declarations::ResolvedDeclarations;
use badness_parser::parser::{
    Edit, LatexFlavor, LexConfig, ReparseBase, ReparseTier, Reparsed, fingerprint,
    parse_with_declarations_resolved, reparse, reparse_edits,
};
use badness_parser::syntax::SyntaxNode;

/// A seeded linear congruential generator (MMIX constants).
///
/// Hand-rolled rather than a dependency: the parser crate is wasm-clean and
/// publishable, and a dev-dependency on a PRNG for one test is not worth the
/// supply-chain surface. Determinism is the point — every failure prints its seed,
/// so a reproducer is a one-line change.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next() >> 33) as usize % n
    }
}

/// Insert candidates, each chosen because it changes how later text lexes or parses.
const HAZARD_ALPHABET: &[&str] = &[
    // Catcode-bearing characters: the whole reason a LaTeX token tier needs a ban list.
    "\\",
    "{",
    "}",
    "$",
    "$$",
    "%",
    "&",
    "#",
    "^",
    "_",
    "~",
    "[",
    "]",
    // Environment and math delimiters.
    "\\begin{",
    "\\end{",
    "\\[",
    "\\]",
    "\\(",
    "\\)",
    "\\\\",
    // Names that route the parser: verbatim bodies, math environments, aliases.
    "verbatim",
    "lstlisting",
    "equation",
    "align",
    "itemize",
    "document",
    // Protected regions and short verbs.
    "\\verb|",
    "\\verb+",
    "\\MakeShortVerb{\\|}",
    // Letter-mode and expl3 region toggles, whose scope runs to end of file.
    "\\makeatletter",
    "\\makeatother",
    "\\ExplSyntaxOn",
    "\\ExplSyntaxOff",
    ":nn",
    ":Nn",
    "_int",
    // Conditionals, which pair by a forward scan for their closer.
    "\\iffalse",
    "\\ifnum",
    "\\else",
    "\\or",
    "\\fi",
    "\\newif",
    // `.dtx` structure: doc margins, guards, macrocode frames.
    "%",
    "%<*debug>",
    "%</debug>",
    "%    \\begin{macrocode}",
    "%    \\end{macrocode}",
    // Definition heads, which drive the second parse pass.
    "\\newcommand",
    "\\def",
    "\\let",
    "\\renewcommand",
    // Picture-body statement terminators.
    ";",
    "\\draw",
    "\\node",
    // Line structure, including the CRLF forms.
    "\n",
    "\n\n",
    "\r\n",
    "\r\n\r\n",
    " ",
    "  ",
    // Ordinary content, including a multi-byte char so offsets are exercised.
    "x",
    "1",
    "α",
    "…",
];

/// Hand-written inputs, one per construct whose recognition depends on text a
/// keystroke can reach. These are where the interesting edits live; the corpus
/// sweep below is breadth.
const HAZARD_SNIPPETS: &[&str] = &[
    // Plain prose: the case the token tier exists for.
    "Some ordinary prose with a \\emph{command} in it.\n\nA second paragraph.\n",
    // Protected regions.
    "\\begin{verbatim}\n  raw { $ % \\ text\n\\end{verbatim}\n",
    "\\begin{lstlisting}[language=C]\nint main() { return 0; }\n\\end{lstlisting}\n",
    "Inline \\verb|raw $ % {| and after.\n",
    "\\MakeShortVerb{\\|}\nnow |raw| is verbatim\n\\DeleteShortVerb{\\|}\n",
    // Math, including the shape-gated delimiters.
    "Text $x^2 + y_1$ and \\[ \\int_0^1 f \\] and \\(a\\).\n",
    "\\begin{align}\n  a &= b \\\\\n  c &= d\n\\end{align}\n",
    "$\\left( \\frac{a}{b} \\right)$\n",
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

/// The parse inputs every case in this harness runs under.
fn config() -> LexConfig {
    LatexFlavor::Document.into()
}

/// Full-parse `text` and check one edit against the invariant.
///
/// Returns whether a tier accepted the edit, so drivers can report a splice rate —
/// a harness that stops splicing is a harness that stops testing, and that has to
/// be visible rather than silent.
fn check_edit(text: &str, edit: &Edit, seed: u64, label: &str) -> bool {
    let declared = ResolvedDeclarations::default();
    let (base_parse, ctx) = parse_with_declarations_resolved(text, config(), &declared);
    let base = ReparseBase {
        text,
        green: &base_parse.green,
        errors: &base_parse.errors,
        ctx: &ctx,
        config: config(),
        declared: &declared,
    };

    if !edit.fits(text) {
        return false;
    }
    let new_text = edit.apply(text);

    let Some(result) = reparse(&base, edit, &new_text) else {
        return false;
    };
    assert_result_matches(&result, &new_text, &declared, seed, label, edit);
    true
}

fn assert_result_matches(
    result: &Reparsed,
    new_text: &str,
    declared: &ResolvedDeclarations,
    seed: u64,
    label: &str,
    edit: &Edit,
) {
    let full = parse_with_declarations_resolved(new_text, config(), declared).0;
    let spliced = SyntaxNode::new_root(result.green.clone());

    assert_eq!(
        fingerprint(&spliced),
        fingerprint(&full.syntax()),
        "tree diverged\n  case: {label}\n  seed: {seed}\n  tier: {:?}\n  edit: {edit:?}\n  text: {new_text:?}",
        result.tier,
    );
    assert_eq!(
        result.errors, full.errors,
        "errors diverged\n  case: {label}\n  seed: {seed}\n  tier: {:?}\n  edit: {edit:?}\n  text: {new_text:?}",
        result.tier,
    );
    // Losslessness is implied by tree equality plus the full parse's own guarantee,
    // but it is the house oracle and it costs a string compare.
    assert_eq!(
        spliced.to_string(),
        new_text,
        "losslessness failed\n  case: {label}\n  seed: {seed}\n  edit: {edit:?}",
    );
}

/// Draw one random edit against `text`: ~70% insert, ~20% delete, ~10% replace.
fn random_edit(rng: &mut Lcg, text: &str) -> Edit {
    let at = char_boundary_at_or_below(text, rng.below(text.len() + 1));
    match rng.below(10) {
        0..=6 => Edit {
            range: at..at,
            insert: HAZARD_ALPHABET[rng.below(HAZARD_ALPHABET.len())].to_string(),
        },
        7..=8 => {
            let end = next_char_boundary(text, at);
            Edit {
                range: at..end,
                insert: String::new(),
            }
        }
        _ => {
            let end = next_char_boundary(text, at);
            Edit {
                range: at..end,
                insert: HAZARD_ALPHABET[rng.below(HAZARD_ALPHABET.len())].to_string(),
            }
        }
    }
}

fn char_boundary_at_or_below(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn next_char_boundary(text: &str, at: usize) -> usize {
    let mut end = (at + 1).min(text.len());
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    end
}

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
    let mut spliced = 0usize;
    let mut attempted = 0usize;

    for (i, snippet) in HAZARD_SNIPPETS.iter().enumerate() {
        for n in 0..iterations(64) {
            let seed = (i as u64) << 32 | n as u64;
            let mut rng = Lcg(seed.wrapping_add(0x9E37_79B9_7F4A_7C15));
            let edit = random_edit(&mut rng, snippet);
            attempted += 1;
            if check_edit(snippet, &edit, seed, &format!("snippet #{i}")) {
                spliced += 1;
            }
        }
    }

    eprintln!("single edits: {spliced}/{attempted} spliced");
    // A floor, not a target. The hazard alphabet is deliberately most of what a
    // tier must refuse — `\`, `{`, `$`, `%`, `\begin{`, `\ExplSyntaxOn` — so the
    // rate here is low by construction and its only job is to catch a guard that
    // empties the harness rather than narrowing it.
    assert_splice_floor("single edits over hazard snippets", spliced, attempted, 5);
}

/// Assert a driver still splices often enough to be testing anything.
///
/// Phrased as a percentage of attempts so `BADNESS_REPARSE_FUZZ_ITERS` does not
/// change the verdict, and reported with both numbers so a failure says how far it
/// fell rather than merely that it did.
fn assert_splice_floor(driver: &str, spliced: usize, attempted: usize, floor_percent: usize) {
    assert!(attempted > 0, "{driver}: nothing was attempted");
    let percent = spliced * 100 / attempted;
    assert!(
        percent >= floor_percent,
        "{driver}: splice rate fell to {percent}% ({spliced}/{attempted}), floor is \
         {floor_percent}%. A guard narrowed the tier; either it is wrong or this \
         floor moves — deliberately, in its own commit.",
    );
}

#[test]
fn chained_edits_over_hazard_snippets() {
    let mut spliced = 0usize;
    let mut attempted = 0usize;

    for (i, snippet) in HAZARD_SNIPPETS.iter().enumerate() {
        for n in 0..iterations(16) {
            let seed = (i as u64) << 40 | n as u64;
            let mut rng = Lcg(seed.wrapping_add(0x1234_5678_9ABC_DEF0));

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

            let declared = ResolvedDeclarations::default();
            let (base_parse, ctx) = parse_with_declarations_resolved(snippet, config(), &declared);
            let base = ReparseBase {
                text: snippet,
                green: &base_parse.green,
                errors: &base_parse.errors,
                ctx: &ctx,
                config: config(),
                declared: &declared,
            };

            attempted += 1;
            if let Some(result) = reparse_edits(&base, &chain, &text) {
                spliced += 1;
                assert_result_matches(
                    &result,
                    &text,
                    &declared,
                    seed,
                    &format!("chain over snippet #{i}"),
                    chain.last().expect("non-empty chain"),
                );
            }
        }
    }

    eprintln!("chained edits: {spliced}/{attempted} spliced");
    // Lower than the single-edit floor on purpose: a chain splices only if *every*
    // step does, so the rate is roughly the single-edit rate raised to the chain
    // length.
    assert_splice_floor("chained edits over hazard snippets", spliced, attempted, 1);
}

/// Breadth over real documents: the corpus carries constructs no hand-written
/// snippet thought of, and the `.dtx` files carry the doc-margin and guard layers
/// that no `.tex` does.
#[test]
fn single_edits_over_the_corpus() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut files = 0usize;
    let mut spliced = 0usize;
    let mut attempted = 0usize;

    for entry in fs::read_dir(&dir).expect("read corpus dir") {
        let path = entry.expect("dir entry").path();
        let ext = path.extension().and_then(|e| e.to_str());
        if !matches!(ext, Some("tex" | "dtx")) {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read corpus file");
        files += 1;

        let label = path.display().to_string();
        for n in 0..iterations(24) {
            let seed = n as u64;
            let mut rng = Lcg(seed.wrapping_add(path.as_os_str().len() as u64));
            let edit = random_edit(&mut rng, &text);
            attempted += 1;
            if check_edit(&text, &edit, seed, &label) {
                spliced += 1;
            }
        }
    }

    assert!(files > 0, "no corpus files found in {dir:?}");
    eprintln!("corpus edits: {spliced}/{attempted} spliced across {files} files");
    assert_splice_floor("single edits over the corpus", spliced, attempted, 5);
}

/// Typing a word one character at a time, the workload the token tier exists for
/// and the one a tier is most likely to get subtly wrong at a boundary.
#[test]
fn char_by_char_typing_into_prose() {
    let prefix = "A paragraph of prose ";
    let suffix = " and more after it.\n";
    let typed = "incremental";

    let mut text = format!("{prefix}{suffix}");
    let mut spliced = 0usize;
    let mut attempted = 0usize;
    for (i, ch) in typed.char_indices() {
        let at = prefix.len() + i;
        let edit = Edit {
            range: at..at,
            insert: ch.to_string(),
        };
        attempted += 1;
        if check_edit(&text, &edit, i as u64, "char-by-char typing") {
            spliced += 1;
        }
        text = edit.apply(&text);
    }

    assert_eq!(text, format!("{prefix}{typed}{suffix}"));
    eprintln!("prose typing: {spliced}/{attempted} spliced");
    // The one floor that is a *target* rather than a tripwire. This is the workload
    // the tier exists for, so anything short of every keystroke after the first is a
    // regression worth failing over. The first character is genuinely outside the
    // tier: inserted between two spaces it splits one `WHITESPACE` token into three
    // tokens, which is a change to the kind sequence.
    assert_splice_floor("char-by-char typing into prose", spliced, attempted, 90);
}

/// The same, inside a protected body — where newlines are safe but the closing
/// delimiter is what makes the region a region.
#[test]
fn char_by_char_typing_into_a_verbatim_body() {
    let prefix = "\\begin{verbatim}\n";
    let suffix = "\n\\end{verbatim}\n";
    let typed = "raw $ % { text";

    let mut text = format!("{prefix}{suffix}");
    for (i, ch) in typed.char_indices() {
        let at = prefix.len() + i;
        let edit = Edit {
            range: at..at,
            insert: ch.to_string(),
        };
        check_edit(&text, &edit, i as u64, "typing into verbatim");
        text = edit.apply(&text);
    }
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
    let base = ReparseBase {
        text,
        green: &base_parse.green,
        errors: &base_parse.errors,
        ctx: &ctx,
        config: config(),
        declared: &declared,
    };

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
        tier: badness_parser::parser::ReparseTier::Token,
    };
    assert_result_matches(
        &result,
        "\\section{Hi}\n",
        &declared,
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
        tier: badness_parser::parser::ReparseTier::Token,
    };
    assert_result_matches(
        &result,
        text,
        &declared,
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
