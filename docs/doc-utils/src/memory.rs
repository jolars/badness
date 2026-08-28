//! Rendering for the committed external LSP speed and memory benchmark artifact.

use mdbook_preprocessor::book::Book;
use serde::Deserialize;
use std::path::Path;

const META_MARKER: &str = "{{ memory-benchmark-meta }}";
const SPEED_MARKER: &str = "{{ lsp-benchmark-results }}";
const RESULTS_MARKER: &str = "{{ memory-benchmark-results }}";

#[derive(Deserialize)]
struct MemoryBenchmarks {
    generated_at: String,
    host: Host,
    versions: Versions,
    corpus: Corpus,
    session: Session,
    servers: Vec<Server>,
}

#[derive(Deserialize)]
struct Host {
    os: String,
    cpu: String,
    memory_gb: f64,
}

#[derive(Deserialize)]
struct Versions {
    badness: String,
    texlab: String,
}

#[derive(Deserialize)]
struct Corpus {
    repository: String,
    tag: String,
    commit: String,
    source_files: u64,
    source_bytes: u64,
}

#[derive(Deserialize)]
struct Session {
    runs_per_server: u64,
    sample_interval_seconds: f64,
    quiet_seconds: f64,
    settle_timeout_seconds: f64,
    open_files: Vec<String>,
    open_bytes: u64,
    #[serde(default)]
    latency_runs: Option<u64>,
    #[serde(default)]
    latency_warmups: Option<u64>,
    #[serde(default)]
    latency_files: Option<u64>,
    #[serde(default)]
    navigation_target: Option<NavigationTarget>,
}

#[derive(Deserialize)]
struct NavigationTarget {
    file: String,
    symbol: String,
    position: NavigationPosition,
}

#[derive(Deserialize)]
struct NavigationPosition {
    line: u64,
}

#[derive(Deserialize)]
struct Server {
    label: String,
    summary: Summary,
}

#[derive(Deserialize)]
struct Summary {
    baseline_rss_mb: f64,
    settled_rss_mb: f64,
    settled_pss_mb: f64,
    peak_rss_mb: f64,
    relative_to_badness: f64,
    #[serde(default)]
    initialize_seconds: Option<f64>,
    #[serde(default)]
    workspace_ready_seconds: Option<f64>,
    #[serde(default)]
    documents_ready_seconds: Option<f64>,
    #[serde(default)]
    request_latencies: Vec<RequestLatency>,
}

#[derive(Deserialize)]
struct RequestLatency {
    key: String,
    label: String,
    median_ms: Option<f64>,
    p95_ms: Option<f64>,
    #[serde(default)]
    failures: u64,
    #[serde(default)]
    empty_results: u64,
    #[serde(default)]
    result_unit: Option<String>,
    #[serde(default)]
    result_count_min: Option<u64>,
    #[serde(default)]
    result_count_median: Option<f64>,
    #[serde(default)]
    result_count_max: Option<u64>,
    #[serde(default)]
    result_files_min: Option<u64>,
    #[serde(default)]
    result_files_median: Option<f64>,
    #[serde(default)]
    result_files_max: Option<u64>,
    #[serde(default)]
    payload_bytes_median: Option<u64>,
}

pub(crate) fn insert(book: &mut Book, project_root: &Path) {
    let needs_render = {
        let mut found = false;
        book.for_each_chapter_mut(|chapter| {
            found |= chapter.content.contains(META_MARKER)
                || chapter.content.contains(SPEED_MARKER)
                || chapter.content.contains(RESULTS_MARKER);
        });
        found
    };
    if !needs_render {
        return;
    }

    let path = project_root.join("benches/memory_results.json");
    let rendered = std::fs::read_to_string(&path)
        .ok()
        .and_then(|contents| serde_json::from_str::<MemoryBenchmarks>(&contents).ok())
        .map(|benchmarks| {
            (
                render_meta(&benchmarks),
                render_speed(&benchmarks),
                render_results(&benchmarks),
            )
        })
        .unwrap_or_else(|| {
            let note = format!(
                "_LSP benchmark data unavailable (`{}` missing or unreadable; run `task bench:lsp`)._",
                path.display()
            );
            (note.clone(), note.clone(), note)
        });

    book.for_each_chapter_mut(|chapter| {
        if chapter.content.contains(META_MARKER) {
            chapter.content = chapter.content.replace(META_MARKER, &rendered.0);
        }
        if chapter.content.contains(SPEED_MARKER) {
            chapter.content = chapter.content.replace(SPEED_MARKER, &rendered.1);
        }
        if chapter.content.contains(RESULTS_MARKER) {
            chapter.content = chapter.content.replace(RESULTS_MARKER, &rendered.2);
        }
    });
}

