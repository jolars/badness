//! A minimal language server for badness (Phase 4).
//!
//! Deliberately diverges from ravel: badness uses **`lsp-server` + `lsp-types`**
//! (rust-analyzer's synchronous stack), *not* tower-lsp-server — see the LSP note
//! in `AGENTS.md`. salsa's single-writer / snapshot-readers model composes cleanly
//! with `lsp-server`'s sync main loop.
//!
//! Scope is intentionally thin: full-document **formatting** and pushed parser
//! **diagnostics**, nothing else. Rich features (hover, completion, go-to-def,
//! symbols, range formatting) are deferred to Phase 7.
//!
//! ## Design
//!
//! - **Single-threaded.** One blocking loop owns one [`IncrementalDatabase`] by
//!   value and answers every request inline. A whole-file `.tex` reparse is sub-ms
//!   (AGENTS.md decision #6), so the ra-style writer/threadpool split and
//!   `salsa::Cancelled` cancellation are deferred to Phase 7.
//! - **Salsa-backed.** Edits land via [`IncrementalDatabase::upsert_file`] and
//!   diagnostics ride the cached `parse_diagnostics` query, honoring the
//!   "salsa-first" tenet. (Formatting still calls [`format_with_style`], which
//!   reparses internally — there is no `format_node(tree)` entry yet.)
//! - **URI as the salsa key.** We key the salsa file cache by the document's URI
//!   string (`PathBuf::from(uri.as_str())`), sidestepping URI↔filesystem-path
//!   conversion. Document text always comes from `didOpen`/`didChange`, never from
//!   disk; real path resolution (for cross-file `\input`) is a Phase 7 concern.

use std::path::PathBuf;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{Formatting, Request as _};
use lsp_types::{
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentFormattingParams, OneOf, Position, PublishDiagnosticsParams,
    Range, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Uri,
};

use crate::formatter::{FormatStyle, format_with_style};
use crate::incremental::IncrementalDatabase;
use crate::text::LineIndex;

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

/// Advertise just what the MVP supports: full-text sync + whole-document
/// formatting. Diagnostics are *pushed* via `publishDiagnostics`, which needs no
/// capability flag.
fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        document_formatting_provider: Some(OneOf::Left(true)),
        ..Default::default()
    }
}

/// The blocking message loop. Holds the single salsa database; dispatches each
/// request/notification inline.
fn main_loop(connection: Connection, _init_params: serde_json::Value) -> Result<(), DynError> {
    let mut db = IncrementalDatabase::default();
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                // `handle_shutdown` answers the `shutdown` request and waits for
                // the following `exit` notification, then returns `true`.
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                match req.method.as_str() {
                    Formatting::METHOD => on_formatting(&connection, &db, req),
                    _ => respond_unhandled(&connection, req),
                }
            }
            Message::Notification(not) => match not.method.as_str() {
                DidOpenTextDocument::METHOD => on_did_open(&connection, &mut db, not),
                DidChangeTextDocument::METHOD => on_did_change(&connection, &mut db, not),
                DidCloseTextDocument::METHOD => on_did_close(&connection, not),
                _ => {}
            },
            // The MVP issues no client-bound requests, so any response is unexpected.
            Message::Response(_) => {}
        }
    }
    Ok(())
}

/// `didOpen`: record the buffer and publish its parse diagnostics.
fn on_did_open(connection: &Connection, db: &mut IncrementalDatabase, not: Notification) {
    let Ok(params) = not.extract::<DidOpenTextDocumentParams>(DidOpenTextDocument::METHOD) else {
        return;
    };
    let doc = params.text_document;
    db.upsert_file(&PathBuf::from(doc.uri.as_str()), doc.text);
    publish_diagnostics(connection, db, doc.uri, Some(doc.version));
}

/// `didChange`: full-text sync, so the last content change carries the whole new
/// buffer. Re-upsert and re-publish.
fn on_did_change(connection: &Connection, db: &mut IncrementalDatabase, not: Notification) {
    let Ok(mut params) = not.extract::<DidChangeTextDocumentParams>(DidChangeTextDocument::METHOD)
    else {
        return;
    };
    let Some(change) = params.content_changes.pop() else {
        return;
    };
    let uri = params.text_document.uri;
    db.upsert_file(&PathBuf::from(uri.as_str()), change.text);
    publish_diagnostics(connection, db, uri, Some(params.text_document.version));
}

/// `didClose`: clear the document's diagnostics so stale squiggles disappear. We
/// keep the salsa entry cached (no eviction API yet); it is memory-only.
fn on_did_close(connection: &Connection, not: Notification) {
    let Ok(params) = not.extract::<DidCloseTextDocumentParams>(DidCloseTextDocument::METHOD) else {
        return;
    };
    send_diagnostics(connection, params.text_document.uri, Vec::new(), None);
}

/// `textDocument/formatting`: reformat the whole document and reply with a single
/// replacing edit. Replies `null` when the document is unknown or the formatter
/// refuses (parse errors / unsupported constructs).
fn on_formatting(connection: &Connection, db: &IncrementalDatabase, req: Request) {
    let id = req.id.clone();
    let (id, params) = match req.extract::<DocumentFormattingParams>(Formatting::METHOD) {
        Ok(pair) => pair,
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

    let result = format_document(db, &params).unwrap_or(serde_json::Value::Null);
    let _ = connection
        .sender
        .send(Message::Response(Response::new_ok(id, result)));
}

/// Format the buffer behind `params`, returning the JSON edit array, or `None`
/// when there is nothing to send (unknown document, no-op, or a format error).
fn format_document(
    db: &IncrementalDatabase,
    params: &DocumentFormattingParams,
) -> Option<serde_json::Value> {
    let path = PathBuf::from(params.text_document.uri.as_str());
    let file = db.lookup_file(&path)?;
    let text = db.file_text(file).to_owned();

    let mut style = FormatStyle::default();
    if params.options.tab_size > 0 {
        style.indent_width = params.options.tab_size as usize;
    }

    let formatted = format_with_style(&text, style).ok()?;
    if formatted == text {
        return None;
    }

    let idx = LineIndex::new(&text);
    let (end_line, end_col) = idx.utf16_position(&text, text.len());
    let edit = TextEdit {
        range: Range {
            start: Position::new(0, 0),
            end: Position::new(end_line, end_col),
        },
        new_text: formatted,
    };
    serde_json::to_value(vec![edit]).ok()
}

/// Build and push the parse diagnostics for the buffer currently tracked under
/// `uri`.
fn publish_diagnostics(
    connection: &Connection,
    db: &IncrementalDatabase,
    uri: Uri,
    version: Option<i32>,
) {
    let path = PathBuf::from(uri.as_str());
    let Some(file) = db.lookup_file(&path) else {
        return;
    };
    let text = db.file_text(file).to_owned();
    let idx = LineIndex::new(&text);
    let diagnostics = db
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
    send_diagnostics(connection, uri, diagnostics, version);
}

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
