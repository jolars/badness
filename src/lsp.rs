//! The badness language server (Phase 4 + the ra-style threading follow-up).
//!
//! Deliberately diverges from ravel: badness uses **`lsp-server` + `lsp-types`**
//! (rust-analyzer's synchronous stack), *not* tower-lsp-server — see the LSP note
//! in `AGENTS.md`. salsa's single-writer / snapshot-readers model composes cleanly
//! with `lsp-server`'s sync main loop.
//!
//! Scope: full-document **formatting** and pushed parser **diagnostics**. Rich
//! features (hover, completion, go-to-def, symbols, range formatting) are deferred.
//!
//! ## Architecture (mirrors ravel's `src/lsp.rs`, so the eventual shared-crate
//! extraction stays a mechanical lift)
//!
//! Three roles, message-passing between them:
//!
//! - **Main loop** ([`main_loop`]) — owns [`GlobalState`] (the open-document
//!   buffers + editor settings), holds **no** database. It routes
//!   `connection.receiver` messages to the worker, applies incremental
//!   `didChange` edits to its buffers, resolves the [`FormatStyle`] for each
//!   format request, and forwards [`Outbound`] results from the workers back to
//!   the client (version-gating diagnostics).
//! - **Worker thread** ([`Worker`]) — the *sole* database writer. A buffer edit
//!   is a write-phase `upsert_file` (`&mut db`) followed by a read-phase
//!   *analyze* (compute parse diagnostics) dispatched onto the read pool, kept to
//!   at most one in flight via [`decide`] and superseded by a fresher edit of the
//!   same URI. `didClose` evicts the file.
//! - **Read pool** (`task_pool`) — runs the diagnostics analyze and formatting
//!   reads off a short-lived [`Analysis`] snapshot, each wrapped in
//!   [`salsa::Cancelled::catch`] so a racing write either drops the read
//!   (diagnostics) or makes it recompute from the captured text (formatting).
//!
//! > Note (raised per AGENTS tenet): a whole-file `.tex` parse is sub-ms, so the
//! > `decide`/supersede scheduler has little to actually preempt *today* — it is
//! > built to match the documented target architecture and starts paying off the
//! > moment an expensive async read (hover/completion/cross-file lint) lands.
//!
//! **URI as the salsa key.** Files are keyed by the document URI string
//! (`PathBuf::from(uri.as_str())`); buffer text always comes from
//! `didOpen`/`didChange`, never disk.

// `lsp_types::Uri` (a `fluent_uri` newtype) carries an internal `Cell` tag for
// its mutable-view mechanism, which trips `clippy::mutable_key_type` when a `Uri`
// is used as a map key. Our URIs are owned + parsed (never "taken"), and `Uri`'s
// `Hash`/`Eq` go through `as_str()`, so this is sound. Allow it module-wide.
#![allow(clippy::mutable_key_type)]

mod task_pool;

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, select, unbounded};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeConfiguration, DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{Formatting, Request as _};
use lsp_types::{
    Diagnostic, DiagnosticSeverity, DidChangeConfigurationParams, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentFormattingParams,
    FormattingOptions, OneOf, Position, PublishDiagnosticsParams, Range, ServerCapabilities,
    TextDocumentContentChangeEvent, TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit,
    Uri,
};
use salsa::Database as _;
use serde::Deserialize;

use crate::formatter::{FormatStyle, format_node, format_with_style};
use crate::incremental::{Analysis, IncrementalDatabase};
use crate::text::LineIndex;

use task_pool::{Spawner, TaskPool, read_pool_size};

/// A boxed error suitable for the LSP entry point.
type DynError = Box<dyn std::error::Error + Sync + Send>;

/// Start the language server over stdio, blocking until the client disconnects.
pub fn run() -> Result<(), DynError> {
    let (connection, io_threads) = Connection::stdio();
    serve(connection)?;
    io_threads.join()?;
    Ok(())
}

