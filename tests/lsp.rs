//! End-to-end smoke test for the minimal LSP server (Phase 4).
//!
//! Drives the full transcript over an in-process `Connection::memory()` pair:
//! `initialize` → `initialized` → `didOpen` (a doc with a parse error) →
//! assert pushed diagnostics → `didChange` (to a valid, messy doc) → assert the
//! diagnostics clear → `textDocument/formatting` → assert the edit equals the
//! formatter's own output → `shutdown` → `exit`.

// `lsp_types::Uri` carries an internal `Cell` that trips `clippy::mutable_key_type`
// when used as a map key. Our URIs are owned + parsed and hash through `as_str()`,
// so this is sound — allow it test-wide, as `src/lsp.rs` does module-wide.
#![allow(clippy::mutable_key_type)]

use std::time::Duration;

use badness::formatter::{FormatStyle, format_with_style};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    ApplyWorkspaceEditParams, ClientCapabilities, CodeActionContext, CodeActionOrCommand,
    CodeActionParams, CodeActionProviderCapability, CompletionItem, CompletionItemKind,
    CompletionParams, CompletionResponse, DiagnosticClientCapabilities,
    DiagnosticWorkspaceClientCapabilities, DidChangeTextDocumentParams,
    DidChangeWatchedFilesClientCapabilities, DidChangeWatchedFilesParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentDiagnosticParams,
    DocumentDiagnosticReport, DocumentDiagnosticReportResult, DocumentFormattingParams,
    DocumentHighlight, DocumentHighlightKind, DocumentHighlightParams, DocumentLink,
    DocumentLinkOptions, DocumentLinkParams, DocumentOnTypeFormattingOptions,
    DocumentOnTypeFormattingParams, DocumentRangeFormattingParams, DocumentSymbol,
    DocumentSymbolParams, DocumentSymbolResponse, ExecuteCommandParams, FileChangeType, FileEvent,
    FoldingRange, FoldingRangeKind, FoldingRangeParams, FoldingRangeProviderCapability,
    FormattingOptions, GeneralClientCapabilities, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverContents, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InitializedParams, InsertTextFormat, Location, NumberOrString, OneOf, PartialResultParams,
    Position, PositionEncodingKind, PrepareRenameResponse, PublishDiagnosticsParams, Range,
    ReferenceContext, ReferenceParams, RegistrationParams, RenameOptions, RenameParams,
    SignatureHelp, SignatureHelpParams, SymbolKind, TextDocumentClientCapabilities,
    TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, TextEdit, Uri, VersionedTextDocumentIdentifier,
    WorkDoneProgressParams, WorkspaceClientCapabilities, WorkspaceEdit, WorkspaceSymbolParams,
    WorkspaceSymbolResponse,
};

/// Build a valid `file://` URI from a filesystem path, cross-platform. A raw
/// path can't be string-formatted into a URI directly: on Windows it uses
/// backslashes and a drive-letter colon (`C:\dir`), which is not valid URI
/// syntax. Normalize separators to `/` and ensure a leading `/` so a drive
/// path becomes `file:///C:/dir` (matching the server's `uri_to_fs_path`).
fn path_to_file_uri(path: &std::path::Path) -> Uri {
    let mut s = path.display().to_string().replace('\\', "/");
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    format!("file://{s}")
        .parse()
        .expect("path should form a valid file:// URI")
}

fn recv(client: &Connection) -> Message {
    client
        .receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("timed out waiting for a server message")
}

fn recv_response(client: &Connection) -> Response {
    match recv(client) {
        Message::Response(resp) => resp,
        other => panic!("expected a response, got {other:?}"),
    }
}

fn recv_diagnostics(client: &Connection) -> PublishDiagnosticsParams {
    match recv(client) {
        Message::Notification(not) if not.method == "textDocument/publishDiagnostics" => {
            serde_json::from_value(not.params).expect("valid PublishDiagnosticsParams")
        }
        other => panic!("expected publishDiagnostics, got {other:?}"),
    }
}

fn send_request(client: &Connection, id: i32, method: &str, params: serde_json::Value) {
    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(id),
            method: method.to_owned(),
            params,
        }))
        .unwrap();
}

fn send_notification(client: &Connection, method: &str, params: serde_json::Value) {
    client
        .sender
        .send(Message::Notification(Notification {
            method: method.to_owned(),
            params,
        }))
        .unwrap();
}

/// Spawn an in-process server, perform the `initialize`/`initialized` handshake
/// (passing `init_options` as `initializationOptions`), and return the client end
/// plus the server thread handle.
fn start_server(
    init_options: Option<serde_json::Value>,
) -> (Connection, std::thread::JoinHandle<()>) {
    let (server, client) = Connection::memory();
    let server_thread = std::thread::spawn(move || badness::lsp::serve(server).unwrap());

    let params = InitializeParams {
        initialization_options: init_options,
        ..Default::default()
    };
    send_request(
        &client,
        1,
        "initialize",
        serde_json::to_value(params).unwrap(),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(1));
    let init: InitializeResult = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert!(
        init.capabilities.document_formatting_provider.is_some(),
        "server must advertise documentFormattingProvider"
    );
    assert!(
        init.capabilities
            .document_range_formatting_provider
            .is_some(),
        "server must advertise documentRangeFormattingProvider"
    );
    assert!(
        matches!(
            init.capabilities.document_on_type_formatting_provider,
            Some(DocumentOnTypeFormattingOptions { ref first_trigger_character, .. })
                if first_trigger_character == "}"
        ),
        "server must advertise documentOnTypeFormattingProvider triggered by `}}`"
    );
    assert!(
        matches!(
            init.capabilities.document_symbol_provider,
            Some(OneOf::Left(true))
        ),
        "server must advertise documentSymbolProvider"
    );
    assert!(
        matches!(
            init.capabilities.workspace_symbol_provider,
            Some(OneOf::Left(true))
        ),
        "server must advertise workspaceSymbolProvider"
    );
    assert!(
        init.capabilities.completion_provider.is_some(),
        "server must advertise completionProvider"
    );
    assert!(
        matches!(
            init.capabilities.definition_provider,
            Some(OneOf::Left(true))
        ),
        "server must advertise definitionProvider"
    );
    assert!(
        matches!(
            init.capabilities.references_provider,
            Some(OneOf::Left(true))
        ),
        "server must advertise referencesProvider"
    );
    assert!(
        matches!(
            init.capabilities.rename_provider,
            Some(OneOf::Right(RenameOptions {
                prepare_provider: Some(true),
                ..
            }))
        ),
        "server must advertise renameProvider with prepare support"
    );
    assert!(
        matches!(
            init.capabilities.folding_range_provider,
            Some(FoldingRangeProviderCapability::Simple(true))
        ),
        "server must advertise foldingRangeProvider"
    );
    assert!(
        matches!(
            init.capabilities.document_link_provider,
            Some(DocumentLinkOptions {
                resolve_provider: Some(false),
                ..
            })
        ),
        "server must advertise documentLinkProvider"
    );
    assert!(
        matches!(
            init.capabilities.hover_provider,
            Some(HoverProviderCapability::Simple(true))
        ),
        "server must advertise hoverProvider"
    );
    assert!(
        init.capabilities
            .signature_help_provider
            .as_ref()
            .and_then(|opts| opts.trigger_characters.as_deref())
            == Some(&["{".to_owned(), "[".to_owned()][..]),
        "server must advertise signatureHelpProvider triggered by `{{` and `[`"
    );
    assert!(
        matches!(
            init.capabilities.code_action_provider,
            Some(CodeActionProviderCapability::Simple(true))
        ),
        "server must advertise codeActionProvider"
    );
    assert!(
        init.capabilities
            .execute_command_provider
            .as_ref()
            .is_some_and(|opts| {
                opts.commands
                    .contains(&"badness.changeEnvironment".to_owned())
                    && opts
                        .commands
                        .contains(&"texlab.changeEnvironment".to_owned())
            }),
        "server must advertise the changeEnvironment execute-commands"
    );
    assert!(init.capabilities.text_document_sync.is_some());
    assert!(
        init.capabilities.diagnostic_provider.is_none(),
        "diagnosticProvider is gated on client pull support, which this client lacks"
    );
    assert_eq!(
        init.capabilities.position_encoding,
        Some(PositionEncodingKind::UTF16),
        "a client offering no positionEncodings gets the mandatory UTF-16 default"
    );
    send_notification(
        &client,
        "initialized",
        serde_json::to_value(InitializedParams {}).unwrap(),
    );
    (client, server_thread)
}

