//! What one keystroke costs through the salsa pipeline, end to end.
//!
//! `benches/formatting.rs` times the per-byte formatter work and
//! `benches/compare_format.sh` times the CLI, but neither touches
//! [`IncrementalDatabase`] or the `didChange` splice — the composition a real
//! editor session runs on every character. `tests/scaling.rs` guards growth
//! ratios, not the pipeline. This bench is that missing row: the path from a
//! `didChange` notification to a parse tree, timed as one thing.
//!
//! Four rows per document, of which the first is a reference rather than a stage:
//!
//! 0. `text copy (reference)` — one allocation and one linear copy of the
//!    document. Nothing in the pipeline calls this; it is here so the write phase
//!    has a machine-independent unit to be measured in, since what a splice
//!    *should* cost is a small number of linear passes.
//! 1. `upsert, text unchanged` — the staleness guard alone: what a no-op upsert
//!    costs to prove there is nothing to do. This is the cost the language
//!    server pays whenever a job re-writes text salsa already has (a disk
//!    re-read, a redundant sync), and the one row that must stay flat in the
//!    document size.
//! 2. `splice + upsert (write phase)` — [`apply_content_changes`] on the live
//!    [`TextBuffer`], handing the result to `upsert_file`, and staging the edit
//!    chain for the incremental reparse, with no parse demanded. This is where
//!    the per-keystroke *text copies* live (or don't), so it is the row on which
//!    text-storage designs — `String`, `Arc<str>`, a rope — are actually
//!    comparable.
//! 3. `keystroke end-to-end (parse included)` — the same plus `parsed_tree`.
//!
//! This bench deliberately reports **no reparse row**. It used to derive one as
//! row 3 minus row 2, which was a fair proxy while a full parse was 97% of the
//! keystroke. With both leaf tiers landed it is a difference of two ~800 us
//! numbers on the thesis: five runs of one binary gave 150, 75, 67 and 30 us, and
//! one clamped to zero when row 3 came out *below* row 2. `benches/reparse.rs`
//! times `parser::reparse` directly instead, which is also the only way a case
//! can assert which tier it reached.
//!
//! Each timed iteration alternates inserting and deleting one character, so the
//! text genuinely changes every round: salsa sees a fresh revision and the
//! printed number is one keystroke, never a memoized no-op. Every row is the
//! median of nine blocks, and **rows 0 and 2 are measured interleaved** — a block
//! of one, then a block of the other — because their ratio is the contract and
//! anything that drifts between two rows timed to completion in turn lands in the
//! quotient and nowhere else.
//!
//! The one line that differs per branch is [`handoff`], which converts the live
//! buffer into what `upsert_file` takes on that branch — the knob for A/B'ing a
//! text-storage change.
//!
//! `harness = false` (a plain `main`), like the sibling `formatting` bench, so a
//! profiler attaches cleanly to a single document:
//!
//! ```bash
//! task bench:keystroke
//! BADNESS_BENCH_DOC=phd_dissertation.tex cargo bench --bench keystroke
//! ```
//!
//! Env knobs:
//!   - `BADNESS_BENCH_DOC` — bench only this doc under `benches/documents/`.
//!   - `BADNESS_BENCH_SITE` — which keystroke to measure: `word` (the default),
//!     `verbatim`, or `decline` (see [`Site`]).
//!   - `BADNESS_BENCH_TARGET_MS` — per-row timing budget (default 500 ms); each
//!     row auto-calibrates its iteration count to fill it.
//!   - `BADNESS_BENCH_OUTPUT_JSON` — write a machine-readable report to this
//!     path (the A/B diff between two builds).
//!   - `BADNESS_BENCH_ASSERT=1` — check every row against its declared contract
//!     and exit non-zero on a violation (see [`check_expectations`]).
//!
//! # The gate
//!
//! ```bash
//! task bench:keystroke-gate
//! BADNESS_BENCH_ASSERT=1 cargo bench --bench keystroke
//! ```
//!
//! Absolute microseconds are machine-dependent, so **every check is a ratio
//! between two numbers from the same run** — how a row scales across an order of
//! magnitude of document size, and how many copies of the document the write
//! phase costs. Each is waived below [`MIN_ABSOLUTE_US`], because a ratio on a
//! sub-microsecond baseline measures noise.
//!
//! Run it on an **idle** machine. Interleaving rows 0 and 2 fixed the variance
//! (eight runs held under 4%, where sequential timing spread 49.8 to 64.4), but
//! not the level: under twenty spinning threads the same binary reads half again
//! as many copies, tight to under 1%, because contention costs the branchy write
//! phase more than it costs a memcpy. See [`WRITE_MAX_COPIES`].
//!
//! There is deliberately **no "write phase is N% of the keystroke" check**, which
//! is the obvious one to reach for. Rows 2 and 3 are separately calibrated loops
//! and their difference is smaller than the noise on either, so on the thesis row
//! 2 measures *above* row 3 and the share reads over 100% — the same reason the
//! derived reparse row was deleted. Row 0 exists so the write phase has something
//! to be a ratio of that is genuinely nested with it — and is measured *beside*
//! it, block by block, so the pairing is real and not just nominal.
//!
//! The gate is off by default, so a run that only wants the numbers stays a
//! measurement and never fails the shell it was typed into.
//!
//! # Results
//!
//! One idle dev machine, release, `word` site. Treat the ratios as the finding.
//!
//! ```text
//!                          small      cv    masters       phd
//!   text copy (ref)         42 ns   81 ns    1.20 us   11.15 us
//!   noop upsert            136 ns  137 ns     140 ns     138 ns
//!   write phase            413 ns  558 ns    3.19 us   28.02 us
//!     as document copies    9.9x    6.9x       2.7x       2.5x
//!   keystroke end to end  2.60 us 4.20 us   19.93 us   91.04 us
//! ```
//!
//! 2.5 copies of the document is the floor plus change. Two are the text rebuild
//! an `Arc<str>` cannot avoid — it adopts no `String`'s allocation, so the splice
//! is built and then copied — and the rest is cloning the line table and shifting
//! its tail. Getting below it means `Arc<String>` and `Arc::make_mut`, which taxes
//! every read; it is not filed as work.
//!
//! It arrived in two measured steps, and the middle column is why the first was
//! worth taking separately: the reshape made the table cheap to *build* and the
//! patch made it not get built.
//!
//! ```text
//!   write phase, thesis    baseline    reshaped    patched
//!   absolute              575.28 us   161.57 us   28.02 us
//!   document copies           51.8x       14.7x       2.5x
//!   end to end            640.37 us   246.43 us   91.04 us
//! ```

