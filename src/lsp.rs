//! The badness language server (Phase 4 + the ra-style threading follow-up).
//!
//! Deliberately diverges from arity: badness uses **`lsp-server` + `lsp-types`**
//! (rust-analyzer's synchronous stack), *not* tower-lsp-server — see the LSP note
//! in `AGENTS.md`. salsa's single-writer / snapshot-readers model composes cleanly
//! with `lsp-server`'s sync main loop.
//!
//! Scope: full-document **formatting**, a **document-symbol** outline,
//! **completion** (command/environment names, `\ref` keys, file paths), and
//! pushed parser **diagnostics**. Further features (hover, go-to-def, range
//! formatting) are deferred.
//!
//! ## Architecture (mirrors arity's `src/lsp.rs`, so the eventual shared-crate
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
use lsp_types::request::{Completion, DocumentSymbolRequest, Formatting, Request as _};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionList, CompletionOptions, CompletionParams,
    CompletionResponse, Diagnostic, DiagnosticSeverity, DidChangeConfigurationParams,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentFormattingParams, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse,
    FormattingOptions, InsertTextFormat, NumberOrString, OneOf, Position, PublishDiagnosticsParams,
    Range, ServerCapabilities, SymbolKind, TextDocumentContentChangeEvent,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Uri,
};
use salsa::Database as _;
use serde::Deserialize;

