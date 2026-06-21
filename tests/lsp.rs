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
    CompletionItem, CompletionItemKind, CompletionParams, CompletionResponse,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentFormattingParams, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse,
    FoldingRange, FoldingRangeKind, FoldingRangeParams, FoldingRangeProviderCapability,
    FormattingOptions, GotoDefinitionParams, GotoDefinitionResponse, InitializeParams,
    InitializeResult, InitializedParams, InsertTextFormat, Location, NumberOrString, OneOf,
    PartialResultParams, Position, PublishDiagnosticsParams, Range, ReferenceContext,
    ReferenceParams, SymbolKind, TextDocumentContentChangeEvent, TextDocumentIdentifier,
    TextDocumentItem, TextDocumentPositionParams, TextEdit, Uri, VersionedTextDocumentIdentifier,
    WorkDoneProgressParams,
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
        matches!(
            init.capabilities.document_symbol_provider,
            Some(OneOf::Left(true))
        ),
        "server must advertise documentSymbolProvider"
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
            init.capabilities.folding_range_provider,
            Some(FoldingRangeProviderCapability::Simple(true))
        ),
        "server must advertise foldingRangeProvider"
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