use std::env;
use std::fs;
use std::hint::black_box;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use badness::incremental::{IncrementalDatabase, IncrementalDb};
use badness::lsp::apply_content_changes;
use badness::text::{PositionEncoding, TextBuffer};
use lsp_types::{Position, Range, TextDocumentContentChangeEvent};
use serde::Serialize;

mod sites;

use sites::{DOCUMENTS, Site, check_corpus, check_site_pin, load_document, prepare};

/// The encoding an LSP client negotiates by default, and so the one the live
/// buffer's [`LineIndex`](badness::text::LineIndex) is built in.
const UTF16: PositionEncoding = PositionEncoding::Utf16;

/// The live buffer, as `upsert_file` takes it on this branch.
///
/// Comparing two text-storage designs means editing this body (and its return
/// type) to match what that branch's `upsert_file` accepts:
///
/// - before `perf(lsp): share document text and line index`: `live.text().to_string()`
///   (an O(N) copy per keystroke, plus a `LineIndex` rebuilt per request)
/// - now: `live.text_arc()` (a refcount bump)
fn handoff(live: &TextBuffer) -> Arc<str> {
    live.text_arc()
}

/// How many blocks a row is measured in, and takes the median of.
///
/// Odd, so the median is a measured block rather than the mean of two. Matching
/// `benches/reparse.rs`: a single mean over one batch lets a scheduler hiccup
/// anywhere in the run move the number, and on the largest document a budget buys
/// only a couple of dozen iterations, so one bad iteration is a large share.
const BLOCKS: usize = 9;