use crate::completion::{CandidateKind, CompletionCandidate, CompletionContext, FileArgKind};
use crate::formatter::{FormatStyle, format_node, format_with_style};
use crate::incremental::{Analysis, IncrementalDatabase};
use crate::linter::{Severity, lint_document};
use crate::parser::parse;
use crate::semantic::{OutlineItem, OutlineSymbol, SemanticModel, SignatureDb, outline};
use crate::syntax::SyntaxNode;
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
        document_symbol_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            // `\` opens command/env names; `{` opens a name/key/path argument;
            // `/` re-triggers path segments. Snippet support is read off the
            // client's capabilities, so no extra server flag is needed.
            trigger_characters: Some(vec!["\\".to_owned(), "{".to_owned(), "/".to_owned()]),
            resolve_provider: Some(false),
            ..Default::default()
        }),
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
/// per-request [`FormattingOptions`]. Mirrors arity's `EditorSettings`.
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
    /// A document-symbol request: build the outline on the read pool and reply to
    /// `id`.
    Symbols {
        id: RequestId,
        path: PathBuf,
        text: String,
    },
    /// A completion request: classify the cursor and build candidates on the read
    /// pool and reply to `id`. Carries the `uri` (not just the salsa-key `path`)
    /// so file-path completion can derive the document's on-disk directory.
    Completion {
        id: RequestId,
        uri: Uri,
        path: PathBuf,
        text: String,
        position: Position,
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
                            DocumentSymbolRequest::METHOD => {
                                on_document_symbol(&connection, &state, &job_tx, req)
                            }
                            Completion::METHOD => on_completion(&connection, &state, &job_tx, req),
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

/// `textDocument/documentSymbol`: build an outline job for the worker, or reply
/// `null` when the document is unknown.
fn on_document_symbol(
    connection: &Connection,
    state: &GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let params = match req.extract::<DocumentSymbolParams>(DocumentSymbolRequest::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid documentSymbol params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    let uri = params.text_document.uri;
    let Some(doc) = state.documents.get(&uri) else {
        // Unknown document: no symbols.
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    };
    let _ = job_tx.send(WorkerJob::Symbols {
        id,
        path: uri_to_path(&uri),
        text: doc.text.clone(),
    });
}

/// `textDocument/completion`: build a completion job for the worker, or reply
/// `null` when the document is unknown.
fn on_completion(
    connection: &Connection,
    state: &GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let params = match req.extract::<CompletionParams>(Completion::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid completion params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    let uri = params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;
    let Some(doc) = state.documents.get(&uri) else {
        // Unknown document: nothing to complete.
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    };
    let _ = job_tx.send(WorkerJob::Completion {
        id,
        path: uri_to_path(&uri),
        uri,
        text: doc.text.clone(),
        position,
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
// Worker thread (sole database writer) — mirrors arity's lint thread.
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
            WorkerJob::Symbols { id, path, text } => {
                // Symbol reads, like formatting, run on the read pool against a
                // snapshot (id-bound responses, not coalesced).
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner
                    .spawn(move || run_symbols(&snapshot, id, &path, &text, &out_tx));
            }
            WorkerJob::Completion {
                id,
                uri,
                path,
                text,
                position,
            } => {
                // Completion reads run on the read pool against a snapshot, like
                // formatting/symbols (id-bound responses, not coalesced).
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_completion(&snapshot, id, &uri, &path, &text, position, &out_tx)
                });
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
                let mut diags: Vec<Diagnostic> = snapshot
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
                // Lint-rule findings over the same salsa-cached tree + model.
                // Cross-file resolution is passed as `None`: the server tracks
                // only open buffers and assembles no project, so the cross-file
                // rules (`undefined-ref`, the cross-file branch of
                // `duplicate-label`) stay inert here until a workspace scan +
                // `Project` assembly lands. Per-file rules are unaffected.
                let root = snapshot.parsed_tree(file);
                let model = snapshot.semantic_model(file);
                for d in lint_document(&path, &root, model, None) {
                    diags.push(Diagnostic {
                        range: byte_range_to_lsp(&idx, &text, d.start, d.end),
                        severity: Some(severity_to_lsp(d.severity)),
                        code: Some(NumberOrString::String(d.rule.to_owned())),
                        source: Some("badness".to_owned()),
                        message: d.message,
                        ..Default::default()
                    });
                }
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

/// Build the document-symbol outline for a [`WorkerJob::Symbols`] on the read pool
/// and reply with a nested [`DocumentSymbolResponse`].
///
/// Fast path: reuse the snapshot's cached tree. On a racing write
/// (`salsa::Cancelled`), a stale snapshot (`file_text != text`), or a cache miss,
/// reparse the captured `text` directly. Best-effort — unlike formatting, a parse
/// error does *not* suppress the outline (the tree is error-tolerant).
fn run_symbols(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &str,
    out_tx: &Sender<Outbound>,
) {
    let symbols = compute_symbols(snapshot, path, text);
    let result = serde_json::to_value(DocumentSymbolResponse::Nested(symbols))
        .unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Compute the outline for `text`, preferring the snapshot's cached tree and
/// falling back to a direct reparse when it is unavailable or stale.
fn compute_symbols(snapshot: &Analysis, path: &Path, text: &str) -> Vec<DocumentSymbol> {
    let idx = LineIndex::new(text);
    let cached = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let file = snapshot.lookup_file(path)?;
        if snapshot.file_text(file) != text {
            return None;
        }
        Some(outline(&snapshot.parsed_tree(file)))
    }));
    let items = match cached {
        Ok(Some(items)) => items,
        // Cache miss, stale snapshot, or a cancelled read: reparse the buffer.
        Ok(None) | Err(_) => outline(&SyntaxNode::new_root(parse(text).green)),
    };
    items
        .iter()
        .map(|item| to_document_symbol(item, &idx, text))
        .collect()
}

/// Convert an [`OutlineItem`] tree into an LSP [`DocumentSymbol`], mapping byte
/// ranges through the (UTF-16-aware) [`LineIndex`].
#[allow(deprecated)] // `DocumentSymbol::deprecated` is a required struct field.
fn to_document_symbol(item: &OutlineItem, idx: &LineIndex, text: &str) -> DocumentSymbol {
    let kind = match item.kind {
        OutlineSymbol::Section => SymbolKind::MODULE,
        OutlineSymbol::Float => SymbolKind::OBJECT,
        OutlineSymbol::Theorem => SymbolKind::CLASS,
        OutlineSymbol::Label => SymbolKind::CONSTANT,
    };
    let range = item.range;
    let selection = item.selection_range;
    let children: Vec<DocumentSymbol> = item
        .children
        .iter()
        .map(|child| to_document_symbol(child, idx, text))
        .collect();
    DocumentSymbol {
        name: item.name.clone(),
        detail: None,
        kind,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(idx, text, range.start().into(), range.end().into()),
        selection_range: byte_range_to_lsp(
            idx,
            text,
            selection.start().into(),
            selection.end().into(),
        ),
        children: (!children.is_empty()).then_some(children),
    }
}

/// Build completion items for a [`WorkerJob::Completion`] on the read pool and
/// reply with a [`CompletionResponse`].
///
/// Fast path: reuse the snapshot's cached tree + the `document_signatures` /
/// `semantic_model` queries when the tracked buffer still matches `text`. On a
/// racing write (`salsa::Cancelled`), a stale snapshot, or a cache miss, reparse
/// the captured `text` and recompute the signatures/model directly. Best-effort —
/// like symbols, a parse error does not suppress completion (the tree is
/// error-tolerant).
fn run_completion(
    snapshot: &Analysis,
    id: RequestId,
    uri: &Uri,
    path: &Path,
    text: &str,
    position: Position,
    out_tx: &Sender<Outbound>,
) {
    let items = compute_completion(snapshot, uri, path, text, position);
    // `is_incomplete`: command/label universes are prefix-filtered server-side, so
    // the client re-queries as the typed prefix narrows (matches arity).
    let result = serde_json::to_value(CompletionResponse::List(CompletionList {
        is_incomplete: true,
        items,
    }))
    .unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Compute completion items at `position`, preferring the snapshot's cached tree
/// and queries, falling back to a direct reparse when unavailable or stale.
fn compute_completion(
    snapshot: &Analysis,
    uri: &Uri,
    path: &Path,
    text: &str,
    position: Position,
) -> Vec<CompletionItem> {
    let idx = LineIndex::new(text);
    let offset = idx.offset_at(text, position.line, position.character);

    let cached = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let file = snapshot.lookup_file(path)?;
        if snapshot.file_text(file) != text {
            return None;
        }
        let root = snapshot.parsed_tree(file);
        let ctx = crate::completion::classify_context(&root, offset);
        Some(build_completion_items(
            &ctx,
            snapshot.document_signatures(file),
            snapshot.semantic_model(file),
            uri,
        ))
    }));
    match cached {
        Ok(Some(items)) => items,
        // Cache miss, stale snapshot, or a cancelled read: reparse the buffer.
        Ok(None) | Err(_) => {
            let root = SyntaxNode::new_root(parse(text).green);
            let ctx = crate::completion::classify_context(&root, offset);
            let sigs = crate::semantic::scan_definitions(&root);
            let model = SemanticModel::build(&root);
            build_completion_items(&ctx, &sigs, &model, uri)
        }
    }
}

/// Turn a classified [`CompletionContext`] into LSP items. Name/label contexts go
/// through the pure [`crate::completion::candidates`]; a file-path context reads
/// the document's directory off disk (see [`file_completion_items`]).
fn build_completion_items(
    ctx: &CompletionContext,
    sigs: &SignatureDb,
    model: &SemanticModel,
    uri: &Uri,
) -> Vec<CompletionItem> {
    match ctx {
        CompletionContext::FilePath { prefix, kind } => file_completion_items(uri, prefix, *kind),
        CompletionContext::None => Vec::new(),
        _ => crate::completion::candidates(ctx, sigs, model)
            .into_iter()
            .map(candidate_to_item)
            .collect(),
    }
}

/// Map a neutral [`CompletionCandidate`] onto an `lsp_types::CompletionItem`.
fn candidate_to_item(candidate: CompletionCandidate) -> CompletionItem {
    let kind = match candidate.kind {
        CandidateKind::Command => CompletionItemKind::FUNCTION,
        CandidateKind::Environment => CompletionItemKind::CLASS,
        CandidateKind::Label => CompletionItemKind::REFERENCE,
    };
    CompletionItem {
        label: candidate.label,
        kind: Some(kind),
        insert_text: candidate.insert_text,
        insert_text_format: candidate.snippet.then_some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }
}

/// File-path candidates for a `\includegraphics`/`\input`/… argument: read the
/// directory the partial path points into (relative to the document's on-disk
/// directory) and offer matching files (by [`FileArgKind`] extension) and
/// subdirectories. Empty for an unsaved buffer (no `file://` path) or an
/// unreadable directory. The label is the bare entry name; editors treat `/` as a
/// word boundary, so completing after `img/` replaces only the trailing segment.
fn file_completion_items(uri: &Uri, prefix: &str, kind: FileArgKind) -> Vec<CompletionItem> {
    let Some(doc_path) = uri_to_fs_path(uri) else {
        return Vec::new();
    };
    let Some(doc_dir) = doc_path.parent() else {
        return Vec::new();
    };
    // Split the typed prefix into its directory part and the trailing filename
    // prefix; the directory part is resolved relative to the document.
    let (dir_part, file_prefix) = match prefix.rfind('/') {
        Some(slash) => (&prefix[..=slash], &prefix[slash + 1..]),
        None => ("", prefix),
    };
    let Ok(entries) = std::fs::read_dir(doc_dir.join(dir_part)) else {
        return Vec::new();
    };

    let mut items = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Skip hidden entries and those not matching the typed filename prefix.
        if name.starts_with('.') || !name.starts_with(file_prefix) {
            continue;
        }
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        if is_dir {
            items.push(CompletionItem {
                label: name,
                kind: Some(CompletionItemKind::FOLDER),
                ..Default::default()
            });
        } else if has_extension(&name, kind.extensions()) {
            items.push(CompletionItem {
                label: name,
                kind: Some(CompletionItemKind::FILE),
                ..Default::default()
            });
        }
    }
    items
}

