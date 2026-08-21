//! Live heap retained by equivalent language-server histories.
//!
//! The harness drives the real in-process LSP server and compares sessions that
//! end with the same open buffers and project membership. A counting wrapper
//! around the system allocator measures live requested bytes, rather than RSS,
//! so allocator pages that are available for reuse do not look like a leak.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    ClientCapabilities, DiagnosticClientCapabilities, DidChangeTextDocumentParams,
    DidOpenTextDocumentParams, DocumentDiagnosticParams, HoverParams, InitializeParams,
    InitializedParams, Position, TextDocumentClientCapabilities, TextDocumentContentChangeEvent,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, Uri,
    VersionedTextDocumentIdentifier, WorkDoneProgressParams,
};

struct CountingAllocator;

static LIVE_BYTES: AtomicIsize = AtomicIsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Delegating the request unchanged preserves `System`'s contract.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            add_live(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Delegating the request unchanged preserves `System`'s contract.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            add_live(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        // SAFETY: `ptr` and `layout` come from the matching allocator call.
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: Delegating the request unchanged preserves `System`'s contract.
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            let delta = new_size as isize - layout.size() as isize;
            let live = LIVE_BYTES.fetch_add(delta, Ordering::Relaxed) + delta;
            update_peak(live.max(0) as usize);
        }
        new_ptr
    }
}

fn add_live(bytes: usize) {
    let live = LIVE_BYTES.fetch_add(bytes as isize, Ordering::Relaxed) + bytes as isize;
    update_peak(live.max(0) as usize);
}

fn update_peak(candidate: usize) {
    let mut peak = PEAK_BYTES.load(Ordering::Relaxed);
    while candidate > peak {
        match PEAK_BYTES.compare_exchange_weak(
            peak,
            candidate,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(actual) => peak = actual,
        }
    }
}

fn live_bytes() -> usize {
    LIVE_BYTES.load(Ordering::Relaxed).max(0) as usize
}

fn begin_measurement() -> usize {
    let baseline = live_bytes();
    PEAK_BYTES.store(baseline, Ordering::Relaxed);
    baseline
}

#[derive(Clone, Copy)]
struct Memory {
    live: usize,
    peak: usize,
}

fn finish_measurement(baseline: usize) -> Memory {
    Memory {
        live: live_bytes().saturating_sub(baseline),
        peak: PEAK_BYTES.load(Ordering::Relaxed).saturating_sub(baseline),
    }
}

struct Client {
    connection: Connection,
    server: Option<JoinHandle<()>>,
    next_id: i32,
}

impl Client {
    fn start() -> Self {
        let (server, connection) = Connection::memory();
        let thread = std::thread::spawn(move || badness::lsp::serve(server).unwrap());
        let mut client = Self {
            connection,
            server: Some(thread),
            next_id: 1,
        };
        let params = InitializeParams {
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    diagnostic: Some(DiagnosticClientCapabilities::default()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let response = client.request("initialize", serde_json::to_value(params).unwrap());
        assert!(
            response.response_result.is_ok(),
            "initialize failed: {response:?}"
        );
        client.notify(
            "initialized",
            serde_json::to_value(InitializedParams {}).unwrap(),
        );
        client
    }

    fn request(&mut self, method: &str, params: serde_json::Value) -> Response {
        let id = RequestId::from(self.next_id);
        self.next_id += 1;
        self.connection
            .sender
            .send(Message::Request(Request {
                id: id.clone(),
                method: method.to_owned(),
                params,
            }))
            .unwrap();
        loop {
            match self
                .connection
                .receiver
                .recv_timeout(Duration::from_secs(30))
                .expect("timed out waiting for the language server")
            {
                Message::Response(response) if response.id == id => return response,
                Message::Response(response) => {
                    panic!("expected response {id:?}, got {:?}", response.id)
                }
                Message::Notification(_) => {}
                Message::Request(request) => {
                    self.connection
                        .sender
                        .send(Message::Response(Response::new_ok(
                            request.id,
                            serde_json::Value::Null,
                        )))
                        .unwrap();
                }
            }
        }
    }

    fn notify(&self, method: &str, params: serde_json::Value) {
        self.connection
            .sender
            .send(Message::Notification(Notification {
                method: method.to_owned(),
                params,
            }))
            .unwrap();
    }

    fn open(&self, uri: &Uri, version: i32, text: &str) {
        self.notify(
            "textDocument/didOpen",
            serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: "latex".to_owned(),
                    version,
                    text: text.to_owned(),
                },
            })
            .unwrap(),
        );
    }

    fn change(&self, uri: &Uri, version: i32, text: &str) {
        self.notify(
            "textDocument/didChange",
            serde_json::to_value(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: text.to_owned(),
                }],
            })
            .unwrap(),
        );
    }

    fn diagnostic(&mut self, uri: &Uri) {
        let response = self.request(
            "textDocument/diagnostic",
            serde_json::to_value(DocumentDiagnosticParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                identifier: None,
                previous_result_id: None,
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            })
            .unwrap(),
        );
        assert!(
            response.response_result.is_ok(),
            "diagnostic failed: {response:?}"
        );
    }

    fn hover(&mut self, uri: &Uri) {
        let response = self.request(
            "textDocument/hover",
            serde_json::to_value(HoverParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri: uri.clone() },
                    position: Position::new(4, 2),
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .unwrap(),
        );
        assert!(
            response.response_result.is_ok(),
            "hover failed: {response:?}"
        );
    }

    fn barrier(&mut self, uri: &Uri) {
        self.diagnostic(uri);
        self.hover(uri);
    }

    fn shutdown(mut self) {
        let response = self.request("shutdown", serde_json::Value::Null);
        assert!(
            response.response_result.is_ok(),
            "shutdown failed: {response:?}"
        );
        self.notify("exit", serde_json::Value::Null);
        self.server
            .take()
            .unwrap()
            .join()
            .expect("language-server thread panicked");
    }
}

