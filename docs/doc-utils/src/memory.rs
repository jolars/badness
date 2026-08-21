//! Rendering for the committed external LSP memory benchmark artifact.

use mdbook_preprocessor::book::Book;
use serde::Deserialize;
use std::path::Path;

const META_MARKER: &str = "{{ memory-benchmark-meta }}";
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
}

pub(crate) fn insert(book: &mut Book, project_root: &Path) {
    let needs_render = {
        let mut found = false;
        book.for_each_chapter_mut(|chapter| {
            found |= chapter.content.contains(META_MARKER)
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
        .map(|benchmarks| (render_meta(&benchmarks), render_results(&benchmarks)))
        .unwrap_or_else(|| {
            let note = format!(
                "_Memory benchmark data unavailable (`{}` missing or unreadable; run `task bench:memory`)._",
                path.display()
            );
            (note.clone(), note)
        });

    book.for_each_chapter_mut(|chapter| {
        if chapter.content.contains(META_MARKER) {
            chapter.content = chapter.content.replace(META_MARKER, &rendered.0);
        }
        if chapter.content.contains(RESULTS_MARKER) {
            chapter.content = chapter.content.replace(RESULTS_MARKER, &rendered.1);
        }
    });
}

fn render_meta(benchmarks: &MemoryBenchmarks) -> String {
    let session = &benchmarks.session;
    let corpus = &benchmarks.corpus;
    format!(
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
    )
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
              "session":{"runs_per_server":3,"sample_interval_seconds":0.15,"quiet_seconds":5.0,"settle_timeout_seconds":60.0,"open_files":["thesis.tex","Chapter1/chapter1.tex"],"open_bytes":17400,"operations":[]},
              "servers":[
                {"label":"Badness","summary":{"baseline_rss_mb":18.0,"settled_rss_mb":25.0,"settled_pss_mb":22.0,"peak_rss_mb":27.0,"peak_pss_mb":24.0,"settled_seconds":12.0,"relative_to_badness":1.0}},
                {"label":"TexLab","summary":{"baseline_rss_mb":22.0,"settled_rss_mb":30.0,"settled_pss_mb":27.0,"peak_rss_mb":31.0,"peak_pss_mb":28.0,"settled_seconds":13.0,"relative_to_badness":1.2}}
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
}