/// The per-block iteration count for `f`, calibrated from a short probe.
///
/// Calibrated rather than fixed: the documents span three orders of magnitude in
/// size and the four rows another three in cost, so one hardcoded count would
/// either take minutes on the largest or measure noise on the cheapest. The clamp
/// is a sanity bound, not the budget — `block_budget / per_iter` is what actually
/// sizes the loop, so a cheap row fills its block rather than finishing early and
/// leaving its paired row to be timed at a different moment.
fn block_iters<T>(target: Duration, f: &mut impl FnMut() -> T) -> usize {
    let probe = 5;
    let start = Instant::now();
    for _ in 0..probe {
        black_box(f());
    }
    let per_iter = start.elapsed().as_nanos() as f64 / probe as f64;
    let block_budget = target.as_nanos() as f64 / BLOCKS as f64;
    if per_iter > 0.0 {
        ((block_budget / per_iter) as usize).clamp(3, 5_000_000)
    } else {
        5_000_000
    }
}

/// Warm `f` for one block's worth of iterations before anything is timed.
///
/// The probe is deliberately *not* reused as the warmup. On the largest document
/// the end-to-end row costs tens of milliseconds, so a block buys only a couple of
/// dozen iterations and the first few — cold allocator, cold caches — are a large
/// share of the total: a probe-warmed run once reported the reparse 16% high,
/// enough to invent a regression that a repeat run erased.
fn warm<T>(iters: usize, f: &mut impl FnMut() -> T) {
    for _ in 0..iters {
        black_box(f());
    }
}

/// One timed block, in nanoseconds per iteration.
fn block<T>(iters: usize, f: &mut impl FnMut() -> T) -> f64 {
    let start = Instant::now();
    for _ in 0..iters {
        black_box(f());
    }
    start.elapsed().as_nanos() as f64 / iters as f64
}

fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

/// Time one row, returning its median block in nanoseconds per iteration.
fn time<T>(target: Duration, mut f: impl FnMut() -> T) -> f64 {
    let iters = block_iters(target, &mut f);
    warm(iters, &mut f);
    median((0..BLOCKS).map(|_| block(iters, &mut f)).collect())
}

/// Time two rows **interleaved**, block by block, returning each row's median
/// block and the median of the per-block ratios.
///
/// Measured this way because the ratio is the assertion and the absolutes are not.
/// Timing the two rows to completion one after the other puts seconds between
/// them, so any load that drifts across that gap lands in the quotient and nowhere
/// else — which is what the write-phase ceiling's first calibration ran into (see
/// [`WRITE_MAX_COPIES`]), while `benches/reparse.rs`, whose ratios come from
/// adjacent measurements, holds under 1%. Alternating blocks makes drift
/// common-mode within each pair, and taking the median of the per-block *ratios*
/// (rather than the ratio of the two medians) keeps that pairing instead of
/// discarding it.
fn time_ratio<A, B>(
    target: Duration,
    mut numerator: impl FnMut() -> A,
    mut denominator: impl FnMut() -> B,
) -> (f64, f64, f64) {
    let n_iters = block_iters(target, &mut numerator);
    let d_iters = block_iters(target, &mut denominator);
    warm(n_iters, &mut numerator);
    warm(d_iters, &mut denominator);

    let mut numerators = Vec::with_capacity(BLOCKS);
    let mut denominators = Vec::with_capacity(BLOCKS);
    let mut ratios = Vec::with_capacity(BLOCKS);
    for _ in 0..BLOCKS {
        let n = block(n_iters, &mut numerator);
        let d = block(d_iters, &mut denominator);
        numerators.push(n);
        denominators.push(d);
        ratios.push(n / d.max(f64::EPSILON));
    }
    (median(numerators), median(denominators), median(ratios))
}

fn format_ns(ns: f64) -> String {
    if ns < 1_000.0 {
        format!("{ns:>9.0} ns")
    } else if ns < 1_000_000.0 {
        format!("{:>9.2} us", ns / 1_000.0)
    } else {
        format!("{:>9.3} ms", ns / 1_000_000.0)
    }
}

fn row(name: &str, ns: f64) {
    println!("  {name:<40}{}", format_ns(ns));
}