/// Spawn an in-process server and handshake as a **pull-capable** client (advertises
/// `textDocument/diagnostic` and `workspace.diagnostic.refreshSupport`). Such a
/// client is served diagnostics pull-only — the server suppresses `publishDiagnostics`.
fn start_server_pull() -> (Connection, std::thread::JoinHandle<()>) {
    let (server, client) = Connection::memory();
    let server_thread = std::thread::spawn(move || badness::lsp::serve(server).unwrap());

    let params = InitializeParams {
        capabilities: ClientCapabilities {
            text_document: Some(TextDocumentClientCapabilities {
                diagnostic: Some(DiagnosticClientCapabilities::default()),
                ..Default::default()
            }),
            workspace: Some(WorkspaceClientCapabilities {
                diagnostic: Some(DiagnosticWorkspaceClientCapabilities {
                    refresh_support: Some(true),
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    send_request(
        &client,
        1,
        "initialize",
        serde_json::to_value(params).unwrap(),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(1));
    let init: InitializeResult = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert!(
        init.capabilities.diagnostic_provider.is_some(),
        "server must advertise diagnosticProvider (pull diagnostics)"
    );
    send_notification(
        &client,
        "initialized",
        serde_json::to_value(InitializedParams {}).unwrap(),
    );
    (client, server_thread)
}

/// Send a `textDocument/diagnostic` pull request.
fn pull_diagnostic(client: &Connection, id: i32, uri: &Uri, previous_result_id: Option<String>) {
    send_request(
        client,
        id,
        "textDocument/diagnostic",
        serde_json::to_value(DocumentDiagnosticParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            identifier: None,
            previous_result_id,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .unwrap(),
    );
}

/// Receive the response to a pull request `id`, parsed as a report. Asserts that **no
/// `publishDiagnostics` push** arrives first (pull and push are mutually exclusive);
/// tolerates and acks any server-initiated request (e.g. `workspace/diagnostic/refresh`).
fn recv_document_diagnostic_report(client: &Connection, id: i32) -> DocumentDiagnosticReport {
    loop {
        match recv(client) {
            Message::Response(resp) => {
                assert_eq!(resp.id, RequestId::from(id));
                let result: DocumentDiagnosticReportResult =
                    serde_json::from_value(resp.result.unwrap()).unwrap();
                match result {
                    DocumentDiagnosticReportResult::Report(report) => return report,
                    DocumentDiagnosticReportResult::Partial(_) => {
                        panic!("server returned a partial report; none was requested")
                    }
                }
            }
            Message::Notification(not) if not.method == "textDocument/publishDiagnostics" => {
                panic!("pull-mode client must not receive a publishDiagnostics push")
            }
            Message::Notification(_) => continue,
            // Ack a server→client request (e.g. workspace/diagnostic/refresh) and keep waiting.
            Message::Request(req) => {
                client
                    .sender
                    .send(Message::Response(Response::new_ok(
                        req.id,
                        serde_json::Value::Null,
                    )))
                    .unwrap();
            }
        }
    }
}

/// Extract the items from a full report (or `None` if it is an `unchanged` report).
fn report_items(report: &DocumentDiagnosticReport) -> Option<&[lsp_types::Diagnostic]> {
    match report {
        DocumentDiagnosticReport::Full(full) => Some(&full.full_document_diagnostic_report.items),
        DocumentDiagnosticReport::Unchanged(_) => None,
    }
}

/// The `result_id` carried by either report kind.
fn report_result_id(report: &DocumentDiagnosticReport) -> Option<String> {
    match report {
        DocumentDiagnosticReport::Full(full) => {
            full.full_document_diagnostic_report.result_id.clone()
        }
        DocumentDiagnosticReport::Unchanged(unchanged) => Some(
            unchanged
                .unchanged_document_diagnostic_report
                .result_id
                .clone(),
        ),
    }
}

fn did_open(client: &Connection, uri: &Uri, version: i32, text: &str) {
    send_notification(
        client,
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

fn shutdown(client: &Connection, server_thread: std::thread::JoinHandle<()>) {
    send_request(client, 99, "shutdown", serde_json::Value::Null);
    // Drain any in-flight notifications (e.g. a project re-lint racing the response).
    let resp = loop {
        match recv(client) {
            Message::Response(resp) => break resp,
            Message::Notification(_) => continue,
            other => panic!("expected the shutdown response, got {other:?}"),
        }
    };
    assert_eq!(resp.id, RequestId::from(99));
    send_notification(client, "exit", serde_json::Value::Null);
    server_thread.join().expect("server thread panicked");
}

#[test]
fn lsp_formatting_and_diagnostics_transcript() {
    let (client, server_thread) = start_server(None);

    let uri: Uri = "file:///test.tex".parse().unwrap();

    // didOpen a document with an unclosed environment → diagnostics.
    let broken = "\\begin{itemize}\n\\item a\n";
    did_open(&client, &uri, 1, broken);
    let diags = recv_diagnostics(&client);
    assert_eq!(diags.uri, uri);
    assert!(
        !diags.diagnostics.is_empty(),
        "an unclosed environment must produce at least one diagnostic"
    );

    // didChange to a valid but messy document → diagnostics clear.
    let messy = "\\section{Hi}   \n\n\n\ntext.  ";
    send_notification(
        &client,
        "textDocument/didChange",
        serde_json::to_value(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: messy.to_owned(),
            }],
        })
        .unwrap(),
    );
    let diags = recv_diagnostics(&client);
    assert!(
        diags.diagnostics.is_empty(),
        "a valid document must clear diagnostics, got {:?}",
        diags.diagnostics
    );

    // textDocument/formatting → a single whole-document edit equal to the
    // formatter's own output at the requested tab size.
    send_request(
        &client,
        2,
        "textDocument/formatting",
        serde_json::to_value(DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            options: FormattingOptions {
                tab_size: 2,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
        })
        .unwrap(),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(2));
    let edits: Vec<TextEdit> = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert_eq!(edits.len(), 1, "expected one whole-document edit");
    let expected = format_with_style(
        messy,
        FormatStyle {
            line_width: 80,
            indent_width: 2,
            ..FormatStyle::default()
        },
    )
    .unwrap();
    assert_eq!(edits[0].new_text, expected);

    shutdown(&client, server_thread);
}

#[test]
fn lsp_range_formatting_formats_only_the_selected_block() {
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///range.tex".parse().unwrap();

    // Two messy top-level paragraphs (extra inter-word spaces). Range formatting
    // the first must collapse only its spaces, leaving the second untouched — the
    // distinguishing property versus whole-document formatting.
    let doc = "first    paragraph.\n\nsecond    paragraph.\n";
    did_open(&client, &uri, 1, doc);
    let diags = recv_diagnostics(&client);
    assert!(diags.diagnostics.is_empty(), "clean doc → no diagnostics");

    // A *partial* selection inside the first paragraph: it must clamp out to the
    // whole block.
    let select_first = Range {
        start: Position::new(0, 2),
        end: Position::new(0, 5),
    };
    let range_params = |range: Range| {
        serde_json::to_value(DocumentRangeFormattingParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range,
            options: FormattingOptions {
                tab_size: 2,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
        })
        .unwrap()
    };

    send_request(
        &client,
        2,
        "textDocument/rangeFormatting",
        range_params(select_first),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(2));
    let edits: Vec<TextEdit> = serde_json::from_value(resp.result.unwrap()).unwrap();
    let formatted = apply_edits(doc, &edits);
    assert_eq!(
        formatted, "first paragraph.\n\nsecond    paragraph.\n",
        "only the selected block is formatted; the second paragraph is untouched"
    );

    // Seam idempotence: with the buffer now holding the post-edit text, the same
    // selection yields no edits.
    send_notification(
        &client,
        "textDocument/didChange",
        serde_json::to_value(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: formatted.clone(),
            }],
        })
        .unwrap(),
    );
    let diags = recv_diagnostics(&client);
    assert!(diags.diagnostics.is_empty());

    send_request(
        &client,
        3,
        "textDocument/rangeFormatting",
        range_params(select_first),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(3));
    let edits: Vec<TextEdit> = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert!(
        edits.is_empty(),
        "an already-formatted selection yields no edits, got {edits:?}"
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_range_formatting_reindents_multiline_environment() {
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///range_env.tex".parse().unwrap();

    // A poorly-indented multi-line environment followed by a messy paragraph.
    let doc = "\\begin{itemize}\n\\item one\n\\item two\n\\end{itemize}\n\nsecond    paragraph.\n";
    did_open(&client, &uri, 1, doc);
    let diags = recv_diagnostics(&client);
    assert!(diags.diagnostics.is_empty(), "clean doc → no diagnostics");

    // Cursor inside the environment body (line 1). It clamps out to the whole
    // environment block.
    send_request(
        &client,
        2,
        "textDocument/rangeFormatting",
        serde_json::to_value(DocumentRangeFormattingParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range {
                start: Position::new(1, 3),
                end: Position::new(1, 3),
            },
            options: FormattingOptions {
                tab_size: 2,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
        })
        .unwrap(),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(2));
    let edits: Vec<TextEdit> = serde_json::from_value(resp.result.unwrap()).unwrap();
    let formatted = apply_edits(doc, &edits);
    assert_eq!(
        formatted,
        "\\begin{itemize}\n  \\item one\n  \\item two\n\\end{itemize}\n\nsecond    paragraph.\n",
        "the environment body is reindented; the trailing paragraph is untouched"
    );

    shutdown(&client, server_thread);
}

/// Build `textDocument/onTypeFormatting` params for a `}` typed at `position`
/// (the cursor sits just past the brace).
fn on_type_params(uri: &Uri, position: Position) -> serde_json::Value {
    serde_json::to_value(DocumentOnTypeFormattingParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position,
        },
        ch: "}".to_owned(),
        options: FormattingOptions {
            tab_size: 2,
            insert_spaces: true,
            ..Default::default()
        },
    })
    .unwrap()
}

#[test]
fn lsp_on_type_formatting_reindents_on_environment_close() {
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///ontype_env.tex".parse().unwrap();

    // A poorly-indented multi-line environment followed by a messy paragraph.
    let doc = "\\begin{itemize}\n\\item one\n\\item two\n\\end{itemize}\n\nsecond    paragraph.\n";
    did_open(&client, &uri, 1, doc);
    let diags = recv_diagnostics(&client);
    assert!(diags.diagnostics.is_empty(), "clean doc → no diagnostics");

    // The user just typed the `}` closing `\end{itemize}` on line 3 (13 columns:
    // `\end{itemize}`), so the cursor is at column 13.
    send_request(
        &client,
        2,
        "textDocument/onTypeFormatting",
        on_type_params(&uri, Position::new(3, 13)),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(2));
    let edits: Vec<TextEdit> = serde_json::from_value(resp.result.unwrap()).unwrap();
    let formatted = apply_edits(doc, &edits);
    assert_eq!(
        formatted,
        "\\begin{itemize}\n  \\item one\n  \\item two\n\\end{itemize}\n\nsecond    paragraph.\n",
        "closing `\\end{{itemize}}` reindents the environment; the paragraph is untouched"
    );

    // Idempotence at the seam: with the buffer now holding the reindented text,
    // typing the same `}` again yields no edits.
    send_notification(
        &client,
        "textDocument/didChange",
        serde_json::to_value(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: formatted.clone(),
            }],
        })
        .unwrap(),
    );
    let diags = recv_diagnostics(&client);
    assert!(diags.diagnostics.is_empty());

    send_request(
        &client,
        3,
        "textDocument/onTypeFormatting",
        on_type_params(&uri, Position::new(3, 13)),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(3));
    let edits: Vec<TextEdit> = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert!(
        edits.is_empty(),
        "an already-reindented environment yields no edits, got {edits:?}"
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_on_type_formatting_ignores_inline_group_close() {
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///ontype_inline.tex".parse().unwrap();

    // An inline `\textbf{x}` inside a paragraph with messy spacing. Closing its
    // single-line group must NOT trigger a reformat (no prose reflow on `}`).
    let doc = "a    \\textbf{x} and    more.\n";
    did_open(&client, &uri, 1, doc);
    let diags = recv_diagnostics(&client);
    assert!(diags.diagnostics.is_empty(), "clean doc → no diagnostics");

    // `a    \textbf{x}`: `a`(0) + 4 spaces + `\textbf`(5-11) + `{`(12) + `x`(13)
    // + `}`(14), so the cursor just past the `}` is at column 15.
    send_request(
        &client,
        2,
        "textDocument/onTypeFormatting",
        on_type_params(&uri, Position::new(0, 15)),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(2));
    let edits: Vec<TextEdit> = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert!(
        edits.is_empty(),
        "closing an inline single-line group yields no edits, got {edits:?}"
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_on_type_formatting_refuses_when_buffer_has_parse_errors() {
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///ontype_broken.tex".parse().unwrap();

    // A multi-line environment (whose `\end{itemize}` close would normally fire)
    // followed by a stray `\end{extra}` that makes the buffer fail to parse
    // cleanly. On-type formatting must refuse: no edits.
    let doc = "\\begin{itemize}\n\\item one\n\\end{itemize}\n\\end{extra}\n";
    did_open(&client, &uri, 1, doc);
    let diags = recv_diagnostics(&client);
    assert!(
        !diags.diagnostics.is_empty(),
        "the stray `\\end` must produce a parse diagnostic"
    );

    // Cursor just past the `}` of the (well-formed) `\end{itemize}` on line 2.
    send_request(
        &client,
        2,
        "textDocument/onTypeFormatting",
        on_type_params(&uri, Position::new(2, 13)),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(2));
    // A refusal serializes to JSON `null`; either that or an empty array means
    // "no change".
    let edits: Option<Vec<TextEdit>> = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert!(
        edits.unwrap_or_default().is_empty(),
        "a buffer with parse errors yields no on-type edits"
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_document_symbol_outline() {
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///outline.tex".parse().unwrap();

    // A section containing a figure (with a label) and a theorem.
    let doc = "\\section{Intro}\n\
        \\begin{figure}\n\
        \\label{fig:one}\n\
        \\end{figure}\n\
        \\begin{theorem}\n\
        x\n\
        \\end{theorem}\n";
    did_open(&client, &uri, 1, doc);
    let diags = recv_diagnostics(&client);
    assert!(diags.diagnostics.is_empty(), "clean doc → no diagnostics");

    send_request(
        &client,
        2,
        "textDocument/documentSymbol",
        serde_json::to_value(DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .unwrap(),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(2));
    let response: DocumentSymbolResponse =
        serde_json::from_value(resp.result.unwrap()).expect("a documentSymbol response");
    let DocumentSymbolResponse::Nested(symbols) = response else {
        panic!("expected a nested documentSymbol response");
    };

    // One root section; the figure and theorem nest under it.
    assert_eq!(symbols.len(), 1);
    let section = &symbols[0];
    assert_eq!(section.name, "Intro");
    assert_eq!(section.kind, SymbolKind::MODULE);
    let children = section.children.as_deref().unwrap_or_default();
    let names: Vec<&str> = children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["figure", "theorem"]);

    // The figure carries its label as a leaf.
    let figure = &children[0];
    assert_eq!(figure.kind, SymbolKind::OBJECT);
    let figure_kids: &[DocumentSymbol] = figure.children.as_deref().unwrap_or_default();
    assert_eq!(figure_kids.len(), 1);
    assert_eq!(figure_kids[0].name, "fig:one");
    assert_eq!(figure_kids[0].kind, SymbolKind::CONSTANT);

    assert_eq!(children[1].kind, SymbolKind::CLASS);

    shutdown(&client, server_thread);
}

#[test]
fn lsp_document_symbol_numbers_from_aux() {
    // A compiled project: the sibling `.aux` prefixes section names with their toc
    // numbers and attaches label/float numbers as `detail`.
    let dir = tempfile::tempdir().expect("temp dir");
    let doc = "\\documentclass{article}\n\
        \\begin{document}\n\
        \\section{Intro}\n\
        \\label{sec:intro}\n\
        \\begin{figure}\n\
        \\label{fig:one}\n\
        \\end{figure}\n\
        \\section{Methods}\n\
        \\end{document}\n";
    let main_path = dir.path().join("main.tex");
    std::fs::write(&main_path, doc).unwrap();
    std::fs::write(
        dir.path().join("main.aux"),
        "\\@writefile{toc}{\\contentsline {section}{\\numberline {1}Intro}{1}{section.1}}\n\
         \\newlabel{sec:intro}{{1}{1}{Intro}{section.1}{}}\n\
         \\newlabel{fig:one}{{3}{2}{}{figure.3}{}}\n\
         \\@writefile{toc}{\\contentsline {section}{\\numberline {2}Methods}{2}{section.2}}\n",
    )
    .unwrap();

    let (client, server_thread) = start_server(None);
    let uri = path_to_file_uri(&main_path);
    did_open(&client, &uri, 1, doc);
    let _ = recv_diagnostics(&client);

    send_request(
        &client,
        2,
        "textDocument/documentSymbol",
        serde_json::to_value(DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .unwrap(),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(2));
    let response: DocumentSymbolResponse =
        serde_json::from_value(resp.result.unwrap()).expect("a documentSymbol response");
    let DocumentSymbolResponse::Nested(symbols) = response else {
        panic!("expected a nested documentSymbol response");
    };

    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["1 Intro", "2 Methods"]);

    let intro_kids = symbols[0].children.as_deref().unwrap_or_default();
    let label = intro_kids
        .iter()
        .find(|c| c.name == "sec:intro")
        .expect("label leaf");
    assert_eq!(label.detail.as_deref(), Some("1"));

    let figure = intro_kids
        .iter()
        .find(|c| c.name == "figure")
        .expect("figure symbol");
    assert_eq!(figure.detail.as_deref(), Some("3"), "via its child label");
    let figure_kids = figure.children.as_deref().unwrap_or_default();
    assert_eq!(figure_kids[0].detail.as_deref(), Some("3"));

    shutdown(&client, server_thread);
}

#[test]
fn lsp_document_symbol_dtx_documented_macros() {
    let (client, server_thread) = start_server(None);
    // A `.dtx` is parsed in docstrip mode, so the leading-`%` ltxdoc lines become
    // real `macro`/`\DescribeMacro` constructs surfaced as document symbols.
    let uri: Uri = "file:///pkg.dtx".parse().unwrap();

    let doc = "\\section{Implementation}\n\
        % \\DescribeMacro{\\foo}\n\
        % \\begin{macro}{\\bar}\n\
        %    \\begin{macrocode}\n\
        \\def\\bar{b}\n\
        %    \\end{macrocode}\n\
        % \\end{macro}\n";
    did_open(&client, &uri, 1, doc);
    let _ = recv_diagnostics(&client);

    send_request(
        &client,
        2,
        "textDocument/documentSymbol",
        serde_json::to_value(DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .unwrap(),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(2));
    let response: DocumentSymbolResponse =
        serde_json::from_value(resp.result.unwrap()).expect("a documentSymbol response");
    let DocumentSymbolResponse::Nested(symbols) = response else {
        panic!("expected a nested documentSymbol response");
    };

    // One root section; the documented macros nest under it as FUNCTION symbols.
    assert_eq!(symbols.len(), 1);
    let section = &symbols[0];
    assert_eq!(section.name, "Implementation");
    assert_eq!(section.kind, SymbolKind::MODULE);
    let children = section.children.as_deref().unwrap_or_default();
    let names: Vec<&str> = children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["\\foo", "\\bar"]);
    assert!(children.iter().all(|c| c.kind == SymbolKind::FUNCTION));

    shutdown(&client, server_thread);
}

#[test]
fn lsp_folding_ranges() {
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///fold.tex".parse().unwrap();

    // line 0: \section{Intro}
    //      1-3: a comment block
    //      4-6: a multi-line environment
    let doc = "\\section{Intro}\n\
        % a\n\
        % b\n\
        % c\n\
        \\begin{itemize}\n\
        \\item x\n\
        \\end{itemize}\n";
    did_open(&client, &uri, 1, doc);
    let diags = recv_diagnostics(&client);
    assert!(diags.diagnostics.is_empty(), "clean doc → no diagnostics");

    send_request(
        &client,
        2,
        "textDocument/foldingRange",
        serde_json::to_value(FoldingRangeParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .unwrap(),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(2));
    let ranges: Vec<FoldingRange> =
        serde_json::from_value(resp.result.unwrap()).expect("a foldingRange response");

    let triples: Vec<(u32, u32, Option<FoldingRangeKind>)> = ranges
        .iter()
        .map(|r| (r.start_line, r.end_line, r.kind.clone()))
        .collect();
    // The section spans the whole document; the comment block folds 1..3; the
    // itemize folds 4..6.
    assert!(
        triples.contains(&(0, 6, None)),
        "section fold, got {triples:?}"
    );
    assert!(
        triples.contains(&(1, 3, Some(FoldingRangeKind::Comment))),
        "comment fold, got {triples:?}"
    );
    assert!(
        triples.contains(&(4, 6, None)),
        "itemize fold, got {triples:?}"
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_document_links() {
    // Document links are disk-aware: only targets that exist on disk are linked.
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("part.tex"), "").unwrap();
    std::fs::write(dir.path().join("mypkg.sty"), "").unwrap();
    std::fs::write(dir.path().join("refs.bib"), "").unwrap();
    std::fs::write(dir.path().join("fig.png"), "").unwrap();
    // Disable the installed-tree (TEXMF) fallback so this test stays hermetic: it
    // asserts the *local-only* contract, and a machine with TeX installed would
    // otherwise resolve the system `amsmath` and add a fifth link.
    std::fs::write(
        dir.path().join("badness.toml"),
        "[texmf]\nenabled = false\n",
    )
    .unwrap();
    let main_path = dir.path().join("main.tex");
    // `\usepackage{amsmath}` has no local file, so it must NOT be linked.
    let main = "\\input{part}\n\
        \\usepackage{mypkg}\n\
        \\usepackage{amsmath}\n\
        \\addbibresource{refs.bib}\n\
        \\includegraphics{fig}\n";
    std::fs::write(&main_path, main).unwrap();

    let (client, server_thread) = start_server(None);
    let uri = path_to_file_uri(&main_path);
    did_open(&client, &uri, 1, main);
    let _ = recv_diagnostics(&client);

    send_request(
        &client,
        2,
        "textDocument/documentLink",
        serde_json::to_value(DocumentLinkParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .unwrap(),
    );
    // A freshly-seeded project re-lints, so drain any stray diagnostics first.
    let resp = loop {
        match recv(&client) {
            Message::Response(resp) => break resp,
            Message::Notification(_) => continue,
            other => panic!("expected a response, got {other:?}"),
        }
    };
    assert_eq!(resp.id, RequestId::from(2));
    let links: Vec<DocumentLink> =
        serde_json::from_value(resp.result.unwrap()).expect("a documentLink response");

    // Four resolvable targets; the system `amsmath` package is absent, so no link.
    let targets: Vec<Uri> = links.iter().filter_map(|l| l.target.clone()).collect();
    assert_eq!(links.len(), 4, "got {targets:?}");
    assert!(targets.contains(&path_to_file_uri(&dir.path().join("part.tex"))));
    assert!(targets.contains(&path_to_file_uri(&dir.path().join("mypkg.sty"))));
    assert!(targets.contains(&path_to_file_uri(&dir.path().join("refs.bib"))));
    assert!(targets.contains(&path_to_file_uri(&dir.path().join("fig.png"))));

    // The `\input{part}` link underlines just the `part` argument (line 0).
    let part_link = links
        .iter()
        .find(|l| l.target == Some(path_to_file_uri(&dir.path().join("part.tex"))))
        .unwrap();
    assert_eq!(part_link.range.start, Position::new(0, 7));
    assert_eq!(part_link.range.end, Position::new(0, 11));

    shutdown(&client, server_thread);
}

#[test]
fn lsp_code_action_quickfix() {
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///ca.tex".parse().unwrap();

    // `\bf` is a deprecated font switch; the `deprecated-command` rule flags it and
    // carries a `\bf` → `\bfseries` safe autofix.
    let doc = "\\bf hi\n";
    did_open(&client, &uri, 1, doc);
    let diags = recv_diagnostics(&client);
    assert!(
        diags.diagnostics.iter().any(|d| d.message.contains("\\bf")),
        "deprecated-command should flag \\bf, got {:?}",
        diags.diagnostics
    );

    // A range over the `\bf` control word (line 0, chars 0..3).
    let on_bf = Range {
        start: Position::new(0, 0),
        end: Position::new(0, 3),
    };
    send_request(
        &client,
        2,
        "textDocument/codeAction",
        serde_json::to_value(CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: on_bf,
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .unwrap(),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(2));
    let actions: Vec<CodeActionOrCommand> =
        serde_json::from_value(resp.result.unwrap()).expect("a codeAction response");
    let CodeActionOrCommand::CodeAction(action) = actions
        .iter()
        .find(|a| matches!(a, CodeActionOrCommand::CodeAction(a) if a.title.contains("bfseries")))
        .expect("a `\\bf` → `\\bfseries` quick-fix")
    else {
        unreachable!()
    };
    let edits = action
        .edit
        .as_ref()
        .and_then(|e| e.changes.as_ref())
        .and_then(|c| c.get(&uri))
        .expect("a single-file edit on the document");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].new_text, "\\bfseries");
    assert_eq!(edits[0].range.start, Position::new(0, 0));
    assert_eq!(edits[0].range.end, Position::new(0, 3));

    // A range that misses the command (the trailing prose) yields no actions.
    let off_bf = Range {
        start: Position::new(0, 5),
        end: Position::new(0, 5),
    };
    send_request(
        &client,
        3,
        "textDocument/codeAction",
        serde_json::to_value(CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: off_bf,
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .unwrap(),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(3));
    let actions: Vec<CodeActionOrCommand> =
        serde_json::from_value(resp.result.unwrap()).expect("a codeAction response");
    assert!(
        actions.is_empty(),
        "a range off the command yields no quick-fix, got {actions:?}"
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_bib_diagnostics_formatting_and_symbols() {
    let (client, server_thread) = start_server(None);
    // A `.bib` URI: the server routes by extension to the BibTeX pipeline.
    let uri: Uri = "file:///refs.bib".parse().unwrap();

    // Two entries sharing a cite key (a lint warning, not a parse error) plus
    // unformatted spacing. The duplicate is reported; formatting still works.
    let doc = "@article{k, title={A}}\n@misc{k, title={B}}\n";
    did_open(&client, &uri, 1, doc);
    let diags = recv_diagnostics(&client);
    assert_eq!(diags.uri, uri);
    assert!(
        diags.diagnostics.iter().any(|d| d.code
            == Some(lsp_types::NumberOrString::String(
                "duplicate-key".to_owned()
            ))),
        "a duplicate cite key must produce a duplicate-key diagnostic, got {:?}",
        diags.diagnostics
    );

    // textDocument/formatting → a whole-document edit equal to the bib formatter's
    // own output at the requested tab size.
    send_request(
        &client,
        2,
        "textDocument/formatting",
        serde_json::to_value(DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            options: FormattingOptions {
                tab_size: 2,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
        })
        .unwrap(),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(2));
    let edits: Vec<TextEdit> = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert_eq!(edits.len(), 1, "expected one whole-document edit");
    let expected = badness::bib::format_with_style(
        doc,
        FormatStyle {
            line_width: 80,
            indent_width: 2,
            ..FormatStyle::default()
        },
    )
    .unwrap();
    assert_eq!(edits[0].new_text, expected);

    // textDocument/documentSymbol → a flat list of entries (cite key + type).
    send_request(
        &client,
        3,
        "textDocument/documentSymbol",
        serde_json::to_value(DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .unwrap(),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(3));
    let DocumentSymbolResponse::Nested(symbols) =
        serde_json::from_value(resp.result.unwrap()).expect("a documentSymbol response")
    else {
        panic!("expected a nested documentSymbol response");
    };
    assert_eq!(symbols.len(), 2, "two entries → two flat symbols");
    assert!(symbols.iter().all(|s| s.name == "k"));
    assert!(symbols.iter().all(|s| s.kind == SymbolKind::CONSTANT));
    assert!(symbols.iter().all(|s| s.children.is_none()));
    let details: Vec<&str> = symbols
        .iter()
        .map(|s| s.detail.as_deref().unwrap_or_default())
        .collect();
    assert_eq!(details, vec!["article", "misc"]);

    shutdown(&client, server_thread);
}

#[test]
fn incremental_did_change_splices_buffer() {
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///inc.tex".parse().unwrap();

    // Open a clean doc → diagnostics clear.
    did_open(&client, &uri, 1, "\\section{Hi}\nworld\n");
    let diags = recv_diagnostics(&client);
    assert!(diags.diagnostics.is_empty());

    // Ranged change: replace "world" (line 1, cols 0..5) with an unclosed
    // environment. It must surface as a diagnostic — proving the splice landed in
    // the buffer the parser sees, not the original clean text.
    send_notification(
        &client,
        "textDocument/didChange",
        serde_json::to_value(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: Some(Range {
                    start: Position::new(1, 0),
                    end: Position::new(1, 5),
                }),
                range_length: None,
                text: "\\begin{itemize}".to_owned(),
            }],
        })
        .unwrap(),
    );
    let diags = recv_diagnostics(&client);
    assert!(
        !diags.diagnostics.is_empty(),
        "the spliced unclosed environment must produce a diagnostic"
    );

    // Format the spliced buffer: the edit must equal formatting "\\section{Hi}\n"
    // + the spliced line. We assert the server formats the *spliced* text, not the
    // original — i.e. the new text contains the inserted command.
    send_request(
        &client,
        2,
        "textDocument/formatting",
        serde_json::to_value(DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            options: FormattingOptions {
                tab_size: 2,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
        })
        .unwrap(),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(2));
    // The buffer now has a parse error (unclosed group), so the formatter refuses:
    // a `null` result. This still proves the splice took effect (the original
    // clean buffer would have formatted).
    assert!(
        resp.result.is_none() || resp.result == Some(serde_json::Value::Null),
        "formatter must refuse the now-broken spliced buffer, got {:?}",
        resp.result
    );

    shutdown(&client, server_thread);
}

#[test]
fn line_width_from_initialization_options() {
    // A narrow line width must reflow a long paragraph the default-80 width would
    // leave on one line.
    let (client, server_thread) = start_server(Some(serde_json::json!({ "lineWidth": 20 })));
    let uri: Uri = "file:///wrap.tex".parse().unwrap();

    let para = "alpha beta gamma delta epsilon zeta eta theta\n";
    did_open(&client, &uri, 1, para);
    let diags = recv_diagnostics(&client);
    assert!(diags.diagnostics.is_empty());

    send_request(
        &client,
        2,
        "textDocument/formatting",
        serde_json::to_value(DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            // No tab_size override, so the editor settings drive the style.
            options: FormattingOptions {
                tab_size: 0,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
        })
        .unwrap(),
    );
    let resp = recv_response(&client);
    let edits: Vec<TextEdit> = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert_eq!(edits.len(), 1, "the narrow width must reflow the paragraph");
    let expected = format_with_style(
        para,
        FormatStyle {
            line_width: 20,
            ..FormatStyle::default()
        },
    )
    .unwrap();
    assert_eq!(edits[0].new_text, expected);
    // Sanity: the configured width actually changed the output vs. the default.
    let default_out = format_with_style(para, FormatStyle::default()).unwrap();
    assert_ne!(
        expected, default_out,
        "the test paragraph must format differently at width 20 vs 80"
    );

    shutdown(&client, server_thread);
}

#[test]
fn did_close_clears_and_allows_reopen() {
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///close.tex".parse().unwrap();

    did_open(&client, &uri, 1, "\\begin{itemize}\n");
    let diags = recv_diagnostics(&client);
    assert!(!diags.diagnostics.is_empty(), "unclosed env → diagnostic");

    // Close → diagnostics cleared.
    send_notification(
        &client,
        "textDocument/didClose",
        serde_json::to_value(DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
        })
        .unwrap(),
    );
    let diags = recv_diagnostics(&client);
    assert!(diags.diagnostics.is_empty(), "close must clear diagnostics");

    // Reopen the same URI with a clean doc → a fresh input, clean diagnostics.
    did_open(&client, &uri, 1, "\\section{Hi}\n");
    let diags = recv_diagnostics(&client);
    assert_eq!(diags.uri, uri);
    assert!(
        diags.diagnostics.is_empty(),
        "reopened clean doc must parse cleanly, got {:?}",
        diags.diagnostics
    );

    shutdown(&client, server_thread);
}

/// Send a `textDocument/completion` at `position` and return the items.
fn complete(client: &Connection, id: i32, uri: &Uri, position: Position) -> Vec<CompletionItem> {
    send_request(
        client,
        id,
        "textDocument/completion",
        serde_json::to_value(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .unwrap(),
    );
    let resp = recv_response(client);
    assert_eq!(resp.id, RequestId::from(id));
    match serde_json::from_value::<CompletionResponse>(resp.result.unwrap()).unwrap() {
        CompletionResponse::Array(items) => items,
        CompletionResponse::List(list) => list.items,
    }
}

fn labels(items: &[CompletionItem]) -> Vec<&str> {
    items.iter().map(|i| i.label.as_str()).collect()
}

/// Send `textDocument/hover` and return the rendered markdown, or `None` when the
/// server replies with `null` (nothing to describe at the position).
fn hover_markdown(client: &Connection, id: i32, uri: &Uri, position: Position) -> Option<String> {
    send_request(
        client,
        id,
        "textDocument/hover",
        serde_json::to_value(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .unwrap(),
    );
    let resp = recv_response(client);
    assert_eq!(resp.id, RequestId::from(id));
    let result = resp.result.unwrap();
    if result.is_null() {
        return None;
    }
    let hover: Hover = serde_json::from_value(result).unwrap();
    match hover.contents {
        HoverContents::Markup(m) => Some(m.value),
        other => panic!("expected markup hover, got {other:?}"),
    }
}

#[test]
fn lsp_hover_command_signature_and_null() {
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///hover.tex".parse().unwrap();

    let doc = "\\section{Intro}\n\nPlain words here.\n";
    did_open(&client, &uri, 1, doc);
    let diags = recv_diagnostics(&client);
    assert!(diags.diagnostics.is_empty(), "{:?}", diags.diagnostics);

    // Hover on `\section` (line 0, on the command name).
    let md = hover_markdown(&client, 2, &uri, Position::new(0, 3)).expect("hover for \\section");
    assert!(md.contains("\\section"), "prototype: {md}");
    assert!(md.contains("sectioning level"), "facts: {md}");

    // Hover on plain prose resolves to nothing → `null`.
    assert!(
        hover_markdown(&client, 3, &uri, Position::new(2, 2)).is_none(),
        "prose hover should be null"
    );

    shutdown(&client, server_thread);
}

/// Send a `textDocument/signatureHelp` request and decode the reply (`None` for
/// `null`).
fn signature_help(
    client: &Connection,
    id: i32,
    uri: &Uri,
    position: Position,
) -> Option<SignatureHelp> {
    send_request(
        client,
        id,
        "textDocument/signatureHelp",
        serde_json::to_value(SignatureHelpParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            context: None,
        })
        .unwrap(),
    );
    let resp = recv_response(client);
    assert_eq!(resp.id, RequestId::from(id));
    match resp.result.unwrap() {
        serde_json::Value::Null => None,
        value => Some(serde_json::from_value(value).expect("valid SignatureHelp")),
    }
}

#[test]
fn lsp_signature_help_active_argument_and_null() {
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///sighelp.tex".parse().unwrap();

    let doc = "\\frac{a}{b}\n\nPlain words here.\n";
    did_open(&client, &uri, 1, doc);
    let _ = recv_diagnostics(&client);

    // Inside the second `{…}` of `\frac` (line 0, right after `b`).
    let help = signature_help(&client, 2, &uri, Position::new(0, 10)).expect("help for \\frac");
    assert_eq!(help.signatures.len(), 1);
    assert_eq!(help.signatures[0].label, "\\frac{#1}{#2}");
    assert_eq!(help.active_parameter, Some(1));

    // A prose position resolves to nothing → `null`.
    assert!(
        signature_help(&client, 3, &uri, Position::new(2, 2)).is_none(),
        "prose position should be null"
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_completion_commands_environments_and_refs() {
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///complete.tex".parse().unwrap();

    // A clean document so diagnostics stay empty (the env is matched).
    let doc = "\\section{Intro}\n\
        \\label{sec:intro}\n\
        \\ref{sec:i}\n\
        \\begin{itemize}\n\
        \\item x\n\
        \\end{itemize}\n\
        \\sub\n";
    did_open(&client, &uri, 1, doc);
    let diags = recv_diagnostics(&client);
    assert!(
        diags.diagnostics.is_empty(),
        "clean doc → no diagnostics, got {:?}",
        diags.diagnostics
    );

    // Command names: cursor at the end of `\sub` (line 6).
    let cmds = complete(&client, 2, &uri, Position::new(6, 4));
    let names = labels(&cmds);
    assert!(names.contains(&"subsection"), "{names:?}");
    assert!(names.contains(&"subsubsection"), "{names:?}");
    assert!(
        cmds.iter()
            .all(|i| i.kind == Some(CompletionItemKind::FUNCTION)),
        "command items are FUNCTION"
    );

    // Environment names inside `\begin{it|emize}` (line 3) carry the auto-`\end`
    // snippet.
    let envs = complete(&client, 3, &uri, Position::new(3, 9));
    let itemize = envs
        .iter()
        .find(|i| i.label == "itemize")
        .expect("itemize env candidate");
    assert_eq!(itemize.insert_text_format, Some(InsertTextFormat::SNIPPET));
    assert_eq!(
        itemize.insert_text.as_deref(),
        Some("itemize}\n\t$0\n\\end{itemize}")
    );

    // `\ref{sec:i|}` (line 2) completes the defined label.
    let refs = complete(&client, 4, &uri, Position::new(2, 10));
    assert_eq!(labels(&refs), vec!["sec:intro"]);
    assert_eq!(refs[0].kind, Some(CompletionItemKind::REFERENCE));

    shutdown(&client, server_thread);
}

#[test]
fn lsp_completion_file_paths() {
    // A real on-disk directory the document lives in, holding a `.tex`, an image,
    // and a subdirectory. The buffer text itself is in-memory.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("intro.tex"), "x").unwrap();
    std::fs::write(dir.path().join("logo.png"), "x").unwrap();
    std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
    std::fs::create_dir(dir.path().join("chapters")).unwrap();

    let uri = path_to_file_uri(&dir.path().join("main.tex"));
    let (client, server_thread) = start_server(None);

    // `\input{|}` → `.tex` files and directories (not the image or the `.txt`).
    did_open(&client, &uri, 1, "\\input{}\n");
    let _ = recv_diagnostics(&client);
    let inputs = complete(&client, 2, &uri, Position::new(0, 7));
    let names = labels(&inputs);
    assert!(names.contains(&"intro.tex"), "{names:?}");
    assert!(names.contains(&"chapters"), "{names:?}");
    assert!(!names.contains(&"logo.png"), "{names:?}");
    assert!(!names.contains(&"notes.txt"), "{names:?}");
    let intro = inputs.iter().find(|i| i.label == "intro.tex").unwrap();
    assert_eq!(intro.kind, Some(CompletionItemKind::FILE));
    let chapters = inputs.iter().find(|i| i.label == "chapters").unwrap();
    assert_eq!(chapters.kind, Some(CompletionItemKind::FOLDER));

    // `\includegraphics{|}` → the image and directories (not the `.tex`).
    send_notification(
        &client,
        "textDocument/didChange",
        serde_json::to_value(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "\\includegraphics{}\n".to_owned(),
            }],
        })
        .unwrap(),
    );
    let _ = recv_diagnostics(&client);
    let graphics = complete(&client, 3, &uri, Position::new(0, 17));
    let names = labels(&graphics);
    assert!(names.contains(&"logo.png"), "{names:?}");
    assert!(names.contains(&"chapters"), "{names:?}");
    assert!(!names.contains(&"intro.tex"), "{names:?}");

    shutdown(&client, server_thread);
}

#[test]
fn lsp_hover_label_number_from_aux_dir() {
    // A compiled project with an out-of-tree aux dir: `[build] aux-dir` routes the
    // label-number lookup, so hovering a `\ref` key shows the resolved number.
    let dir = tempfile::tempdir().unwrap();
    let main = "\\documentclass{article}\n\\begin{document}\n\\section{Intro}\n\\label{sec:a}\nSee \\ref{sec:a}.\n\\end{document}\n";
    let main_path = dir.path().join("main.tex");
    std::fs::write(&main_path, main).unwrap();
    std::fs::write(
        dir.path().join("badness.toml"),
        "[build]\naux-dir = \"out\"\n",
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("out")).unwrap();
    std::fs::write(
        dir.path().join("out/main.aux"),
        "\\newlabel{sec:a}{{2}{1}{Intro}{section.2}{}}\n",
    )
    .unwrap();

    let (client, server_thread) = start_server(None);
    let uri = path_to_file_uri(&main_path);
    did_open(&client, &uri, 1, main);
    let _ = recv_diagnostics(&client);

    // Cursor inside the `sec:a` key of `\ref` on line 4 (`See \ref{sec:a}.`).
    let md = hover_markdown(&client, 2, &uri, Position::new(4, 10)).expect("label hover");
    assert_eq!(md, "Section 2 (Intro)");

    shutdown(&client, server_thread);
}

#[test]
fn lsp_completion_package_names() {
    // A local `.sty` sibling plus the baked name list: `\usepackage{|}` merges
    // both, deduping the local file against its baked namesake.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("mylocal.sty"), "x").unwrap();
    std::fs::write(dir.path().join("amsmath.sty"), "x").unwrap();
    // Disable the installed-tree tier so the test is hermetic (a machine with TeX
    // would otherwise fold thousands of installed stems into the results). The
    // local + baked contract under test is unaffected.
    std::fs::write(
        dir.path().join("badness.toml"),
        "[texmf]\nenabled = false\n",
    )
    .unwrap();

    let uri = path_to_file_uri(&dir.path().join("main.tex"));
    let (client, server_thread) = start_server(None);

    // `\usepackage{|}` at column 12: local `.sty` files (as MODULE names, extension
    // stripped) and baked package names, all deduped.
    did_open(&client, &uri, 1, "\\usepackage{}\n");
    let _ = recv_diagnostics(&client);
    let items = complete(&client, 2, &uri, Position::new(0, 12));
    let names = labels(&items);
    assert!(names.contains(&"mylocal"), "local .sty offered: {names:?}");
    assert!(names.contains(&"amsmath"), "baked name offered: {names:?}");
    // `amsmath` is present once (local file deduped against baked name).
    assert_eq!(
        names.iter().filter(|n| **n == "amsmath").count(),
        1,
        "amsmath deduped: {names:?}"
    );
    let amsmath = items.iter().find(|i| i.label == "amsmath").unwrap();
    assert_eq!(amsmath.kind, Some(CompletionItemKind::MODULE));
    // Ranking is carried by `sortText`, not the alphabetical label order.
    assert!(amsmath.sort_text.is_some(), "sortText set for ranking");

    // `\documentclass{art|}` → the baked class list (`article`), not packages.
    send_notification(
        &client,
        "textDocument/didChange",
        serde_json::to_value(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "\\documentclass{art}\n".to_owned(),
            }],
        })
        .unwrap(),
    );
    let _ = recv_diagnostics(&client);
    let classes = complete(&client, 3, &uri, Position::new(0, 18));
    let names = labels(&classes);
    assert!(names.contains(&"article"), "{names:?}");
    assert!(names.iter().all(|n| n.starts_with("art")), "{names:?}");

    shutdown(&client, server_thread);
}

/// The lint-rule codes carried by a diagnostics batch (parse diagnostics have no
/// code and are dropped), for asserting which cross-file rules fired.
fn rule_codes(diags: &PublishDiagnosticsParams) -> Vec<String> {
    diags
        .diagnostics
        .iter()
        .filter_map(|d| match &d.code {
            Some(NumberOrString::String(code)) => Some(code.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn lsp_cross_file_resolution_clears_diagnostics() {
    // A real on-disk project: a root that `\input`s a chapter (defining the
    // referenced label) and an `\addbibresource` bibliography (defining the cited
    // key). Only the root is opened; the server discovers the siblings on disk,
    // assembles a project, and the cross-file rules resolve — no diagnostics.
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("part.tex"), "\\label{sec:intro}\n").unwrap();
    std::fs::write(
        dir.path().join("refs.bib"),
        "@article{knuth1984, title={The TeXbook}}\n",
    )
    .unwrap();
    let main_path = dir.path().join("main.tex");
    let main = "\\documentclass{article}\n\
        \\addbibresource{refs.bib}\n\
        \\begin{document}\n\
        \\input{part}\n\
        \\ref{sec:intro}\n\
        \\cite{knuth1984}\n\
        \\end{document}\n";
    std::fs::write(&main_path, main).unwrap();

    let (client, server_thread) = start_server(None);
    let uri = path_to_file_uri(&main_path);
    did_open(&client, &uri, 1, main);

    let diags = recv_diagnostics(&client);
    assert_eq!(diags.uri, uri);
    assert!(
        diags.diagnostics.is_empty(),
        "cross-file label + citation must resolve, got {:?}",
        diags.diagnostics
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_cross_file_undefined_ref_and_citation_fire() {
    // The same shape, but the root references a label and a cite key that nothing
    // in the (now closed, rooted) project defines — so `undefined-ref` and
    // `undefined-citation` fire live in the editor.
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("part.tex"), "\\label{sec:intro}\n").unwrap();
    std::fs::write(
        dir.path().join("refs.bib"),
        "@article{knuth1984, title={The TeXbook}}\n",
    )
    .unwrap();
    let main_path = dir.path().join("main.tex");
    let main = "\\documentclass{article}\n\
        \\addbibresource{refs.bib}\n\
        \\begin{document}\n\
        \\input{part}\n\
        \\ref{sec:missing}\n\
        \\cite{lamport1986}\n\
        \\end{document}\n";
    std::fs::write(&main_path, main).unwrap();

    let (client, server_thread) = start_server(None);
    let uri = path_to_file_uri(&main_path);
    did_open(&client, &uri, 1, main);

    let diags = recv_diagnostics(&client);
    assert_eq!(diags.uri, uri);
    let codes = rule_codes(&diags);
    assert!(
        codes.iter().any(|c| c == "undefined-ref"),
        "expected undefined-ref, got {codes:?}"
    );
    assert!(
        codes.iter().any(|c| c == "undefined-citation"),
        "expected undefined-citation, got {codes:?}"
    );

    shutdown(&client, server_thread);
}

/// Send a `textDocument/definition` at `position` and return the locations,
/// draining any stray diagnostics (a freshly-seeded project re-lints) first.
fn definition(client: &Connection, id: i32, uri: &Uri, position: Position) -> Vec<Location> {
    send_request(
        client,
        id,
        "textDocument/definition",
        serde_json::to_value(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap(),
    );
    let resp = loop {
        match recv(client) {
            Message::Response(resp) => break resp,
            Message::Notification(_) => continue,
            other => panic!("expected a response, got {other:?}"),
        }
    };
    assert_eq!(resp.id, RequestId::from(id));
    match serde_json::from_value::<GotoDefinitionResponse>(resp.result.unwrap()).unwrap() {
        GotoDefinitionResponse::Array(locs) => locs,
        GotoDefinitionResponse::Scalar(loc) => vec![loc],
        GotoDefinitionResponse::Link(_) => panic!("unexpected LocationLink response"),
    }
}

#[test]
fn lsp_definition_same_file_ref_to_label() {
    let (client, server_thread) = start_server(None);
    // Build the URI from a platform-absolute path: go-to-definition re-derives the
    // reply `Location`'s URI from the db's normalized (absolutized) path, so a
    // bare `file:///def.tex` round-trips on Unix but not on Windows, where
    // `/def.tex` lacks a drive and gets the cwd drive prepended. No file is
    // created; the buffer stays in-memory.
    let abs = std::path::absolute("def.tex").expect("absolute path");
    let uri = path_to_file_uri(&abs);
    // A label and a reference to it in the same buffer.
    let doc = "\\label{sec:intro}\n\\ref{sec:intro}\n";
    did_open(&client, &uri, 1, doc);
    let _ = recv_diagnostics(&client);

    // Cursor inside `\ref{sec:intro}` on line 1 → jumps to the `\label` on line 0.
    let locs = definition(&client, 2, &uri, Position::new(1, 6));
    assert_eq!(locs.len(), 1, "one definition, got {locs:?}");
    assert_eq!(locs[0].uri, uri);
    assert_eq!(locs[0].range.start, Position::new(0, 0));

    // Cursor in plain prose / on nothing → no definition (empty array).
    let none = definition(&client, 3, &uri, Position::new(0, 0));
    assert!(none.is_empty(), "the `\\label` site is not a reference");

    shutdown(&client, server_thread);
}

#[test]
fn lsp_definition_cross_file_ref_and_cite() {
    // A real on-disk project: the root `\input`s a chapter that defines the label
    // and `\addbibresource`s a `.bib` that defines the cite key. Go-to-definition
    // crosses files to both.
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("part.tex"), "\\label{sec:intro}\n").unwrap();
    std::fs::write(
        dir.path().join("refs.bib"),
        "@article{knuth1984, title={The TeXbook}}\n",
    )
    .unwrap();
    let main_path = dir.path().join("main.tex");
    let main = "\\documentclass{article}\n\
        \\addbibresource{refs.bib}\n\
        \\begin{document}\n\
        \\input{part}\n\
        \\ref{sec:intro}\n\
        \\cite{knuth1984}\n\
        \\end{document}\n";
    std::fs::write(&main_path, main).unwrap();

    let (client, server_thread) = start_server(None);
    let uri = path_to_file_uri(&main_path);
    did_open(&client, &uri, 1, main);
    let _ = recv_diagnostics(&client);

    // `\ref{sec:intro}` (line 4) → the `\label` in part.tex (line 0).
    let ref_locs = definition(&client, 2, &uri, Position::new(4, 6));
    assert_eq!(ref_locs.len(), 1, "one label definition, got {ref_locs:?}");
    assert_eq!(
        ref_locs[0].uri,
        path_to_file_uri(&dir.path().join("part.tex"))
    );
    assert_eq!(ref_locs[0].range.start, Position::new(0, 0));

    // `\cite{knuth1984}` (line 5) → the `@article` key in refs.bib (line 0).
    let cite_locs = definition(&client, 3, &uri, Position::new(5, 8));
    assert_eq!(cite_locs.len(), 1, "one bib entry, got {cite_locs:?}");
    assert_eq!(
        cite_locs[0].uri,
        path_to_file_uri(&dir.path().join("refs.bib"))
    );
    assert_eq!(cite_locs[0].range.start.line, 0);

    shutdown(&client, server_thread);
}

#[test]
fn lsp_definition_jumps_to_include_and_package_files() {
    // Go-to-definition on a file-referencing argument (include/package) jumps to the
    // resolved file, reusing the document-link resolution. TEXMF is disabled so the
    // test stays hermetic (it asserts the local-file contract).
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("part.tex"), "\\label{a}\n").unwrap();
    std::fs::write(dir.path().join("mypkg.sty"), "% pkg\n").unwrap();
    std::fs::write(
        dir.path().join("badness.toml"),
        "[texmf]\nenabled = false\n",
    )
    .unwrap();
    let main_path = dir.path().join("main.tex");
    let main = "\\usepackage{mypkg}\n\\input{part}\n";
    std::fs::write(&main_path, main).unwrap();

    let (client, server_thread) = start_server(None);
    let uri = path_to_file_uri(&main_path);
    did_open(&client, &uri, 1, main);
    let _ = recv_diagnostics(&client);

    // Cursor in `\input{part}` (line 1) → part.tex, whole-file target.
    let input_locs = definition(&client, 2, &uri, Position::new(1, 9));
    assert_eq!(
        input_locs.len(),
        1,
        "one include target, got {input_locs:?}"
    );
    assert_eq!(
        input_locs[0].uri,
        path_to_file_uri(&dir.path().join("part.tex"))
    );
    assert_eq!(input_locs[0].range.start, Position::new(0, 0));

    // Cursor in `\usepackage{mypkg}` (line 0) → the local mypkg.sty.
    let pkg_locs = definition(&client, 3, &uri, Position::new(0, 13));
    assert_eq!(pkg_locs.len(), 1, "one package target, got {pkg_locs:?}");
    assert_eq!(
        pkg_locs[0].uri,
        path_to_file_uri(&dir.path().join("mypkg.sty"))
    );

    shutdown(&client, server_thread);
}

/// Send a `workspace/symbol` request and return the matched symbols as
/// `(name, kind, uri)`, draining any stray diagnostics (a freshly-seeded project
/// re-lints) before the response. The server replies with the modern
/// [`WorkspaceSymbol`] shape, but its `location` is a bare `Location` on the wire,
/// so the untagged [`WorkspaceSymbolResponse`] can deserialize as either variant;
/// normalize both to the fields the assertions care about.
fn workspace_symbols(client: &Connection, id: i32, query: &str) -> Vec<(String, SymbolKind, Uri)> {
    send_request(
        client,
        id,
        "workspace/symbol",
        serde_json::to_value(WorkspaceSymbolParams {
            query: query.to_owned(),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap(),
    );
    let resp = loop {
        match recv(client) {
            Message::Response(resp) => break resp,
            Message::Notification(_) => continue,
            other => panic!("expected a response, got {other:?}"),
        }
    };
    assert_eq!(resp.id, RequestId::from(id));
    match serde_json::from_value::<WorkspaceSymbolResponse>(resp.result.unwrap())
        .expect("a workspace/symbol response")
    {
        WorkspaceSymbolResponse::Nested(symbols) => symbols
            .into_iter()
            .map(|s| {
                let uri = match s.location {
                    OneOf::Left(loc) => loc.uri,
                    OneOf::Right(loc) => loc.uri,
                };
                (s.name, s.kind, uri)
            })
            .collect(),
        WorkspaceSymbolResponse::Flat(symbols) => symbols
            .into_iter()
            .map(|s| (s.name, s.kind, s.location.uri))
            .collect(),
    }
}

#[test]
fn lsp_workspace_symbol_cross_file() {
    // A real on-disk project: the root `\input`s a chapter that defines a section
    // and a label. `workspace/symbol` aggregates the chapter's outline even though
    // only the root buffer is open (the sibling is seeded off disk).
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("part.tex"),
        "\\section{Chapter One}\n\\label{sec:intro}\n",
    )
    .unwrap();
    let main_path = dir.path().join("main.tex");
    let main = "\\documentclass{article}\n\
        \\begin{document}\n\
        \\input{part}\n\
        \\end{document}\n";
    std::fs::write(&main_path, main).unwrap();

    let (client, server_thread) = start_server(None);
    let uri = path_to_file_uri(&main_path);
    did_open(&client, &uri, 1, main);
    let _ = recv_diagnostics(&client);

    let part_uri = path_to_file_uri(&dir.path().join("part.tex"));

    // A query matching the section title finds it in the seeded sibling.
    let secs = workspace_symbols(&client, 2, "Chapter");
    assert_eq!(secs.len(), 1, "one section match, got {secs:?}");
    assert_eq!(secs[0].0, "Chapter One");
    assert_eq!(secs[0].1, SymbolKind::MODULE);
    assert_eq!(
        secs[0].2, part_uri,
        "the section lives in the included file"
    );

    // A query matching the label finds it (also in the sibling).
    let labels = workspace_symbols(&client, 3, "sec:intro");
    assert_eq!(labels.len(), 1, "one label match, got {labels:?}");
    assert_eq!(labels[0].0, "sec:intro");
    assert_eq!(labels[0].1, SymbolKind::CONSTANT);

    // Matching is case-insensitive.
    let lower = workspace_symbols(&client, 4, "chapter one");
    assert_eq!(lower.len(), 1, "case-insensitive match, got {lower:?}");

    // A query matching nothing yields an empty result.
    let none = workspace_symbols(&client, 5, "zzz-no-such-symbol");
    assert!(none.is_empty(), "no matches → empty, got {none:?}");

    shutdown(&client, server_thread);
}

/// Send a `textDocument/references` at `position` and return the locations,
/// draining any stray diagnostics first (mirrors [`definition`]).
fn references(
    client: &Connection,
    id: i32,
    uri: &Uri,
    position: Position,
    include_declaration: bool,
) -> Vec<Location> {
    send_request(
        client,
        id,
        "textDocument/references",
        serde_json::to_value(ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext {
                include_declaration,
            },
        })
        .unwrap(),
    );
    let resp = loop {
        match recv(client) {
            Message::Response(resp) => break resp,
            Message::Notification(_) => continue,
            other => panic!("expected a response, got {other:?}"),
        }
    };
    assert_eq!(resp.id, RequestId::from(id));
    serde_json::from_value::<Vec<Location>>(resp.result.unwrap()).unwrap()
}

/// Sort locations by (line, character) of their start so assertions don't depend
/// on the namespace-member iteration order.
fn sorted_starts(mut locs: Vec<Location>) -> Vec<Position> {
    locs.sort_by_key(|l| (l.range.start.line, l.range.start.character));
    locs.into_iter().map(|l| l.range.start).collect()
}

#[test]
fn lsp_references_same_file_label_uses() {
    let (client, server_thread) = start_server(None);
    let abs = std::path::absolute("def.tex").expect("absolute path");
    let uri = path_to_file_uri(&abs);
    // A label and two references to it in the same buffer.
    let doc = "\\label{sec:intro}\n\\ref{sec:intro}\n\\ref{sec:intro}\n";
    did_open(&client, &uri, 1, doc);
    let _ = recv_diagnostics(&client);

    // Cursor inside the first `\ref` → both `\ref` use sites, declaration excluded.
    let uses = references(&client, 2, &uri, Position::new(1, 6), false);
    assert_eq!(
        sorted_starts(uses),
        vec![Position::new(1, 0), Position::new(2, 0)],
        "both \\ref uses, no \\label"
    );

    // includeDeclaration → the `\label` site joins the two uses.
    let with_decl = references(&client, 3, &uri, Position::new(1, 6), true);
    assert_eq!(
        sorted_starts(with_decl),
        vec![
            Position::new(0, 0),
            Position::new(1, 0),
            Position::new(2, 0),
        ],
        "the \\label declaration is included"
    );

    // Invoking on the `\label` definition itself resolves the same use set.
    let from_def = references(&client, 4, &uri, Position::new(0, 8), false);
    assert_eq!(
        sorted_starts(from_def),
        vec![Position::new(1, 0), Position::new(2, 0)],
        "find-references works from the definition site"
    );

    // Cursor past the document content (the trailing newline) is on no label/ref.
    let none = references(&client, 5, &uri, Position::new(3, 0), true);
    assert!(none.is_empty(), "no reference under an empty position");

    shutdown(&client, server_thread);
}

fn document_highlight(
    client: &Connection,
    id: i32,
    uri: &Uri,
    position: Position,
) -> Vec<DocumentHighlight> {
    send_request(
        client,
        id,
        "textDocument/documentHighlight",
        serde_json::to_value(DocumentHighlightParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap(),
    );
    let resp = loop {
        match recv(client) {
            Message::Response(resp) => break resp,
            Message::Notification(_) => continue,
            other => panic!("expected a response, got {other:?}"),
        }
    };
    assert_eq!(resp.id, RequestId::from(id));
    // An unknown document replies `null`; a resolved-but-empty highlight is `[]`.
    match resp.result {
        Some(serde_json::Value::Null) | None => Vec::new(),
        Some(v) => serde_json::from_value(v).unwrap(),
    }
}

/// Sort highlights by their start and pair each with its kind, so assertions don't
/// depend on the label-then-ref emission order.
fn highlight_starts(
    mut hl: Vec<DocumentHighlight>,
) -> Vec<(Position, Option<DocumentHighlightKind>)> {
    hl.sort_by_key(|h| (h.range.start.line, h.range.start.character));
    hl.into_iter().map(|h| (h.range.start, h.kind)).collect()
}

#[test]
fn lsp_document_highlight_label_and_refs() {
    let (client, server_thread) = start_server(None);
    let abs = std::path::absolute("def.tex").expect("absolute path");
    let uri = path_to_file_uri(&abs);
    // A label and two references to it in the same buffer.
    let doc = "\\label{sec:intro}\n\\ref{sec:intro}\n\\ref{sec:intro}\n";
    did_open(&client, &uri, 1, doc);
    let _ = recv_diagnostics(&client);

    // Cursor on the first `\ref` key → the key spans of the `\label` definition
    // (WRITE) and both `\ref` uses (READ).
    let expected = vec![
        (Position::new(0, 7), Some(DocumentHighlightKind::WRITE)),
        (Position::new(1, 5), Some(DocumentHighlightKind::READ)),
        (Position::new(2, 5), Some(DocumentHighlightKind::READ)),
    ];
    let from_ref = document_highlight(&client, 2, &uri, Position::new(1, 6));
    assert_eq!(
        highlight_starts(from_ref),
        expected,
        "the \\label definition and both \\ref uses, by key span"
    );

    // The same set when invoked from the `\label` definition key.
    let from_def = document_highlight(&client, 3, &uri, Position::new(0, 8));
    assert_eq!(
        highlight_starts(from_def),
        expected,
        "highlight resolves identically from the definition site"
    );

    // Strict key gating: the cursor on the command word `\ref` (not its key)
    // highlights nothing.
    let on_word = document_highlight(&client, 4, &uri, Position::new(1, 1));
    assert!(on_word.is_empty(), "cursor on the command word, not a key");

    // A position past the content is on no key.
    let none = document_highlight(&client, 5, &uri, Position::new(3, 0));
    assert!(none.is_empty(), "no key under an empty position");

    shutdown(&client, server_thread);
}

#[test]
fn lsp_document_highlight_cref_isolates_each_key() {
    let (client, server_thread) = start_server(None);
    let abs = std::path::absolute("def.tex").expect("absolute path");
    let uri = path_to_file_uri(&abs);
    // `\cref{a,b}` is a multi-key list command; `a` and `b` are independent keys.
    let doc = "\\cref{a,b}\n\\ref{a}\n\\label{a}\n\\label{b}\n";
    did_open(&client, &uri, 1, doc);
    let _ = recv_diagnostics(&client);

    // Cursor on `a` in `\cref{a,b}` → only the `a` family; the sibling `b` and
    // `\label{b}` are untouched.
    let on_a = document_highlight(&client, 2, &uri, Position::new(0, 6));
    assert_eq!(
        highlight_starts(on_a),
        vec![
            (Position::new(0, 6), Some(DocumentHighlightKind::READ)), // `\cref` key `a`
            (Position::new(1, 5), Some(DocumentHighlightKind::READ)), // `\ref{a}`
            (Position::new(2, 7), Some(DocumentHighlightKind::WRITE)), // `\label{a}`
        ],
        "only the `a` key family, the sibling `b` excluded"
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_document_highlight_citation_keys() {
    let (client, server_thread) = start_server(None);
    let abs = std::path::absolute("def.tex").expect("absolute path");
    let uri = path_to_file_uri(&abs);
    let doc = "\\cite{foo}\n\\cite{foo}\n\\cite{bar}\n";
    did_open(&client, &uri, 1, doc);
    let _ = recv_diagnostics(&client);

    // Cursor on a `\cite{foo}` key → both `foo` citations (READ); `bar` excluded.
    let on_foo = document_highlight(&client, 2, &uri, Position::new(0, 7));
    assert_eq!(
        highlight_starts(on_foo),
        vec![
            (Position::new(0, 6), Some(DocumentHighlightKind::READ)),
            (Position::new(1, 6), Some(DocumentHighlightKind::READ)),
        ],
        "both \\cite{{foo}} keys, the \\cite{{bar}} excluded"
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_document_highlight_environment_pair() {
    let (client, server_thread) = start_server(None);
    let abs = std::path::absolute("env.tex").expect("absolute path");
    let uri = path_to_file_uri(&abs);
    let doc = "\\begin{equation}\nx\n\\end{equation}\n";
    did_open(&client, &uri, 1, doc);
    let _ = recv_diagnostics(&client);

    // Cursor on the `\begin` name → both the begin and end name spans (TEXT).
    let expected = vec![
        (Position::new(0, 7), Some(DocumentHighlightKind::TEXT)),
        (Position::new(2, 5), Some(DocumentHighlightKind::TEXT)),
    ];
    let on_begin = document_highlight(&client, 2, &uri, Position::new(0, 8));
    assert_eq!(
        highlight_starts(on_begin),
        expected,
        "the paired \\begin/\\end names, from the \\begin side"
    );

    // The same pair when invoked from the `\end` name.
    let on_end = document_highlight(&client, 3, &uri, Position::new(2, 6));
    assert_eq!(
        highlight_starts(on_end),
        expected,
        "the pair resolves identically from the \\end side"
    );

    // A cursor in the body (not on a delimiter) highlights nothing.
    let in_body = document_highlight(&client, 4, &uri, Position::new(1, 0));
    assert!(in_body.is_empty(), "cursor in the body, not on a delimiter");

    shutdown(&client, server_thread);
}

/// Send a change-environment `workspace/executeCommand` under `command` (the
/// `badness.…` id or its `texlab.…` alias), with the texlab-shaped single
/// `RenameParams` argument.
fn change_environment(
    client: &Connection,
    id: i32,
    command: &str,
    uri: &Uri,
    position: Position,
    new_name: &str,
) {
    send_request(
        client,
        id,
        "workspace/executeCommand",
        serde_json::to_value(ExecuteCommandParams {
            command: command.to_owned(),
            arguments: vec![
                serde_json::to_value(RenameParams {
                    text_document_position: TextDocumentPositionParams {
                        text_document: TextDocumentIdentifier { uri: uri.clone() },
                        position,
                    },
                    new_name: new_name.to_owned(),
                    work_done_progress_params: Default::default(),
                })
                .unwrap(),
            ],
            work_done_progress_params: Default::default(),
        })
        .unwrap(),
    );
}

/// Receive the server's `workspace/applyEdit` push for command `id`, ack it, then
/// assert the command itself resolves with a `null` result. Skips interleaved
/// notifications (diagnostics pushes).
fn recv_apply_edit_then_ok(client: &Connection, id: i32) -> ApplyWorkspaceEditParams {
    let params = loop {
        match recv(client) {
            Message::Request(req) if req.method == "workspace/applyEdit" => {
                let params = serde_json::from_value(req.params).expect("valid applyEdit params");
                client
                    .sender
                    .send(Message::Response(Response::new_ok(
                        req.id,
                        serde_json::json!({ "applied": true }),
                    )))
                    .unwrap();
                break params;
            }
            Message::Notification(_) => continue,
            other => panic!("expected a workspace/applyEdit request, got {other:?}"),
        }
    };
    loop {
        match recv(client) {
            Message::Response(resp) => {
                assert_eq!(resp.id, RequestId::from(id));
                assert!(
                    resp.error.is_none(),
                    "the command must succeed, got {:?}",
                    resp.error
                );
                break;
            }
            Message::Notification(_) => continue,
            other => panic!("expected the command response, got {other:?}"),
        }
    }
    params
}

/// Receive an **error** response to command `id` (no `applyEdit` may precede it)
/// and return its message. Skips interleaved notifications.
fn recv_command_error(client: &Connection, id: i32) -> String {
    loop {
        match recv(client) {
            Message::Response(resp) => {
                assert_eq!(resp.id, RequestId::from(id));
                return resp.error.expect("the command must fail").message;
            }
            Message::Notification(_) => continue,
            other => panic!("expected an error response, got {other:?}"),
        }
    }
}

/// The `(line, start_col, end_col, new_text)` of each edit for `uri`, sorted.
fn edit_summary(params: &ApplyWorkspaceEditParams, uri: &Uri) -> Vec<(u32, u32, u32, String)> {
    let mut edits: Vec<_> = params
        .edit
        .changes
        .as_ref()
        .and_then(|changes| changes.get(uri))
        .expect("edits keyed by the document URI")
        .iter()
        .map(|edit| {
            assert_eq!(edit.range.start.line, edit.range.end.line);
            (
                edit.range.start.line,
                edit.range.start.character,
                edit.range.end.character,
                edit.new_text.clone(),
            )
        })
        .collect();
    edits.sort();
    edits
}

#[test]
fn lsp_change_environment_rewrites_pair() {
    let (client, server_thread) = start_server(None);
    let abs = std::path::absolute("change-env.tex").expect("absolute path");
    let uri = path_to_file_uri(&abs);
    let doc = "\\begin{center}\n\\begin{itemize}\nhi\n\\end{itemize}\n\\end{center}\n";
    did_open(&client, &uri, 1, doc);
    let _ = recv_diagnostics(&client);

    // A cursor in the inner body targets the *innermost* environment.
    change_environment(
        &client,
        2,
        "badness.changeEnvironment",
        &uri,
        Position::new(2, 0),
        "enumerate",
    );
    let params = recv_apply_edit_then_ok(&client, 2);
    assert_eq!(
        edit_summary(&params, &uri),
        vec![
            (1, 7, 14, "enumerate".to_owned()),
            (3, 5, 12, "enumerate".to_owned()),
        ],
        "both itemize names, nothing else"
    );
    assert_eq!(
        params.label.as_deref(),
        Some("change environment: itemize -> enumerate")
    );

    // The texlab alias, invoked from the outer `\end` name, targets the outer pair.
    change_environment(
        &client,
        3,
        "texlab.changeEnvironment",
        &uri,
        Position::new(4, 6),
        "figure",
    );
    let params = recv_apply_edit_then_ok(&client, 3);
    assert_eq!(
        edit_summary(&params, &uri),
        vec![
            (0, 7, 13, "figure".to_owned()),
            (4, 5, 11, "figure".to_owned()),
        ],
        "the outer center pair"
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_change_environment_unclosed_rewrites_begin_only() {
    let (client, server_thread) = start_server(None);
    let abs = std::path::absolute("change-env-unclosed.tex").expect("absolute path");
    let uri = path_to_file_uri(&abs);
    let doc = "\\begin{center}\nhi\n";
    did_open(&client, &uri, 1, doc);
    let _ = recv_diagnostics(&client);

    change_environment(
        &client,
        2,
        "badness.changeEnvironment",
        &uri,
        Position::new(1, 0),
        "figure",
    );
    let params = recv_apply_edit_then_ok(&client, 2);
    assert_eq!(
        edit_summary(&params, &uri),
        vec![(0, 7, 13, "figure".to_owned())],
        "just the unclosed \\begin name"
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_change_environment_declines_without_environment() {
    let (client, server_thread) = start_server(None);
    let abs = std::path::absolute("change-env-none.tex").expect("absolute path");
    let uri = path_to_file_uri(&abs);
    let doc = "hello world\n\\begin{center}\nhi\n\\end{center}\n";
    did_open(&client, &uri, 1, doc);
    let _ = recv_diagnostics(&client);

    // No environment encloses the cursor → an error, never a silent no-op.
    change_environment(
        &client,
        2,
        "badness.changeEnvironment",
        &uri,
        Position::new(0, 2),
        "figure",
    );
    let message = recv_command_error(&client, 2);
    assert!(
        message.contains("no environment"),
        "explains the miss, got {message:?}"
    );

    // A new name that would corrupt the surface syntax is rejected up front.
    change_environment(
        &client,
        3,
        "badness.changeEnvironment",
        &uri,
        Position::new(2, 0),
        "fig{ure",
    );
    let message = recv_command_error(&client, 3);
    assert!(
        message.contains("not a valid environment name"),
        "rejects the brace, got {message:?}"
    );

    // An unknown command id is an error too.
    send_request(
        &client,
        4,
        "workspace/executeCommand",
        serde_json::to_value(ExecuteCommandParams {
            command: "badness.doesNotExist".to_owned(),
            arguments: vec![],
            work_done_progress_params: Default::default(),
        })
        .unwrap(),
    );
    let message = recv_command_error(&client, 4);
    assert!(
        message.contains("unknown workspace command"),
        "got {message:?}"
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_references_cross_file_cite_from_tex_and_bib() {
    // A real on-disk project: the root `\input`s a chapter and `\addbibresource`s a
    // `.bib`. Both the chapter and the root cite the same key.
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("refs.bib"),
        "@article{knuth1984, title={The TeXbook}}\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("part.tex"), "\\cite{knuth1984}\n").unwrap();
    let main_path = dir.path().join("main.tex");
    let main = "\\documentclass{article}\n\
        \\addbibresource{refs.bib}\n\
        \\begin{document}\n\
        \\input{part}\n\
        \\cite{knuth1984}\n\
        \\end{document}\n";
    std::fs::write(&main_path, main).unwrap();

    let (client, server_thread) = start_server(None);
    let main_uri = path_to_file_uri(&main_path);
    did_open(&client, &main_uri, 1, main);
    let _ = recv_diagnostics(&client);

    let part_uri = path_to_file_uri(&dir.path().join("part.tex"));
    let bib_uri = path_to_file_uri(&dir.path().join("refs.bib"));

    // From the `\cite` in main (line 4) → both cite sites across the namespace.
    let uses = references(&client, 2, &main_uri, Position::new(4, 8), false);
    let mut found: Vec<(Uri, u32)> = uses
        .iter()
        .map(|l| (l.uri.clone(), l.range.start.line))
        .collect();
    found.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()).then(a.1.cmp(&b.1)));
    assert_eq!(
        found,
        vec![(main_uri.clone(), 4), (part_uri.clone(), 0)],
        "both \\cite sites, declaration excluded, got {uses:?}"
    );

    // includeDeclaration adds the `.bib` entry.
    let with_decl = references(&client, 3, &main_uri, Position::new(4, 8), true);
    assert!(
        with_decl.iter().any(|l| l.uri == bib_uri),
        "the bib entry is included, got {with_decl:?}"
    );
    assert_eq!(
        with_decl.len(),
        3,
        "two cites + one entry, got {with_decl:?}"
    );

    // Invoking on the `@article` key in the `.bib` finds the same `\cite` uses.
    did_open(
        &client,
        &bib_uri,
        1,
        "@article{knuth1984, title={The TeXbook}}\n",
    );
    let _ = recv_diagnostics(&client);
    let from_bib = references(&client, 4, &bib_uri, Position::new(0, 12), false);
    let mut bib_found: Vec<(Uri, u32)> = from_bib
        .iter()
        .map(|l| (l.uri.clone(), l.range.start.line))
        .collect();
    bib_found.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()).then(a.1.cmp(&b.1)));
    assert_eq!(
        bib_found,
        vec![(main_uri.clone(), 4), (part_uri.clone(), 0)],
        "cite uses resolved from the bib entry, got {from_bib:?}"
    );

    shutdown(&client, server_thread);
}

/// Send a `textDocument/prepareRename` at `position` and return the raw response
/// value (`null` when declined), draining stray diagnostics first.
fn prepare_rename(
    client: &Connection,
    id: i32,
    uri: &Uri,
    position: Position,
) -> serde_json::Value {
    send_request(
        client,
        id,
        "textDocument/prepareRename",
        serde_json::to_value(TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position,
        })
        .unwrap(),
    );
    let resp = loop {
        match recv(client) {
            Message::Response(resp) => break resp,
            Message::Notification(_) => continue,
            other => panic!("expected a response, got {other:?}"),
        }
    };
    assert_eq!(resp.id, RequestId::from(id));
    resp.result.unwrap()
}

/// Send a `textDocument/rename` at `position` and return the resulting per-URI
/// edit map (empty when the server declined with `null`), draining stray
/// diagnostics first.
fn rename(
    client: &Connection,
    id: i32,
    uri: &Uri,
    position: Position,
    new_name: &str,
) -> std::collections::HashMap<Uri, Vec<TextEdit>> {
    send_request(
        client,
        id,
        "textDocument/rename",
        serde_json::to_value(RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            new_name: new_name.to_owned(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .unwrap(),
    );
    let resp = loop {
        match recv(client) {
            Message::Response(resp) => break resp,
            Message::Notification(_) => continue,
            other => panic!("expected a response, got {other:?}"),
        }
    };
    assert_eq!(resp.id, RequestId::from(id));
    match resp.result.unwrap() {
        serde_json::Value::Null => std::collections::HashMap::new(),
        value => serde_json::from_value::<WorkspaceEdit>(value)
            .unwrap()
            .changes
            .unwrap_or_default(),
    }
}

/// Apply a single file's `TextEdit`s to `text` and return the result. Edits are
/// applied last-to-first so earlier byte offsets stay valid.
fn apply_edits(text: &str, edits: &[TextEdit]) -> String {
    let index = badness::text::LineIndex::new(text);
    let mut spans: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|edit| {
            let start = index.offset_at(text, edit.range.start.line, edit.range.start.character);
            let end = index.offset_at(text, edit.range.end.line, edit.range.end.character);
            (start, end, edit.new_text.as_str())
        })
        .collect();
    spans.sort_by_key(|(start, _, _)| *start);
    let mut out = text.to_owned();
    for (start, end, new_text) in spans.into_iter().rev() {
        out.replace_range(start..end, new_text);
    }
    out
}

#[test]
fn lsp_prepare_rename_anchors_to_key_token() {
    let (client, server_thread) = start_server(None);
    let abs = std::path::absolute("def.tex").expect("absolute path");
    let uri = path_to_file_uri(&abs);
    let doc = "\\label{sec:intro}\n\\ref{sec:intro}\n";
    did_open(&client, &uri, 1, doc);
    let _ = recv_diagnostics(&client);

    // Cursor inside the `\ref` key → the prepare range covers exactly `sec:intro`
    // (line 1, characters 5..14), with the key as the placeholder.
    let prepared = prepare_rename(&client, 2, &uri, Position::new(1, 6));
    let response: PrepareRenameResponse = serde_json::from_value(prepared).unwrap();
    match response {
        PrepareRenameResponse::RangeWithPlaceholder { range, placeholder } => {
            assert_eq!(range, Range::new(Position::new(1, 5), Position::new(1, 14)));
            assert_eq!(placeholder, "sec:intro");
        }
        other => panic!("expected RangeWithPlaceholder, got {other:?}"),
    }

    // Cursor on the `\ref` command word (not the key) → declined (`null`).
    let on_command = prepare_rename(&client, 3, &uri, Position::new(1, 2));
    assert!(on_command.is_null(), "the command word is not renameable");

    // Cursor on an empty line → declined.
    let on_nothing = prepare_rename(&client, 4, &uri, Position::new(2, 0));
    assert!(on_nothing.is_null(), "no key under an empty position");

    shutdown(&client, server_thread);
}

#[test]
fn lsp_rename_label_rewrites_def_and_uses_cross_file() {
    // A real on-disk project: the root `\input`s a chapter that defines the label,
    // and references it from both files (one via a `\cref` list alongside a sibling
    // key that must stay untouched).
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("part.tex"),
        "\\label{sec:intro}\n\\cref{sec:intro,other}\n",
    )
    .unwrap();
    let main_path = dir.path().join("main.tex");
    let main = "\\documentclass{article}\n\
        \\begin{document}\n\
        \\input{part}\n\
        \\ref{sec:intro}\n\
        \\end{document}\n";
    std::fs::write(&main_path, main).unwrap();

    let (client, server_thread) = start_server(None);
    let main_uri = path_to_file_uri(&main_path);
    did_open(&client, &main_uri, 1, main);
    let _ = recv_diagnostics(&client);

    let part_uri = path_to_file_uri(&dir.path().join("part.tex"));

    // Rename from the `\ref` in main (line 3) → edits in both files.
    let changes = rename(&client, 2, &main_uri, Position::new(3, 6), "sec:overview");
    assert_eq!(changes.len(), 2, "edits span both files, got {changes:?}");

    // The chapter: the `\label` def and the matching `\cref` key are rewritten; the
    // sibling key `other` is left alone.
    let part_src = "\\label{sec:intro}\n\\cref{sec:intro,other}\n";
    let part_out = apply_edits(part_src, &changes[&part_uri]);
    assert_eq!(
        part_out, "\\label{sec:overview}\n\\cref{sec:overview,other}\n",
        "definition + list key renamed, sibling key untouched"
    );

    // The root: the `\ref` use is rewritten.
    let main_out = apply_edits(main, &changes[&main_uri]);
    assert!(
        main_out.contains("\\ref{sec:overview}"),
        "the \\ref use is renamed, got {main_out:?}"
    );

    // An invalid new name (contains a brace) is declined.
    let declined = rename(&client, 3, &main_uri, Position::new(3, 6), "bad}name");
    assert!(
        declined.is_empty(),
        "a syntactically unsafe key is declined"
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_rename_cite_key_rewrites_entry_and_uses() {
    // The root `\addbibresource`s a `.bib` and cites its key; rename rewrites the
    // `@entry` key and every `\cite` use, case-insensitively.
    let dir = tempfile::tempdir().expect("temp dir");
    let bib_src = "@article{Knuth1984, title={The TeXbook}}\n";
    std::fs::write(dir.path().join("refs.bib"), bib_src).unwrap();
    let main_path = dir.path().join("main.tex");
    let main = "\\documentclass{article}\n\
        \\addbibresource{refs.bib}\n\
        \\begin{document}\n\
        \\cite{knuth1984}\n\
        \\end{document}\n";
    std::fs::write(&main_path, main).unwrap();

    let (client, server_thread) = start_server(None);
    let main_uri = path_to_file_uri(&main_path);
    did_open(&client, &main_uri, 1, main);
    let _ = recv_diagnostics(&client);

    let bib_uri = path_to_file_uri(&dir.path().join("refs.bib"));

    // Rename from the `\cite` use (line 3) → the bib entry and the cite use.
    let changes = rename(&client, 2, &main_uri, Position::new(3, 8), "knuth-texbook");
    assert_eq!(changes.len(), 2, "edits span the .tex and the .bib");

    let bib_out = apply_edits(bib_src, &changes[&bib_uri]);
    assert_eq!(
        bib_out, "@article{knuth-texbook, title={The TeXbook}}\n",
        "the @entry key is rewritten despite the case mismatch"
    );
    let main_out = apply_edits(main, &changes[&main_uri]);
    assert!(
        main_out.contains("\\cite{knuth-texbook}"),
        "the \\cite use is rewritten, got {main_out:?}"
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_lone_fragment_has_no_cross_file_diagnostics() {
    // A bare chapter opened standalone (no `\documentclass`): its namespace is
    // rootless, so `undefined-ref` stays inert even though `\ref{x}` resolves to
    // nothing — exactly the gate that keeps fragments quiet.
    let dir = tempfile::tempdir().expect("temp dir");
    let frag_path = dir.path().join("frag.tex");
    let frag = "\\section{Loose}\n\\ref{nowhere}\n";
    std::fs::write(&frag_path, frag).unwrap();

    let (client, server_thread) = start_server(None);
    let uri = path_to_file_uri(&frag_path);
    did_open(&client, &uri, 1, frag);

    let diags = recv_diagnostics(&client);
    assert_eq!(diags.uri, uri);
    assert!(
        rule_codes(&diags).iter().all(|c| c != "undefined-ref"),
        "a rootless fragment must not flag undefined-ref, got {:?}",
        diags.diagnostics
    );

    shutdown(&client, server_thread);
}

/// Like [`complete`], but drains interleaved notifications (a freshly-opened `.bib`
/// or a seeded project re-lints) before the response.
fn complete_draining(
    client: &Connection,
    id: i32,
    uri: &Uri,
    position: Position,
) -> Vec<CompletionItem> {
    send_request(
        client,
        id,
        "textDocument/completion",
        serde_json::to_value(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .unwrap(),
    );
    let resp = loop {
        match recv(client) {
            Message::Response(resp) => break resp,
            Message::Notification(_) => continue,
            other => panic!("expected a response, got {other:?}"),
        }
    };
    assert_eq!(resp.id, RequestId::from(id));
    match serde_json::from_value::<CompletionResponse>(resp.result.unwrap()).unwrap() {
        CompletionResponse::Array(items) => items,
        CompletionResponse::List(list) => list.items,
    }
}

#[test]
fn lsp_bib_completion_entry_types() {
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///refs.bib".parse().unwrap();

    // Cursor at the end of `@art` → entry-type candidates.
    did_open(&client, &uri, 1, "@art\n");
    let items = complete_draining(&client, 2, &uri, Position::new(0, 4));
    let names = labels(&items);
    assert!(names.contains(&"article"), "{names:?}");
    let article = items.iter().find(|i| i.label == "article").unwrap();
    assert_eq!(article.kind, Some(CompletionItemKind::STRUCT));

    shutdown(&client, server_thread);
}

#[test]
fn lsp_bib_completion_field_names() {
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///fields.bib".parse().unwrap();

    // Cursor at the end of `au` (a field-name position inside an `@article`).
    did_open(&client, &uri, 1, "@article{k,\n  au\n}\n");
    let items = complete_draining(&client, 2, &uri, Position::new(1, 4));
    let names = labels(&items);
    assert!(names.contains(&"author"), "{names:?}");
    let author = items.iter().find(|i| i.label == "author").unwrap();
    assert_eq!(author.kind, Some(CompletionItemKind::FIELD));

    shutdown(&client, server_thread);
}

#[test]
fn lsp_bib_completion_string_macros() {
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///strings.bib".parse().unwrap();

    // A `@string` def then a value position referencing it → macro candidates.
    let doc = "@string{els = {Elsevier}}\n@article{k, publisher = e}\n";
    did_open(&client, &uri, 1, doc);
    let items = complete_draining(&client, 2, &uri, Position::new(1, 25));
    let names = labels(&items);
    assert!(names.contains(&"els"), "{names:?}");
    let els = items.iter().find(|i| i.label == "els").unwrap();
    assert_eq!(els.kind, Some(CompletionItemKind::CONSTANT));

    shutdown(&client, server_thread);
}

#[test]
fn lsp_cite_completion_cross_file() {
    // A real on-disk project: the root `\addbibresource`s a `.bib` defining the key.
    // Only the root is opened; the server seeds the sibling and offers its keys.
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("refs.bib"),
        "@article{knuth1984, title={The TeXbook}}\n",
    )
    .unwrap();
    let main_path = dir.path().join("main.tex");
    let main = "\\addbibresource{refs.bib}\n\\cite{kn}\n";
    std::fs::write(&main_path, main).unwrap();

    let (client, server_thread) = start_server(None);
    let uri = path_to_file_uri(&main_path);
    did_open(&client, &uri, 1, main);

    // Cursor inside `\cite{kn|}` → the project's entry key, prefix-filtered.
    let items = complete_draining(&client, 2, &uri, Position::new(1, 8));
    let names = labels(&items);
    assert!(names.contains(&"knuth1984"), "{names:?}");
    let key = items.iter().find(|i| i.label == "knuth1984").unwrap();
    assert_eq!(key.kind, Some(CompletionItemKind::REFERENCE));

    shutdown(&client, server_thread);
}

#[test]
fn lsp_gls_completion_cross_file() {
    // A real on-disk project: the root defines an acronym in its preamble and
    // `\input`s a chapter that uses `\gls`. Only the chapter is opened; the
    // server seeds the sibling root and offers its keys across the namespace.
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("main.tex"),
        "\\documentclass{article}\n\\newacronym{fps}{FPS}{frames per second}\n\\input{chap}\n",
    )
    .unwrap();
    let chap_path = dir.path().join("chap.tex");
    let chap = "\\gls{fp}\n";
    std::fs::write(&chap_path, chap).unwrap();

    let (client, server_thread) = start_server(None);
    let uri = path_to_file_uri(&chap_path);
    did_open(&client, &uri, 1, chap);

    // Cursor inside `\gls{fp|}` → the preamble-defined key, prefix-filtered.
    let items = complete_draining(&client, 2, &uri, Position::new(0, 7));
    let names = labels(&items);
    assert!(names.contains(&"fps"), "{names:?}");
    let key = items.iter().find(|i| i.label == "fps").unwrap();
    assert_eq!(key.kind, Some(CompletionItemKind::REFERENCE));

    shutdown(&client, server_thread);
}

#[test]
fn lsp_gls_completion_via_loadglsentries() {
    // Entries live in a dedicated file pulled in by `\loadglsentries`; the edge
    // joins it to the document namespace, so its keys complete in the root.
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("entries.tex"),
        "\\newglossaryentry{ex}{name={example},description={an example}}\n",
    )
    .unwrap();
    let main_path = dir.path().join("main.tex");
    let main = "\\loadglsentries{entries}\n\\gls{e}\n";
    std::fs::write(&main_path, main).unwrap();

    let (client, server_thread) = start_server(None);
    let uri = path_to_file_uri(&main_path);
    did_open(&client, &uri, 1, main);

    // Cursor inside `\gls{e|}`.
    let items = complete_draining(&client, 2, &uri, Position::new(1, 6));
    let names = labels(&items);
    assert!(names.contains(&"ex"), "{names:?}");

    shutdown(&client, server_thread);
}

/// Helper: send a whole-buffer `didChange` (version `v`) replacing the document text.
fn did_change_full(client: &Connection, uri: &Uri, v: i32, text: &str) {
    send_notification(
        client,
        "textDocument/didChange",
        serde_json::to_value(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: v,
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

#[test]
fn lsp_pull_diagnostics_suppress_push_and_report_full() {
    let (client, server_thread) = start_server_pull();
    let uri: Uri = "file:///pull.tex".parse().unwrap();

    // didOpen a broken document. A pull-capable client must NOT receive a push.
    did_open(&client, &uri, 1, "\\begin{itemize}\n\\item a\n");

    // Pull on demand: the report carries the parse error.
    pull_diagnostic(&client, 10, &uri, None);
    let report = recv_document_diagnostic_report(&client, 10);
    let items = report_items(&report).expect("a broken document yields a full report");
    assert!(
        !items.is_empty(),
        "an unclosed environment must produce at least one pulled diagnostic"
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_pull_diagnostics_current_right_after_change() {
    // The regression guard mirroring panache's fix: a pull issued immediately after
    // an edit (without waiting) must reflect the CURRENT buffer, never one behind.
    let (client, server_thread) = start_server_pull();
    let uri: Uri = "file:///pull_change.tex".parse().unwrap();

    // Open broken, then change to a valid document and pull at once.
    did_open(&client, &uri, 1, "\\begin{itemize}\n\\item a\n");
    did_change_full(&client, &uri, 2, "\\section{Hi}\n\ntext.\n");
    pull_diagnostic(&client, 11, &uri, None);

    let report = recv_document_diagnostic_report(&client, 11);
    let items = report_items(&report).expect("expected a full report");
    assert!(
        items.is_empty(),
        "the pull must reflect the fixed buffer, got {items:?}"
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_pull_diagnostics_unchanged_result_id() {
    let (client, server_thread) = start_server_pull();
    let uri: Uri = "file:///pull_unchanged.tex".parse().unwrap();

    did_open(&client, &uri, 1, "\\section{Hi}\n\ntext.\n");

    // First pull: a full report with a result_id.
    pull_diagnostic(&client, 12, &uri, None);
    let first = recv_document_diagnostic_report(&client, 12);
    assert!(matches!(first, DocumentDiagnosticReport::Full(_)));
    let result_id = report_result_id(&first).expect("full report carries a result_id");

    // Second pull with that result_id and no edit between: an unchanged report.
    pull_diagnostic(&client, 13, &uri, Some(result_id.clone()));
    let second = recv_document_diagnostic_report(&client, 13);
    assert!(
        matches!(second, DocumentDiagnosticReport::Unchanged(_)),
        "an unchanged document must report `unchanged`, got {second:?}"
    );
    assert_eq!(report_result_id(&second), Some(result_id));

    shutdown(&client, server_thread);
}

#[test]
fn lsp_push_client_still_receives_pushes() {
    // A client that does NOT advertise pull support keeps the push model.
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///push.tex".parse().unwrap();

    did_open(&client, &uri, 1, "\\begin{itemize}\n\\item a\n");
    let diags = recv_diagnostics(&client);
    assert_eq!(diags.uri, uri);
    assert!(
        !diags.diagnostics.is_empty(),
        "push-mode client must still receive pushed diagnostics"
    );

    shutdown(&client, server_thread);
}

/// Handshake as a client that advertises dynamic file-watcher registration. After
/// `initialized` the server registers watchers via a `client/registerCapability`
/// request; capture and ack it, returning its [`RegistrationParams`].
fn start_server_watching() -> (Connection, std::thread::JoinHandle<()>, RegistrationParams) {
    let (server, client) = Connection::memory();
    let server_thread = std::thread::spawn(move || badness::lsp::serve(server).unwrap());

    let params = InitializeParams {
        capabilities: ClientCapabilities {
            workspace: Some(WorkspaceClientCapabilities {
                did_change_watched_files: Some(DidChangeWatchedFilesClientCapabilities {
                    dynamic_registration: Some(true),
                    relative_pattern_support: None,
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    send_request(
        &client,
        1,
        "initialize",
        serde_json::to_value(params).unwrap(),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(1));
    send_notification(
        &client,
        "initialized",
        serde_json::to_value(InitializedParams {}).unwrap(),
    );
    let reg = match recv(&client) {
        Message::Request(req) if req.method == "client/registerCapability" => {
            client
                .sender
                .send(Message::Response(Response::new_ok(
                    req.id,
                    serde_json::Value::Null,
                )))
                .unwrap();
            serde_json::from_value::<RegistrationParams>(req.params)
                .expect("valid RegistrationParams")
        }
        other => panic!("expected client/registerCapability, got {other:?}"),
    };
    (client, server_thread, reg)
}

/// Send a `workspace/didChangeWatchedFiles` notification for the given `(uri, type)`
/// events.
fn did_change_watched_files(client: &Connection, events: &[(Uri, FileChangeType)]) {
    let changes = events
        .iter()
        .map(|(uri, typ)| FileEvent::new(uri.clone(), *typ))
        .collect();
    send_notification(
        client,
        "workspace/didChangeWatchedFiles",
        serde_json::to_value(DidChangeWatchedFilesParams { changes }).unwrap(),
    );
}

/// Drain server messages until a `publishDiagnostics` for `uri` whose rule codes
/// satisfy `want`, acking any server→client request along the way. Panics on timeout.
fn recv_diagnostics_matching(
    client: &Connection,
    uri: &Uri,
    want: impl Fn(&[String]) -> bool,
) -> PublishDiagnosticsParams {
    loop {
        match recv(client) {
            Message::Notification(not) if not.method == "textDocument/publishDiagnostics" => {
                let diags: PublishDiagnosticsParams =
                    serde_json::from_value(not.params).expect("valid PublishDiagnosticsParams");
                if &diags.uri == uri && want(&rule_codes(&diags)) {
                    return diags;
                }
            }
            Message::Notification(_) => {}
            Message::Request(req) => {
                client
                    .sender
                    .send(Message::Response(Response::new_ok(
                        req.id,
                        serde_json::Value::Null,
                    )))
                    .unwrap();
            }
            Message::Response(_) => {}
        }
    }
}

#[test]
fn lsp_registers_file_watchers_on_initialized() {
    // A watcher-capable client receives a `client/registerCapability` for
    // `workspace/didChangeWatchedFiles` covering both the project leaves and the config.
    let (client, server_thread, reg) = start_server_watching();
    assert_eq!(reg.registrations.len(), 1, "one registration: {reg:?}");
    let r = &reg.registrations[0];
    assert_eq!(r.method, "workspace/didChangeWatchedFiles");
    let opts = r.register_options.as_ref().expect("watcher options");
    let globs: Vec<String> = opts["watchers"]
        .as_array()
        .expect("watchers array")
        .iter()
        .map(|w| w["globPattern"].as_str().expect("string glob").to_owned())
        .collect();
    assert!(
        globs.contains(&"**/*.{tex,bib}".to_owned()),
        "expected the tex/bib glob, got {globs:?}"
    );
    assert!(
        globs.contains(&"**/badness.toml".to_owned()),
        "expected the config glob, got {globs:?}"
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_no_watcher_registration_without_capability() {
    // The default client advertises no dynamic registration, so the server must not
    // send a `client/registerCapability`. `initialized` is processed before `didOpen`,
    // so a stray registration would arrive ahead of the diagnostics push — asserting
    // the first message is `publishDiagnostics` is sufficient.
    let (client, server_thread) = start_server(None);
    let uri: Uri = "file:///nowatch.tex".parse().unwrap();
    did_open(&client, &uri, 1, "\\bf hi\n");
    let diags = recv_diagnostics(&client); // panics if a register request comes first
    assert_eq!(diags.uri, uri);

    shutdown(&client, server_thread);
}

#[test]
fn lsp_watched_tex_change_reanalyzes_open_doc() {
    // A root references a label defined in a non-open `\input` sibling. Editing that
    // sibling on disk (out of editor) and signalling the watcher makes the now-dangling
    // `\ref` fire `undefined-ref` in the still-open root.
    let dir = tempfile::tempdir().expect("temp dir");
    let part_path = dir.path().join("part.tex");
    std::fs::write(&part_path, "\\label{sec:intro}\n").unwrap();
    let main_path = dir.path().join("main.tex");
    let main = "\\documentclass{article}\n\
        \\begin{document}\n\
        \\input{part}\n\
        \\ref{sec:intro}\n\
        \\end{document}\n";
    std::fs::write(&main_path, main).unwrap();

    let (client, server_thread) = start_server(None);
    let uri = path_to_file_uri(&main_path);
    did_open(&client, &uri, 1, main);
    // Initially the label resolves cross-file: no `undefined-ref`.
    recv_diagnostics_matching(&client, &uri, |codes| {
        !codes.iter().any(|c| c == "undefined-ref")
    });

    // Drop the label on disk, then notify the watcher.
    std::fs::write(&part_path, "% the label is gone now\n").unwrap();
    did_change_watched_files(
        &client,
        &[(path_to_file_uri(&part_path), FileChangeType::CHANGED)],
    );

    let diags = recv_diagnostics_matching(&client, &uri, |codes| {
        codes.iter().any(|c| c == "undefined-ref")
    });
    assert!(
        rule_codes(&diags).iter().any(|c| c == "undefined-ref"),
        "expected undefined-ref after the label was removed on disk, got {:?}",
        diags.diagnostics
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_watched_bib_change_reanalyzes_open_doc() {
    // The same shape over a `.bib` resource: rewriting the bibliography on disk to drop
    // the cited key makes `undefined-citation` fire in the open root.
    let dir = tempfile::tempdir().expect("temp dir");
    let bib_path = dir.path().join("refs.bib");
    std::fs::write(&bib_path, "@article{knuth1984, title={The TeXbook}}\n").unwrap();
    let main_path = dir.path().join("main.tex");
    let main = "\\documentclass{article}\n\
        \\addbibresource{refs.bib}\n\
        \\begin{document}\n\
        \\cite{knuth1984}\n\
        \\end{document}\n";
    std::fs::write(&main_path, main).unwrap();

    let (client, server_thread) = start_server(None);
    let uri = path_to_file_uri(&main_path);
    did_open(&client, &uri, 1, main);
    recv_diagnostics_matching(&client, &uri, |codes| {
        !codes.iter().any(|c| c == "undefined-citation")
    });

    std::fs::write(&bib_path, "@article{lamport1986, title={LaTeX}}\n").unwrap();
    did_change_watched_files(
        &client,
        &[(path_to_file_uri(&bib_path), FileChangeType::CHANGED)],
    );

    let diags = recv_diagnostics_matching(&client, &uri, |codes| {
        codes.iter().any(|c| c == "undefined-citation")
    });
    assert!(
        rule_codes(&diags).iter().any(|c| c == "undefined-citation"),
        "expected undefined-citation after the key was removed on disk, got {:?}",
        diags.diagnostics
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_watched_config_change_reanalyzes_open_doc() {
    // Dropping a `badness.toml` beside an open doc (out of editor) and signalling the
    // watcher re-resolves settings: a rule disabled in the new config stops firing.
    let dir = tempfile::tempdir().expect("temp dir");
    let main_path = dir.path().join("main.tex");
    let main = "\\bf hi\n"; // `\bf` trips the `deprecated-command` rule
    std::fs::write(&main_path, main).unwrap();

    let (client, server_thread) = start_server(None);
    let uri = path_to_file_uri(&main_path);
    did_open(&client, &uri, 1, main);
    let diags = recv_diagnostics_matching(&client, &uri, |codes| {
        codes.iter().any(|c| c == "deprecated-command")
    });
    assert!(
        rule_codes(&diags).iter().any(|c| c == "deprecated-command"),
        "expected deprecated-command before the config ignores it, got {:?}",
        diags.diagnostics
    );

    // Write a config that ignores the rule, then notify the watcher.
    let config_path = dir.path().join("badness.toml");
    std::fs::write(&config_path, "[lint]\nignore = [\"deprecated-command\"]\n").unwrap();
    did_change_watched_files(
        &client,
        &[(path_to_file_uri(&config_path), FileChangeType::CREATED)],
    );

    let diags = recv_diagnostics_matching(&client, &uri, |codes| {
        !codes.iter().any(|c| c == "deprecated-command")
    });
    assert!(
        !rule_codes(&diags).iter().any(|c| c == "deprecated-command"),
        "deprecated-command must stop firing once the config ignores it, got {:?}",
        diags.diagnostics
    );

    shutdown(&client, server_thread);
}

#[test]
fn lsp_watched_change_for_open_buffer_is_ignored() {
    // A watcher event for a file open in the editor must not clobber the live buffer
    // with disk text: the editor overlay is authoritative. Open a clean buffer, change
    // the file on disk to something dirty, and assert no new diagnostics fire for it.
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("open.tex");
    std::fs::write(&path, "\\bf on disk\n").unwrap(); // dirty on disk
    let uri = path_to_file_uri(&path);

    let (client, server_thread) = start_server(None);
    // The editor buffer is clean (no deprecated command), unlike the disk.
    did_open(&client, &uri, 1, "clean buffer\n");
    let diags = recv_diagnostics(&client);
    assert_eq!(diags.uri, uri);
    assert!(
        !rule_codes(&diags).iter().any(|c| c == "deprecated-command"),
        "the clean buffer must not report the disk's deprecated command, got {:?}",
        diags.diagnostics
    );

    // A watcher event for the open file must be ignored (no re-read of the dirty disk).
    did_change_watched_files(&client, &[(uri.clone(), FileChangeType::CHANGED)]);

    // The buffer is still clean. Round-trip a real edit to flush the pipeline: if the
    // watcher event had wrongly re-read disk, a deprecated-command push would be queued
    // ahead of this edit's push. We assert the next push for the buffer stays clean.
    did_change_full(&client, &uri, 2, "still clean\n");
    let diags = recv_diagnostics_matching(&client, &uri, |_| true);
    assert!(
        !rule_codes(&diags).iter().any(|c| c == "deprecated-command"),
        "an open buffer's diagnostics must track the overlay, not disk, got {:?}",
        diags.diagnostics
    );

    shutdown(&client, server_thread);
}

/// A client offering `utf-8` in `general.positionEncodings` is answered with a
/// `positionEncoding: "utf-8"` capability, and every position on the wire then
/// counts columns in bytes: symbol ranges come back byte-counted, and a ranged
/// `didChange` splice is interpreted byte-wise. ("→" is 3 UTF-8 bytes but 1
/// UTF-16 unit, so the two encodings are cleanly told apart.)
#[test]
fn lsp_negotiates_utf8_position_encoding() {
    let (server, client) = Connection::memory();
    let server_thread = std::thread::spawn(move || badness::lsp::serve(server).unwrap());

    let params = InitializeParams {
        capabilities: ClientCapabilities {
            general: Some(GeneralClientCapabilities {
                position_encodings: Some(vec![
                    PositionEncodingKind::UTF8,
                    PositionEncodingKind::UTF16,
                ]),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    send_request(
        &client,
        1,
        "initialize",
        serde_json::to_value(params).unwrap(),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(1));
    let init: InitializeResult = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert_eq!(
        init.capabilities.position_encoding,
        Some(PositionEncodingKind::UTF8),
        "a client offering utf-8 must be served utf-8 positions"
    );
    send_notification(
        &client,
        "initialized",
        serde_json::to_value(InitializedParams {}).unwrap(),
    );

    let uri: Uri = "file:///utf8.tex".parse().unwrap();
    // Byte layout: "→" 0..3, "\section" 3..11, "{" 11, "Intro" 12..17, "}" 17.
    did_open(&client, &uri, 1, "→\\section{Intro}\n");
    let diags = recv_diagnostics(&client);
    assert!(diags.diagnostics.is_empty(), "clean doc → no diagnostics");

    let symbols = |id: i32, client: &Connection| -> Vec<DocumentSymbol> {
        send_request(
            client,
            id,
            "textDocument/documentSymbol",
            serde_json::to_value(DocumentSymbolParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            })
            .unwrap(),
        );
        let resp = recv_response(client);
        assert_eq!(resp.id, RequestId::from(id));
        match serde_json::from_value(resp.result.unwrap()).expect("a documentSymbol response") {
            DocumentSymbolResponse::Nested(symbols) => symbols,
            other => panic!("expected a nested documentSymbol response, got {other:?}"),
        }
    };

    // Output positions count bytes: the section starts after the 3-byte arrow
    // (a UTF-16 server would say character 1).
    let syms = symbols(2, &client);
    assert_eq!(syms.len(), 1);
    assert_eq!(syms[0].name, "Intro");
    assert_eq!(syms[0].range.start, Position::new(0, 3));

    // Input positions count bytes too: splice "Intro" (bytes 12..17) by its
    // byte-counted range. A UTF-16 server would splice inside "\section".
    send_notification(
        &client,
        "textDocument/didChange",
        serde_json::to_value(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: Some(Range {
                    start: Position::new(0, 12),
                    end: Position::new(0, 17),
                }),
                range_length: None,
                text: "Body".to_owned(),
            }],
        })
        .unwrap(),
    );
    let diags = recv_diagnostics(&client);
    assert!(
        diags.diagnostics.is_empty(),
        "the byte-spliced doc still parses cleanly, got {:?}",
        diags.diagnostics
    );

    let syms = symbols(3, &client);
    assert_eq!(syms.len(), 1);
    assert_eq!(
        syms[0].name, "Body",
        "the didChange range must be interpreted in utf-8"
    );

    shutdown(&client, server_thread);
}
