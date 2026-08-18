//! What one incremental reparse costs, and which tier answered.
//!
//! `benches/keystroke.rs` times the whole `didChange` pipeline, but it cannot
//! isolate the reparse: it used to derive one as row 3 minus row 2, which was a
//! fair proxy while a full parse was 97% of the keystroke and became a difference
//! of two ~800 us numbers once the leaf tiers landed — five runs of one binary
//! gave 150, 75, 67 and 30 us, and one clamped to zero. This bench calls
//! [`reparse`] directly instead, against a [`ReparseBase`] it builds itself.
//!
//! Timing the entry point directly is also the only way a case can observe
//! [`ReparseTier`]. Through the salsa layer the tier is computed and dropped, and
//! the side channel may not grow an accessor for it (`AGENTS.md`:
//! the reparse cache is not a salsa input and may not become one).
//!
//! # What a case declares
//!
//! Every case carries an [`Expect`]: the outcome it must reach — a named tier, or
//! a decline — and what it claims for speed. A tier assertion is not decoration.
//! A speedup floor says a case got faster; the tier says it got faster *for the
//! declared reason*. Without it a case that claims the protected-body tier would
//! still pass its floor after silently regressing to a full parse on a small
//! document, and declining is always sound, so nothing else would fail.
//!
//! The set includes a **declining** case per document, because the fallback path
//! is what most edits still take and a ratio that does not say which path it
//! timed is not a gate. The decline is typed at the *same offset* as the word
//! case, differing only in the character, so the pair isolates the guard rather
//! than confounding it with position.
//!
//! # Running it
//!
//! ```bash
//! cargo bench --bench reparse                          # numbers only
//! task bench:reparse-gate                              # numbers + contracts
//! BADNESS_BENCH_ASSERT=1 cargo bench --bench reparse   # the gate alone
//! ```
//!
//! Run it in release, which `cargo bench` does. Two consequences. In a debug
//! build the parser's own oracle full-parses on every successful reparse, so the
//! numbers would measure the oracle. And in a release build that oracle is
//! compiled out, which is why this bench **verifies every case untimed before
//! measuring it** — otherwise a gate could certify a fast wrong answer.
//!
//! # Results
//!
//! Median of nine blocks, release, one dev machine. Treat the *ratios* as the
//! finding; the absolutes are the machine's.
//!
//! ```text
//!                              reparse    full parse    speedup   tier
//!   small.tex/word              1.70 us      32.76 us      19.3x   Token
//!   small.tex/verbatim          3.47 us      35.44 us      10.2x   Verbatim
//!   small.tex/decline           1.66 us      33.41 us          —   declined
//!   cv.tex/word                 3.15 us     120.99 us      38.4x   Token
//!   cv.tex/verbatim             4.45 us     121.33 us      27.3x   Verbatim
//!   cv.tex/decline              3.22 us     118.72 us          —   declined
//!   masters/word               15.46 us       2.818 ms    182.3x   Token
//!   masters/verbatim           18.85 us       2.848 ms    151.1x   Verbatim
//!   masters/decline            17.18 us       2.897 ms         —   declined
//!   phd/word                   36.72 us      28.261 ms    769.5x   Token
//!   phd/verbatim               40.39 us      26.832 ms    664.4x   Verbatim
//!   phd/decline                39.18 us      27.003 ms         —   declined
//!   small.tex/region-words       4.63 us      71.45 us      15.4x   Region
//!   small.tex/region-seam        6.23 us      70.04 us      11.2x   Region
//! ```
//!
//! These are **not** comparable with the keystroke bench's end-to-end numbers,
//! which carry the `didChange` splice, the salsa upsert and the cache lookups
//! around this call. A thesis keystroke costs ~0.71 ms end to end, of which the
//! reparse measured here is ~37 us.
//!
//! Two things the shape of this table says. A decline costs about what a splice
//! does, because both pay the same `O(top-level arity)` descent to the leaf —
//! that is what is left of a cheap reparse, and it is why the declining cases
//! carry a bail budget rather than being assumed free. And the speedups climb
//! with document size because both leaf tiers are `O(depth)` while a full parse
//! is `O(file)`: that ratio *is* the feature.
//!
//! Env knobs:
//!   - `BADNESS_BENCH_TARGET_MS` — per-measurement budget (default 500 ms).
//!   - `BADNESS_BENCH_CASE` — run only cases whose id contains this substring, for
//!     profiling one of them or for isolating it from the others in its process.
//!     Refused under the gate, which needs the whole set.
//!   - `BADNESS_BENCH_OUTPUT_JSON` — write a machine-readable report to this path.
//!   - `BADNESS_BENCH_ASSERT=1` — check every case against its contract and exit
//!     non-zero on a violation. Off by default, so a run that only wants the
//!     numbers stays a measurement and never fails the shell it was typed into.

