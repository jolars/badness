//! Preprocessor for the docs, plus shared post-build helpers (see [`postbuild`]).

pub mod postbuild;

use mdbook_preprocessor::book::Book;
use mdbook_preprocessor::errors::Result;
use mdbook_preprocessor::{Preprocessor, PreprocessorContext};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;

/// Preprocessing entry point.
pub fn handle_preprocessing() -> Result<()> {
    let pre = GuideHelper;
    let (ctx, book) = mdbook_preprocessor::parse_input(io::stdin())?;

    let book_version = Version::parse(&ctx.mdbook_version)?;
    let version_req = VersionReq::parse(mdbook_preprocessor::MDBOOK_VERSION)?;

    if !version_req.matches(&book_version) {
        eprintln!(
            "warning: The {} plugin was built against version {} of mdbook, \
             but we're being called from version {}",
            pre.name(),
            mdbook_preprocessor::MDBOOK_VERSION,
            ctx.mdbook_version
        );
    }

    let processed_book = pre.run(&ctx, book)?;
    serde_json::to_writer(io::stdout(), &processed_book)?;

    Ok(())
}

struct GuideHelper;

impl Preprocessor for GuideHelper {
    fn name(&self) -> &str {
        "doc-utils"
    }

    fn run(&self, _ctx: &PreprocessorContext, mut book: Book) -> Result<Book> {
        insert_version(&mut book);
        insert_benchmarks(&mut book);
        insert_changelog(&mut book);
        Ok(book)
    }
}

/// The project root, one level up from the book root (`docs/`), which is the
/// working directory mdbook runs preprocessors in.
fn project_root() -> PathBuf {
    std::env::current_dir()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Substitute the `{{ badness-version }}` marker with the crate version read from
/// the project's root `Cargo.toml`. mdbook runs preprocessors with the book root
/// (`docs/`) as the working directory, so the project manifest is one level up.
fn insert_version(book: &mut Book) {
    let path = project_root().join("Cargo.toml");
    let manifest_contents = std::fs::read_to_string(&path).unwrap();
    let manifest: toml::Value = toml::from_str(&manifest_contents).unwrap();
    let version = manifest["package"]["version"].as_str().unwrap();
    const MARKER: &str = "{{ badness-version }}";
    book.for_each_chapter_mut(|ch| {
        if ch.content.contains(MARKER) {
            ch.content = ch.content.replace(MARKER, version);
        }
    });
}

/// Substitute the `{{ changelog }}` marker with the body of the project's root
/// `CHANGELOG.md`, so the docs changelog page is a build-time copy of the canonical
/// (release-tooling-generated) changelog and never drifts. The file's leading
/// `# Changelog` heading is stripped because the docs page supplies its own.
fn insert_changelog(book: &mut Book) {
    const MARKER: &str = "{{ changelog }}";
    let needs_render = {
        let mut found = false;
        book.for_each_chapter_mut(|ch| {
            if ch.content.contains(MARKER) {
                found = true;
            }
        });
        found
    };
    if !needs_render {
        return;
    }

    let path = project_root().join("CHANGELOG.md");
    let body = match std::fs::read_to_string(&path) {
        Ok(s) => strip_changelog_heading(&s).to_string(),
        Err(_) => format!(
            "_Changelog unavailable (`{}` missing or unreadable)._",
            path.display()
        ),
    };

    book.for_each_chapter_mut(|ch| {
        if ch.content.contains(MARKER) {
            ch.content = ch.content.replace(MARKER, &body);
        }
    });
}

/// Drop a leading top-level `# Changelog` heading (and the blank lines after it)
/// so the inlined body slots under the docs page's own title. Anything else is
/// returned untouched.
fn strip_changelog_heading(contents: &str) -> &str {
    match contents.strip_prefix("# Changelog") {
        Some(rest) => rest.trim_start_matches(['\n', '\r']),
        None => contents,
    }
}

// --- Benchmarks --------------------------------------------------------------

const BENCH_META_MARKER: &str = "{{ benchmark-meta }}";
const BENCH_RESULTS_MARKER: &str = "{{ benchmark-results }}";

/// The committed benchmark artifact, deserialized straight from
/// `benches/benchmark_results.json`. See `benches/compare_format.sh` for the
/// producer and the schema.
#[derive(Deserialize)]
struct Benchmarks {
    meta: Meta,
    documents: Vec<Document>,
    results: Vec<BenchResult>,
}

#[derive(Deserialize)]
struct Meta {
    generated_at: String,
    host: Host,
    backend: String,
    min_runs: u32,
    tools: Tools,
}

#[derive(Deserialize)]
struct Host {
    os: String,
    arch: String,
    cpu: String,
}

#[derive(Deserialize)]
struct Tools {
    badness: Tool,
    #[serde(rename = "tex-fmt")]
    tex_fmt: Option<Tool>,
    latexindent: Option<Tool>,
}

#[derive(Deserialize)]
struct Tool {
    version: String,
}

#[derive(Deserialize)]
struct Document {
    id: String,
    name: String,
    size_bytes: u64,
    lines: u64,
}

#[derive(Deserialize)]
struct BenchResult {
    document: String,
    formatter: String,
    mean_ms: f64,
    stddev_ms: Option<f64>,
    min_ms: Option<f64>,
    max_ms: Option<f64>,
}

/// One dot in the results chart: a (document, formatter) timing, its ratio to
/// the badness baseline, and the numbers the tooltip shows. Serialized inline
/// into the page for `docs/theme/bench-charts.js` to plot with Vega-Lite.
#[derive(Serialize)]
struct ChartPoint {
    document: String,
    formatter: String,
    mean_ms: f64,
    ratio: f64,
    ratio_label: String,
    stddev_ms: Option<f64>,
    min_ms: Option<f64>,
    max_ms: Option<f64>,
}

/// Substitute the `{{ benchmark-meta }}` and `{{ benchmark-results }}` markers
/// with tables rendered from the committed `benches/benchmark_results.json`. The
/// JSON is read but never regenerated here, so the benchmark is only ever run
/// manually (via `task bench`), not at site-build time.
fn insert_benchmarks(book: &mut Book) {
    let needs_render = {
        let mut found = false;
        book.for_each_chapter_mut(|ch| {
            if ch.content.contains(BENCH_META_MARKER) || ch.content.contains(BENCH_RESULTS_MARKER) {
                found = true;
            }
        });
        found
    };
    if !needs_render {
        return;
    }

    let path = project_root().join("benches/benchmark_results.json");
    let (meta, results) = match std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Benchmarks>(&s).ok())
    {
        Some(b) => (render_meta(&b.meta), render_results(&b)),
        None => {
            let note = format!(
                "_Benchmark data unavailable (`{}` missing or unreadable; run `task bench`)._",
                path.display()
            );
            (note.clone(), note)
        }
    };

    book.for_each_chapter_mut(|ch| {
        if ch.content.contains(BENCH_META_MARKER) {
            ch.content = ch.content.replace(BENCH_META_MARKER, &meta);
        }
        if ch.content.contains(BENCH_RESULTS_MARKER) {
            ch.content = ch.content.replace(BENCH_RESULTS_MARKER, &results);
        }
    });
}

