//! In-process formatter micro-benchmarks: parse vs format vs full pipeline.
//!
//! Unlike `benches/compare_format.sh` (a wall-clock CLI comparison against
//! tex-fmt/latexindent that *includes* process startup), this bench runs the
//! library entry points in-process, so it measures the **per-byte** cost with
//! no startup floor (binary load, allocator warmup, salsa/db setup). It is the
//! tool for the TODO's "separate startup floor from per-byte cost" attribution.
//!
//! It splits the pipeline three ways, like the sibling panache's
//! `benches/formatting.rs`:
//!   - **parse**: [`parse_with_flavor`] only (CST construction).
//!   - **format** (CST pre-built): [`format_node`] — lower CST → `Doc` IR →
//!     print, the reparse-free entry the LSP uses.
//!   - **full**: [`format_with_style_flavored`] — parse + lower + print, what a
//!     `badness format` invocation does per byte.
//!
//! `harness = false`: a plain `main` with fixed iteration counts (not criterion)
//! so a flamegraph attaches cleanly to a single hot document. Profile one doc:
//!
//! ```bash
//! BADNESS_BENCH_DOC=masters_dissertation.tex BADNESS_BENCH_ITERATIONS=20 \
//!     cargo flamegraph --bench formatting
//! ```
//!
//! Env knobs:
//!   - `BADNESS_BENCH_DOC` — profile only this doc under `benches/documents/`.
//!   - `BADNESS_BENCH_ITERATIONS` — iteration count for the selected doc (10).
//!   - `BADNESS_BENCH_OUTPUT_JSON` — write a machine-readable report to this path.

use badness::formatter::{FormatStyle, format_node, format_with_style_flavored};
use badness::parser::{LatexFlavor, parse_with_flavor};
use serde::Serialize;
use std::env;
use std::fs;
use std::hint::black_box;
use std::path::Path;
use std::time::{Duration, Instant};

/// Format flavor/style the CLI uses for a `.tex` document (`format --stdin`):
/// the [`Document`](LatexFlavor::Document) flavor, default style (Reflow wrap).
fn style() -> FormatStyle {
    FormatStyle::default()
}

fn bench_parse(input: &str, iterations: usize) -> Duration {
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(parse_with_flavor(black_box(input), LatexFlavor::Document));
    }
    start.elapsed()
}

fn bench_full(input: &str, iterations: usize) -> Duration {
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = black_box(format_with_style_flavored(
            black_box(input),
            style(),
            LatexFlavor::Document,
        ));
    }
    start.elapsed()
}

/// Format from a pre-built CST: lower → `Doc` IR → print, no parse. Mirrors the
/// LSP's reparse-free path ([`format_node`] over the salsa-cached tree).
fn bench_format_only(input: &str, iterations: usize) -> Duration {
    let tree = parse_with_flavor(input, LatexFlavor::Document).syntax();
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = black_box(format_node(black_box(&tree), style()));
    }
    start.elapsed()
}

#[derive(Debug, Serialize)]
struct BenchmarkResult {
    id: String,
    name: String,
    size_bytes: usize,
    line_count: usize,
    iterations: usize,
    full_avg_us: f64,
    parse_avg_us: f64,
    format_avg_us: f64,
    /// Parse's share of the full pipeline, percent.
    parse_pct: f64,
    /// Lower+print's share of the full pipeline, percent.
    format_pct: f64,
    throughput_mb_s: f64,
}

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    schema_version: u32,
    results: Vec<BenchmarkResult>,
}

