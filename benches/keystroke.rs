//! What one keystroke costs through the salsa pipeline, end to end.
//!
//! `benches/formatting.rs` times the per-byte formatter work and
//! `benches/compare_format.sh` times the CLI, but neither touches
//! [`IncrementalDatabase`] or the `didChange` splice — the composition a real
//! editor session runs on every character. `tests/scaling.rs` guards growth
//! ratios, not the pipeline. This bench is that missing row: the path from a
//! `didChange` notification to a parse tree, timed as one thing.
//!
//! Three rows per document:
//!
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
//!    Badness has no intra-file reparse yet (`AGENTS.md` decision #6: salsa
//!    first, intra-file later), so row 3 minus row 2 *is* the full-reparse cost,
//!    and it is the number a future incremental reparse has to beat.
//!
//! Each timed iteration alternates inserting and deleting one character, so the
//! text genuinely changes every round: salsa sees a fresh revision and the
//! printed number is one keystroke, never a memoized no-op.
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
//!   - `BADNESS_BENCH_SITE` — which keystroke to measure: `word` (the default) or
//!     `verbatim` (see [`Site`]).
//!   - `BADNESS_BENCH_TARGET_MS` — per-row timing budget (default 500 ms); each
//!     row auto-calibrates its iteration count to fill it.
//!   - `BADNESS_BENCH_OUTPUT_JSON` — write a machine-readable report to this
//!     path (the A/B diff between two builds).

use std::env;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use badness::incremental::{IncrementalDatabase, IncrementalDb};
use badness::lsp::apply_content_changes;
use badness::text::{PositionEncoding, TextBuffer};
use lsp_types::{Position, Range, TextDocumentContentChangeEvent};
use serde::Serialize;

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

/// Time `f` in a warm loop, returning nanoseconds per iteration.
///
/// The iteration count is calibrated rather than fixed: the documents span three
/// orders of magnitude in size and the three rows another three in cost, so one
/// hardcoded count would either take minutes on the largest or measure noise on
/// the cheapest.
///
/// The probe is deliberately *not* reused as the warmup. On the largest document
/// the end-to-end row costs tens of milliseconds, so the budget buys only a
/// couple of dozen iterations and the first few — cold allocator, cold caches —
/// are a large share of the total: a probe-warmed run reported the reparse up to
/// 16% high, enough to invent a regression that a repeat run erased. So the
/// calibrated count is warmed at a tenth of itself, and the floor is high enough
/// that the timed loop never averages over a handful of samples.
fn time<T>(target: Duration, mut f: impl FnMut() -> T) -> f64 {
    let probe = 5;
    let start = Instant::now();
    for _ in 0..probe {
        black_box(f());
    }
    let per_iter = start.elapsed().as_nanos() as f64 / probe as f64;
    let iters = if per_iter > 0.0 {
        ((target.as_nanos() as f64 / per_iter) as usize).clamp(20, 200_000)
    } else {
        200_000
    };

    for _ in 0..(iters / 10).max(1) {
        black_box(f());
    }
    let start = Instant::now();
    for _ in 0..iters {
        black_box(f());
    }
    start.elapsed().as_nanos() as f64 / iters as f64
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

#[derive(Debug, Serialize)]
struct DocumentResult {
    name: String,
    /// Which keystroke this row measured ([`Site`]). A report that does not say is
    /// not comparable with another: the two sites reach different reparse tiers.
    site: &'static str,
    size_bytes: usize,
    line_count: usize,
    /// Row 1: a no-op upsert (the staleness guard alone).
    noop_upsert_ns: f64,
    /// Row 2: splice + upsert, no parse demanded.
    write_phase_ns: f64,
    /// Row 3: row 2 plus the parse the keystroke triggers.
    end_to_end_ns: f64,
    /// Row 3 minus row 2: the full reparse, until an intra-file one lands.
    reparse_ns: f64,
}

#[derive(Debug, Serialize)]
struct Report {
    schema_version: u32,
    documents: Vec<DocumentResult>,
}

/// The first offset at or after `from` that sits strictly inside a word on a line
/// of plain prose — a place where inserting a letter extends one word and changes
/// nothing else about the document.
///
/// "Two adjacent letters" is not enough on its own: that is also the inside of
/// `\lesssim`, and at 80% of a real thesis that is exactly what it finds. So the
/// line has to be free of every character that makes it something other than
/// prose, which is what keeps the bench measuring the keystroke it claims to.
///
/// Falls back to `from` (snapped to a char boundary) when the tail holds no such
/// line, so a pathological document still benches — it just benches a different
/// keystroke, which the printed site lets you see.
fn word_interior_at_or_after(text: &str, from: usize) -> usize {
    const STRUCTURE: [char; 8] = ['\\', '{', '}', '$', '%', '&', '#', '~'];
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        if offset < from || line.trim().len() < 40 || line.contains(STRUCTURE) {
            continue;
        }
        let bytes = line.as_bytes();
        if let Some(i) = (1..line.len())
            .find(|&i| bytes[i - 1].is_ascii_alphabetic() && bytes[i].is_ascii_alphabetic())
        {
            return line_start + i;
        }
    }
    let mut at = from.min(text.len());
    while at < text.len() && !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

/// Which keystroke the bench measures.
///
/// A tier's number is only as trustworthy as its edit site — Phase 3 of the
/// incremental-reparse work found this bench silently timing a construct the token
/// tier declines, two runs of the same binary differing 45x. So the site is an
/// explicit choice, it is printed, and it rides the JSON report.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Site {
    /// A letter typed into a word of plain prose: the token tier.
    Word,
    /// A line typed inside an `lstlisting` body: the protected-body tier, and the
    /// one keystroke that carries a newline.
    Verbatim,
}