/// Perform the `initialize` handshake on `connection`, then run the message loop
/// until shutdown. Split out from [`run`] so tests can drive it over a
/// `Connection::memory()` pair.
pub fn serve(connection: Connection) -> Result<(), DynError> {
    let capabilities = serde_json::to_value(server_capabilities())?;
    let init_params = connection.initialize(capabilities)?;
    main_loop(connection, init_params)
}

/// Advertise what we support: **incremental** text sync + whole-document
/// formatting. Diagnostics are *pushed* via `publishDiagnostics`, which needs no
/// capability flag.
fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            TextDocumentSyncKind::INCREMENTAL,
        )),
        document_formatting_provider: Some(OneOf::Left(true)),
        ..Default::default()
    }
}

/// An open document buffer: its current text and the version it is at.
struct Document {
    text: String,
    version: i32,
}

/// The main loop's state: open-document buffers and the client's editor settings.
/// Holds no database — the worker thread owns that.
struct GlobalState {
    documents: HashMap<Uri, Document>,
    editor_settings: EditorSettings,
}

/// Formatting settings supplied by the editor, as `initializationOptions` at
/// startup or via `workspace/didChangeConfiguration`. A fallback beneath the
/// per-request [`FormattingOptions`]. Mirrors ravel's `EditorSettings`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct EditorSettings {
    line_width: Option<u32>,
    indent_width: Option<u32>,
}

impl EditorSettings {
    /// Extract our settings from a client-supplied JSON value. Accepts either the
    /// bare options object or a tree namespaced under a `"badness"` key (how
    /// `workspace/didChangeConfiguration` clients typically scope settings).
    fn from_client_value(value: &serde_json::Value) -> Self {
        let section = value
            .get("badness")
            .filter(|v| v.is_object())
            .unwrap_or(value);
        serde_json::from_value(section.clone()).unwrap_or_default()
    }

    /// Overlay these settings onto the formatter defaults.
    fn to_format_style(&self) -> FormatStyle {
        let mut style = FormatStyle::default();
        if let Some(width) = self.line_width {
            style.line_width = width as usize;
        }
        if let Some(width) = self.indent_width {
            style.indent_width = width as usize;
        }
        style
    }
}

/// Resolve the effective style for a format request: editor settings as the base,
/// then the request's `tab_size` (when set) overrides the indent width — matching
/// the MVP's original behavior.
fn resolve_style(settings: &EditorSettings, options: &FormattingOptions) -> FormatStyle {
    let mut style = settings.to_format_style();
    if options.tab_size > 0 {
        style.indent_width = options.tab_size as usize;
    }
    style
}

/// A job from the main loop to the worker thread.
enum WorkerJob {
    /// A buffer edit (from `didOpen` or `didChange`): write the full text into the
    /// db, then (re)analyze diagnostics.
    Edit {
        uri: Uri,
        path: PathBuf,
        text: String,
        version: i32,
    },
    /// `didClose`: evict the file from the db. Diagnostics are cleared directly by
    /// the main loop.
    Close { path: PathBuf },
    /// A formatting request: format on the read pool and reply to `id`.
    Format {
        id: RequestId,
        path: PathBuf,
        text: String,
        style: FormatStyle,
    },
}

/// A result from a worker (the lint thread or a read-pool job) back to the main
/// loop, which forwards it to the client.
enum Outbound {
    /// Push diagnostics for `uri` at `version` (gated against the live buffer).
    Diagnostics {
        uri: Uri,
        version: i32,
        diags: Vec<Diagnostic>,
    },
    /// A request response (e.g. a formatting edit array).
    Response(Response),
}

/// Map a document URI to the synthetic path the salsa file cache is keyed by.
fn uri_to_path(uri: &Uri) -> PathBuf {
    PathBuf::from(uri.as_str())
}