fn render_meta(benchmarks: &MemoryBenchmarks) -> String {
    let session = &benchmarks.session;
    let corpus = &benchmarks.corpus;
    let mut output = format!(
        "- **Badness**: `{}`\n\
         - **TexLab**: `{}`\n\
         - **corpus**: [`{}` @ `{}`]({}) (`{}`, {} source files, {} bytes)\n\
         - **session**: {} fresh runs per server; {} open files ({} bytes)\n\
         - **sampling**: every {:.2} s; quiet for {:.1} s; {:.0} s phase timeout\n\
         - **host**: {}, {} ({:.1} GiB RAM)\n\
         - **generated**: {}\n",
        benchmarks.versions.badness,
        benchmarks.versions.texlab,
        repository_name(&corpus.repository),
        short_commit(&corpus.commit),
        corpus.repository,
        corpus.tag,
        corpus.source_files,
        corpus.source_bytes,
        session.runs_per_server,
        session.open_files.len(),
        session.open_bytes,
        session.sample_interval_seconds,
        session.quiet_seconds,
        session.settle_timeout_seconds,
        benchmarks.host.os,
        benchmarks.host.cpu,
        benchmarks.host.memory_gb,
        benchmarks.generated_at,
    );
    if let Some(target) = &session.navigation_target {
        output.push_str(&format!(
            "- **navigation target**: `{}` in `{}` at line {}\n",
            target.symbol,
            target.file,
            target.position.line + 1,
        ));
    }
    output
}

fn render_speed(benchmarks: &MemoryBenchmarks) -> String {
    let mut output = String::from(
        "#### Readiness\n\n\
         | Server | Initialize | Workspace ready | Open files ready |\n\
         | --- | ---: | ---: | ---: |\n",
    );
    for server in &benchmarks.servers {
        let summary = &server.summary;
        output.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            escape_table_cell(&server.label),
            duration(summary.initialize_seconds),
            duration(summary.workspace_ready_seconds),
            duration(summary.documents_ready_seconds),
        ));
    }

    output.push_str(
        "\n#### Warm requests\n\n\
         | Server | Request | Median | p95 | Returned work |\n\
         | --- | --- | ---: | ---: | --- |\n",
    );
    for server in &benchmarks.servers {
        for latency in &server.summary.request_latencies {
            output.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                escape_table_cell(&server.label),
                escape_table_cell(&latency.label),
                milliseconds(latency.median_ms),
                milliseconds(latency.p95_ms),
                returned_work(latency),
            ));
        }
    }

    let session = &benchmarks.session;
    let runs = session
        .latency_runs
        .map(|value| value.to_string())
        .unwrap_or_else(|| "several".to_string());
    let sessions = session.runs_per_server;
    let warmups = session
        .latency_warmups
        .map(|value| value.to_string())
        .unwrap_or_else(|| "an unrecorded number of".to_string());
    let files = session
        .latency_files
        .map(|value| value.to_string())
        .unwrap_or_else(|| "several".to_string());
    let target = session
        .navigation_target
        .as_ref()
        .map(|target| format!("`{}` in `{}`", target.symbol, target.file))
        .unwrap_or_else(|| "the pinned navigation target".to_string());
    output.push_str(&format!(
        "\n_Warm requests show median / p95 across all samples. Each target ran {runs} measured rounds in each of {sessions} fresh sessions after {warmups} warmup rounds; symbols and hover span {files} files, while definition, references, and rename use {target}. Rename constructs the workspace edit but does not apply it._\n"
    ));

    let notes: Vec<String> = benchmarks
        .servers
        .iter()
        .flat_map(|server| {
            server.summary.request_latencies.iter().flat_map(move |latency| {
                let operation = latency.key.replace('_', " ");
                let mut notes = Vec::new();
                if latency.failures > 0 {
                    notes.push(format!(
                        "{}: {} failed {operation} requests",
                        server.label, latency.failures
                    ));
                }
                if latency.empty_results > 0 {
                    notes.push(format!(
                        "{}: {} {operation} requests returned no result",
                        server.label, latency.empty_results
                    ));
                }
                notes
            })
        })
        .collect();
    if !notes.is_empty() {
        output.push_str(&format!("\n_Harness notes: {}._\n", notes.join("; ")));
    }
    output
}

