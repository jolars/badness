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
//!   is a write-phase `upsert_file` (`&mut db`) — plus a one-time
//!   [`seed_dir`](Worker::seed_dir) that pulls the rest of the project off disk —
//!   followed by a read-phase *analyze* (parse diagnostics + lint over an interned
//!   `Project`) dispatched onto the read pool, kept to at most one in flight via
//!   [`decide`] and superseded by a fresher edit of the same URI. When seeding
//!   grows the member set, every open document is re-linted ([`Outbound::RelintAll`]).
//!   `didClose` evicts the file.
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
//! **Filesystem path as the salsa key.** A `file:` document URI is decoded to its
//! real (normalized) filesystem path ([`uri_to_path`]); a non-`file` buffer
//! (untitled, etc.) falls back to the URI string as a synthetic key and never
//! joins a project. Open-buffer text always comes from `didOpen`/`didChange`,
//! while non-open project members (siblings reached via `\input`/`\bibliography`)
//! are read once off disk — see [`Worker::seed_dir`] — so `undefined-ref`,
//! cross-file `duplicate-label`, and `undefined-citation` can fire live. Edits to
//! a non-open member on disk are not yet watched (`workspace/didChangeWatchedFiles`
//! is a follow-up; see `TODO.md`).

// `lsp_types::Uri` (a `fluent_uri` newtype) carries an internal `Cell` tag for
// its mutable-view mechanism, which trips `clippy::mutable_key_type` when a `Uri`
// is used as a map key. Our URIs are owned + parsed (never "taken"), and `Uri`'s
// `Hash`/`Eq` go through `as_str()`, so this is sound. Allow it module-wide.
#![allow(clippy::mutable_key_type)]

mod folding;
mod task_pool;

use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, select, unbounded};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeConfiguration, DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, FoldingRangeRequest, Formatting, GotoDefinition, References,
    Request as _,
};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionList, CompletionOptions, CompletionParams,
    CompletionResponse, Diagnostic, DiagnosticSeverity, DidChangeConfigurationParams,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentFormattingParams, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse,
    FoldingRange, FoldingRangeParams, FoldingRangeProviderCapability, FormattingOptions,
    GotoDefinitionParams, GotoDefinitionResponse, InsertTextFormat, Location, NumberOrString,
    OneOf, Position, PublishDiagnosticsParams, Range, ReferenceParams, ServerCapabilities,
    SymbolKind, TextDocumentContentChangeEvent, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextEdit, Uri,
};
use rowan::{TextRange, TextSize};
use salsa::Database as _;
use serde::Deserialize;
use smol_str::SmolStr;

