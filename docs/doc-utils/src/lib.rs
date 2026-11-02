//! Generated Markdown fragments shared by the documentation and Markdown exports.

mod memory;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub type Fragments = BTreeMap<String, String>;

/// Read committed data without running benchmarks or changing source documents.
pub fn fragments(root: &Path) -> Result<Fragments, Box<dyn std::error::Error>> {
    let manifest: toml::Value = toml::from_str(&std::fs::read_to_string(root.join("Cargo.toml"))?)?;
    let version = manifest["package"]["version"]
        .as_str()
        .ok_or("missing package version")?;
    let mut fragments = Fragments::from([
        ("{{ badness-version }}".into(), version.into()),
        (
            "{{ changelog }}".into(),
            changelog_body(&std::fs::read_to_string(root.join("CHANGELOG.md"))?),
        ),
    ]);
    insert_benchmarks(&mut fragments, root);
    memory::insert(&mut fragments, root);
    Ok(fragments)
}

/// Render the generated changelog beneath the docs page's own heading.
fn changelog_body(contents: &str) -> String {
    let body = match contents.strip_prefix("# Changelog") {
        Some(rest) => rest.trim_start_matches(['\n', '\r']),
        None => contents,
    };
    // The first release has no v0.0.1 tag. Repair its historical link when
    // publishing the docs without modifying Versionary's generated changelog.
    body.replace(
        "https://github.com/jolars/badness/compare/v0.0.1...v0.1.0",
        "https://github.com/jolars/badness/releases/tag/v0.1.0",
    )
}

#[cfg(test)]
mod changelog_tests {
    #[test]
    fn initial_release_links_to_its_tag_instead_of_a_nonexistent_predecessor() {
        let input = "# Changelog\n\n## [0.2.0](https://github.com/jolars/badness/compare/v0.1.0...v0.2.0)\n\n\
            ## [0.1.0](https://github.com/jolars/badness/compare/v0.0.1...v0.1.0)\n\nInitial release.\n";
        let rendered = super::changelog_body(input);
        assert!(rendered.contains("[0.1.0](https://github.com/jolars/badness/releases/tag/v0.1.0)"));
        assert!(rendered.contains("compare/v0.1.0...v0.2.0"));
        assert!(rendered.ends_with("Initial release.\n"));
        assert!(!rendered.contains("# Changelog"));
    }
}

// --- Benchmarks --------------------------------------------------------------

const BENCH_META_MARKER: &str = "{{ benchmark-meta }}";
const BENCH_RESULTS_MARKER: &str = "{{ benchmark-results }}";
const BENCH_PROJECT_RESULTS_MARKER: &str = "{{ benchmark-project-results }}";
const LINT_META_MARKER: &str = "{{ lint-benchmark-meta }}";
const LINT_RESULTS_MARKER: &str = "{{ lint-benchmark-results }}";

/// The document id the folder (whole-project) benchmark is recorded under; see
/// `benches/compare_format.sh`. Single-file rows carry any other id, so this is
/// the split point between the single-file and project charts.
const PROJECT_DOC_ID: &str = "project";

/// The committed benchmark artifact, deserialized straight from
/// `benches/benchmark_results.json`. See `benches/compare_format.sh` for the
/// producer and the schema.
#[derive(Deserialize)]
struct Benchmarks {
    meta: Meta,
    documents: Vec<Document>,
    results: Vec<BenchResult>,
    /// Linter timings (badness lint vs lacheck/chktex), same row shape as
    /// `results`. Defaulted so a pre-schema-v2 artifact still deserializes.
    #[serde(default)]
    lint_results: Vec<BenchResult>,
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
    lacheck: Option<Tool>,
    chktex: Option<Tool>,
}

#[derive(Deserialize)]
struct Tool {
    version: String,
}

#[derive(Deserialize, Clone)]
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
/// into the page for `docs/.vitepress/theme/bench-charts.js` to plot with Vega-Lite.
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
fn insert_benchmarks(fragments: &mut Fragments, root: &Path) {
    let path = root.join("benches/benchmark_results.json");
    let rendered = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Benchmarks>(&s).ok())
        .map(|b| {
            // The single-file and whole-project rows share one `documents` list;
            // split it so each drives its own chart (never mixed in one plot).
            let single: Vec<Document> = b
                .documents
                .iter()
                .filter(|d| d.id != PROJECT_DOC_ID)
                .cloned()
                .collect();
            let project: Vec<Document> = b
                .documents
                .iter()
                .filter(|d| d.id == PROJECT_DOC_ID)
                .cloned()
                .collect();
            Rendered {
                meta: render_meta(&b.meta),
                results: render_results(&single, &b.results, FORMAT_CAPTION),
                project_results: if project.is_empty() {
                    "_Whole-project benchmark not measured in this run._".to_string()
                } else {
                    render_results(&project, &b.results, FORMAT_PROJECT_CAPTION)
                },
                lint_meta: render_lint_meta(&b.meta),
                lint_results: render_results(&single, &b.lint_results, LINT_CAPTION),
            }
        })
        .unwrap_or_else(|| {
            let note = format!(
                "_Benchmark data unavailable (`{}` missing or unreadable; run `task bench`)._",
                path.display()
            );
            Rendered {
                meta: note.clone(),
                results: note.clone(),
                project_results: note.clone(),
                lint_meta: note.clone(),
                lint_results: note,
            }
        });

    for (marker, content) in [
        (BENCH_META_MARKER, rendered.meta),
        (BENCH_RESULTS_MARKER, rendered.results),
        (BENCH_PROJECT_RESULTS_MARKER, rendered.project_results),
        (LINT_META_MARKER, rendered.lint_meta),
        (LINT_RESULTS_MARKER, rendered.lint_results),
    ] {
        fragments.insert(marker.into(), content);
    }
}

