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
    DidChangeTextDocumentParams, DidOpenTextDocumentParams, DocumentFormattingParams,
    FormattingOptions, InitializeParams, InitializeResult, InitializedParams,
    PublishDiagnosticsParams, TextDocumentContentChangeEvent, TextDocumentIdentifier,
    TextDocumentItem, TextEdit, Uri, VersionedTextDocumentIdentifier,
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

#[test]
fn lsp_formatting_and_diagnostics_transcript() {
    let (server, client) = Connection::memory();
    let server_thread = std::thread::spawn(move || badness::lsp::serve(server).unwrap());

    // initialize → expect the formatting capability advertised.
    send_request(
        &client,
        1,
        "initialize",
        serde_json::to_value(InitializeParams::default()).unwrap(),
    );
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(1));
    let init: InitializeResult = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert!(
        init.capabilities.document_formatting_provider.is_some(),
        "server must advertise documentFormattingProvider"
    );
    assert!(init.capabilities.text_document_sync.is_some());
    send_notification(
        &client,
        "initialized",
        serde_json::to_value(InitializedParams {}).unwrap(),
    );

    let uri: Uri = "file:///test.tex".parse().unwrap();

    // didOpen a document with an unclosed environment → diagnostics.
    let broken = "\\begin{itemize}\n\\item a\n";
    send_notification(
        &client,
        "textDocument/didOpen",
        serde_json::to_value(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "latex".to_owned(),
                version: 1,
                text: broken.to_owned(),
            },
        })
        .unwrap(),
    );
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

    // shutdown → exit.
    send_request(&client, 3, "shutdown", serde_json::Value::Null);
    let resp = recv_response(&client);
    assert_eq!(resp.id, RequestId::from(3));
    send_notification(&client, "exit", serde_json::Value::Null);

    server_thread.join().expect("server thread panicked");
}