use crate::bib::completion::{
    BibCandidateKind, BibCompletionCandidate, bib_candidates, classify_bib_context,
};
use crate::bib::outline::{BibOutlineItem, outline as bib_outline};
use crate::bib::semantic::Model as BibModel;
use crate::bib::{
    format_node as bib_format_node, format_with_style as bib_format_with_style, parse as bib_parse,
};
use crate::completion::{CandidateKind, CompletionCandidate, CompletionContext, FileArgKind};
use crate::file_discovery::{FileKind, collect_lint_files, file_kind_or_tex};
use crate::formatter::{FormatStyle, format_node_with_signatures, format_with_style_flavored};
use crate::incremental::{Analysis, IncrementalDatabase};
use crate::linter::{Severity, lint_document};
use crate::parser::parse;
use crate::project::{ProjectMember, ResolvedCitations, ResolvedLabels};
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
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        completion_provider: Some(CompletionOptions {
            // `\` opens command/env names; `{` opens a name/key/path argument;
            // `/` re-triggers path segments. Snippet support is read off the
            // client's capabilities, so no extra server flag is needed.
            trigger_characters: Some(vec![
                "\\".to_owned(),
                "{".to_owned(),
                "/".to_owned(),
                "@".to_owned(),
            ]),
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
        kind: FileKind,
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
        kind: FileKind,
    },
    /// A document-symbol request: build the outline on the read pool and reply to
    /// `id`.
    Symbols {
        id: RequestId,
        path: PathBuf,
        text: String,
        kind: FileKind,
    },
    /// A folding-range request: compute foldable regions on the read pool and reply
    /// to `id`. Single-file like [`Symbols`](Self::Symbols), with no project snapshot.
    FoldingRange {
        id: RequestId,
        path: PathBuf,
        text: String,
        kind: FileKind,
    },
    /// A completion request: classify the cursor and build candidates on the read
    /// pool and reply to `id`. Carries the `uri` (the salsa-key path is derived from
    /// it) so file-path completion can read the document's on-disk directory.
    Completion {
        id: RequestId,
        uri: Uri,
        text: String,
        position: Position,
    },
    /// A go-to-definition request: resolve the `\ref`/`\cite` under the cursor to
    /// its `\label`/bib entry on the read pool and reply to `id`. Cross-file, so
    /// the worker snapshots project membership when it dispatches (like an analyze).
    GotoDefinition {
        id: RequestId,
        path: PathBuf,
        text: String,
        position: Position,
    },
    /// A find-references request: enumerate every `\ref`/`\cite` use of the
    /// label/key under the cursor on the read pool and reply to `id`. Cross-file
    /// (and invokable from a definition site), so the worker snapshots project
    /// membership when it dispatches, like [`GotoDefinition`](Self::GotoDefinition).
    References {
        id: RequestId,
        path: PathBuf,
        text: String,
        position: Position,
        include_declaration: bool,
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
    /// Project membership grew (the worker discovered on-disk siblings), so the
    /// cross-file resolution may have changed for *every* open document. Re-lint
    /// them all. Mirrors arity's `Outbound::RelintAll`.
    RelintAll,
}

/// Map a document URI to the path the salsa file cache is keyed by. For a `file:`
/// URI this is the real filesystem path (percent-decoded), so `\input`/bib
/// resolution and on-disk sibling reads share one path space and a project can be
/// assembled. A non-`file` buffer (untitled, etc.) falls back to the URI string as
/// a synthetic key; it simply never joins a project.
fn uri_to_path(uri: &Uri) -> PathBuf {
    uri_to_fs_path(uri).unwrap_or_else(|| PathBuf::from(uri.as_str()))
}

/// Which language pipeline a document feeds, by its path extension. Defaults to
/// [`FileKind::Tex`] for anything that is not a `.bib` file (including unsaved
/// buffers with no extension), matching the conservative CLI/stdin behavior. The
/// resolution itself lives in [`file_kind_or_tex`], shared with the CLI's
/// `--stdin-filepath`.
fn file_kind_for(path: &Path) -> FileKind {
    file_kind_or_tex(path)
}

/// The current project membership of a read snapshot, as sorted-by-caller
/// [`ProjectMember`]s — the snapshot-side counterpart of
/// [`GlobalState`]'s `project_members`, used by a format read to intern a
/// `Project` for [`Analysis::scope_signatures`].
fn members_of(snapshot: &Analysis) -> Vec<ProjectMember> {
    snapshot
        .tracked_files()
        .into_iter()
        .map(|(path, file)| {
            let kind = file_kind_for(&path);
            ProjectMember { file, path, kind }
        })
        .collect()
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
                            GotoDefinition::METHOD => {
                                on_goto_definition(&connection, &state, &job_tx, req)
                            }
                            References::METHOD => on_references(&connection, &state, &job_tx, req),
                            FoldingRangeRequest::METHOD => {
                                on_folding_range(&connection, &state, &job_tx, req)
                            }
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
                forward_outbound(&connection, &state, &job_tx, outbound);
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
            let path = uri_to_path(&uri);
            let kind = file_kind_for(&path);
            let _ = job_tx.send(WorkerJob::Edit {
                path,
                uri,
                text: doc.text,
                version: doc.version,
                kind,
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
            let path = uri_to_path(&uri);
            let kind = file_kind_for(&path);
            let _ = job_tx.send(WorkerJob::Edit {
                path,
                uri,
                text,
                version,
                kind,
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
    let mut style = resolve_style(&state.editor_settings, &params.options);
    let path = uri_to_path(&uri);
    let kind = file_kind_for(&path);
    // `EditorSettings` carries no wrap mode yet (it is hardcoded `Reflow`), so the
    // file kind decides it: a package/class body is code, defaulting to `Preserve`.
    style.wrap = kind.default_wrap();
    let _ = job_tx.send(WorkerJob::Format {
        id,
        path,
        text: doc.text.clone(),
        style,
        kind,
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
    let path = uri_to_path(&uri);
    let kind = file_kind_for(&path);
    let _ = job_tx.send(WorkerJob::Symbols {
        id,
        path,
        text: doc.text.clone(),
        kind,
    });
}

/// `textDocument/foldingRange`: build a folding job for the worker, or reply `null`
/// when the document is unknown.
fn on_folding_range(
    connection: &Connection,
    state: &GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let params = match req.extract::<FoldingRangeParams>(FoldingRangeRequest::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid foldingRange params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    let uri = params.text_document.uri;
    let Some(doc) = state.documents.get(&uri) else {
        // Unknown document: no folds.
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    };
    let path = uri_to_path(&uri);
    let kind = file_kind_for(&path);
    let _ = job_tx.send(WorkerJob::FoldingRange {
        id,
        path,
        text: doc.text.clone(),
        kind,
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
        uri,
        text: doc.text.clone(),
        position,
    });
}

/// `textDocument/definition`: build a go-to-definition job for the worker, or reply
/// `null` when the document is unknown or is a `.bib` (cite/ref sites live in
/// `.tex`, so a `.bib` cursor has nothing to jump *from*).
fn on_goto_definition(
    connection: &Connection,
    state: &GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let params = match req.extract::<GotoDefinitionParams>(GotoDefinition::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid definition params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    let uri = params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    let path = uri_to_path(&uri);
    let Some(doc) = state.documents.get(&uri) else {
        // Unknown document: nothing to resolve.
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    };
    if file_kind_for(&path) == FileKind::Bib {
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    }
    let _ = job_tx.send(WorkerJob::GotoDefinition {
        id,
        path,
        text: doc.text.clone(),
        position,
    });
}

/// `textDocument/references`: build a find-references job for the worker, or reply
/// `null` when the document is unknown. Unlike go-to-definition, a `.bib` cursor is
/// *not* rejected — find-references can start on an `@entry` key and report its
/// `\cite` use sites.
fn on_references(
    connection: &Connection,
    state: &GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let params = match req.extract::<ReferenceParams>(References::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid references params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    let uri = params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;
    let include_declaration = params.context.include_declaration;
    let path = uri_to_path(&uri);
    let Some(doc) = state.documents.get(&uri) else {
        // Unknown document: nothing to resolve.
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    };
    let _ = job_tx.send(WorkerJob::References {
        id,
        path,
        text: doc.text.clone(),
        position,
        include_declaration,
    });
}

/// Forward a worker result to the client. Diagnostics are version-gated: a result
/// is sent only when its document is still open at exactly that version, so a
/// stale (superseded or post-close) analyze never repaints squiggles.
fn forward_outbound(
    connection: &Connection,
    state: &GlobalState,
    job_tx: &Sender<WorkerJob>,
    outbound: Outbound,
) {
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
        Outbound::RelintAll => {
            // Re-queue a fresh analyze for every open document at its current
            // version. The worker coalesces per-URI, so this is cheap; salsa
            // memos make the actual recompute incremental. A re-lint of a doc in
            // an already-seeded directory discovers no new members, so it can't
            // re-trigger `RelintAll` (no loop).
            for (uri, doc) in &state.documents {
                let path = uri_to_path(uri);
                let kind = file_kind_for(&path);
                let _ = job_tx.send(WorkerJob::Edit {
                    uri: uri.clone(),
                    path,
                    text: doc.text.clone(),
                    version: doc.version,
                    kind,
                });
            }
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
    kind: FileKind,
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
                seeded_dirs: HashSet::new(),
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
    /// Directories already walked for on-disk `.tex`/`.bib` siblings, so each is
    /// seeded at most once (the membership-discovery hot-path guard).
    seeded_dirs: HashSet<PathBuf>,
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
                kind,
            } => {
                // Write-phase: push the live buffer into the db. Cheap — the parse
                // is a lazy salsa query deferred to the analyze. Acquiring `&mut
                // db` blocks until any outstanding read snapshot drops (single
                // writer), which is how a fresher edit preempts an in-flight read.
                self.db.upsert_file(&path, text);
                // Lazily pull the rest of the project off disk so cross-file rules
                // can fire. If this grows the member set, every open document's
                // resolution may have changed — re-lint them all.
                if self.seed_dir(&path) {
                    let _ = self.out_tx.send(Outbound::RelintAll);
                }
                self.enqueue(AnalyzeRequest {
                    uri,
                    path,
                    version,
                    kind,
                });
            }
            WorkerJob::Close { path } => {
                self.db.remove_file(&path);
            }
            WorkerJob::Format {
                id,
                path,
                text,
                style,
                kind,
            } => {
                // Format reads run on the read pool against a snapshot, concurrent
                // with the analyze slot (they are id-bound responses, not coalesced).
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner
                    .spawn(move || run_format(&snapshot, id, &path, &text, style, kind, &out_tx));
            }
            WorkerJob::Symbols {
                id,
                path,
                text,
                kind,
            } => {
                // Symbol reads, like formatting, run on the read pool against a
                // snapshot (id-bound responses, not coalesced).
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner
                    .spawn(move || run_symbols(&snapshot, id, &path, &text, kind, &out_tx));
            }
            WorkerJob::FoldingRange {
                id,
                path,
                text,
                kind,
            } => {
                // Folding reads run on the read pool against a snapshot, like
                // symbols (id-bound responses, not coalesced).
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner
                    .spawn(move || run_folding(&snapshot, id, &path, &text, kind, &out_tx));
            }
            WorkerJob::Completion {
                id,
                uri,
                text,
                position,
            } => {
                // Completion reads run on the read pool against a snapshot, like
                // formatting/symbols (id-bound responses, not coalesced). Cite-key
                // completion is cross-file, so — like go-to-def — we snapshot project
                // membership here on the write side.
                let snapshot = self.db.snapshot();
                let members = self.project_members();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_completion(&snapshot, id, &uri, &text, position, members, &out_tx)
                });
            }
            WorkerJob::GotoDefinition {
                id,
                path,
                text,
                position,
            } => {
                // Go-to-def is cross-file, so it needs the same membership snapshot
                // an analyze captures (open buffers plus seeded on-disk siblings),
                // taken on the write side so the read job interns the latest project.
                let snapshot = self.db.snapshot();
                let members = self.project_members();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_goto_definition(&snapshot, id, &path, &text, position, members, &out_tx)
                });
            }
            WorkerJob::References {
                id,
                path,
                text,
                position,
                include_declaration,
            } => {
                // Find-references is cross-file like go-to-def, so it captures the
                // same membership snapshot on the write side before the read job runs.
                let snapshot = self.db.snapshot();
                let members = self.project_members();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_references(
                        &snapshot,
                        id,
                        &path,
                        &text,
                        position,
                        members,
                        include_declaration,
                        &out_tx,
                    )
                });
            }
        }
    }

    /// Walk the active file's directory once for `.tex`/`.bib` siblings, reading
    /// and upserting any not already tracked, so the cross-file resolvers see the
    /// whole project. Returns whether the member set grew. Mirrors arity's
    /// `seed_workspace_for`.
    ///
    /// Skips unsaved/synthetic buffers (whose path isn't a real file) and the
    /// filesystem root, so we never walk `/`. A sibling that is already tracked —
    /// an open buffer, or one seeded earlier — keeps its live text (we never read
    /// it back from disk). Each directory is walked at most once (`seeded_dirs`).
    fn seed_dir(&mut self, path: &Path) -> bool {
        if !path.is_file() {
            return false;
        }
        let Some(dir) = path.parent() else {
            return false;
        };
        // Never walk the filesystem root (a `/foo.tex` would otherwise walk all of `/`).
        if dir.parent().is_none() {
            return false;
        }
        let dir = dir.to_path_buf();
        if !self.seeded_dirs.insert(dir.clone()) {
            return false; // already walked
        }
        let Ok(files) = collect_lint_files(&[dir]) else {
            return false;
        };
        let mut grew = false;
        for (sibling, _kind) in files {
            if self.db.lookup_file(&sibling).is_some() {
                continue; // open buffer or already seeded — keep its live text
            }
            if let Ok(text) = std::fs::read_to_string(&sibling) {
                self.db.upsert_file(&sibling, text);
                grew = true;
            }
        }
        grew
    }

    /// Snapshot the current project membership as sorted [`ProjectMember`]s, so a
    /// read job can intern a `Project` against its db snapshot.
    fn project_members(&self) -> Vec<ProjectMember> {
        self.db
            .tracked_files()
            .into_iter()
            .map(|(path, file)| {
                let kind = file_kind_for(&path);
                ProjectMember { file, path, kind }
            })
            .collect()
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
        // Snapshot membership now (write side) so the read job interns the same
        // `Project` the latest edit produced.
        let members = self.project_members();
        let out_tx = self.out_tx.clone();
        let done_tx = self.done_tx.clone();
        let AnalyzeRequest {
            uri,
            path,
            version,
            kind,
        } = req;
        self.inflight = Some(InflightAnalyze {
            uri: uri.clone(),
            version,
        });
        self.read_spawner.spawn(move || {
            let result = salsa::Cancelled::catch(AssertUnwindSafe(|| match kind {
                FileKind::Tex | FileKind::Sty | FileKind::Cls | FileKind::Dtx | FileKind::Ins => {
                    analyze_tex(&snapshot, &path, members)
                }
                FileKind::Bib => analyze_bib(&snapshot, &path),
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

/// Compute diagnostics for a `.tex` file off the snapshot: parse diagnostics plus
/// lint-rule findings over the same salsa-cached tree + model, with cross-file
/// resolution from the `members` snapshot.
///
/// The `Project` is interned from the membership the worker captured (open buffers
/// plus lazily-read on-disk siblings); `resolved_labels` / `resolved_citations`
/// then drive `undefined-ref`, the cross-file branch of `duplicate-label`, and
/// `undefined-citation`. Their gates (closed, rooted namespace) keep a bare
/// fragment opened alone from being flagged.
fn analyze_tex(
    snapshot: &Analysis,
    path: &Path,
    members: Vec<ProjectMember>,
) -> Option<Vec<Diagnostic>> {
    let file = snapshot.lookup_file(path)?;
    let text = snapshot.file_text(file).to_owned();
    // The file's normalized identity, which keys the cross-file resolvers (it
    // equals this file's `ProjectMember::path`).
    let lint_path = snapshot.file_path(file).to_path_buf();
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
    let root = snapshot.parsed_tree(file);
    let model = snapshot.semantic_model(file);
    let (resolution, citations) = snapshot.resolve_project(members);
    for d in lint_document(&lint_path, &root, model, Some(resolution), Some(citations)) {
        diags.push(lint_to_lsp(&idx, &text, d));
    }
    Some(diags)
}

/// Compute diagnostics for a `.bib` file off the snapshot: bib parse diagnostics
/// plus bib lint-rule findings over the cached bib tree + model. The bib linter
/// has no cross-file resolution argument (no bib rule is cross-file-sensitive
/// yet).
fn analyze_bib(snapshot: &Analysis, path: &Path) -> Option<Vec<Diagnostic>> {
    let file = snapshot.lookup_file(path)?;
    let text = snapshot.file_text(file).to_owned();
    let idx = LineIndex::new(&text);
    let mut diags: Vec<Diagnostic> = snapshot
        .bib_parse_diagnostics(file)
        .iter()
        .map(|d| Diagnostic {
            range: byte_range_to_lsp(&idx, &text, d.start, d.end),
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("badness".to_owned()),
            message: d.message.clone(),
            ..Default::default()
        })
        .collect();
    let root = snapshot.parsed_bib_tree(file);
    let model = snapshot.bib_semantic_model(file);
    for d in crate::bib::linter::lint_document(path, &root, model) {
        diags.push(lint_to_lsp(&idx, &text, d));
    }
    Some(diags)
}

/// Map a linter [`crate::linter::Diagnostic`] (shared by the LaTeX and BibTeX
/// linters) onto an LSP [`Diagnostic`].
fn lint_to_lsp(idx: &LineIndex, text: &str, d: crate::linter::Diagnostic) -> Diagnostic {
    Diagnostic {
        range: byte_range_to_lsp(idx, text, d.start, d.end),
        severity: Some(severity_to_lsp(d.severity)),
        code: Some(NumberOrString::String(d.rule.to_owned())),
        source: Some("badness".to_owned()),
        message: d.message,
        ..Default::default()
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
    kind: FileKind,
    out_tx: &Sender<Outbound>,
) {
    let result = match compute_format(snapshot, path, text, style, kind) {
        Some(edit) => serde_json::to_value(vec![edit]).unwrap_or(serde_json::Value::Null),
        None => serde_json::Value::Null,
    };
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Produce the whole-document replacing edit, or `None` for a no-op / refusal /
/// unknown buffer. See [`run_format`] for the cancellation/fallback contract.
/// Routes to the LaTeX or BibTeX formatter by [`FileKind`].
fn compute_format(
    snapshot: &Analysis,
    path: &Path,
    text: &str,
    style: FormatStyle,
    kind: FileKind,
) -> Option<TextEdit> {
    // `Some(Some(s))` = formatted; `Some(None)` = clean refusal (parse/format
    // error); `None` = cache miss / stale snapshot (fall back to the captured text).
    let cached = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let file = snapshot.lookup_file(path)?;
        if snapshot.file_text(file) != text {
            return None;
        }
        match kind {
            FileKind::Tex | FileKind::Sty | FileKind::Cls | FileKind::Dtx | FileKind::Ins => {
                if !snapshot.parse_diagnostics(file).is_empty() {
                    return Some(None);
                }
                // The cached tree was already parsed with the file's flavor (the
                // salsa `parsed_document` query flavors by path), so this needs no
                // flavor. The merged signature scope folds in the file's loaded
                // local packages (those tracked as project members).
                let root = snapshot.parsed_tree(file);
                let sigs = snapshot.scope_signatures(members_of(snapshot), file);
                Some(format_node_with_signatures(&root, style, sigs).ok())
            }
            FileKind::Bib => {
                if !snapshot.bib_parse_diagnostics(file).is_empty() {
                    return Some(None);
                }
                let root = snapshot.parsed_bib_tree(file);
                Some(bib_format_node(&root, style).ok())
            }
        }
    }));

    let formatted = match cached {
        Ok(Some(opt)) => opt,
        Ok(None) | Err(_) => match kind {
            FileKind::Tex | FileKind::Sty | FileKind::Cls | FileKind::Dtx | FileKind::Ins => {
                format_with_style_flavored(text, style, kind.lex_config()).ok()
            }
            FileKind::Bib => bib_format_with_style(text, style).ok(),
        },
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
    kind: FileKind,
    out_tx: &Sender<Outbound>,
) {
    let symbols = match kind {
        FileKind::Tex | FileKind::Sty | FileKind::Cls | FileKind::Dtx | FileKind::Ins => {
            compute_symbols(snapshot, path, text)
        }
        FileKind::Bib => compute_bib_symbols(snapshot, path, text),
    };
    let result = serde_json::to_value(DocumentSymbolResponse::Nested(symbols))
        .unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Compute the LaTeX outline for `text`, preferring the snapshot's cached tree and
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

/// Compute the BibTeX outline (a flat entry list) for `text`, preferring the
/// snapshot's cached bib model and falling back to a direct reparse when it is
/// unavailable or stale.
fn compute_bib_symbols(snapshot: &Analysis, path: &Path, text: &str) -> Vec<DocumentSymbol> {
    let idx = LineIndex::new(text);
    let cached = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let file = snapshot.lookup_file(path)?;
        if snapshot.file_text(file) != text {
            return None;
        }
        Some(bib_outline(snapshot.bib_semantic_model(file)))
    }));
    let items = match cached {
        Ok(Some(items)) => items,
        // Cache miss, stale snapshot, or a cancelled read: reparse the buffer.
        Ok(None) | Err(_) => bib_outline(&BibModel::build(&bib_parse(text).syntax())),
    };
    items
        .iter()
        .map(|item| bib_to_document_symbol(item, &idx, text))
        .collect()
}

/// Compute folding ranges for a [`WorkerJob::FoldingRange`] on the read pool and
/// reply with a `Vec<FoldingRange>`. Same snapshot fast-path / reparse fallback as
/// [`run_symbols`].
fn run_folding(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &str,
    kind: FileKind,
    out_tx: &Sender<Outbound>,
) {
    let ranges = compute_folding(snapshot, path, text, kind);
    let result = serde_json::to_value(ranges).unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Compute LaTeX folding ranges for `text`, preferring the snapshot's cached tree and
/// falling back to a direct reparse when it is unavailable or stale. `.bib` files have
/// no LaTeX structure to fold (the LaTeX parser does not apply), so they yield none.
fn compute_folding(
    snapshot: &Analysis,
    path: &Path,
    text: &str,
    kind: FileKind,
) -> Vec<FoldingRange> {
    if kind == FileKind::Bib {
        return Vec::new();
    }
    let idx = LineIndex::new(text);
    let cached = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let file = snapshot.lookup_file(path)?;
        if snapshot.file_text(file) != text {
            return None;
        }
        Some(folding::folding_ranges(
            &snapshot.parsed_tree(file),
            &idx,
            text,
        ))
    }));
    match cached {
        Ok(Some(ranges)) => ranges,
        // Cache miss, stale snapshot, or a cancelled read: reparse the buffer.
        Ok(None) | Err(_) => {
            folding::folding_ranges(&SyntaxNode::new_root(parse(text).green), &idx, text)
        }
    }
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

/// Convert a flat [`BibOutlineItem`] into an LSP [`DocumentSymbol`]. Bib entries
/// have no nesting, so there are never children; the cite key is the name and the
/// entry type the detail.
#[allow(deprecated)] // `DocumentSymbol::deprecated` is a required struct field.
fn bib_to_document_symbol(item: &BibOutlineItem, idx: &LineIndex, text: &str) -> DocumentSymbol {
    let range = item.range;
    let selection = item.selection_range;
    DocumentSymbol {
        name: item.name.clone(),
        detail: Some(item.detail.clone()),
        kind: SymbolKind::CONSTANT,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(idx, text, range.start().into(), range.end().into()),
        selection_range: byte_range_to_lsp(
            idx,
            text,
            selection.start().into(),
            selection.end().into(),
        ),
        children: None,
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
    text: &str,
    position: Position,
    members: Vec<ProjectMember>,
    out_tx: &Sender<Outbound>,
) {
    // The salsa-key path is derived from the URI (the same mapping `on_completion` uses).
    let path = uri_to_path(uri);
    let items = compute_completion(snapshot, uri, &path, text, position, members);
    // `is_incomplete`: command/label/key universes are prefix-filtered server-side, so
    // the client re-queries as the typed prefix narrows (matches arity).
    let result = serde_json::to_value(CompletionResponse::List(CompletionList {
        is_incomplete: true,
        items,
    }))
    .unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Compute completion items at `position`. A `.bib` cursor goes through the bib
/// classifier; a `.tex` cursor through the LaTeX one, preferring the snapshot's cached
/// tree/queries and falling back to a direct reparse when unavailable or stale.
fn compute_completion(
    snapshot: &Analysis,
    uri: &Uri,
    path: &Path,
    text: &str,
    position: Position,
    members: Vec<ProjectMember>,
) -> Vec<CompletionItem> {
    let idx = LineIndex::new(text);
    let offset = idx.offset_at(text, position.line, position.character);

    if file_kind_for(path) == FileKind::Bib {
        return compute_bib_completion(text, offset);
    }
    compute_tex_completion(snapshot, uri, path, text, offset, members)
}

/// Bib completion: a fresh parse + model (sub-ms, and there is no cached bib tree
/// query) drives the bib classifier and candidate builder.
fn compute_bib_completion(text: &str, offset: usize) -> Vec<CompletionItem> {
    let root = bib_parse(text).syntax();
    let ctx = classify_bib_context(&root, offset);
    let model = BibModel::build(&root);
    bib_candidates(&ctx, &model)
        .into_iter()
        .map(bib_candidate_to_item)
        .collect()
}

/// The outcome of classifying a `.tex` cursor: either ready-to-send pure items, or a
/// cite-key context whose candidates need the cross-file bibliography (resolved
/// against the snapshot, like a file-path read).
enum TexCompletion {
    Items(Vec<CompletionItem>),
    Cite { prefix: String, lint_path: PathBuf },
}

/// LaTeX completion, mirroring go-to-def's cached-or-reparse-then-resolve shape: the
/// pure (command/env/label/file-path) contexts resolve immediately; a `\cite` context
/// defers to [`cite_completion_items`] against the project bibliography.
fn compute_tex_completion(
    snapshot: &Analysis,
    uri: &Uri,
    path: &Path,
    text: &str,
    offset: usize,
    members: Vec<ProjectMember>,
) -> Vec<CompletionItem> {
    // Classify off the cached tree when current; reparse on stale/miss. A cancelled
    // read also falls back to a reparse (`unwrap_or_else`) — neither touches `members`.
    let resolved = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        if let Some(file) = snapshot.lookup_file(path)
            && snapshot.file_text(file) == text
        {
            let root = snapshot.parsed_tree(file);
            let ctx = crate::completion::classify_context(&root, offset);
            return match ctx {
                CompletionContext::CitationKey { prefix } => TexCompletion::Cite {
                    prefix,
                    lint_path: snapshot.file_path(file).to_path_buf(),
                },
                _ => TexCompletion::Items(build_completion_items(
                    &ctx,
                    // The merged scope folds in loaded local packages' macros; the
                    // `members` clone leaves the original for the cite branch below.
                    snapshot.scope_signatures(members.clone(), file),
                    snapshot.semantic_model(file),
                    uri,
                )),
            };
        }
        reparse_tex_completion(text, offset, uri, path)
    }))
    .unwrap_or_else(|_| reparse_tex_completion(text, offset, uri, path));

    match resolved {
        TexCompletion::Items(items) => items,
        TexCompletion::Cite { prefix, lint_path } => {
            // Cross-file resolve against the db snapshot; a racing write yields none.
            salsa::Cancelled::catch(AssertUnwindSafe(|| {
                let (_, citations) = snapshot.resolve_project(members);
                cite_completion_items(snapshot, citations, &lint_path, &prefix)
            }))
            .unwrap_or_default()
        }
    }
}

/// Classify a `.tex` cursor off a fresh parse (the snapshot-free fallback). For a
/// `\cite` context this still defers resolution to the snapshot, keying off `path`.
fn reparse_tex_completion(text: &str, offset: usize, uri: &Uri, path: &Path) -> TexCompletion {
    let root = SyntaxNode::new_root(parse(text).green);
    let ctx = crate::completion::classify_context(&root, offset);
    match ctx {
        CompletionContext::CitationKey { prefix } => TexCompletion::Cite {
            prefix,
            lint_path: path.to_path_buf(),
        },
        _ => {
            let sigs = crate::semantic::scan_definitions(&root);
            let model = SemanticModel::build(&root);
            TexCompletion::Items(build_completion_items(&ctx, &sigs, &model, uri))
        }
    }
}

/// Cite-key candidates: every entry key in the citing file's bibliography namespace,
/// prefix-filtered (case-insensitive, as BibTeX folds key case) and deduped. Mirrors
/// [`resolve_citation_locations`] but collects all keys rather than matching a target.
fn cite_completion_items(
    snapshot: &Analysis,
    citations: &ResolvedCitations,
    lint_path: &Path,
    prefix: &str,
) -> Vec<CompletionItem> {
    let prefix = prefix.to_lowercase();
    let mut keys: Vec<SmolStr> = Vec::new();
    for bib_path in citations.bib_definers(lint_path) {
        let Some(file) = snapshot.lookup_file(bib_path) else {
            continue;
        };
        for entry in snapshot.bib_semantic_model(file).entries() {
            if entry.key.to_lowercase().starts_with(&prefix) {
                keys.push(entry.key.clone());
            }
        }
    }
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .map(|key| CompletionItem {
            label: key.to_string(),
            kind: Some(CompletionItemKind::REFERENCE),
            ..Default::default()
        })
        .collect()
}

/// Map a neutral [`BibCompletionCandidate`] onto an `lsp_types::CompletionItem`.
fn bib_candidate_to_item(candidate: BibCompletionCandidate) -> CompletionItem {
    let kind = match candidate.kind {
        BibCandidateKind::EntryType => CompletionItemKind::STRUCT,
        BibCandidateKind::FieldName => CompletionItemKind::FIELD,
        BibCandidateKind::StringMacro => CompletionItemKind::CONSTANT,
    };
    CompletionItem {
        label: candidate.label,
        kind: Some(kind),
        ..Default::default()
    }
}

/// Resolve the `\ref`/`\cite` under the cursor and reply with the matching
/// definition [`Location`]s (always an array — empty when nothing resolves).
fn run_goto_definition(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &str,
    position: Position,
    members: Vec<ProjectMember>,
    out_tx: &Sender<Outbound>,
) {
    let locations = compute_goto_definition(snapshot, path, text, position, members);
    let result = serde_json::to_value(GotoDefinitionResponse::Array(locations))
        .unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Resolve the label/key under the cursor and reply with every use [`Location`]
/// across its namespace (always an array — empty when nothing resolves).
#[allow(clippy::too_many_arguments)]
fn run_references(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &str,
    position: Position,
    members: Vec<ProjectMember>,
    include_declaration: bool,
    out_tx: &Sender<Outbound>,
) {
    let locations =
        compute_references(snapshot, path, text, position, members, include_declaration);
    let result = serde_json::to_value(locations).unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// What the cursor points at inside a `.tex` buffer: the keys whose command range
/// covers the offset. Refs and citations are kept distinct so each resolves against
/// its own namespace (labels vs. bibliography). A multi-key list command
/// (`\cref{a,b}`, `\cite{a,b}`) shares one range, so every key at that offset is
/// returned and resolved — per-key sub-ranges are deferred (see
/// [`crate::semantic::label::LabelRef::range`]).
#[derive(Debug)]
enum CursorTarget {
    Labels(Vec<SmolStr>),
    Citations(Vec<SmolStr>),
}

/// Compute the definition locations for a go-to-definition at `position`, preferring
/// the snapshot's cached model and falling back to a fresh parse when it is stale or
/// uncached. Cross-file resolution always runs against the db snapshot's resolvers
/// (`resolved_labels`/`resolved_citations`), interned from `members`.
fn compute_goto_definition(
    snapshot: &Analysis,
    path: &Path,
    text: &str,
    position: Position,
    members: Vec<ProjectMember>,
) -> Vec<Location> {
    let idx = LineIndex::new(text);
    let offset = idx.offset_at(text, position.line, position.character);

    let cached = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        // Find the reference under the cursor (off the cached model when current,
        // else a fresh parse), then resolve cross-file against the db snapshot.
        let (target, lint_path) = match snapshot.lookup_file(path) {
            Some(file) if snapshot.file_text(file) == text => (
                reference_under_cursor(snapshot.semantic_model(file), offset),
                snapshot.file_path(file).to_path_buf(),
            ),
            _ => {
                let root = SyntaxNode::new_root(parse(text).green);
                let model = SemanticModel::build(&root);
                (reference_under_cursor(&model, offset), path.to_path_buf())
            }
        };
        let Some(target) = target else {
            return Vec::new();
        };
        let (resolution, citations) = snapshot.resolve_project(members);
        match target {
            CursorTarget::Labels(names) => {
                resolve_label_locations(snapshot, resolution, &lint_path, &names)
            }
            CursorTarget::Citations(names) => {
                resolve_citation_locations(snapshot, citations, &lint_path, &names)
            }
        }
    }));
    cached.unwrap_or_default()
}

/// The cite/ref keys whose command range covers `offset`, refs taking precedence
/// (a position is never both). Returns owned keys so the borrowed model can drop.
fn reference_under_cursor(model: &SemanticModel, offset: usize) -> Option<CursorTarget> {
    let at = TextSize::new(offset as u32);
    let label_names: Vec<SmolStr> = model
        .refs()
        .iter()
        .filter(|r| r.range.contains_inclusive(at))
        .map(|r| r.name.clone())
        .collect();
    if !label_names.is_empty() {
        return Some(CursorTarget::Labels(label_names));
    }
    let cite_names: Vec<SmolStr> = model
        .citations()
        .iter()
        .filter(|c| c.range.contains_inclusive(at))
        .map(|c| c.name.clone())
        .collect();
    (!cite_names.is_empty()).then_some(CursorTarget::Citations(cite_names))
}

/// Compute every use location for a find-references at `position`. The inverse of
/// [`compute_goto_definition`]: resolves a label/key (from a `\ref`/`\cite` use,
/// a `\label` definition, or — in a `.bib` buffer — an `@entry` key) to all of its
/// `\ref`/`\cite` use sites across the namespace. The cursor's own buffer is read
/// off the cached tree when current, else a fresh parse. `include_declaration`
/// appends the `\label`/`@entry` definition to the results.
fn compute_references(
    snapshot: &Analysis,
    path: &Path,
    text: &str,
    position: Position,
    members: Vec<ProjectMember>,
    include_declaration: bool,
) -> Vec<Location> {
    let idx = LineIndex::new(text);
    let offset = idx.offset_at(text, position.line, position.character);

    let computed = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let (resolution, citations) = snapshot.resolve_project(members);

        // `.bib` origin: the `@entry` key under the cursor → its `\cite` uses. A
        // `.bib` path is not keyed in the citation `component_of`, so resolution
        // goes through `bib_citers`.
        if file_kind_for(path) == FileKind::Bib {
            let Some((key, key_range)) = bib_entry_under_cursor(snapshot, path, text, offset)
            else {
                return Vec::new();
            };
            let origin = snapshot
                .lookup_file(path)
                .map(|file| snapshot.file_path(file).to_path_buf())
                .unwrap_or_else(|| path.to_path_buf());
            let decl = if include_declaration {
                location_for(&origin, &idx, text, key_range)
            } else {
                None
            };
            return reference_citation_locations(
                snapshot,
                citations,
                &origin,
                FileKind::Bib,
                &[key],
                include_declaration,
                decl,
            );
        }

        // `.tex` origin: a `\ref`/`\cite` use *or* a `\label` definition.
        let (target, origin) = match snapshot.lookup_file(path) {
            Some(file) if snapshot.file_text(file) == text => (
                references_target_under_cursor(snapshot.semantic_model(file), offset),
                snapshot.file_path(file).to_path_buf(),
            ),
            _ => {
                let root = SyntaxNode::new_root(parse(text).green);
                let model = SemanticModel::build(&root);
                (
                    references_target_under_cursor(&model, offset),
                    path.to_path_buf(),
                )
            }
        };
        let Some(target) = target else {
            return Vec::new();
        };
        match target {
            CursorTarget::Labels(names) => reference_label_locations(
                snapshot,
                resolution,
                &origin,
                &names,
                include_declaration,
            ),
            CursorTarget::Citations(names) => reference_citation_locations(
                snapshot,
                citations,
                &origin,
                FileKind::Tex,
                &names,
                include_declaration,
                None,
            ),
        }
    }));
    computed.unwrap_or_default()
}

/// Like [`reference_under_cursor`] but also recognizes a `\label` *definition*
/// under the cursor, so find-references can be invoked from the definition site
/// (a `\ref` and a `\label` both resolve to the same label name). Precedence
/// matches [`reference_under_cursor`] (refs, then citations), with label defs
/// slotted last; a position is in at most one of the three.
fn references_target_under_cursor(model: &SemanticModel, offset: usize) -> Option<CursorTarget> {
    if let Some(target) = reference_under_cursor(model, offset) {
        return Some(target);
    }
    let at = TextSize::new(offset as u32);
    let label_names: Vec<SmolStr> = model
        .labels()
        .iter()
        .filter(|l| l.range.contains_inclusive(at))
        .map(|l| l.name.clone())
        .collect();
    (!label_names.is_empty()).then_some(CursorTarget::Labels(label_names))
}

/// The cite key of the `@entry` whose key range covers `offset` in a `.bib`
/// buffer, with that key's byte range. Reads the cached model when current, else a
/// fresh bib parse (the bib analog of [`compute_references`]'s `.tex` guard).
fn bib_entry_under_cursor(
    snapshot: &Analysis,
    path: &Path,
    text: &str,
    offset: usize,
) -> Option<(SmolStr, TextRange)> {
    let at = TextSize::new(offset as u32);
    let find = |model: &BibModel| {
        model
            .entries()
            .iter()
            .find(|e| e.key_range.contains_inclusive(at))
            .map(|e| (e.key.clone(), e.key_range))
    };
    match snapshot.lookup_file(path) {
        Some(file) if snapshot.file_text(file) == text => find(snapshot.bib_semantic_model(file)),
        _ => find(&BibModel::build(&bib_parse(text).syntax())),
    }
}

/// Every `\ref`-family use of `names` across `origin`'s label namespace, plus the
/// `\label` definitions when `include_declaration`. The inverse of
/// [`resolve_label_locations`]: scans each namespace member's uses, not its defs.
fn reference_label_locations(
    snapshot: &Analysis,
    resolution: &ResolvedLabels,
    origin: &Path,
    names: &[SmolStr],
    include_declaration: bool,
) -> Vec<Location> {
    let mut locations = Vec::new();
    for member in resolution.namespace_members(origin) {
        let Some(file) = snapshot.lookup_file(member) else {
            continue;
        };
        let text = snapshot.file_text(file);
        let idx = LineIndex::new(text);
        let model = snapshot.semantic_model(file);
        for r in model.refs() {
            if names.contains(&r.name) {
                locations.push(location_for(member, &idx, text, r.range));
            }
        }
        if include_declaration {
            for label in model.labels() {
                if names.contains(&label.name) {
                    locations.push(location_for(member, &idx, text, label.range));
                }
            }
        }
    }
    dedup_locations(locations)
}

/// Every `\cite`-family use of `names` across `origin`'s citation namespace, plus
/// the bibliography `@entry` definitions when `include_declaration`. Use sites
/// live in `.tex` members — `bib_citers` for a `.bib` origin (whose path is not
/// keyed in the citation `component_of`), else `namespace_members`. The
/// declaration is the cursor's own entry (`decl_for_bib`) for a `.bib` origin, or
/// [`resolve_citation_locations`] for a `.tex` origin.
#[allow(clippy::too_many_arguments)]
fn reference_citation_locations(
    snapshot: &Analysis,
    citations: &ResolvedCitations,
    origin: &Path,
    kind: FileKind,
    names: &[SmolStr],
    include_declaration: bool,
    decl_for_bib: Option<Location>,
) -> Vec<Location> {
    let members = if kind == FileKind::Bib {
        citations.bib_citers(origin)
    } else {
        citations.namespace_members(origin)
    };
    let mut locations = Vec::new();
    for member in members {
        let Some(file) = snapshot.lookup_file(member) else {
            continue;
        };
        let text = snapshot.file_text(file);
        let idx = LineIndex::new(text);
        for c in snapshot.semantic_model(file).citations() {
            if names.iter().any(|n| n.eq_ignore_ascii_case(&c.name)) {
                locations.push(location_for(member, &idx, text, c.range));
            }
        }
    }
    let mut locations = dedup_locations(locations);
    if include_declaration {
        match kind {
            FileKind::Bib => locations.extend(decl_for_bib),
            _ => locations.extend(resolve_citation_locations(
                snapshot, citations, origin, names,
            )),
        }
    }
    locations
}

/// For each `\ref` key, the `\label{key}` definition sites across the file's
/// namespace: `resolution.definers` gives the defining files, each file's
/// `semantic_model` the matching `LabelDef.range`.
fn resolve_label_locations(
    snapshot: &Analysis,
    resolution: &ResolvedLabels,
    lint_path: &Path,
    names: &[SmolStr],
) -> Vec<Location> {
    let mut locations = Vec::new();
    for name in names {
        for def_path in resolution.definers(lint_path, name) {
            let Some(file) = snapshot.lookup_file(def_path) else {
                continue;
            };
            let text = snapshot.file_text(file);
            let idx = LineIndex::new(text);
            for label in snapshot.semantic_model(file).labels() {
                if &label.name == name {
                    locations.push(location_for(def_path, &idx, text, label.range));
                }
            }
        }
    }
    dedup_locations(locations)
}

/// For each `\cite` key, the `@entry{key,…}` sites in the `.bib` files of the
/// citation namespace: `citations.bib_definers` gives the analyzed bibliographies,
/// each `bib_semantic_model` the matching `Entry.key_range` (case-insensitive, as
/// BibTeX folds key case).
fn resolve_citation_locations(
    snapshot: &Analysis,
    citations: &ResolvedCitations,
    lint_path: &Path,
    names: &[SmolStr],
) -> Vec<Location> {
    let mut locations = Vec::new();
    for bib_path in citations.bib_definers(lint_path) {
        let Some(file) = snapshot.lookup_file(bib_path) else {
            continue;
        };
        let text = snapshot.file_text(file);
        let idx = LineIndex::new(text);
        for entry in snapshot.bib_semantic_model(file).entries() {
            if names.iter().any(|n| n.eq_ignore_ascii_case(&entry.key)) {
                locations.push(location_for(bib_path, &idx, text, entry.key_range));
            }
        }
    }
    dedup_locations(locations)
}

/// Build an LSP [`Location`] from a definer file's path and a byte range in its
/// text. A path that cannot form a `file://` URI yields `None` (skipped).
fn location_for(path: &Path, idx: &LineIndex, text: &str, range: TextRange) -> Option<Location> {
    Some(Location {
        uri: path_to_uri(path)?,
        range: byte_range_to_lsp(
            idx,
            text,
            usize::from(range.start()),
            usize::from(range.end()),
        ),
    })
}

/// Drop duplicate locations (same URI + range), which can arise when several keys
/// in a list command resolve to the same site.
fn dedup_locations(locations: Vec<Option<Location>>) -> Vec<Location> {
    let mut seen = HashSet::new();
    locations
        .into_iter()
        .flatten()
        .filter(|loc| seen.insert((loc.uri.as_str().to_owned(), loc.range.start, loc.range.end)))
        .collect()
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

/// Build a `file://` URI from a filesystem path — the inverse of [`uri_to_fs_path`],
/// for the `Location`s a go-to-definition reply carries. Normalizes separators to
/// `/`, ensures a leading `/` (so a Windows `C:\dir` becomes `file:///C:/dir`), and
/// percent-encodes path bytes that are not URI path characters (spaces, etc.).
/// Returns `None` if the result still does not parse, so a stray path is skipped
/// rather than crashing the read job.
fn path_to_uri(path: &Path) -> Option<Uri> {
    let mut s = path.display().to_string().replace('\\', "/");
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    format!("file://{}", percent_encode_path(&s)).parse().ok()
}

/// Percent-encode a filesystem path for use in a `file://` URI, leaving the path
/// structure (`/`), a Windows drive colon (`:`), and the URI-unreserved set
/// (`A–Z a–z 0–9 - . _ ~`) intact and escaping everything else (e.g. a space →
/// `%20`). The dual of [`percent_decode`].
fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/' | b':') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(
                char::from_digit((b >> 4) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
            out.push(
                char::from_digit((b & 0xf) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
        }
    }
    out
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

    /// The byte offset of the first occurrence of `needle` in `text`.
    fn offset_of(text: &str, needle: &str) -> usize {
        text.find(needle).expect("needle present")
    }

    #[test]
    fn reference_under_cursor_finds_ref_and_cite() {
        let text = "\\label{a}\n\\ref{a}\n\\cite{k}\n";
        let model = SemanticModel::build(&SyntaxNode::new_root(parse(text).green));

        // Inside `\ref{a}` → the label key `a`.
        let at_ref = offset_of(text, "\\ref{a}") + 5; // on the `a`
        match reference_under_cursor(&model, at_ref) {
            Some(CursorTarget::Labels(names)) => assert_eq!(names, vec![SmolStr::new("a")]),
            other => panic!("expected a label target, got {other:?}"),
        }

        // Inside `\cite{k}` → the cite key `k`.
        let at_cite = offset_of(text, "\\cite{k}") + 6; // on the `k`
        match reference_under_cursor(&model, at_cite) {
            Some(CursorTarget::Citations(names)) => assert_eq!(names, vec![SmolStr::new("k")]),
            other => panic!("expected a citation target, got {other:?}"),
        }

        // On the `\label` definition (not a reference) → nothing to jump *from*.
        let at_label = offset_of(text, "\\label{a}") + 1;
        assert!(reference_under_cursor(&model, at_label).is_none());
    }

    #[test]
    fn reference_under_cursor_splits_cref_list() {
        let text = "\\cref{a,b,c}\n";
        let model = SemanticModel::build(&SyntaxNode::new_root(parse(text).green));
        // The whole command shares one range, so every key is returned (per-key
        // sub-ranges are deferred).
        let at = offset_of(text, "\\cref") + 2;
        match reference_under_cursor(&model, at) {
            Some(CursorTarget::Labels(names)) => assert_eq!(
                names,
                vec![SmolStr::new("a"), SmolStr::new("b"), SmolStr::new("c")]
            ),
            other => panic!("expected a label target, got {other:?}"),
        }
    }

    #[test]
    fn path_to_uri_round_trips_through_uri_to_fs_path() {
        let p = PathBuf::from("/tmp/my dir/main.tex");
        let u = path_to_uri(&p).expect("a file path forms a URI");
        // The space is percent-encoded in the URI text…
        assert!(u.as_str().contains("%20"), "got {}", u.as_str());
        // …and decodes back to the original filesystem path.
        assert_eq!(uri_to_fs_path(&u), Some(p));
    }
}