fn render_results(benchmarks: &MemoryBenchmarks) -> String {
    let mut output = String::from(
        "| Server | Baseline RSS | Settled RSS | Settled PSS | Peak RSS | Relative settled RSS |\n\
         | --- | ---: | ---: | ---: | ---: | ---: |\n",
    );
    for server in &benchmarks.servers {
        let summary = &server.summary;
        let relative = if (summary.relative_to_badness - 1.0).abs() < f64::EPSILON {
            "baseline".to_string()
        } else {
            format!("{:.2}×", summary.relative_to_badness)
        };
        output.push_str(&format!(
            "| {} | {:.1} MB | {:.1} MB | {:.1} MB | {:.1} MB | {} |\n",
            escape_table_cell(&server.label),
            summary.baseline_rss_mb,
            summary.settled_rss_mb,
            summary.settled_pss_mb,
            summary.peak_rss_mb,
            relative,
        ));
    }
    output
}

fn repository_name(repository: &str) -> &str {
    repository.rsplit('/').next().unwrap_or(repository)
}

fn short_commit(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

fn escape_table_cell(value: &str) -> String {
    value.replace('|', "\\|")
}

fn duration(seconds: Option<f64>) -> String {
    match seconds {
        Some(value) if value < 1.0 => format!("{:.0} ms", value * 1000.0),
        Some(value) => format!("{value:.2} s"),
        None => "-".to_string(),
    }
}

fn milliseconds(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.2} ms"))
        .unwrap_or_else(|| "-".to_string())
}

fn returned_work(latency: &RequestLatency) -> String {
    let mut output = match (
        latency.result_count_min,
        latency.result_count_median,
        latency.result_count_max,
        latency.result_unit.as_deref(),
    ) {
        (Some(min), Some(median), Some(max), Some(unit)) => {
            quantity_range(min, median, max, unit)
        }
        _ => return "-".to_string(),
    };
    if let (Some(min), Some(median), Some(max)) = (
        latency.result_files_min,
        latency.result_files_median,
        latency.result_files_max,
    ) {
        output.push_str(" in ");
        output.push_str(&quantity_range(min, median, max, "file"));
    }
    if let Some(bytes) = latency.payload_bytes_median {
        output.push_str(", ");
        output.push_str(&human_bytes(bytes));
    }
    output
}

fn quantity_range(min: u64, median: f64, max: u64, unit: &str) -> String {
    if min == max {
        return format!("{min} {unit}{}", if min == 1 { "" } else { "s" });
    }
    format!(
        "{min}–{max} {unit}s (median {})",
        if median.fract() == 0.0 {
            format!("{median:.0}")
        } else {
            format!("{median:.1}")
        }
    )
}