impl Site {
    fn from_env() -> Self {
        match env::var("BADNESS_BENCH_SITE").as_deref() {
            Ok("verbatim") => Self::Verbatim,
            Ok("word") | Err(_) => Self::Word,
            Ok(other) => panic!("BADNESS_BENCH_SITE={other:?}: expected `word` or `verbatim`"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Word => "word",
            Self::Verbatim => "verbatim",
        }
    }
}

/// One line of the synthetic listing, sized so the block is a few kilobytes — big
/// enough that relexing it is a real cost, small enough to stay a plausible listing.
const LISTING_LINE: &str = "    for (int i = 0; i < n; i++) { total += weights[i] * values[i]; }\n";
const LISTING_LINES: usize = 32;

/// The document the bench actually edits, and the byte offset it edits at.
///
/// No document under `benches/documents/` contains a verbatim environment, a
/// `\verb`, or a `\url`, so the protected-body site has to *inject* its construct.
/// That makes the `verbatim` numbers comparable across runs but not against the
/// `word` ones on a different document — which is why the site is printed and
/// recorded rather than inferred.
fn prepare(text: &str, site: Site) -> (String, usize) {
    match site {
        Site::Word => {
            let at = word_interior_at_or_after(text, text.len() * 4 / 5);
            (text.to_owned(), at)
        }
        Site::Verbatim => {
            let anchor = line_start_at_or_after(text, text.len() * 4 / 5);
            let body: String = LISTING_LINE.repeat(LISTING_LINES);
            let block =
                format!("\n\\begin{{lstlisting}}[language=C]\n{body}\\end{{lstlisting}}\n\n");
            // Halfway down the body, inside a code line rather than at its edge, so
            // the edit is unambiguously interior to the one `VERBATIM_BODY` leaf.
            let into_body = "\n\\begin{lstlisting}[language=C]\n".len()
                + LISTING_LINE.len() * (LISTING_LINES / 2)
                + LISTING_LINE.len() / 2;
            let mut out = String::with_capacity(text.len() + block.len());
            out.push_str(&text[..anchor]);
            out.push_str(&block);
            out.push_str(&text[anchor..]);
            (out, anchor + into_body)
        }
    }
}

/// The start of the first line at or after `from`.
fn line_start_at_or_after(text: &str, from: usize) -> usize {
    match text[from.min(text.len())..].find('\n') {
        Some(rel) => from + rel + 1,
        None => text.len(),
    }
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
    // The `word` site types one letter; the `verbatim` site types a whole line, since
    // a line terminator is the edit the protected-body tier exists to claim and the
    // token tier refuses outright.
    let typed = match site {
        Site::Word => "z",
        Site::Verbatim => "\n    total = 0;",
    };
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
    row("upsert, text unchanged", noop);

    // Alternate an insert and a delete so every iteration is a genuine text
    // change: a fresh salsa revision, never a memoized no-op.
    let mut flip = false;
    let write = time(target, || {
        flip = !flip;
        let batch = if flip { insert.clone() } else { delete.clone() };
        let edits = apply_content_changes(&mut live, batch);
        let file = db.upsert_file(&path, handoff(&live));
        // The language server's write phase is splice + upsert + stage, so the row
        // has to carry the stage too — it is what a Phase 3 tier reads, and its
        // cost (one `Vec<Edit>` and one lock) is paid per keystroke either way.
        db.reparse_stage_edits(file, edits);
        file
    });
    row("splice + upsert (write phase)", write);

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
    row(
        "  of which reparse (row 3 - row 2)",
        (end_to_end - write).max(0.0),
    );

    DocumentResult {
        name: name.to_owned(),
        site: site.name(),
        size_bytes: text.len(),
        line_count: text.lines().count(),
        noop_upsert_ns: noop,
        write_phase_ns: write,
        end_to_end_ns: end_to_end,
        reparse_ns: (end_to_end - write).max(0.0),
    }
}

fn load_document(name: &str) -> Option<String> {
    fs::read_to_string(Path::new("benches/documents").join(name)).ok()
}

fn main() {
    println!("badness keystroke pipeline bench (didChange -> upsert -> parse)");

    let target = Duration::from_millis(
        env::var("BADNESS_BENCH_TARGET_MS")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .unwrap_or(500),
    );

    // The same size gradient the formatter bench uses: small.tex is committed
    // (zero-network), the rest come from benches/documents/download.sh and are
    // skipped with a note when absent.
    let site = Site::from_env();

    let names: Vec<String> = match env::var("BADNESS_BENCH_DOC") {
        Ok(doc) => vec![doc],
        Err(_) => [
            "small.tex",
            "cv.tex",
            "masters_dissertation.tex",
            "phd_dissertation.tex",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect(),
    };

    let mut documents = Vec::new();
    for name in &names {
        match load_document(name) {
            Some(text) => documents.push(bench_document(name, &text, target, site)),
            None => println!("\n{name}: not found — run `task bench:download`, skipping"),
        }
    }

    if let Ok(path) = env::var("BADNESS_BENCH_OUTPUT_JSON") {
        let report = Report {
            schema_version: 2,
            documents,
        };
        match serde_json::to_string_pretty(&report) {
            Ok(json) => match fs::write(&path, json) {
                Ok(()) => println!("\nwrote {path}"),
                Err(e) => eprintln!("\ncould not write {path}: {e}"),
            },
            Err(e) => eprintln!("\ncould not serialize report: {e}"),
        }
    }
}