fn path_to_uri(path: &Path) -> Uri {
    let mut path = path.display().to_string().replace('\\', "/");
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    format!("file://{path}").parse().expect("valid file URI")
}

fn query_session(path: &Path, generations: usize, change_text: bool) -> Memory {
    let baseline = begin_measurement();
    let mut client = Client::start();
    let uri = path_to_uri(path);
    let base = fs::read_to_string(path).unwrap();
    let changed = format!("{base}% changed\n");
    client.open(&uri, 1, &base);
    client.barrier(&uri);
    for generation in 1..=generations {
        let text = if change_text && generation % 2 == 1 {
            &changed
        } else {
            &base
        };
        client.change(&uri, generation as i32 + 1, text);
        client.barrier(&uri);
    }
    let memory = finish_measurement(baseline);
    client.shutdown();
    memory
}

struct ProjectCorpus {
    _dir: tempfile::TempDir,
    root: PathBuf,
    leaves: Vec<PathBuf>,
}

fn project_corpus(generations: usize) -> ProjectCorpus {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("all.tex");
    fs::write(
        &root,
        "\\documentclass{article}\n% root\n% root\n% root\n\\section{All}\n",
    )
    .unwrap();
    let mut leaves = Vec::with_capacity(generations);
    for generation in 0..generations {
        let group = dir.path().join(format!("group{generation:03}"));
        fs::create_dir(&group).unwrap();
        let main = group.join("main.tex");
        fs::write(
            &main,
            format!(
                "\\documentclass{{article}}\n\\usepackage{{local}}\n\\addbibresource{{refs.bib}}\n\\input{{part}}\n\\section{{Group {generation}}}\\ref{{label{generation}}}\\cite{{key{generation}}}\n"
            ),
        )
        .unwrap();
        fs::write(
            group.join("part.tex"),
            format!("\\label{{label{generation}}}\n"),
        )
        .unwrap();
        fs::write(
            group.join("local.sty"),
            format!("\\newcommand{{\\local{generation}}}{{value}}\n"),
        )
        .unwrap();
        fs::write(
            group.join("refs.bib"),
            format!("@article{{key{generation}, title={{Title {generation}}}}}\n"),
        )
        .unwrap();
        leaves.push(main);
    }
    ProjectCorpus {
        _dir: dir,
        root,
        leaves,
    }
}