fn human_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.0} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / 1024.0 / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> MemoryBenchmarks {
        serde_json::from_str(
            r#"{
              "generated_at":"2026-08-21T12:00:00Z",
              "host":{"hostname":"host","os":"Linux x86_64","cpu":"Example CPU","memory_gb":32.0},
              "versions":{"badness":"0.17.0","texlab":"5.26.0"},
              "corpus":{"repository":"https://github.com/kks32/phd-thesis-template","tag":"v2.4","commit":"3ce347686d75747f69d9e736acd46a9393a1b332","source_files":16,"source_bytes":279004},
              "session":{"runs_per_server":3,"sample_interval_seconds":0.15,"quiet_seconds":5.0,"settle_timeout_seconds":60.0,
                         "latency_runs":20,"latency_warmups":2,"latency_files":3,
                         "open_files":["thesis.tex","Chapter1/chapter1.tex"],"open_bytes":17400,
                         "navigation_target":{"file":"Chapter1/chapter1.tex","symbol":"Aup91","position":{"line":18,"character":45}},
                         "operations":[]},
              "servers":[
                {"label":"Badness","summary":{"baseline_rss_mb":18.0,"settled_rss_mb":25.0,"settled_pss_mb":22.0,"peak_rss_mb":27.0,"peak_pss_mb":24.0,"settled_seconds":12.0,"relative_to_badness":1.0,
                  "initialize_seconds":0.02,"workspace_ready_seconds":0.08,"documents_ready_seconds":0.12,
                  "request_latencies":[
                    {"key":"document_symbol","label":"Document symbols","median_ms":0.21,"p95_ms":0.32,"samples":180,"failures":0,"empty_results":0,"targets":3,"result_unit":"symbol","result_count_min":2,"result_count_median":4.0,"result_count_max":6,"payload_bytes_median":800},
                    {"key":"definition","label":"Go to definition","median_ms":0.30,"p95_ms":0.45,"samples":60,"failures":0,"empty_results":0,"targets":1,"result_unit":"location","result_count_min":1,"result_count_median":1.0,"result_count_max":1,"result_files_min":1,"result_files_median":1.0,"result_files_max":1,"payload_bytes_median":180},
                    {"key":"references","label":"Find references","median_ms":4.2,"p95_ms":5.1,"samples":60,"failures":0,"empty_results":0,"targets":1,"result_unit":"location","result_count_min":2,"result_count_median":2.0,"result_count_max":2,"result_files_min":2,"result_files_median":2.0,"result_files_max":2,"payload_bytes_median":16384},
                    {"key":"rename","label":"Rename","median_ms":5.2,"p95_ms":6.1,"samples":60,"failures":0,"empty_results":0,"targets":1,"result_unit":"edit","result_count_min":2,"result_count_median":2.0,"result_count_max":2,"result_files_min":2,"result_files_median":2.0,"result_files_max":2,"payload_bytes_median":20480}
                  ]}},
                {"label":"TexLab","summary":{"baseline_rss_mb":22.0,"settled_rss_mb":30.0,"settled_pss_mb":27.0,"peak_rss_mb":31.0,"peak_pss_mb":28.0,"settled_seconds":13.0,"relative_to_badness":1.2,
                  "initialize_seconds":0.05,"workspace_ready_seconds":0.20,"documents_ready_seconds":0.30,"request_latencies":[]}}
              ]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn renders_versions_corpus_and_protocol_parameters() {
        let rendered = render_meta(&fixture());
        assert!(rendered.contains("**Badness**: `0.17.0`"));
        assert!(rendered.contains("`v2.4`, 16 source files, 279004 bytes"));
        assert!(rendered.contains("3 fresh runs per server; 2 open files"));
        assert!(rendered.contains("every 0.15 s; quiet for 5.0 s; 60 s phase timeout"));
    }

    #[test]
    fn renders_memory_table_relative_to_badness() {
        let rendered = render_results(&fixture());
        assert!(rendered.contains("| Badness | 18.0 MB | 25.0 MB | 22.0 MB | 27.0 MB | baseline |"));
        assert!(rendered.contains("| TexLab | 22.0 MB | 30.0 MB | 27.0 MB | 31.0 MB | 1.20× |"));
    }

    #[test]
    fn renders_readiness_latency_and_returned_work() {
        let rendered = render_speed(&fixture());
        assert!(rendered.contains("| Badness | 20 ms | 80 ms | 120 ms |"));
        assert!(
            rendered.contains(
                "| Badness | Go to definition | 0.30 ms | 0.45 ms | 1 location in 1 file, 180 B |"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "| Badness | Find references | 4.20 ms | 5.10 ms | 2 locations in 2 files, 16 KiB |"
            ),
            "{rendered}"
        );
        assert!(rendered.contains("20 measured rounds in each of 3 fresh sessions"));
        assert!(rendered.contains("`Aup91` in `Chapter1/chapter1.tex`"));
    }
}