/// Whether `name`'s extension (case-insensitive) is one of `exts`.
fn has_extension(name: &str, exts: &[&str]) -> bool {
    match name.rsplit_once('.') {
        Some((_, ext)) => {
            let ext = ext.to_ascii_lowercase();
            exts.contains(&ext.as_str())
        }
        None => false,
    }
}

/// Convert a `file://` document URI to a filesystem path, percent-decoding the
/// path. Returns `None` for a non-`file` scheme (an in-memory/unsaved buffer),
/// so file-path completion simply yields nothing there. Minimal by design — local
/// `file:///abs/path` URIs only; no `file://host/...` authority handling (rare for
/// editor documents) and no new dependency.
fn uri_to_fs_path(uri: &Uri) -> Option<PathBuf> {
    let rest = uri.as_str().strip_prefix("file://")?;
    // An empty authority leaves `rest` starting at the absolute path's `/`. Drop a
    // non-empty authority defensively (everything up to the first `/`).
    let path = match rest.strip_prefix('/') {
        Some(_) => rest,
        None => rest.split_once('/').map(|(_, p)| p)?,
    };
    let path = percent_decode(path);
    // A Windows file URI carries the absolute path as `/C:/dir/...`; the leading
    // slash is URI syntax, not part of the filesystem path (`C:\dir`). Strip it
    // when a drive-letter component follows so `read_dir` sees a real path. On
    // Unix the leading `/` is the filesystem root and must stay.
    let path = strip_drive_letter_slash(&path);
    Some(PathBuf::from(path))
}