fn run_benchmark(name: &str, input: &str, iterations: usize) -> Option<BenchmarkResult> {
    println!("\n{}", "=".repeat(64));
    println!("Benchmark: {name}");
    println!("{}", "=".repeat(64));
    println!(
        "Document size: {} bytes, {} lines",
        input.len(),
        input.lines().count()
    );

    // Sanity gate (mirrors compare_format.sh): only documents that format
    // cleanly are benched — the formatter refuses input with parser diagnostics.
    match format_with_style_flavored(input, style(), LatexFlavor::Document) {
        Ok(_) => {}
        Err(e) => {
            println!("⚠️  skipped — badness cannot format it yet ({e})");
            return None;
        }
    }

    // Warmup (allocator, branch predictor) before timing.
    for _ in 0..5 {
        let _ = format_with_style_flavored(input, style(), LatexFlavor::Document);
    }

    let full_avg = bench_full(input, iterations).as_nanos() as f64 / iterations as f64 / 1000.0;
    let parse_avg = bench_parse(input, iterations).as_nanos() as f64 / iterations as f64 / 1000.0;
    let format_avg =
        bench_format_only(input, iterations).as_nanos() as f64 / iterations as f64 / 1000.0;

    let parse_pct = if full_avg > 0.0 {
        parse_avg / full_avg * 100.0
    } else {
        0.0
    };
    let format_pct = if full_avg > 0.0 {
        format_avg / full_avg * 100.0
    } else {
        0.0
    };
    // MB/s = bytes / seconds / 1e6; full_avg is microseconds.
    let throughput_mb_s = if full_avg > 0.0 {
        input.len() as f64 / (full_avg / 1_000_000.0) / 1_000_000.0
    } else {
        0.0
    };

    println!("\n  full (parse+lower+print): {full_avg:>10.2} µs");
    println!("  parse only:               {parse_avg:>10.2} µs  ({parse_pct:.0}% of full)");
    println!("  format only (CST built):  {format_avg:>10.2} µs  ({format_pct:.0}% of full)");
    println!("  throughput (full):        {throughput_mb_s:>10.2} MB/s");

    Some(BenchmarkResult {
        id: name.to_owned(),
        name: name.to_owned(),
        size_bytes: input.len(),
        line_count: input.lines().count(),
        iterations,
        full_avg_us: full_avg,
        parse_avg_us: parse_avg,
        format_avg_us: format_avg,
        parse_pct,
        format_pct,
        throughput_mb_s,
    })
}

fn load_document(name: &str) -> Option<String> {
    fs::read_to_string(Path::new("benches/documents").join(name)).ok()
}

/// Measure the one-time signature-DB init. The builtin `data/signatures.json` is
/// a `LazyLock` parsed once per process — a fixed *startup-floor* component the
/// CLI pays once and the timed per-byte loops below never see (warmup triggers it
/// first). The CWL tier is now a build-time `phf` map baked into the binary, so
/// its "init" is ~0 (was a ~4.5 ms gz-decompress+JSON-parse `LazyLock`); the line
/// is kept as a regression guard. Must run before any other DB access, or the
/// builtin number is a warm hit (~0).
fn report_signature_db_init() {
    let start = Instant::now();
    black_box(badness::semantic::signature::builtin());
    let builtin = start.elapsed();
    let start = Instant::now();
    black_box(badness::semantic::signature::cwl());
    let cwl = start.elapsed();
    println!(
        "\none-time signature-DB init (startup floor, paid once per process):\n  \
         builtin (signatures.json):  {:>8.2} µs\n  \
         cwl (static phf map, ~0):   {:>8.2} µs",
        builtin.as_nanos() as f64 / 1000.0,
        cwl.as_nanos() as f64 / 1000.0,
    );
}

fn main() {
    println!("badness formatter micro-benchmarks (in-process, no startup floor)");
    report_signature_db_init();

    let mut results = Vec::new();

    // Single-doc profiling mode: profile exactly the named document, e.g. under
    // `cargo flamegraph --bench formatting`.
    if let Ok(doc_name) = env::var("BADNESS_BENCH_DOC") {
        let iterations = env::var("BADNESS_BENCH_ITERATIONS")
            .ok()
            .and_then(|raw| raw.parse::<usize>().ok())
            .unwrap_or(10);
        let doc = load_document(&doc_name).unwrap_or_else(|| {
            panic!("BADNESS_BENCH_DOC '{doc_name}' not found under benches/documents/")
        });
        results.extend(run_benchmark(&doc_name, &doc, iterations));
        maybe_write_json_report(results);
        return;
    }

    // Default corpus: a size gradient. small.tex is committed (zero-network);
    // the larger docs are fetched by benches/documents/download.sh (gitignored)
    // and skipped with a note when absent.
    let small = load_document("small.tex").expect("small.tex not found — it should be committed");
    results.extend(run_benchmark("small.tex (baseline)", &small, 2000));

    for (name, iters) in [
        ("cv.tex", 500),
        ("masters_dissertation.tex", 50),
        ("phd_dissertation.tex", 10),
    ] {
        match load_document(name) {
            Some(doc) => results.extend(run_benchmark(name, &doc, iters)),
            None => println!("\n⚠️  skipping {name} — run benches/documents/download.sh"),
        }
    }

    println!("\n{}", "=".repeat(64));
    println!("Done.");
    maybe_write_json_report(results);
}

fn maybe_write_json_report(results: Vec<BenchmarkResult>) {
    let Ok(path) = env::var("BADNESS_BENCH_OUTPUT_JSON") else {
        return;
    };
    let report = BenchmarkReport {
        schema_version: 1,
        results,
    };
    let json = serde_json::to_string_pretty(&report).expect("serialize benchmark report");
    fs::write(&path, json).unwrap_or_else(|e| panic!("write benchmark report to '{path}': {e}"));
    println!("JSON report written to: {path}");
}