#[derive(Debug, Clone, Serialize)]
struct DocumentResult {
    name: String,
    /// Which keystroke this row measured ([`Site`]). A report that does not say is
    /// not comparable with another: the two sites reach different reparse tiers.
    site: &'static str,
    size_bytes: usize,
    line_count: usize,
    /// Row 0: one allocation and one linear copy of the document — the reference
    /// the write phase is read against, not a stage of the pipeline.
    text_copy_ns: f64,
    /// Row 1: a no-op upsert (the staleness guard alone).
    noop_upsert_ns: f64,
    /// Row 2: splice + upsert, no parse demanded.
    write_phase_ns: f64,
    /// Row 2 as a multiple of row 0 — the write-phase contract's number.
    ///
    /// Not `write_phase_ns / text_copy_ns`: the two rows are measured interleaved
    /// and this is the median of the *per-block* ratios, which is what keeps a
    /// drift between blocks common-mode instead of landing in the quotient.
    write_copies: f64,
    /// Row 3: row 2 plus the parse the keystroke triggers.
    end_to_end_ns: f64,
}

#[derive(Debug, Serialize)]
struct Report {
    schema_version: u32,
    documents: Vec<DocumentResult>,
}

fn bench_document(name: &str, text: &str, target: Duration, site: Site) -> DocumentResult {
    let (prepared, at) = prepare(text, site);
    let text = prepared.as_str();

    println!("\n{}", "=".repeat(64));
    println!(
        "{name}  ({} bytes, {} lines, {} site)",
        text.len(),
        text.lines().count(),
        site.name(),
    );
    println!("{}", "=".repeat(64));

    // The edit site: ~80% of the way through the buffer, so the splice copies a
    // realistic amount of tail, and *strictly inside a word*, so every row measures
    // the same workload — a character typed into prose.
    //
    // The "inside a word" part is load-bearing rather than cosmetic. A site that
    // lands on a `\` or a newline makes the incremental reparse refuse, so the row
    // measures a full parse; and since the rows alternate insert/delete, whether
    // the site still held the synthetic `z` when the next row started decided which
    // of the two it measured. That was a 45x swing between runs of the same binary.
    // Printed because the site decides which workload rows 2 and 3 measure, and a
    // silently-relocated one would look like a performance change.
    println!(
        "edit site: byte {at} ({:.0}% in) — …{}…",
        at as f64 / text.len() as f64 * 100.0,
        text[at.saturating_sub(24)..(at + 24).min(text.len())].replace('\n', "\\n"),
    );
    let mut live = Arc::new(TextBuffer::new(text, UTF16));
    let (line, character) = live.line_index().position(at);
    let position = Position::new(line, character);
    let typed = site.typed();
    let insert = vec![TextDocumentContentChangeEvent {
        range: Some(Range::new(position, position)),
        range_length: None,
        text: typed.to_owned(),
    }];
    let (typed_lines, typed_last) = {
        let mut lines = typed.split('\n');
        let first = lines.next().unwrap_or_default();
        let mut count = 0u32;
        let mut last = first.chars().count() as u32;
        for line in lines {
            count += 1;
            last = line.chars().count() as u32;
        }
        (count, last)
    };
    let after_typed = if typed_lines == 0 {
        Position::new(position.line, position.character + typed_last)
    } else {
        Position::new(position.line + typed_lines, typed_last)
    };
    let delete = vec![TextDocumentContentChangeEvent {
        range: Some(Range::new(position, after_typed)),
        range_length: None,
        text: String::new(),
    }];

    let mut db = IncrementalDatabase::default();
    let path = PathBuf::from("/bench/keystroke.tex");
    let file = db.upsert_file(&path, handoff(&live));
    black_box(db.parsed_tree(file));

    let noop = time(target, || black_box(db.upsert_file(&path, handoff(&live))));

    // Rows 0 and 2, measured interleaved because their *ratio* is the contract.
    //
    // Row 0 is a *reference*, not a stage of the pipeline: one allocation and one
    // linear copy of the document, which is the irreducible per-keystroke cost of
    // handing salsa an owned text. The write phase is read as a multiple of it.
    //
    // This is what makes the write-phase contract machine-independent without
    // being blind. Scaling across document sizes cannot see a regression that is
    // proportional — Phase 2 added 125 us to the write phase on the thesis and 17
    // us on the masters, which leaves the scaling ratio almost exactly where it
    // was — so a gate built only on scaling would have missed the one regression
    // this row exists to catch. A ratio against a raw copy moves with it.
    //
    // Row 2 alternates an insert and a delete so every iteration is a genuine text
    // change: a fresh salsa revision, never a memoized no-op.
    let mut flip = false;
    let (write, copy, copies) = time_ratio(
        target,
        || {
            flip = !flip;
            let batch = if flip { insert.clone() } else { delete.clone() };
            let edits = apply_content_changes(&mut live, batch);
            let file = db.upsert_file(&path, handoff(&live));
            // The language server's write phase is splice + upsert + stage, so the
            // row has to carry the stage too — it is what a Phase 3 tier reads, and
            // its cost (one `Vec<Edit>` and one lock) is paid per keystroke either
            // way.
            db.reparse_stage_edits(file, edits);
            file
        },
        || black_box(Arc::<str>::from(text)),
    );

    row("text copy (reference)", copy);
    row("upsert, text unchanged", noop);
    row("splice + upsert (write phase)", write);
    println!("  {:<40}{copies:>9.2} copies", "  as document copies");

    // Rewind to the original text before row 3, so it measures the same pair of
    // states row 2 did rather than whichever one row 2's iteration count left
    // behind. The count is calibrated, so its parity is a property of the machine —
    // and with an alternating edit, parity decides which text each row starts from.
    // Row 2 also demanded no parse, so the cached tree is many revisions stale;
    // the resync makes row 3's first iteration an ordinary keystroke.
    live = Arc::new(TextBuffer::new(text, UTF16));
    let file = db.upsert_file(&path, handoff(&live));
    db.reparse_stage_edits(file, None);
    black_box(db.parsed_tree(file));

    let mut flip = false;
    let end_to_end = time(target, || {
        flip = !flip;
        let batch = if flip { insert.clone() } else { delete.clone() };
        let edits = apply_content_changes(&mut live, batch);
        let file = db.upsert_file(&path, handoff(&live));
        db.reparse_stage_edits(file, edits);
        black_box(db.parsed_tree(file))
    });
    row("keystroke end-to-end (parse included)", end_to_end);

    DocumentResult {
        name: name.to_owned(),
        site: site.name(),
        size_bytes: text.len(),
        line_count: text.lines().count(),
        text_copy_ns: copy,
        noop_upsert_ns: noop,
        write_phase_ns: write,
        write_copies: copies,
        end_to_end_ns: end_to_end,
    }
}

