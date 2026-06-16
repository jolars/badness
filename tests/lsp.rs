//! End-to-end smoke test for the minimal LSP server (Phase 4).
//!
//! Drives the full transcript over an in-process `Connection::memory()` pair:
//! `initialize` → `initialized` → `didOpen` (a doc with a parse error) →
//! assert pushed diagnostics → `didChange` (to a valid, messy doc) → assert the
//! diagnostics clear → `textDocument/formatting` → assert the edit equals the
//! formatter's own output → `shutdown` → `exit`.

use std::time::Duration;

use badness::formatter::{FormatStyle, format_with_style};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentFormattingParams, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse,
    FormattingOptions, InitializeParams, InitializeResult, InitializedParams, OneOf, Position,
    PublishDiagnosticsParams, Range, SymbolKind, TextDocumentContentChangeEvent,
    TextDocumentIdentifier, TextDocumentItem, TextEdit, Uri, VersionedTextDocumentIdentifier,
};

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
        matches!(
            init.capabilities.document_symbol_provider,
            Some(OneOf::Left(true))
        ),
        "server must advertise documentSymbolProvider"
    );
    assert!(init.capabilities.text_document_sync.is_some());
    send_notification(
        &client,
        "initialized",
        serde_json::to_value(InitializedParams {}).unwrap(),
    );
    (client, server_thread)
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
    let resp = recv_response(client);
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