/// The five rendered fragments substituted into the benchmarks page: the two
/// meta blocks and the three result charts (formatter single-file, formatter
/// whole-project, linter single-file).
struct Rendered {
    meta: String,
    results: String,
    project_results: String,
    lint_meta: String,
    lint_results: String,
}

/// Chart caption for the formatter results (`{{ benchmark-results }}`).
const FORMAT_CAPTION: &str = "Mean formatting time relative to <code>badness</code> \
     on a logarithmic scale. The dashed line marks <code>badness</code> at 1; \
     faster tools fall below it. Color distinguishes documents. \
     Hover over a dot for times in milliseconds.";

/// Chart caption for the whole-project formatter results
/// (`{{ benchmark-project-results }}`). Only `badness` and `tex-fmt` appear
/// (latexindent has no recursive mode), and this is a folder `--check`, a
/// different mode from the single-file rows.
const FORMAT_PROJECT_CAPTION: &str =
    "Mean time to check formatting across the thesis project, relative to \
     <code>badness</code> on a logarithmic scale. The dashed line marks \
     <code>badness</code> at 1; faster tools fall below it. \
     Hover over a dot for times in milliseconds.";

/// Chart caption for the linter results (`{{ lint-benchmark-results }}`).
const LINT_CAPTION: &str =
    "Mean linting time relative to <code>badness</code> on a logarithmic scale. \
     The dashed line marks <code>badness</code> at 1; faster tools fall below it. \
     Color distinguishes documents. Hover over a dot for times in milliseconds.";

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
    push_host_meta(&mut out, meta);
    out
}

/// The linter analog of `render_meta`: badness/lacheck/chktex versions plus the
/// shared backend, host, and run date.
fn render_lint_meta(meta: &Meta) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "- **badness**: `{}`\n",
        meta.tools.badness.version
    ));
    match &meta.tools.lacheck {
        Some(t) => out.push_str(&format!("- **lacheck**: `{}`\n", t.version)),
        None => out.push_str("- **lacheck**: not measured (not installed)\n"),
    }
    match &meta.tools.chktex {
        Some(t) => out.push_str(&format!("- **chktex**: `{}`\n", t.version)),
        None => out.push_str("- **chktex**: not measured (not installed)\n"),
    }
    push_host_meta(&mut out, meta);
    out
}

/// The backend/host/generated bullets shared by both meta blocks.
fn push_host_meta(out: &mut String, meta: &Meta) {
    out.push_str(&format!(
        "- **backend**: {} (min runs: {})\n",
        meta.backend, meta.min_runs
    ));
    out.push_str(&format!(
        "- **host**: {}/{}, {}\n",
        meta.host.os, meta.host.arch, meta.host.cpu
    ));
    out.push_str(&format!("- **generated**: {}\n", meta.generated_at));
}

/// The results marker becomes an interactive dot plot (Vega-Lite, driven by
/// `docs/.vitepress/theme/bench-charts.js` and initialized by the VitePress theme)
/// plus an HTML table with the same numbers as a no-JS/print fallback.
///
/// The chart data rides in an escaped HTML attribute; the JS
/// plots time-relative-to-badness on a log axis, one dot per (document,
/// formatter). Kept as raw HTML (not a Markdown pipe table) so the fallback
/// renders inside `<details>`.
fn render_results(documents: &[Document], results: &[BenchResult], caption: &str) -> String {
    let points = chart_points(documents, results);
    let data_json = serde_json::to_string(&points).unwrap_or_else(|_| "[]".to_string());

    let mut out = String::new();
    out.push_str(&format!(
        "<div class=\"bench-chart-block\" data-bench=\"{}\">\n",
        html_escape::encode_double_quoted_attribute(&data_json)
    ));
    // The chart and its caption form the <figure>; the caption must be the
    // figure's first or last child, so the no-JS/table fallback lives as a
    // sibling below it, not inside the figure.
    out.push_str("<figure class=\"bench-figure\">\n");
    out.push_str("<div class=\"bench-chart\"></div>\n");
    out.push_str("<figcaption>");
    out.push_str(caption);
    out.push_str("</figcaption>\n");
    out.push_str("</figure>\n");
    out.push_str(
        "<noscript>Enable JavaScript for the interactive chart; \
         the data table below has the same numbers.</noscript>\n",
    );
    out.push_str("<details class=\"bench-table\" open>\n<summary>Data table</summary>\n");
    out.push_str(&render_results_tables_html(documents, results));
    out.push_str("</details>\n");
    out.push_str("</div>\n");
    out
}

/// One dot per (document, formatter): its mean time as a ratio to that
/// document's badness baseline (badness itself is `1.0`), in corpus order.
/// Documents whose baseline is missing or non-positive are skipped (they carry
/// no meaningful ratio); they still appear in the fallback table.
fn chart_points(documents: &[Document], results: &[BenchResult]) -> Vec<ChartPoint> {
    let mut points = Vec::new();
    for doc in documents {
        let base = results
            .iter()
            .find(|r| r.document == doc.id && r.formatter == "badness")
            .map(|r| r.mean_ms);
        let Some(base) = base.filter(|&b| b > 0.0) else {
            continue;
        };
        for r in results.iter().filter(|r| r.document == doc.id) {
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
fn render_results_tables_html(documents: &[Document], results: &[BenchResult]) -> String {
    let mut out = String::new();
    for doc in documents {
        // Skip documents absent from this result set (e.g. the folder benchmark
        // has no linter rows), so no empty table is emitted.
        if !results.iter().any(|r| r.document == doc.id) {
            continue;
        }
        let base = results
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
        for r in results.iter().filter(|r| r.document == doc.id) {
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