/// The blocking message loop. Owns [`GlobalState`]; spawns the worker thread and
/// the read pool, then shuttles messages between the client and the workers.
fn main_loop(connection: Connection, init_params: serde_json::Value) -> Result<(), DynError> {
    let editor_settings = init_params
        .get("initializationOptions")
        .map(EditorSettings::from_client_value)
        .unwrap_or_default();
    let mut state = GlobalState {
        documents: HashMap::new(),
        editor_settings,
    };

    let read_pool = TaskPool::new("badness-lsp-read", read_pool_size());
    let (job_tx, job_rx) = unbounded::<WorkerJob>();
    let (out_tx, out_rx) = unbounded::<Outbound>();
    let worker = spawn_worker(job_rx, out_tx, read_pool.spawner());

    loop {
        select! {
            recv(connection.receiver) -> msg => {
                let Ok(msg) = msg else { break };
                match msg {
                    Message::Request(req) => {
                        // `handle_shutdown` answers `shutdown` and waits for the
                        // following `exit`, returning `true` once both are seen.
                        if connection.handle_shutdown(&req)? {
                            break;
                        }
                        match req.method.as_str() {
                            Formatting::METHOD => on_formatting(&connection, &state, &job_tx, req),
                            _ => respond_unhandled(&connection, req),
                        }
                    }
                    Message::Notification(not) => {
                        on_notification(&connection, &mut state, &job_tx, not);
                    }
                    // The MVP issues no client-bound requests, so any response is
                    // unexpected.
                    Message::Response(_) => {}
                }
            }
            recv(out_rx) -> outbound => {
                let Ok(outbound) = outbound else { continue };
                forward_outbound(&connection, &state, outbound);
            }
        }
    }

    // Dropping `job_tx` disconnects the worker's receiver so it exits; the read
    // pool's workers exit when `read_pool` drops at the end of this scope.
    drop(job_tx);
    let _ = worker.join();
    Ok(())
}

/// Route a notification: edits and lifecycle to the worker, config inline.
fn on_notification(
    connection: &Connection,
    state: &mut GlobalState,
    job_tx: &Sender<WorkerJob>,
    not: Notification,
) {
    match not.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let Ok(params) = not.extract::<DidOpenTextDocumentParams>(DidOpenTextDocument::METHOD)
            else {
                return;
            };
            let doc = params.text_document;
            let uri = doc.uri;
            state.documents.insert(
                uri.clone(),
                Document {
                    text: doc.text.clone(),
                    version: doc.version,
                },
            );
            let _ = job_tx.send(WorkerJob::Edit {
                path: uri_to_path(&uri),
                uri,
                text: doc.text,
                version: doc.version,
            });
        }
        DidChangeTextDocument::METHOD => {
            let Ok(params) =
                not.extract::<DidChangeTextDocumentParams>(DidChangeTextDocument::METHOD)
            else {
                return;
            };
            let uri = params.text_document.uri;
            let version = params.text_document.version;
            let Some(doc) = state.documents.get_mut(&uri) else {
                return;
            };
            apply_content_changes(&mut doc.text, params.content_changes);
            doc.version = version;
            let text = doc.text.clone();
            let _ = job_tx.send(WorkerJob::Edit {
                path: uri_to_path(&uri),
                uri,
                text,
                version,
            });
        }
        DidCloseTextDocument::METHOD => {
            let Ok(params) =
                not.extract::<DidCloseTextDocumentParams>(DidCloseTextDocument::METHOD)
            else {
                return;
            };
            let uri = params.text_document.uri;
            state.documents.remove(&uri);
            let _ = job_tx.send(WorkerJob::Close {
                path: uri_to_path(&uri),
            });
            // Clear stale squiggles immediately; the worker just evicts the file.
            send_diagnostics(connection, uri, Vec::new(), None);
        }
        DidChangeConfiguration::METHOD => {
            if let Ok(params) =
                not.extract::<DidChangeConfigurationParams>(DidChangeConfiguration::METHOD)
            {
                state.editor_settings = EditorSettings::from_client_value(&params.settings);
            }
        }
        _ => {}
    }
}

/// Apply a batch of `didChange` content changes to `text`, in order. A change
/// with no range replaces the whole buffer; a ranged change splices via the
/// (UTF-16-aware) [`LineIndex`]. The index is rebuilt per change because each
/// mutation shifts later offsets.
fn apply_content_changes(text: &mut String, changes: Vec<TextDocumentContentChangeEvent>) {
    for change in changes {
        match change.range {
            None => *text = change.text,
            Some(range) => {
                let idx = LineIndex::new(text);
                let start = idx.offset_at(text, range.start.line, range.start.character);
                let end = idx.offset_at(text, range.end.line, range.end.character);
                // Guard against a degenerate (start > end) range from a misbehaving
                // client: clamp rather than panic on `replace_range`.
                let (start, end) = (start.min(end), start.max(end));
                text.replace_range(start..end, &change.text);
            }
        }
    }
}