/// Ratio checks are waived below this.
///
/// A ratio on a sub-microsecond baseline measures noise: on `small.tex` the
/// write phase is under two microseconds, so a hundred nanoseconds of anything
/// reads as a 5% regression.
const MIN_ABSOLUTE_US: f64 = 2.0;

/// The pair every scaling ratio is taken between, an order of magnitude apart.
const SCALE_FROM: &str = "masters_dissertation.tex";
const SCALE_TO: &str = "phd_dissertation.tex";

/// `phd_dissertation.tex` over `masters_dissertation.tex`, from the pinned sizes
/// [`sites::DOCUMENTS`] asserts. Named because [`NOOP_MAX_SCALING`] is stated as a
/// multiple of it rather than as a bare number — a ceiling that does not say what
/// shape it expects cannot be read later.
const SCALE_BYTES: f64 = 730369.0 / 95383.0;

/// Row 1 is the staleness guard alone, which compares an `Arc` pointer and
/// returns. It must stay **flat** in the document size — that is the row's whole
/// claim, and the README has asserted it in prose since before there was a gate.
/// The allowance is for cache effects on the larger buffer, not for growth.
const NOOP_MAX_SCALING: f64 = 2.5;

/// Row 2 splices bytes and hands salsa an `Arc`, so *one linear pass per byte* is
/// the shape to hold it to. What that is not is a fixed multiple of the byte
/// count: a single pass is not linear in *time* at these sizes, and row 0 — one
/// allocation and one memcpy — measures exactly how far off it is. Row 0 scales
/// **8.6-9.3x** over a 7.7x byte ratio in every run recorded here, the baseline
/// included, because the masters fits in L2 and the thesis does not.
///
/// So the ceiling is stated over row 0's own scaling rather than over the bytes.
/// Over the bytes it charged this row for the machine's cache hierarchy — slack
/// while a per-keystroke table rescan dominated, and the binding constraint the
/// moment that went: `SCALE_BYTES * 1.33` is 10.18x, against 10.3-11.6x measured
/// over four runs, tight to 4%, with no regression behind it.
///
/// Row 2 over row 0 *is* [`DocumentResult::write_copies`], so this reads two
/// copies figures — which also keeps it on the interleaved estimator instead of a
/// quotient of medians timed seconds apart. Measured 1.18-1.28; the ceiling sits
/// ~25% over the highest.
///
/// What it catches is work worse than one pass per byte. What it does not catch,
/// and is not asked to, is a *rebuilt* table: that raises the absolute copies on
/// both documents and barely moves this ratio — the pre-reshape baseline read
/// **0.93** here, i.e. below 1. [`WRITE_MAX_COPIES`] is the guard for that.
const WRITE_MAX_COPIES_SCALING: f64 = 1.6;