/// A Markdown bullet list of tool versions, timing backend, host, and run date.
fn render_meta(meta: &Meta) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "- **badness**: `{}`\n",
        meta.tools.badness.version
    ));
    match &meta.tools.tex_fmt {
        Some(t) => out.push_str(&format!("- **tex-fmt**: `{}`\n", t.version)),
        None => out.push_str("- **tex-fmt**: not measured (not installed)\n"),
    }
    match &meta.tools.latexindent {
        Some(t) => out.push_str(&format!("- **latexindent**: `{}`\n", t.version)),
        None => out.push_str("- **latexindent**: not measured (not installed)\n"),
    }
    out.push_str(&format!(
        "- **backend**: {} (min runs: {})\n",
        meta.backend, meta.min_runs
    ));
    out.push_str(&format!(
        "- **host**: {}/{}, {}\n",
        meta.host.os, meta.host.arch, meta.host.cpu
    ));
    out.push_str(&format!("- **generated**: {}\n", meta.generated_at));
    out
}

/// The results marker becomes an interactive dot plot (Vega-Lite, driven by
/// `docs/theme/bench-charts.js` and wired via `book.toml`'s `additional-js`)
/// plus a collapsed HTML table with the same numbers as a no-JS/print fallback.
///
/// The chart data rides inline in a `<script type="application/json">`; the JS
/// plots time-relative-to-badness on a log axis, one dot per (document,
/// formatter). Kept as raw HTML (not a Markdown pipe table) so the fallback
/// renders inside `<details>`.
fn render_results(b: &Benchmarks) -> String {
    let points = chart_points(b);
    let data_json = serde_json::to_string(&points).unwrap_or_else(|_| "[]".to_string());

    let mut out = String::new();
    out.push_str("<div class=\"bench-chart-block\">\n");
    // The chart and its caption form the <figure>; the caption must be the
    // figure's first or last child, so the no-JS/table fallback lives as a
    // sibling below it, not inside the figure.
    out.push_str("<figure class=\"bench-figure\">\n");
    out.push_str("<div class=\"bench-chart\"></div>\n");
    out.push_str("<script type=\"application/json\" class=\"bench-data\">");
    out.push_str(&data_json);
    out.push_str("</script>\n");
    out.push_str(
        "<figcaption>Formatting speed relative to <code>badness</code>. Each dot is one \
         document formatted by one tool; the vertical position is mean wall-clock time as a \
         ratio to <code>badness</code> on a log scale, so <code>badness</code> lies on the \
         dashed baseline at 1, faster tools fall below it and slower tools rise above. Color \
         distinguishes documents; hover a dot for the exact millisecond figures.</figcaption>\n",
    );
    out.push_str("</figure>\n");
    out.push_str(
        "<noscript>Enable JavaScript for the interactive chart; \
         the data table below has the same numbers.</noscript>\n",
    );
    out.push_str("<details class=\"bench-table\">\n<summary>Data table</summary>\n");
    out.push_str(&render_results_tables_html(b));
    out.push_str("</details>\n");
    out.push_str("</div>\n");
    out
}

