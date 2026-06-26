//! Preprocessor for the docs, plus shared post-build helpers (see [`postbuild`]).

pub mod postbuild;

use mdbook_preprocessor::book::Book;
use mdbook_preprocessor::errors::Result;
use mdbook_preprocessor::{Preprocessor, PreprocessorContext};
use semver::{Version, VersionReq};
use serde::Deserialize;
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

/// One `###` section with a results table per benchmarked document, in corpus
/// order; rows follow the order tools appear in `results`. `badness` is the
/// baseline and every other tool's `Relative` column is its mean ratio to it.
fn render_results(b: &Benchmarks) -> String {
    let mut out = String::new();
    for doc in &b.documents {
        let base = b
            .results
            .iter()
            .find(|r| r.document == doc.id && r.formatter == "badness")
            .map(|r| r.mean_ms);

        out.push_str(&format!(
            "### {} ({} bytes, {} lines)\n\n",
            doc.name, doc.size_bytes, doc.lines
        ));
        out.push_str("| Tool | Mean (ms) | Min (ms) | Max (ms) | Relative |\n");
        out.push_str("| --- | ---: | ---: | ---: | --- |\n");
        for r in b.results.iter().filter(|r| r.document == doc.id) {
            let relative = if r.formatter == "badness" {
                "baseline".to_string()
            } else {
                relative_cell(r.mean_ms, base)
            };
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                r.formatter,
                fmt_ms(Some(r.mean_ms)),
                fmt_ms(r.min_ms),
                fmt_ms(r.max_ms),
                relative
            ));
        }
        out.push('\n');
    }
    out
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