use std::env;
use std::fs;
use std::hint::black_box;
use std::time::{Duration, Instant};

use badness_parser::declarations::ResolvedDeclarations;
use badness_parser::parser::{
    Edit, ReparseBase, ReparseTier, fingerprint, parse_with_declarations_resolved, reparse,
};
use badness_parser::syntax::SyntaxNode;
use serde::Serialize;

mod sites;

use sites::{DOCUMENTS, Site, check_corpus, config, load_document, prepare};

/// The regression ceiling every splicing case carries.
///
/// A reparse slower than the full parse it replaces is a regression whatever else
/// it proves. Stated as a ceiling rather than left to the per-case floors because
/// a floor only constrains the cases that claim a win, and the case most likely
/// to regress quietly is the one that claims the least.
const MIN_SPEEDUP_CEILING: f64 = 0.95;

/// A declining case's `reparse` call, as a share of the full parse it then runs.
///
/// The guard cascade is the price of admission for every edit that turns out not
/// to be spliceable, and it is paid *on top of* the parse. A tier that grows an
/// expensive guard makes every declined keystroke slower, which no other check
/// here would see.
const MAX_BAIL_RATIO: f64 = 0.20;

/// The absolute escape both ratio rules carry.
///
/// A ratio on a microsecond baseline measures noise: `small.tex` parses in tens of
/// microseconds, so a few hundred nanoseconds of guard work reads as a percentage
/// swing. Sized from the measured cost of one full guard cascade on the smallest
/// document, so a case is forgiven its ratio only while its absolute cost stays
/// inside a single cascade — the most any declining step can add.
const MAX_ABSOLUTE_OVERHEAD_US: f64 = 20.0;

/// How many blocks [`time_us`] takes the median of.
///
/// Odd, so the median is a measured block rather than the mean of two.
const BLOCKS: usize = 9;

/// What a case must do, beside taking time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Outcome {
    /// The edit must splice, and must reach exactly this tier.
    Splices(ReparseTier),
    /// The edit must decline, and pay no more than [`MAX_BAIL_RATIO`] to do it.
    Declines,
}

/// What a case claims, checked under `BADNESS_BENCH_ASSERT=1`.
///
/// Every case declares one, so a new case cannot be added without saying what it
/// is for, and the regression ceiling applies to all of them without an opt-in.
#[derive(Clone, Copy)]
struct Expect {
    outcome: Outcome,
    /// Floor on the speedup over a full parse, where the case is a speed *claim*
    /// and not only a guard against regression.
    min_speedup: Option<f64>,
    /// A case-specific replacement for [`MIN_SPEEDUP_CEILING`], carrying the
    /// reason it is not the default one.
    ///
    /// The reason is not decoration: it is only legitimate to relax the ceiling
    /// for a case whose overhead has been profiled and attributed, and the string
    /// is printed on every run so the exemption stays visible instead of quietly
    /// becoming the floor.
    ceiling: Option<(f64, &'static str)>,
}

impl Expect {
    fn splices(tier: ReparseTier) -> Self {
        Self {
            outcome: Outcome::Splices(tier),
            min_speedup: None,
            ceiling: None,
        }
    }

    fn declines() -> Self {
        Self {
            outcome: Outcome::Declines,
            min_speedup: None,
            ceiling: None,
        }
    }

    fn min_speedup(mut self, min: f64) -> Self {
        self.min_speedup = Some(min);
        self
    }