/// One dot per (document, formatter): its mean time as a ratio to that
/// document's badness baseline (badness itself is `1.0`), in corpus order.
/// Documents whose baseline is missing or non-positive are skipped (they carry
/// no meaningful ratio); they still appear in the fallback table.
fn chart_points(b: &Benchmarks) -> Vec<ChartPoint> {
    let mut points = Vec::new();
    for doc in &b.documents {
        let base = b
            .results
            .iter()
            .find(|r| r.document == doc.id && r.formatter == "badness")
            .map(|r| r.mean_ms);
        let Some(base) = base.filter(|&b| b > 0.0) else {
            continue;
        };
        for r in b.results.iter().filter(|r| r.document == doc.id) {
            let ratio_label = if r.formatter == "badness" {
                "baseline".to_string()
            } else {
                relative_cell(r.mean_ms, Some(base))
            };
            points.push(ChartPoint {
                document: doc.name.clone(),
                formatter: r.formatter.clone(),
                mean_ms: r.mean_ms,
                ratio: r.mean_ms / base,
                ratio_label,
                stddev_ms: r.stddev_ms,
                min_ms: r.min_ms,
                max_ms: r.max_ms,
            });
        }
    }
    points
}

/// One `<h3>` + HTML `<table>` per benchmarked document, in corpus order; rows
/// follow the order tools appear in `results`. `badness` is the baseline and
/// every other tool's `Relative` cell is its mean ratio to it.
fn render_results_tables_html(b: &Benchmarks) -> String {
    let mut out = String::new();
    for doc in &b.documents {
        let base = b
            .results
            .iter()
            .find(|r| r.document == doc.id && r.formatter == "badness")
            .map(|r| r.mean_ms);

        out.push_str(&format!(
            "<h3>{} ({} bytes, {} lines)</h3>\n",
            esc(&doc.name),
            doc.size_bytes,
            doc.lines
        ));
        out.push_str(
            "<table>\n<thead><tr><th>Tool</th><th>Mean (ms)</th>\
             <th>Min (ms)</th><th>Max (ms)</th><th>Relative</th></tr></thead>\n<tbody>\n",
        );
        for r in b.results.iter().filter(|r| r.document == doc.id) {
            let relative = if r.formatter == "badness" {
                "baseline".to_string()
            } else {
                relative_cell(r.mean_ms, base)
            };
            out.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>\n",
                esc(&r.formatter),
                fmt_ms(Some(r.mean_ms)),
                fmt_ms(r.min_ms),
                fmt_ms(r.max_ms),
                esc(&relative),
            ));
        }
        out.push_str("</tbody>\n</table>\n");
    }
    out
}

/// Minimal HTML text escaping for the fallback table's cell text.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Format a millisecond figure to four decimals, or an em dash when absent
/// (the shell-loop fallback reports no min/max).
fn fmt_ms(v: Option<f64>) -> String {
    match v {
        Some(x) => format!("{x:.4}"),
        None => "—".to_string(),
    }
}

/// Human ratio of a tool's mean to the badness baseline.
fn relative_cell(tool_mean: f64, base: Option<f64>) -> String {
    match base {
        Some(b) if b > 0.0 && tool_mean > 0.0 => {
            let r = tool_mean / b;
            if r >= 1.0 {
                format!("{r:.1}× slower")
            } else {
                format!("{:.1}× faster", 1.0 / r)
            }
        }
        _ => "—".to_string(),
    }
}