fn project_session(corpus: &ProjectCorpus, progressive: bool) -> Memory {
    let baseline = begin_measurement();
    let mut client = Client::start();
    let mut paths = Vec::with_capacity(corpus.leaves.len() + 1);
    if progressive {
        paths.extend(corpus.leaves.iter());
        paths.push(&corpus.root);
    } else {
        paths.push(&corpus.root);
        paths.extend(corpus.leaves.iter());
    }
    for (index, path) in paths.into_iter().enumerate() {
        let uri = path_to_uri(path);
        let text = fs::read_to_string(path).unwrap();
        client.open(&uri, index as i32 + 1, &text);
        client.barrier(&uri);
    }
    let memory = finish_measurement(baseline);
    client.shutdown();
    memory
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn warm_up() {
    let corpus = project_corpus(1);
    let _ = project_session(&corpus, false);
}

fn main() {
    let scenario = std::env::var("BADNESS_MEMORY_SCENARIO").unwrap_or_else(|_| "all".to_owned());
    let query_generations = env_usize("BADNESS_MEMORY_QUERY_GENERATIONS", 10_000);
    let project_generations = env_usize("BADNESS_MEMORY_PROJECT_GENERATIONS", 48);
    let assert = std::env::var_os("BADNESS_MEMORY_ASSERT").is_some();
    assert!(
        matches!(scenario.as_str(), "all" | "query-log" | "project"),
        "BADNESS_MEMORY_SCENARIO must be all, query-log, or project"
    );

    warm_up();
    let scratch = tempfile::tempdir().unwrap();
    let query_path = scratch.path().join("query.tex");
    fs::write(
        &query_path,
        "\\documentclass{article}\n% query\n% query\n% query\n\\section{Query}\n",
    )
    .unwrap();

    let query = matches!(scenario.as_str(), "all" | "query-log").then(|| {
        let no_op = query_session(&query_path, query_generations, false);
        let changed = query_session(&query_path, query_generations, true);
        (no_op, changed)
    });
    let project = matches!(scenario.as_str(), "all" | "project").then(|| {
        let corpus = project_corpus(project_generations);
        let one_shot = project_session(&corpus, false);
        let progressive = project_session(&corpus, true);
        (one_shot, progressive)
    });

    let mut output = serde_json::Map::new();
    output.insert("query_generations".to_owned(), query_generations.into());
    output.insert("project_generations".to_owned(), project_generations.into());
    if let Some((no_op, changed)) = query {
        let excess = changed.live.saturating_sub(no_op.live);
        println!(
            "query log: no-op {:.2} MiB live / {:.2} peak; changed {:.2} MiB live / {:.2} peak; excess {:.2} MiB",
            mib(no_op.live),
            mib(no_op.peak),
            mib(changed.live),
            mib(changed.peak),
            mib(excess),
        );
        output.insert(
            "query_log".to_owned(),
            serde_json::json!({
                "no_op_live_bytes": no_op.live,
                "no_op_peak_bytes": no_op.peak,
                "changed_live_bytes": changed.live,
                "changed_peak_bytes": changed.peak,
                "excess_bytes": excess,
            }),
        );
        if assert {
            assert!(
                excess <= 1024 * 1024,
                "query history retained {:.2} MiB (limit 1 MiB)",
                mib(excess)
            );
        }
    }
    if let Some((one_shot, progressive)) = project {
        let excess = progressive.live.saturating_sub(one_shot.live);
        let limit = (one_shot.live / 10).max(1024 * 1024);
        println!(
            "project: one-shot {:.2} MiB live / {:.2} peak; progressive {:.2} MiB live / {:.2} peak; excess {:.2} MiB",
            mib(one_shot.live),
            mib(one_shot.peak),
            mib(progressive.live),
            mib(progressive.peak),
            mib(excess),
        );
        output.insert(
            "project".to_owned(),
            serde_json::json!({
                "one_shot_live_bytes": one_shot.live,
                "one_shot_peak_bytes": one_shot.peak,
                "progressive_live_bytes": progressive.live,
                "progressive_peak_bytes": progressive.peak,
                "excess_bytes": excess,
                "gate_limit_bytes": limit,
            }),
        );
        if assert {
            assert!(
                excess <= limit,
                "project history retained {:.2} MiB (limit {:.2} MiB)",
                mib(excess),
                mib(limit)
            );
        }
    }

    if let Some(path) = std::env::var_os("BADNESS_MEMORY_OUTPUT_JSON") {
        fs::write(
            path,
            serde_json::to_string_pretty(&serde_json::Value::Object(output)).unwrap(),
        )
        .unwrap();
    }
}