/// How many copies of the document the write phase may cost.
///
/// This is the row Phase 5 exists to cover: with both leaf tiers landed the parse
/// is cheap, so the write phase is most of what a user waits for, and the 125 us
/// Phase 2 measured into it stopped being invisible.
///
/// Stated against row 0 rather than against the end-to-end keystroke. The obvious
/// contract — "the write phase is at most N% of row 3" — is unsound here, and
/// unsound for exactly the reason the derived reparse row was deleted: rows 2 and
/// 3 are separately calibrated loops whose *difference* is smaller than the noise
/// on either, so on the thesis row 2 measures above row 3 and the share comes out
/// over 100%. A ratio is only worth asserting between two numbers that are
/// actually nested, and these are not.
///
/// Read from the *interleaved* measurement ([`time_ratio`]): the median of the
/// per-block ratios, not the quotient of two rows timed seconds apart. That is
/// what makes the number below tight enough to be a guard. The first calibration
/// took the two rows to completion one after the other and saw 49.8-51.7 copies
/// over four runs and then 63.6-64.4 over two more on one machine, so the ceiling
/// shipped at 72 — a doubling catcher, not the ~20% watch this row is for.
/// Interleaved, eight runs of one binary held **51.1-53.1** on the thesis and
/// **54.6-55.9** on the masters; the ceilings sit ~10% over the highest of those.
///
/// **Still calibrate on an idle machine.** Interleaving fixes the *variance*, not
/// the *level*: under twenty spinning threads the same binary read 76.7-76.9 and
/// 83.8-84.1, each cluster tight to under 1% but half again as high, because
/// contention costs the branchy write phase far more than it costs a memcpy. So a
/// loaded run reproduces itself and still fails the gate, which is the honest
/// behaviour — it is the same directive the reparse floors carry.
///
/// The exclusions are the point rather than an oversight, and the reason is now a
/// different one. `small.tex` and `cv.tex` did tighten (12-17% run to run before,
/// 4% and 8% after), but at those sizes the write phase is dominated by *fixed*
/// costs rather than by the linear passes the reference measures, so the ratio is
/// not reading the thing this row exists to watch: an extra small allocation in
/// `upsert_file` would fire it. They are still measured and printed. The
/// prediction this doc used to carry — that a `LineIndex` fix "would barely move
/// cv" — was wrong, and measurably: cv went 76.9x to 6.9x, more in *ratio* than
/// either gated document. Fixed costs dominating is a reason the ratio is noisy
/// there, not a reason it is insensitive.
///
/// **These are now a floor plus change rather than a ceiling over a known-bad
/// number.** The measured 51.8x was the whole `LineIndex` being rescanned per
/// keystroke; with the table patched instead
/// ([`TextBuffer::with_replacement`](badness::text::TextBuffer::with_replacement))
/// it reads **2.45-2.58** on the thesis and **2.61-2.88** on the masters over
/// five runs of each site, and the ceilings sit ~10% over the highest — the same
/// convention as before, over numbers an order of magnitude smaller. Two of those
/// copies are the text rebuild, which is why there is no room left to ratchet and
/// no [`WRITE_MIN_COPIES`] to go with them: the `Arc<String>` step that would go
/// below 2 is a real option, so a floor here would fail the improvement.
///
/// What this catches, and the scaling check no longer can, is a *returning
/// rescan*: it lands both documents back above 10 (post-reshape) or above 50
/// (before it), while barely moving a ratio between them.
const WRITE_MAX_COPIES: [(&str, f64); 2] = [
    ("masters_dissertation.tex", 3.2),
    ("phd_dissertation.tex", 2.9),
];