/// `textDocument/formatting`: build a format job for the worker, or reply `null`
/// when the document is unknown.
fn on_formatting(
    connection: &Connection,
    state: &GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let params = match req.extract::<DocumentFormattingParams>(Formatting::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid formatting params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    let uri = params.text_document.uri;
    let Some(doc) = state.documents.get(&uri) else {
        // Unknown document: nothing to format.
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    };
    let style = resolve_style(&state.editor_settings, &params.options);
    let _ = job_tx.send(WorkerJob::Format {
        id,
        path: uri_to_path(&uri),
        text: doc.text.clone(),
        style,
    });
}

/// Forward a worker result to the client. Diagnostics are version-gated: a result
/// is sent only when its document is still open at exactly that version, so a
/// stale (superseded or post-close) analyze never repaints squiggles.
fn forward_outbound(connection: &Connection, state: &GlobalState, outbound: Outbound) {
    match outbound {
        Outbound::Diagnostics {
            uri,
            version,
            diags,
        } => {
            if state
                .documents
                .get(&uri)
                .is_some_and(|doc| doc.version == version)
            {
                send_diagnostics(connection, uri, diags, Some(version));
            }
        }
        Outbound::Response(resp) => {
            let _ = connection.sender.send(Message::Response(resp));
        }
    }
}

// ---------------------------------------------------------------------------
// Worker thread (sole database writer) — mirrors ravel's lint thread.
// ---------------------------------------------------------------------------

/// Signal from a finished analyze read-phase back to the worker: the analyze for
/// `uri`@`version` completed (or unwound on cancellation) and dropped its db
/// clone, so the in-flight slot is free.
struct AnalyzeDone {
    uri: Uri,
    version: i32,
}

/// The single in-flight analyze, if any.
struct InflightAnalyze {
    uri: Uri,
    version: i32,
}

/// A queued analyze request: the latest pending edit for a URI.
struct AnalyzeRequest {
    uri: Uri,
    path: PathBuf,
    version: i32,
}

/// What [`Worker::try_dispatch`] should do given the in-flight analyze and the
/// pending queue. Pure decision (see [`decide`]) so it can be unit-tested.
#[derive(Debug, PartialEq, Eq)]
enum DispatchAction {
    /// Idle with nothing queued, or busy with no newer edit for the in-flight
    /// URI: leave the running analyze and wait for its `done`.
    Wait,
    /// The slot is free; start a fresh analyze for this URI.
    Start(Uri),
    /// A strictly-newer edit for the *in-flight* URI arrived; cancel the running
    /// analyze and start this URI. Only ever the in-flight URI — a different
    /// pending URI must never cancel the in-flight one.
    SupersedeAndStart(Uri),
}

/// Decide the next dispatch action. `inflight` is the running analyze's
/// `(uri, version)`, if any; `pending` maps each queued URI to its latest
/// version. Cancel only on a strictly-newer edit of the *same* URI.
fn decide(inflight: Option<(&Uri, i32)>, pending: &HashMap<Uri, i32>) -> DispatchAction {
    match inflight {
        None => match pending.keys().next() {
            Some(uri) => DispatchAction::Start(uri.clone()),
            None => DispatchAction::Wait,
        },
        Some((uri, version)) => {
            if pending.get(uri).is_some_and(|&v| v > version) {
                DispatchAction::SupersedeAndStart(uri.clone())
            } else {
                DispatchAction::Wait
            }
        }
    }
}

/// Spawn the worker thread that owns the [`IncrementalDatabase`] (the sole
/// writer) and drives diagnostics analyzes onto the read pool.
fn spawn_worker(
    job_rx: Receiver<WorkerJob>,
    out_tx: Sender<Outbound>,
    read_spawner: Spawner,
) -> JoinHandle<()> {
    let (done_tx, done_rx) = unbounded::<AnalyzeDone>();
    std::thread::Builder::new()
        .name("badness-lsp-worker".to_owned())
        .spawn(move || {
            let mut worker = Worker {
                db: IncrementalDatabase::default(),
                out_tx,
                done_tx,
                read_spawner,
                inflight: None,
                pending: HashMap::new(),
            };
            worker.run(&job_rx, &done_rx);
        })
        .expect("spawn LSP worker thread")
}

struct Worker {
    db: IncrementalDatabase,
    out_tx: Sender<Outbound>,
    /// Read-phase workers signal completion here so the worker can free the
    /// in-flight slot and dispatch the next pending analyze.
    done_tx: Sender<AnalyzeDone>,
    read_spawner: Spawner,
    /// The single in-flight analyze, if any. At most one runs at a time: the
    /// write-phase needs exclusive `&mut db`, and salsa cancellation is global, so
    /// a second concurrent analyze couldn't be cancelled selectively.
    inflight: Option<InflightAnalyze>,
    /// Coalesced analyze queue: the latest pending request per URI.
    pending: HashMap<Uri, AnalyzeRequest>,
}

impl Worker {
    fn run(&mut self, job_rx: &Receiver<WorkerJob>, done_rx: &Receiver<AnalyzeDone>) {
        loop {
            select! {
                recv(job_rx) -> job => {
                    let Ok(job) = job else { break };  // main dropped `job_tx`
                    self.handle_job(job);
                    while let Ok(j) = job_rx.try_recv() {
                        self.handle_job(j);
                    }
                    self.try_dispatch();
                }
                recv(done_rx) -> done => {
                    let Ok(done) = done else { continue };
                    // Free the slot only if this `done` is for the *current*
                    // in-flight analyze — a late `done` from a superseded one must
                    // not clear the new analyze.
                    if matches!(&self.inflight, Some(f) if f.uri == done.uri && f.version == done.version)
                    {
                        self.inflight = None;
                    }
                    self.try_dispatch();
                }
            }
        }
    }

    fn handle_job(&mut self, job: WorkerJob) {
        match job {
            WorkerJob::Edit {
                uri,
                path,
                text,
                version,
            } => {
                // Write-phase: push the live buffer into the db. Cheap — the parse
                // is a lazy salsa query deferred to the analyze. Acquiring `&mut
                // db` blocks until any outstanding read snapshot drops (single
                // writer), which is how a fresher edit preempts an in-flight read.
                self.db.upsert_file(&path, text);
                self.enqueue(AnalyzeRequest { uri, path, version });
            }
            WorkerJob::Close { path } => {
                self.db.remove_file(&path);
            }
            WorkerJob::Format {
                id,
                path,
                text,
                style,
            } => {
                // Format reads run on the read pool against a snapshot, concurrent
                // with the analyze slot (they are id-bound responses, not coalesced).
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner
                    .spawn(move || run_format(&snapshot, id, &path, &text, style, &out_tx));
            }
        }
    }

    /// Add `req` to the pending queue, keeping the highest version per URI.
    fn enqueue(&mut self, req: AnalyzeRequest) {
        match self.pending.get(&req.uri) {
            Some(existing) if existing.version >= req.version => {}
            _ => {
                self.pending.insert(req.uri.clone(), req);
            }
        }
    }

    /// Start the next analyze if the slot is free, superseding the in-flight one
    /// only when a newer edit of the *same* URI is queued (see [`decide`]).
    fn try_dispatch(&mut self) {
        let versions: HashMap<Uri, i32> = self
            .pending
            .iter()
            .map(|(uri, req)| (uri.clone(), req.version))
            .collect();
        let inflight = self.inflight.as_ref().map(|f| (&f.uri, f.version));
        let uri = match decide(inflight, &versions) {
            DispatchAction::Wait => return,
            DispatchAction::Start(uri) => uri,
            DispatchAction::SupersedeAndStart(uri) => {
                // The write-phase already tripped cancellation on a real edit, but
                // make it explicit and robust: block until the old clone drops.
                // Safe — this thread holds no clone.
                self.db.trigger_cancellation();
                self.inflight = None;
                uri
            }
        };
        let Some(req) = self.pending.remove(&uri) else {
            return;
        };
        self.start_analyze(req);
    }

    /// Dispatch the diagnostics read-phase for `req` onto the read pool, holding a
    /// db clone. A superseding edit (or any write) trips `salsa::Cancelled`, caught
    /// so a cancelled analyze publishes nothing.
    fn start_analyze(&mut self, req: AnalyzeRequest) {
        let snapshot = self.db.snapshot();
        let out_tx = self.out_tx.clone();
        let done_tx = self.done_tx.clone();
        let AnalyzeRequest { uri, path, version } = req;
        self.inflight = Some(InflightAnalyze {
            uri: uri.clone(),
            version,
        });
        self.read_spawner.spawn(move || {
            let result = salsa::Cancelled::catch(AssertUnwindSafe(|| {
                let file = snapshot.lookup_file(&path)?;
                let text = snapshot.file_text(file).to_owned();
                let idx = LineIndex::new(&text);
                let diags: Vec<Diagnostic> = snapshot
                    .parse_diagnostics(file)
                    .iter()
                    .map(|d| Diagnostic {
                        range: byte_range_to_lsp(&idx, &text, d.start, d.end),
                        severity: Some(DiagnosticSeverity::ERROR),
                        source: Some("badness".to_owned()),
                        message: d.message.clone(),
                        ..Default::default()
                    })
                    .collect();
                Some(diags)
            }));
            if let Ok(Some(diags)) = result {
                let _ = out_tx.send(Outbound::Diagnostics {
                    uri: uri.clone(),
                    version,
                    diags,
                });
            }
            // The clone MUST drop before we signal `done`: the next write-phase /
            // `trigger_cancellation` blocks until it's gone, so a premature `done`
            // could let the worker start a write that deadlocks on this clone.
            drop(snapshot);
            let _ = done_tx.send(AnalyzeDone { uri, version });
        });
    }
}

/// Format the buffer behind a [`WorkerJob::Format`] on the read pool and reply.
///
/// Fast path: reuse the snapshot's cached tree (no reparse). On a racing write
/// (`salsa::Cancelled`), a stale snapshot (`file_text != text`), or a cache miss,
/// recompute from the captured `text` via [`format_with_style`] (which itself
/// guards parse errors) so the client always gets a correct response.
fn run_format(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &str,
    style: FormatStyle,
    out_tx: &Sender<Outbound>,
) {
    let result = match compute_format(snapshot, path, text, style) {
        Some(edit) => serde_json::to_value(vec![edit]).unwrap_or(serde_json::Value::Null),
        None => serde_json::Value::Null,
    };
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Produce the whole-document replacing edit, or `None` for a no-op / refusal /
/// unknown buffer. See [`run_format`] for the cancellation/fallback contract.
fn compute_format(
    snapshot: &Analysis,
    path: &Path,
    text: &str,
    style: FormatStyle,
) -> Option<TextEdit> {
    // `Some(Some(s))` = formatted; `Some(None)` = clean refusal (parse/format
    // error); `None` = cache miss / stale snapshot (fall back to the captured text).
    let cached = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let file = snapshot.lookup_file(path)?;
        if snapshot.file_text(file) != text {
            return None;
        }
        if !snapshot.parse_diagnostics(file).is_empty() {
            return Some(None);
        }
        let root = snapshot.parsed_tree(file);
        Some(format_node(&root, style).ok())
    }));

    let formatted = match cached {
        Ok(Some(opt)) => opt,
        Ok(None) | Err(_) => format_with_style(text, style).ok(),
    }?;

    if formatted == text {
        return None;
    }
    let idx = LineIndex::new(text);
    let (end_line, end_col) = idx.utf16_position(text, text.len());
    Some(TextEdit {
        range: Range {
            start: Position::new(0, 0),
            end: Position::new(end_line, end_col),
        },
        new_text: formatted,
    })
}