/// Strip the leading slash of a Windows drive-letter path (`/C:/dir` → `C:/dir`),
/// leaving any other path (including Unix absolute paths) untouched. Recognizes a
/// single ASCII-letter drive followed by `:` and a separator or the end.
fn strip_drive_letter_slash(path: &str) -> &str {
    let bytes = path.as_bytes();
    if let [b'/', drive, b':', rest @ ..] = bytes
        && drive.is_ascii_alphabetic()
        && matches!(rest, [] | [b'/', ..] | [b'\\', ..])
    {
        &path[1..]
    } else {
        path
    }
}

/// Percent-decode a URI path component (`%20` → space, …), leaving any malformed
/// escape verbatim. ASCII-oriented but UTF-8-safe for well-formed input.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            )
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
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

/// Map a linter [`Severity`] onto the LSP severity. Parse diagnostics bypass
/// this (always `ERROR`); lint rules carry their own severity.
fn severity_to_lsp(severity: Severity) -> DiagnosticSeverity {
    match severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Info => DiagnosticSeverity::INFORMATION,
        Severity::Hint => DiagnosticSeverity::HINT,
    }
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
    fn uri_to_fs_path_handles_unix_and_windows() {
        // Unix: the leading slash is the filesystem root and must be kept.
        assert_eq!(
            uri_to_fs_path(&uri("file:///tmp/dir/main.tex")),
            Some(PathBuf::from("/tmp/dir/main.tex"))
        );
        // Windows: the leading slash before the drive letter is URI syntax only.
        assert_eq!(
            uri_to_fs_path(&uri("file:///C:/Users/me/main.tex")),
            Some(PathBuf::from("C:/Users/me/main.tex"))
        );
        // Non-file scheme (unsaved buffer) → no path.
        assert_eq!(uri_to_fs_path(&uri("untitled:Untitled-1")), None);
    }

    #[test]
    fn strip_drive_letter_slash_only_strips_real_drives() {
        assert_eq!(strip_drive_letter_slash("/C:/dir"), "C:/dir");
        assert_eq!(strip_drive_letter_slash("/c:"), "c:");
        assert_eq!(strip_drive_letter_slash("/C:\\dir"), "C:\\dir");
        // Not a drive letter: leave untouched.
        assert_eq!(strip_drive_letter_slash("/tmp/dir"), "/tmp/dir");
        assert_eq!(strip_drive_letter_slash("/ab:/dir"), "/ab:/dir");
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