fn row_us(
    documents: &[DocumentResult],
    name: &str,
    pick: fn(&DocumentResult) -> f64,
) -> Option<f64> {
    documents
        .iter()
        .find(|d| d.name == name)
        .map(|d| pick(d) / 1_000.0)
}

/// How a contract came out.
///
/// `Waived` is kept apart from `Ok` on purpose. A ratio waived for sitting under
/// [`MIN_ABSOLUTE_US`] measured *nothing*, and printing it as `ok` reads as
/// coverage the run does not have — which is the failure mode this whole harness
/// is built against, since a guard that has narrowed to nothing leaves every
/// assertion above it vacuously green.
#[derive(PartialEq, Eq)]
enum Verdict {
    Ok,
    Waived,
    Fail,
}

impl Verdict {
    /// `held` is the contract; `waived` is the escape that means it was never
    /// really asked.
    fn of(held: bool, waived: bool) -> Self {
        match (held, waived) {
            (true, _) => Verdict::Ok,
            (false, true) => Verdict::Waived,
            (false, false) => Verdict::Fail,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Verdict::Ok => "ok    ",
            Verdict::Waived => "waived",
            Verdict::Fail => "FAIL  ",
        }
    }
}

/// Check the measured rows against their contracts, printing every check with its
/// margin so drift is visible well before it fails.
///
/// Returns every failure rather than stopping at the first, so one run says
/// everything that moved.
fn check_expectations(documents: &[DocumentResult]) -> Vec<String> {
    let mut checks: Vec<(Verdict, String)> = Vec::new();

    println!("\nThresholds");
    println!("{}", "=".repeat(64));

    let scaling = |pick: fn(&DocumentResult) -> f64| {
        row_us(documents, SCALE_FROM, pick).zip(row_us(documents, SCALE_TO, pick))
    };

    if let Some((from, to)) = scaling(|d| d.noop_upsert_ns) {
        let ratio = to / from.max(f64::EPSILON);
        checks.push((
            Verdict::of(ratio <= NOOP_MAX_SCALING, to <= MIN_ABSOLUTE_US),
            format!(
                "noop upsert: {SCALE_FROM} -> {SCALE_TO} scaling {ratio:.2}x <= \
                 {NOOP_MAX_SCALING:.2}x ({SCALE_BYTES:.1}x the bytes) or {to:.1} us <= \
                 {MIN_ABSOLUTE_US:.0} us"
            ),
        ));
    }

    let copies_of = |name: &str| {
        documents
            .iter()
            .find(|d| d.name == name)
            .map(|d| d.write_copies)
    };
    if let (Some(from), Some(to), Some(to_us)) = (
        copies_of(SCALE_FROM),
        copies_of(SCALE_TO),
        row_us(documents, SCALE_TO, |d| d.write_phase_ns),
    ) {
        let ratio = to / from.max(f64::EPSILON);
        checks.push((
            Verdict::of(ratio <= WRITE_MAX_COPIES_SCALING, to_us <= MIN_ABSOLUTE_US),
            format!(
                "write phase: {SCALE_FROM} -> {SCALE_TO} costs {ratio:.2}x the document \
                 copies ({from:.2} -> {to:.2}) <= {WRITE_MAX_COPIES_SCALING:.2}x \
                 or {to_us:.1} us <= {MIN_ABSOLUTE_US:.0} us"
            ),
        ));
    }

    let mut copies_waived = 0usize;
    for (name, max) in WRITE_MAX_COPIES {
        let (Some(write), Some(copies)) = (
            row_us(documents, name, |d| d.write_phase_ns),
            documents
                .iter()
                .find(|d| d.name == name)
                .map(|d| d.write_copies),
        ) else {
            checks.push((
                Verdict::Fail,
                format!("{name}: declared a ceiling but never ran"),
            ));
            continue;
        };
        let verdict = Verdict::of(copies <= max, write <= MIN_ABSOLUTE_US);
        if verdict == Verdict::Waived {
            copies_waived += 1;
        }
        checks.push((
            verdict,
            format!(
                "{name}: write phase is {copies:.2} document copies <= {max:.2} \
                 or {write:.1} us <= {MIN_ABSOLUTE_US:.0} us"
            ),
        ));
    }
    // The write phase is what this gate is *for*, so it may not pass by having
    // measured nothing. It is closer to that than it looks: the masters row is now
    // 3.2 us against a 2 us waiver, so one more improvement retires the check. The
    // move then is a fifth document between the masters and the thesis
    // (`sites::DOCUMENTS`, which means pinning it in `download.sh` and asserting
    // its size) — not widening the waiver, and not dropping the row.
    if copies_waived == WRITE_MAX_COPIES.len() {
        checks.push((
            Verdict::Fail,
            format!(
                "every write-phase copies ceiling was waived under \
                 {MIN_ABSOLUTE_US:.0} us: this run checked nothing"
            ),
        ));
    }

    let mut failures = Vec::new();
    for (verdict, description) in checks {
        println!("  {} {description}", verdict.label());
        if verdict == Verdict::Fail {
            failures.push(description);
        }
    }
    failures
}