    /// Relax the regression ceiling for one case. No case needs this yet; it is
    /// the sanctioned way to record one that does, so the next person does not
    /// reach for lowering [`MIN_SPEEDUP_CEILING`] for everyone.
    #[allow(dead_code)]
    fn ceiling(mut self, ceiling: f64, reason: &'static str) -> Self {
        self.ceiling = Some((ceiling, reason));
        self
    }
}

/// The speedup floors, calibrated ~5% under the lowest of three runs on an idle
/// machine. A floor set against a loaded machine produces a gate that fails later
/// for no reason.
///
/// **These are the only copy.** A number in `TODO.md` cannot be checked and
/// panache's drifted from its harness inside one phase; read the gate's output,
/// not a table in a document.
///
/// The floors climb with document size because both leaf tiers are `O(depth)` and
/// a full parse is `O(file)` — that ratio *is* the feature, so a case that stopped
/// distinguishing them would be measuring nothing.
///
/// A floor is *not* waived by [`MAX_ABSOLUTE_OVERHEAD_US`], deliberately. That
/// escape exists for the regression rules, where a small absolute overhead reads
/// as a large ratio penalty on a cheap baseline. Applied to a floor it would
/// nullify it: `small.tex` reparses in under two microseconds, so an escape at
/// any useful size would forgive every result the case could produce.
const FLOORS: [(&str, Site, f64); 8] = [
    ("small.tex", Site::Word, 16.5),
    ("small.tex", Site::Verbatim, 9.5),
    ("cv.tex", Site::Word, 32.0),
    ("cv.tex", Site::Verbatim, 19.5),
    ("masters_dissertation.tex", Site::Word, 149.0),
    ("masters_dissertation.tex", Site::Verbatim, 125.0),
    ("phd_dissertation.tex", Site::Word, 560.0),
    ("phd_dissertation.tex", Site::Verbatim, 520.0),
];

const DTX_DOCUMENT: &str = "phase65-inline.dtx";
const DTX_INLINE_TEXT: &str = r"% \section{Phase 6.5 benchmark fixture}
The incremental reparse benchmark needs one dtx document so token tier dtx changes
move a measured row instead of only corpus splice tallies.
This line is deliberately plain prose with many letters so the word site stays in
a WORD leaf under both a full parse and a splice.
%    \begin{macrocode}
\ExplSyntaxOn
\cs_new:Npn \phase_six_five_fixture:n #1 {#1}
\ExplSyntaxOff
%    \end{macrocode}
This trailing prose line keeps the pinned word site in ordinary letters after the
macrocode chunk so the benchmark times a documented dtx word splice.
";
const DTX_FLOOR_WORD: f64 = 3.8;
const REGION_FLOOR_WORDS: f64 = 14.5;
const REGION_FLOOR_SEAM: f64 = 10.5;

struct Case {
    id: String,
    document: &'static str,
    site: Site,
    edit: CaseEdit,
    config: badness_parser::parser::LexConfig,
    expect: Expect,
}

#[derive(Clone, Copy)]
enum CaseEdit {
    Site,
    RegionWords,
    RegionSeam,
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for document in &DOCUMENTS {
        for (site, tier) in [
            (Site::Word, ReparseTier::Token),
            (Site::Verbatim, ReparseTier::Verbatim),
        ] {
            let floor = FLOORS
                .iter()
                .find(|(name, s, _)| *name == document.name && *s == site)
                .map(|(_, _, floor)| *floor)
                .unwrap_or(0.0);
            let mut expect = Expect::splices(tier);
            if floor > 0.0 {
                expect = expect.min_speedup(floor);
            }
            cases.push(Case {
                id: format!("{}/{}", document.name, site.name()),
                document: document.name,
                site,
                edit: CaseEdit::Site,
                config: config(),
                expect,
            });
        }
        cases.push(Case {
            id: format!("{}/{}", document.name, Site::Decline.name()),
            document: document.name,
            site: Site::Decline,
            edit: CaseEdit::Site,
            config: config(),
            expect: Expect::declines(),
        });
    }
    cases.push(Case {
        id: format!("{}/{}", DTX_DOCUMENT, Site::Word.name()),
        document: DTX_DOCUMENT,
        site: Site::Word,
        edit: CaseEdit::Site,
        config: badness_parser::parser::LexConfig {
            flavor: badness_parser::parser::LatexFlavor::Document,
            dtx: true,
        },
        expect: Expect::splices(ReparseTier::Token).min_speedup(DTX_FLOOR_WORD),
    });
    for (name, edit, floor) in [
        ("region-words", CaseEdit::RegionWords, REGION_FLOOR_WORDS),
        ("region-seam", CaseEdit::RegionSeam, REGION_FLOOR_SEAM),
    ] {
        cases.push(Case {
            id: format!("small.tex/{name}"),
            document: "small.tex",
            site: Site::Word,
            edit,
            config: config(),
            expect: Expect::splices(ReparseTier::Region).min_speedup(floor),
        });
    }
    cases
}

fn prepare_case(text: &str, case: &Case) -> (String, Edit, &'static str, usize) {
    match case.edit {
        CaseEdit::Site => {
            let (prepared, at) = prepare(text, case.site);
            let edit = Edit {
                range: at..at,
                insert: case.site.typed().to_owned(),
            };
            (prepared, edit, case.site.name(), at)
        }
        CaseEdit::RegionWords | CaseEdit::RegionSeam => {
            const FIRST: &str = "First benchmark paragraph.\n\n";
            const SECOND: &str = "alpha beta gamma delta.\n\n";
            let anchor = FIRST.len();
            let mut prepared = String::with_capacity(text.len() + FIRST.len() + SECOND.len());
            prepared.push_str(FIRST);
            prepared.push_str(SECOND);
            prepared.push_str(text);
            match case.edit {
                CaseEdit::RegionWords => {
                    let at = anchor;
                    (
                        prepared,
                        Edit {
                            range: at..at + "alpha beta".len(),
                            insert: "better prose".to_owned(),
                        },
                        "region-words",
                        at,
                    )
                }
                CaseEdit::RegionSeam => {
                    // Replace the blank line between the two injected paragraphs
                    // with one space, merging them.
                    let at = FIRST.len() - 2;
                    (
                        prepared,
                        Edit {
                            range: at..at + 2,
                            insert: " ".to_owned(),
                        },
                        "region-seam",
                        at,
                    )
                }
                CaseEdit::Site => unreachable!(),
            }
        }
    }
}

fn load_case_document(case: &Case) -> Option<String> {
    if case.document == DTX_DOCUMENT {
        Some(DTX_INLINE_TEXT.to_owned())
    } else {
        load_document(case.document)
    }
}

fn check_site_pin_for_case(case: &Case, text: &str, at: usize) -> Option<String> {
    let declared = ResolvedDeclarations::default();
    let parse = parse_with_declarations_resolved(text, case.config, &declared).0;
    let root = parse.syntax();
    let offset = rowan::TextSize::try_from(at).ok()?;
    let expected = case.site.expected_leaf();
    let found: Vec<_> = root.token_at_offset(offset).map(|t| t.kind()).collect();
    if found.contains(&expected) {
        return None;
    }
    Some(format!(
        "{}/{}: byte {at} is in {found:?}, expected a {expected:?} — the site relocated or the fixture drifted",
        case.document,
        case.site.name(),
    ))
}

/// Grow the heap to the size a large case needs, once, before anything is timed.
///
/// Without this the **first** case on a large document measures ~40% high, and no
/// amount of warmup inside the measurement fixes it: glibc trims the top of the
/// heap back to the OS as each iteration's tree is freed, and the next iteration
/// faults those pages in again. Once a whole case has run, the heap holds chunks
/// that are no longer trimmable and later cases stop paying it —
/// `MALLOC_TRIM_THRESHOLD_=-1` collapses all three thesis cases onto the same
/// ~26 ms, which is how this was pinned down.
///
/// Left as a pre-warm rather than a `mallopt` call: the allocator's behaviour is
/// the machine's, not the parser's, and a bench that reconfigures it measures a
/// program nobody runs. What matters is that every case sees the *same* heap
/// state, so a floor cannot depend on where its case sits in the list.
fn prewarm(target: Duration) {
    let Some(text) = DOCUMENTS
        .iter()
        .max_by_key(|d| d.bytes)
        .and_then(|d| load_document(d.name))
    else {
        return;
    };
    let declared = ResolvedDeclarations::default();
    // Hold a base parse alive across the loop, as a real case does: it is the
    // resident tree that keeps the heap's top from being trimmed away.
    let (base_parse, _ctx) = parse_with_declarations_resolved(&text, config(), &declared);
    let deadline = Instant::now() + target;
    while Instant::now() < deadline {
        black_box(parse_with_declarations_resolved(&text, config(), &declared));
    }
    black_box(&base_parse);
}

/// Time `f`, returning the median block's cost in microseconds per iteration.
///
/// The median of blocks rather than one mean over one batch: a single mean lets a
/// scheduler hiccup anywhere in the run move the number, and on the largest
/// document a measurement buys only a couple of dozen iterations, so one bad
/// iteration is a large share of them.
///
/// The probe is deliberately not reused as the warmup. The keystroke bench once
/// reported a reparse 16% high that way — cold allocator and cold caches are a
/// large share of a short run — and invented a regression a repeat run erased.
fn time_us<T>(target: Duration, mut f: impl FnMut() -> T) -> f64 {
    let probe = 5;
    let start = Instant::now();
    for _ in 0..probe {
        black_box(f());
    }
    let per_iter = start.elapsed().as_nanos() as f64 / probe as f64;

    let block_budget = target.as_nanos() as f64 / BLOCKS as f64;
    let iters = if per_iter > 0.0 {
        ((block_budget / per_iter) as usize).clamp(3, 200_000)
    } else {
        200_000
    };

    // Warm for a whole measurement budget, and for at least one block's worth of
    // iterations. A handful is not enough: parsing the 730 KB thesis allocates a
    // very large number of small green nodes, and the allocator is still growing
    // its heap for the first few. Measured in its own process with a
    // three-iteration warmup that parse reads 35.7 ms; measured after other cases
    // have warmed the heap it reads 25.5 ms, which matches the number the
    // keystroke bench has always reported. Under-warming does not add noise, it
    // adds a consistent 40% — an error a stable-looking median hides completely,
    // and it made the same document look like two different workloads depending
    // on which case ran first.
    let warm = Instant::now();
    let mut warmed = 0usize;
    while warmed < iters || warm.elapsed() < target {
        black_box(f());
        warmed += 1;
    }

    let mut blocks = Vec::with_capacity(BLOCKS);
    for _ in 0..BLOCKS {
        let start = Instant::now();
        for _ in 0..iters {
            black_box(f());
        }
        blocks.push(start.elapsed().as_nanos() as f64 / iters as f64 / 1_000.0);
    }
    blocks.sort_by(f64::total_cmp);
    blocks[BLOCKS / 2]
}

#[derive(Debug, Clone, Serialize)]
struct CaseResult {
    id: String,
    document: String,
    site: &'static str,
    bytes: usize,
    /// The byte offset the edit was applied at, so a report says which keystroke
    /// it timed and two reports are comparable.
    at: usize,
    /// The tier that answered, or `null` for a decline.
    tier: Option<String>,
    reparse_us: f64,
    full_parse_us: f64,
    /// `full_parse_us / reparse_us`. Meaningful for a splice; for a decline the
    /// interesting number is its reciprocal, the bail ratio.
    speedup: f64,
}

#[derive(Debug, Serialize)]
struct Report {
    schema_version: u32,
    cases: Vec<CaseResult>,
}

fn format_us(us: f64) -> String {
    if us < 1_000.0 {
        format!("{us:>9.2} us")
    } else {
        format!("{:>9.3} ms", us / 1_000.0)
    }
}

/// Run one case: verify it, then measure it.
///
/// Returns the result and, if the untimed verification failed, the description of
/// how. Verification runs first and unconditionally: `cargo bench` is a release
/// build, so the parser's own debug oracle is compiled out and this is the only
/// thing standing between the gate and certifying a fast wrong answer.
fn run_case(case: &Case, text: &str, target: Duration) -> (CaseResult, Vec<String>) {
    let mut problems = Vec::new();
    let (prepared, edit, site_name, at) = prepare_case(text, case);
    let declared = ResolvedDeclarations::default();
    let (base_parse, ctx) = parse_with_declarations_resolved(&prepared, case.config, &declared);
    let base = ReparseBase::from_parts(
        &prepared,
        &base_parse.green,
        &base_parse.errors,
        &ctx,
        case.config,
        &declared,
    );

    let new_text = edit.apply(&prepared);

    let reparsed = reparse(&base, &edit, &new_text);
    let tier = reparsed.as_ref().map(|r| r.tier);

    match (case.expect.outcome, tier) {
        (Outcome::Splices(want), Some(got)) if want == got => {}
        (Outcome::Splices(want), Some(got)) => {
            problems.push(format!("{}: reached {got:?}, declared {want:?}", case.id))
        }
        (Outcome::Splices(want), None) => problems.push(format!(
            "{}: declined, declared {want:?} — the guard that refused it is either \
             wrong or newly stricter, and either way this case has stopped measuring \
             the tier it names",
            case.id
        )),
        (Outcome::Declines, Some(got)) => problems.push(format!(
            "{}: spliced at {got:?}, declared a decline — a tier grew to claim this \
             edit, which may be good news, but the gate's fallback case is gone",
            case.id
        )),
        (Outcome::Declines, None) => {}
    }

    // The invariant, checked here because the in-crate oracle is compiled out of a
    // release build and this bench only ever runs in one.
    if let Some(result) = &reparsed {
        let full = parse_with_declarations_resolved(&new_text, case.config, &declared).0;
        let spliced = SyntaxNode::new_root(result.green.clone());
        if fingerprint(&spliced) != fingerprint(&full.syntax()) {
            problems.push(format!(
                "{}: spliced tree diverged from a full parse",
                case.id
            ));
        }
        if result.errors != full.errors {
            problems.push(format!(
                "{}: spliced errors diverged from a full parse",
                case.id
            ));
        }
        if spliced.to_string() != new_text {
            problems.push(format!("{}: spliced tree is not lossless", case.id));
        }
    }

    let reparse_us = time_us(target, || black_box(reparse(&base, &edit, &new_text)));
    let full_parse_us = time_us(target, || {
        black_box(parse_with_declarations_resolved(
            &new_text,
            case.config,
            &declared,
        ))
    });

    let result = CaseResult {
        id: case.id.clone(),
        document: case.document.to_owned(),
        site: site_name,
        bytes: prepared.len(),
        at,
        tier: tier.map(|t| format!("{t:?}")),
        reparse_us,
        full_parse_us,
        speedup: full_parse_us / reparse_us.max(f64::EPSILON),
    };

    println!(
        "  {:<40} {} reparse, {} full, {:>8.1}x  [{}]",
        case.id,
        format_us(reparse_us),
        format_us(full_parse_us),
        result.speedup,
        result.tier.as_deref().unwrap_or("declined"),
    );

    (result, problems)
}

/// Check the measured cases against their contracts, printing every check with
/// its margin so drift is visible well before it fails.
fn check_expectations(cases: &[Case], results: &[CaseResult]) -> Vec<String> {
    let mut checks: Vec<(bool, String)> = Vec::new();

    println!("\nThresholds");
    println!("{}", "=".repeat(78));

    for case in cases {
        let Some(result) = results.iter().find(|r| r.id == case.id) else {
            // A case that vanished is a failure, not a silent no-op: the whole
            // point of the corpus assertion is that a gate must not pass by not
            // measuring.
            checks.push((false, format!("{}: declared but never ran", case.id)));
            continue;
        };

        if let Some(min) = case.expect.min_speedup {
            checks.push((
                result.speedup >= min,
                format!(
                    "{:<40} speedup {:.1}x >= {min:.1}x",
                    case.id, result.speedup
                ),
            ));
        }

        match case.expect.outcome {
            Outcome::Splices(_) => {
                let overhead_us = result.reparse_us - result.full_parse_us;
                let (ceiling, why) = match case.expect.ceiling {
                    Some((ceiling, reason)) => (ceiling, format!(" [{reason}]")),
                    None => (MIN_SPEEDUP_CEILING, String::new()),
                };
                checks.push((
                    result.speedup >= ceiling || overhead_us <= MAX_ABSOLUTE_OVERHEAD_US,
                    format!(
                        "{:<40} no regression: {:.2}x >= {ceiling:.2}x or {overhead_us:+.1} us \
                         <= {MAX_ABSOLUTE_OVERHEAD_US:.0} us{why}",
                        case.id, result.speedup
                    ),
                ));
            }
            Outcome::Declines => {
                let ratio = result.reparse_us / result.full_parse_us.max(f64::EPSILON);
                checks.push((
                    ratio <= MAX_BAIL_RATIO || result.reparse_us <= MAX_ABSOLUTE_OVERHEAD_US,
                    format!(
                        "{:<40} bail {:.1}% <= {:.0}% or {:.1} us <= {MAX_ABSOLUTE_OVERHEAD_US:.0} us",
                        case.id,
                        ratio * 100.0,
                        MAX_BAIL_RATIO * 100.0,
                        result.reparse_us,
                    ),
                ));
            }
        }
    }

    let mut failures = Vec::new();
    for (passed, description) in checks {
        println!("  {} {description}", if passed { "ok  " } else { "FAIL" });
        if !passed {
            failures.push(description);
        }
    }
    failures
}

fn main() {
    println!("badness incremental reparse bench (parser::reparse, timed directly)");

    let target = Duration::from_millis(
        env::var("BADNESS_BENCH_TARGET_MS")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .unwrap_or(500),
    );

    let assert_mode = matches!(
        env::var("BADNESS_BENCH_ASSERT").as_deref(),
        Ok("1") | Ok("true")
    );

    // Before measuring anything: the corpus has to be the corpus the floors were
    // calibrated against. Every document but `small.tex` is gitignored and
    // `load_document` skips a missing one, so without this a gate run on a fresh
    // checkout would pass by not measuring exactly the strictest cases.
    if assert_mode {
        let problems = check_corpus();
        if !problems.is_empty() {
            eprintln!("BADNESS_BENCH_ASSERT=1 needs the pinned corpus:");
            for problem in &problems {
                eprintln!("  {problem}");
            }
            eprintln!("Run `task bench:download`.");
            std::process::exit(1);
        }
    }

    let filter = env::var("BADNESS_BENCH_CASE").ok();
    if assert_mode && filter.is_some() {
        eprintln!(
            "BADNESS_BENCH_ASSERT=1 cannot run with BADNESS_BENCH_CASE: a gate that measures \
             a subset passes by not measuring the rest."
        );
        std::process::exit(1);
    }

    let cases: Vec<Case> = cases()
        .into_iter()
        .filter(|case| filter.as_deref().is_none_or(|f| case.id.contains(f)))
        .collect();
    let mut results = Vec::new();
    let mut failures = Vec::new();

    prewarm(target);

    println!("\n{}", "=".repeat(78));
    for case in &cases {
        let Some(text) = load_case_document(case) else {
            println!(
                "  {:<40} {} not found — run `task bench:download`, skipping",
                case.id, case.document
            );
            continue;
        };

        // The site pin, checked before the case is timed. A relocated site, an
        // injection that stopped, and a drifted document all land here rather than
        // as a number nobody can attribute.
        if matches!(case.edit, CaseEdit::Site) {
            let (prepared, at) = prepare(&text, case.site);
            if let Some(problem) = check_site_pin_for_case(case, &prepared, at) {
                failures.push(problem);
                continue;
            }
        }

        let (result, problems) = run_case(case, &text, target);
        failures.extend(problems);
        results.push(result);
    }

    // Written before the verdict: a failing gate is exactly when the numbers are
    // worth keeping.
    if let Ok(path) = env::var("BADNESS_BENCH_OUTPUT_JSON") {
        let report = Report {
            schema_version: 1,
            cases: results.clone(),
        };
        match serde_json::to_string_pretty(&report) {
            Ok(json) => match fs::write(&path, json) {
                Ok(()) => println!("\nwrote {path}"),
                Err(e) => eprintln!("\ncould not write {path}: {e}"),
            },
            Err(e) => eprintln!("\ncould not serialize report: {e}"),
        }
    }

    if assert_mode {
        failures.extend(check_expectations(&cases, &results));
        if !failures.is_empty() {
            eprintln!("\n{} contract violation(s):", failures.len());
            for failure in &failures {
                eprintln!("  {failure}");
            }
            std::process::exit(1);
        }
        println!("\nall contracts held");
    } else if !failures.is_empty() {
        // Outside the gate these are still real: a tier that stopped answering is
        // not a threshold question.
        println!(
            "\n{} problem(s) found (not fatal without BADNESS_BENCH_ASSERT=1):",
            failures.len()
        );
        for failure in &failures {
            println!("  {failure}");
        }
    }
}