// ---------------------------------------------------------------------------
// Small helpers (unchanged from the single-threaded MVP).
// ---------------------------------------------------------------------------

/// Send a `publishDiagnostics` notification.
fn send_diagnostics(
    connection: &Connection,
    uri: Uri,
    diagnostics: Vec<Diagnostic>,
    version: Option<i32>,
) {
    let params = PublishDiagnosticsParams {
        uri,
        diagnostics,
        version,
    };
    let not = Notification::new(PublishDiagnostics::METHOD.to_owned(), params);
    let _ = connection.sender.send(Message::Notification(not));
}

/// Reply to an unhandled request with a method-not-found error.
fn respond_unhandled(connection: &Connection, req: Request) {
    let resp = Response::new_err(
        req.id,
        ErrorCode::MethodNotFound as i32,
        format!("unhandled request: {}", req.method),
    );
    let _ = connection.sender.send(Message::Response(resp));
}

/// Convert a byte range into an LSP range via the (UTF-16-aware) [`LineIndex`].
fn byte_range_to_lsp(idx: &LineIndex, text: &str, start: usize, end: usize) -> Range {
    let (sl, sc) = idx.utf16_position(text, start);
    let (el, ec) = idx.utf16_position(text, end);
    Range {
        start: Position::new(sl, sc),
        end: Position::new(el, ec),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(s: &str) -> Uri {
        s.parse().unwrap()
    }

    #[test]
    fn decide_starts_when_idle() {
        let mut pending = HashMap::new();
        pending.insert(uri("file:///a.tex"), 1);
        assert_eq!(
            decide(None, &pending),
            DispatchAction::Start(uri("file:///a.tex"))
        );
    }

    #[test]
    fn decide_waits_when_idle_and_empty() {
        assert_eq!(decide(None, &HashMap::new()), DispatchAction::Wait);
    }

    #[test]
    fn decide_supersedes_only_on_newer_same_uri() {
        let a = uri("file:///a.tex");
        let mut pending = HashMap::new();
        pending.insert(a.clone(), 5);
        assert_eq!(
            decide(Some((&a, 3)), &pending),
            DispatchAction::SupersedeAndStart(a.clone())
        );
        // Same version (not strictly newer): wait.
        assert_eq!(decide(Some((&a, 5)), &pending), DispatchAction::Wait);
    }

    #[test]
    fn decide_never_cancels_inflight_for_a_different_uri() {
        let a = uri("file:///a.tex");
        let b = uri("file:///b.tex");
        let mut pending = HashMap::new();
        pending.insert(b, 9);
        // A's analyze is in flight; only B is queued → wait, never cancel A.
        assert_eq!(decide(Some((&a, 1)), &pending), DispatchAction::Wait);
    }

    #[test]
    fn apply_content_changes_splices_ranged_edit() {
        // Replace "world" with "there" in "hello world".
        let mut text = "hello world\n".to_owned();
        let change = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position::new(0, 6),
                end: Position::new(0, 11),
            }),
            range_length: None,
            text: "there".to_owned(),
        };
        apply_content_changes(&mut text, vec![change]);
        assert_eq!(text, "hello there\n");
    }

    #[test]
    fn apply_content_changes_full_replace_on_no_range() {
        let mut text = "old".to_owned();
        let change = TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "new".to_owned(),
        };
        apply_content_changes(&mut text, vec![change]);
        assert_eq!(text, "new");
    }

    #[test]
    fn editor_settings_namespaced_and_bare() {
        let bare = serde_json::json!({ "lineWidth": 100, "indentWidth": 4 });
        let s = EditorSettings::from_client_value(&bare);
        assert_eq!(s.line_width, Some(100));
        assert_eq!(s.indent_width, Some(4));
        let style = s.to_format_style();
        assert_eq!(style.line_width, 100);
        assert_eq!(style.indent_width, 4);

        let namespaced = serde_json::json!({ "badness": { "lineWidth": 72 } });
        let s = EditorSettings::from_client_value(&namespaced);
        assert_eq!(s.line_width, Some(72));
        assert_eq!(s.indent_width, None);
    }
}