fn main() {
    println!("badness keystroke pipeline bench (didChange -> upsert -> parse)");

    let target = Duration::from_millis(
        env::var("BADNESS_BENCH_TARGET_MS")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .unwrap_or(500),
    );

    // Off by default, so a run that only wants the numbers stays a measurement and
    // never fails the shell it was typed into.
    let assert_mode = matches!(
        env::var("BADNESS_BENCH_ASSERT").as_deref(),
        Ok("1") | Ok("true")
    );

    // The same size gradient the formatter bench uses: small.tex is committed
    // (zero-network), the rest come from benches/documents/download.sh and are
    // skipped with a note when absent.
    let site = Site::from_env();

    let single = env::var("BADNESS_BENCH_DOC").ok();
    if assert_mode && single.is_some() {
        eprintln!(
            "BADNESS_BENCH_ASSERT=1 cannot run with BADNESS_BENCH_DOC: every contract is a \
             ratio between two documents from the same run."
        );
        std::process::exit(1);
    }

    if assert_mode {
        let missing = check_corpus();
        if !missing.is_empty() {
            eprintln!("BADNESS_BENCH_ASSERT=1 needs the pinned corpus:");
            for entry in &missing {
                eprintln!("  {entry}");
            }
            eprintln!("Run `task bench:download`.");
            std::process::exit(1);
        }
    }

    let names: Vec<String> = match single {
        Some(doc) => vec![doc],
        None => DOCUMENTS.iter().map(|d| d.name.to_owned()).collect(),
    };

    let mut documents = Vec::new();
    let mut failures = Vec::new();
    for name in &names {
        match load_document(name) {
            Some(text) => {
                // Before timing: the site has to be where it claims. A relocated site
                // measures a different workload at the same name, which is how this
                // bench once reported a 45x swing between runs of one binary.
                if assert_mode {
                    let (prepared, at) = prepare(&text, site);
                    if let Some(problem) = check_site_pin(name, &prepared, at, site) {
                        failures.push(problem);
                    }
                }
                documents.push(bench_document(name, &text, target, site));
            }
            None => println!("\n{name}: not found — run `task bench:download`, skipping"),
        }
    }

    // Written before the verdict: a failing gate is exactly when the numbers are
    // worth keeping.
    if let Ok(path) = env::var("BADNESS_BENCH_OUTPUT_JSON") {
        let report = Report {
            schema_version: 4,
            documents: documents.clone(),
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
        failures.extend(check_expectations(&documents));
        if !failures.is_empty() {
            eprintln!("\n{} contract violation(s):", failures.len());
            for failure in &failures {
                eprintln!("  {failure}");
            }
            std::process::exit(1);
        }
        println!("\nall contracts held");
    }
}
