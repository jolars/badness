//! The badness language server (Phase 4 + the ra-style threading follow-up).
//!
//! badness uses **`lsp-server` + `lsp-types`** (rust-analyzer's synchronous
//! stack), *not* tower-lsp-server — see the LSP note in `AGENTS.md`. salsa's
//! single-writer / snapshot-readers model composes cleanly with `lsp-server`'s
//! sync main loop.
//!
//! Scope: full-document **formatting**, a **document-symbol** outline,
//! **completion** (command/environment names, `\ref` keys, file paths), and
//! pushed parser **diagnostics**. Further features (hover, go-to-def, range
//! formatting) are deferred.
//!
//! ## Architecture
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

mod code_action;
mod completion_resolve;
mod document_link;
mod folding;
mod forward_search;
mod hover;
mod name_refs;
mod selection_range;
mod signature_help;
mod task_pool;

use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::SystemTime;

use crossbeam_channel::{Receiver, Sender, never, select, unbounded};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeConfiguration, DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument,
    DidOpenTextDocument, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    ApplyWorkspaceEdit, CodeActionRequest, Completion, DocumentDiagnosticRequest,
    DocumentHighlightRequest, DocumentLinkRequest, DocumentSymbolRequest, ExecuteCommand,
    FoldingRangeRequest, Formatting, GotoDefinition, HoverRequest, OnTypeFormatting,
    PrepareRenameRequest, RangeFormatting, References, RegisterCapability, Rename, Request as _,
    ResolveCompletionItem, SelectionRangeRequest, ShowDocument, SignatureHelpRequest,
    WorkspaceDiagnosticRefresh, WorkspaceSymbolRequest,
};
use lsp_types::{
    ApplyWorkspaceEditParams, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, CodeDescription, CompletionItem, CompletionItemKind,
    CompletionList, CompletionOptions, CompletionParams, CompletionResponse, Diagnostic,
    DiagnosticOptions, DiagnosticRelatedInformation, DiagnosticServerCapabilities,
    DiagnosticSeverity, DiagnosticTag, DidChangeConfigurationParams, DidChangeTextDocumentParams,
    DidChangeWatchedFilesParams, DidChangeWatchedFilesRegistrationOptions,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentDiagnosticParams,
    DocumentDiagnosticReport, DocumentDiagnosticReportResult, DocumentFormattingParams,
    DocumentHighlight, DocumentHighlightKind, DocumentHighlightParams, DocumentLink,
    DocumentLinkOptions, DocumentLinkParams, DocumentOnTypeFormattingOptions,
    DocumentOnTypeFormattingParams, DocumentRangeFormattingParams, DocumentSymbol,
    DocumentSymbolParams, DocumentSymbolResponse, ExecuteCommandOptions, ExecuteCommandParams,
    FileChangeType, FileSystemWatcher, FoldingRange, FoldingRangeParams,
    FoldingRangeProviderCapability, FullDocumentDiagnosticReport, GlobPattern,
    GotoDefinitionParams, GotoDefinitionResponse, HoverParams, HoverProviderCapability,
    InsertTextFormat, Location, NumberOrString, OneOf, Position, PositionEncodingKind,
    PrepareRenameResponse, PublishDiagnosticsParams, Range, ReferenceParams, Registration,
    RegistrationParams, RelatedFullDocumentDiagnosticReport,
    RelatedUnchangedDocumentDiagnosticReport, RenameOptions, RenameParams, SelectionRange,
    SelectionRangeParams, SelectionRangeProviderCapability, ServerCapabilities, ShowDocumentParams,
    SignatureHelpOptions, SignatureHelpParams, SymbolKind, TextDocumentContentChangeEvent,
    TextDocumentPositionParams, TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit,
    UnchangedDocumentDiagnosticReport, Uri, WorkspaceEdit, WorkspaceSymbol, WorkspaceSymbolParams,
    WorkspaceSymbolResponse,
};
use rowan::{TextRange, TextSize};
use salsa::Database as _;
use serde::Deserialize;
use smol_str::SmolStr;

use crate::ast::{AstNode, Environment};
use crate::bib::completion::{
    BibCandidateKind, BibCompletionCandidate, bib_candidates, classify_bib_context,
};
use crate::bib::outline::{BibOutlineItem, outline as bib_outline};
use crate::bib::semantic::Model as BibModel;
use crate::bib::{
    format_node as bib_format_node, format_with_style as bib_format_with_style, parse as bib_parse,
};
use crate::completion::{CandidateKind, CompletionCandidate, CompletionContext, FileArgKind};
use crate::config::{BuildConfig, Config, LintConfig};
use crate::declarations::ResolvedDeclarations;
use crate::file_discovery::{ExcludeFilter, FileKind, collect_lint_files, file_kind_or_tex};
use crate::formatter::sentence::{SentenceLanguage, resolve_owned};
use crate::formatter::{
    FormatStyle, SentenceOptions, WrapMode, declared_scope,
    format_node_range_with_signatures_sentence, format_node_with_signatures_sentence,
    format_with_declarations_sentence,
};
use crate::incremental::{Analysis, IncrementalDatabase, IncrementalDb, file_cite_facts};
use crate::linter::{RuleSelection, Severity, lint_document};
use crate::parser::{Edit, parse, parse_with_declarations};
use crate::project::aux::AuxData;
use crate::project::texmf::{TexmfConfig, TexmfIndex};
use crate::project::{BibTarget, PackageGraph, ResolvedCitations, ResolvedLabels};
use crate::semantic::{
    DefSiteKind, OutlineItem, OutlineSymbol, SemanticModel, SignatureDb, outline,
    scan_definition_sites,
};
use crate::syntax::{SyntaxKind, SyntaxNode};
use crate::text::{LineIndex, LineTable, PositionEncoding, TextBuffer};
use forward_search::{ForwardSearchRequest, ForwardSearchStatus};
use name_refs::{NameKind, NameTarget};

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
///
/// The two-step `initialize_start`/`initialize_finish` handshake (rather than the
/// one-shot `Connection::initialize`) is what lets [`server_capabilities`] depend
/// on the *client's* params: the advertised position encoding and the pull-
/// diagnostics provider are negotiated from what the client reports.
pub fn serve(connection: Connection) -> Result<(), DynError> {
    let (initialize_id, init_params) = connection.initialize_start()?;
    let encoding = negotiate_position_encoding(&init_params);
    let (supports_pull_diagnostics, _) = client_diagnostic_support(&init_params);
    let capabilities =
        serde_json::to_value(server_capabilities(encoding, supports_pull_diagnostics))?;
    connection.initialize_finish(
        initialize_id,
        serde_json::json!({ "capabilities": capabilities }),
    )?;
    main_loop(connection, init_params, encoding)
}

/// Pick the position encoding from the client's `general.positionEncodings`:
/// UTF-8 when offered (columns are then plain byte distances — no per-line
/// re-count), else the protocol-mandatory UTF-16 default (also the fallback for
/// a pre-3.17 client that sends no offer). Advertised back via
/// `ServerCapabilities::position_encoding` and honored by every [`LineIndex`]
/// conversion (each is built with [`LineIndex::with_encoding`]).
fn negotiate_position_encoding(init_params: &serde_json::Value) -> PositionEncoding {
    let offers = init_params
        .get("capabilities")
        .and_then(|c| c.get("general"))
        .and_then(|g| g.get("positionEncodings"))
        .and_then(serde_json::Value::as_array);
    match offers {
        Some(list) if list.iter().any(|v| v.as_str() == Some("utf-8")) => PositionEncoding::Utf8,
        _ => PositionEncoding::Utf16,
    }
}

/// Advertise what we support: **incremental** text sync + whole-document
/// formatting. Diagnostics are offered both ways — *pushed* via
/// `publishDiagnostics` (the default, needing no flag) and *pulled* via
/// `textDocument/diagnostic` (the `diagnostic_provider` capability, advertised
/// only to a client that reports pull support). A client that advertises pull
/// support is served pull-only; everyone else keeps push (see
/// `supports_pull_diagnostics`). `workspace/diagnostic` is deferred (see `TODO.md`).
/// The change-environment refactor's `workspace/executeCommand` id, plus the
/// texlab-compatible alias so an editor keybinding written for texlab
/// (`texlab.changeEnvironment`) works against badness unchanged. Both take one
/// [`RenameParams`]-shaped argument (texlab's wire format).
const CHANGE_ENVIRONMENT_COMMAND: &str = "badness.changeEnvironment";
const CHANGE_ENVIRONMENT_COMMAND_TEXLAB: &str = "texlab.changeEnvironment";

fn server_capabilities(
    encoding: PositionEncoding,
    supports_pull_diagnostics: bool,
) -> ServerCapabilities {
    ServerCapabilities {
        // Echo the negotiated encoding (see `negotiate_position_encoding`).
        position_encoding: Some(match encoding {
            PositionEncoding::Utf8 => PositionEncodingKind::UTF8,
            PositionEncoding::Utf16 => PositionEncodingKind::UTF16,
        }),
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            TextDocumentSyncKind::INCREMENTAL,
        )),
        diagnostic_provider: supports_pull_diagnostics.then(|| {
            DiagnosticServerCapabilities::Options(DiagnosticOptions {
                identifier: Some("badness".to_owned()),
                // Editing an `\input` target / `.bib` changes this file's
                // `undefined-ref` / `undefined-citation` set, so a pull in one file
                // can depend on another's content.
                inter_file_dependencies: true,
                // Deferred: workspace pull is a streaming/long-poll protocol that
                // fits the one-shot read-job model poorly (see `TODO.md`).
                workspace_diagnostics: false,
                work_done_progress_options: Default::default(),
            })
        }),
        document_formatting_provider: Some(OneOf::Left(true)),
        // Format the editor selection, expanded to whole document-level blocks (see
        // `compute_range_format`).
        document_range_formatting_provider: Some(OneOf::Left(true)),
        // Re-indent on close: typing `}` re-indents the containing block when that
        // `}` structurally closes a multi-line group or an `\end{…}` (see
        // `compute_on_type_format`). Client opt-in (e.g. `editor.formatOnType`).
        document_on_type_formatting_provider: Some(DocumentOnTypeFormattingOptions {
            first_trigger_character: "}".to_owned(),
            more_trigger_character: None,
        }),
        document_symbol_provider: Some(OneOf::Left(true)),
        // Aggregate the per-file outline (sections, frames, labels, floats,
        // theorems, macros, environments) across every tracked project file. No lazy
        // `resolve`, so each result carries its full `Location`.
        workspace_symbol_provider: Some(OneOf::Left(true)),
        // Surface linter autofixes and syntax-aware refactorings. `Simple(true)`
        // returns fully-built actions (no `codeAction/resolve` step).
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        // The change-environment refactor (see [`on_execute_command`]). The
        // `texlab.…` alias keeps texlab client integrations working as-is.
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: vec![
                CHANGE_ENVIRONMENT_COMMAND.to_owned(),
                CHANGE_ENVIRONMENT_COMMAND_TEXLAB.to_owned(),
            ],
            work_done_progress_options: Default::default(),
        }),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        // Show the active argument while typing a command's/environment's
        // `{…}`/`[…]` arguments. `{`/`[` open an argument; `}`/`]` as *retriggers*
        // make the client re-query when one closes, so the between-arguments
        // `null` dismisses the popup and a still-inside position advances the
        // highlight. No `,`: a `\cite{a,b}` key list is one slot.
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["{".to_owned(), "[".to_owned()]),
            retrigger_characters: Some(vec!["}".to_owned(), "]".to_owned()]),
            work_done_progress_options: Default::default(),
        }),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        // Shade a cross-reference key and every same-key occurrence in the buffer.
        // Single-file (the lightweight cousin of `references_provider`).
        document_highlight_provider: Some(OneOf::Left(true)),
        // Rename a `\label`/`\cite` key and every referencing command across its
        // namespace. `prepare_provider` lets the client pre-validate the cursor and
        // anchor the prepare range to the key token.
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        // Expand-selection: nested ranges walking outward through the CST hierarchy
        // (token -> group -> argument -> command -> environment -> ... -> root).
        selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
        // Clickable include edges: `\input`/`\include`/`\import`, `\usepackage`/
        // `\documentclass`, `\bibliography`/`\addbibresource`, `\includegraphics`.
        // Links are built eagerly (target + range together), so no `resolve` step.
        document_link_provider: Some(DocumentLinkOptions {
            resolve_provider: Some(false),
            work_done_progress_options: Default::default(),
        }),
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
            // A highlighted item is sent back via `completionItem/resolve` to gain
            // its signature/citation detail lazily (see [`completion_resolve`]).
            resolve_provider: Some(true),
            ..Default::default()
        }),
        // texlab's spelling for the custom `textDocument/forwardSearch` method,
        // so a client that already probes for it finds badness too. Advertised
        // unconditionally: the viewer settings can arrive later (or change) via
        // `didChangeConfiguration`, and it is the `Unconfigured` status — not a
        // missing capability — that tells a client to prompt for a viewer.
        experimental: Some(serde_json::json!({ "textDocumentForwardSearch": true })),
        ..Default::default()
    }
}

/// An open document buffer: its current text and the version it is at.
///
/// The text is an [`Arc<TextBuffer>`] rather than a `String` because everything
/// downstream of an edit only reads it: the worker job, the salsa input, and
/// every read job the same keystroke fires. Capturing the buffer for one of
/// those is a refcount bump, and they share the [`LineIndex`] the first of them
/// builds.
struct Document {
    text: Arc<TextBuffer>,
    version: i32,
}

/// The main loop's state: open-document buffers and the client's editor settings.
/// Holds no database — the worker thread owns that.
struct GlobalState {
    documents: HashMap<Uri, Document>,
    editor_settings: EditorSettings,
    /// Per-document config resolutions, keyed by the document's **anchor directory**
    /// (its parent). A discovered `badness.toml` is authoritative; editor settings
    /// are the fallback. Each entry carries a filesystem fingerprint so clients that
    /// cannot register watched files still notice config changes on normal activity.
    /// The cache is also cleared wholesale on `didChangeConfiguration`.
    config_cache: HashMap<PathBuf, CachedSettings>,
    /// The declarations the worker's salsa input currently holds — the last value
    /// [`GlobalState::analysis_settings`] sent it. The main loop keeps the mirror
    /// so it can send a [`WorkerJob::Declarations`] only when the value actually
    /// changes: writing that input reparses the whole database, so a redundant
    /// write per keystroke would defeat every memo salsa holds.
    declarations: Arc<ResolvedDeclarations>,
    /// The client advertised `textDocument/diagnostic` pull support, so we serve
    /// diagnostics pull-only and **suppress** the `publishDiagnostics` push (the two
    /// are mutually exclusive, matching rust-analyzer/panache).
    supports_pull_diagnostics: bool,
    /// The client advertised `workspace.diagnostic.refreshSupport`, so a cross-file
    /// change can nudge it to re-pull via `workspace/diagnostic/refresh` (the pull
    /// analog of the push path's `RelintAll`).
    supports_diagnostic_refresh: bool,
    /// The client advertised `workspace.didChangeWatchedFiles.dynamicRegistration`, so
    /// on `initialized` we register watchers for `**/*.{tex,bib}` and `badness.toml`
    /// and reanalyze on on-disk edits to non-open project files.
    supports_dynamic_watchers: bool,
    /// Monotonic id for server→client requests (e.g. `workspace/diagnostic/refresh`,
    /// `client/registerCapability`). Namespaced from the client's request ids, so they
    /// never collide.
    next_request_id: i32,
    /// The position encoding negotiated at `initialize` (see
    /// [`negotiate_position_encoding`]), governing every `Position` ↔ byte-offset
    /// conversion on the main loop (`didChange` splicing).
    position_encoding: PositionEncoding,
    /// The workspace folders this server was started on (see [`workspace_roots`]),
    /// used to decide which inverse searches are ours. Empty when the client
    /// opened a bare file, which means "anything".
    workspace_roots: Vec<PathBuf>,
}

impl GlobalState {
    /// Whether `path` falls inside this server's workspace. A server with no
    /// roots claims everything — it has no basis to decline, and declining would
    /// leave the request unanswered by anyone.
    fn owns_path(&self, path: &Path) -> bool {
        self.workspace_roots.is_empty()
            || self
                .workspace_roots
                .iter()
                .any(|root| path.starts_with(root))
    }
}

/// Settings supplied by the editor, as `initializationOptions` at startup or via
/// `workspace/didChangeConfiguration`. The width knobs are a fallback beneath a
/// discovered `badness.toml` and the per-request [`FormattingOptions`]; `texmf` is
/// *machine* configuration with no `badness.toml` counterpart — the editor is its
/// only source, and the file-wins rule does not apply to it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct EditorSettings {
    line_width: Option<u32>,
    indent_width: Option<u32>,
    /// Installed-tree discovery for LSP package resolution (see [`TexmfConfig`]).
    /// Session-stable in practice: the
    /// [`texmf::global_index`](crate::project::texmf::global_index) it drives is
    /// first-config-wins.
    texmf: TexmfConfig,
    /// The PDF viewer forward search drives (see [`ForwardSearchSettings`]).
    forward_search: ForwardSearchSettings,
}

/// The external PDF viewer `textDocument/forwardSearch` launches.
///
/// *Machine* configuration with no `badness.toml` counterpart, for the same
/// reason as [`TexmfConfig`]: which viewer is installed, and under what name, is
/// a fact about the machine rather than about the project. The editor is its only
/// source, and the file-wins rule does not apply. Where the *PDF* lives is
/// project data and belongs to `[build]` instead.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ForwardSearchSettings {
    /// The viewer program. Spawned directly, never through a shell, so there is
    /// no word splitting: flags belong in [`args`](Self::args), and
    /// `"zathura --synctex-forward"` is a misconfiguration that cannot launch.
    executable: Option<String>,
    /// The viewer's argument vector, each element admitting `%f`/`%p`/`%l` (see
    /// [`forward_search::viewer_args`]). There is no useful default — every
    /// viewer spells forward search differently — so an unset `args` leaves
    /// forward search unconfigured exactly as an unset `executable` does.
    args: Option<Vec<String>>,
    /// Override for the inverse-search IPC directory. An escape hatch for
    /// containers and sandboxes, where the runtime directory may not be shared
    /// between the viewer and the server.
    ipc_dir: Option<PathBuf>,
}

impl ForwardSearchSettings {
    /// The configured viewer, or `None` when forward search is unconfigured.
    ///
    /// Both halves are required. That matches texlab, and it is not mere
    /// compatibility: an `executable` with no `args` would launch a viewer on no
    /// document at all.
    fn viewer(&self) -> Option<(&str, &[String])> {
        Some((self.executable.as_deref()?, self.args.as_deref()?))
    }
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

/// A document's resolved configuration: the formatter [`FormatStyle`] (with `wrap`
/// still a placeholder — the file kind decides it per request) plus the lint
/// selection. Built from a discovered `badness.toml` (file-wins) or, absent one,
/// from the editor settings. Cached per anchor dir in [`GlobalState::config_cache`].
#[derive(Debug, Clone)]
struct ResolvedSettings {
    /// Width knobs and `math_wrap` set; `wrap` is the [`WrapMode::default`]
    /// placeholder. `math_wrap` needs no per-file resolution here: its `Auto`
    /// default resolves against the effective wrap inside the formatter.
    style: FormatStyle,
    /// Configured paragraph wrap, if any. `None` ⇒ the file-kind default applies.
    wrap_override: Option<WrapMode>,
    /// Whether a `badness.toml` governed this resolution. When `true` the file
    /// config wins outright and a request's `tab_size` is ignored.
    config_present: bool,
    /// The `[lint]` `select`/`ignore` selection (default — every rule — when no file).
    lint: LintConfig,
    /// The sibling-discovery exclude filter, rooted at the config's directory. The
    /// exclude-nothing [`ExcludeFilter::none`] when no config governs (editor
    /// fallback) — preserving the unfiltered walk that path always did.
    exclude: ExcludeFilter,
    /// The `sentence`/`semantic` language, resolved once from `[format] lang`.
    /// English (the default) when no config governs; ignored by other wrap modes.
    sentence_lang: SentenceLanguage,
    /// The merged, normalized user no-break abbreviations from
    /// `[format.no-break-abbreviations]`. Held owned so a worker job can borrow it
    /// when building a [`SentenceOptions`] at format time.
    sentence_no_break: Vec<String>,
    /// The `[build]` settings locating the compiler's `.aux` artifacts. Consumed
    /// only by label hover and document symbols (resolved numbers), never the
    /// formatter — so it cannot affect `badness format` output.
    build: BuildConfig,
    /// The project's resolved declarations (`AGENTS.md` decision #12), the one
    /// non-text input to the parse. Held behind an `Arc` because every dispatch
    /// site clones the settings and the value is identical (and usually empty)
    /// across a workspace. Reaches the worker's salsa input via
    /// [`WorkerJob::Declarations`].
    declarations: Arc<ResolvedDeclarations>,
}

/// The cheap part of a config file's identity. Modification time catches an
/// in-place save; length catches writes on coarse-mtime filesystems when their
/// size changes. Missing files are represented by `None` in the enclosing
/// fingerprint, which also makes creation and deletion observable.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfigFileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

fn config_file_stamp(path: &Path) -> Option<ConfigFileStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    metadata.is_file().then(|| ConfigFileStamp {
        modified: metadata.modified().ok(),
        len: metadata.len(),
    })
}

/// Snapshot every project-config candidate consulted by the ancestor walk. The
/// absent entries matter: if a nearer `badness.toml` is created, it must displace
/// the cached parent, environment, global, or default configuration.
fn project_config_fingerprint(anchor: &Path) -> Option<Vec<(PathBuf, Option<ConfigFileStamp>)>> {
    let canonical = anchor.canonicalize().ok()?;
    let mut fingerprint = Vec::new();
    for dir in canonical.ancestors() {
        let candidate = dir.join(crate::config::CONFIG_FILE_NAME);
        let stamp = config_file_stamp(&candidate);
        let found = stamp.is_some();
        fingerprint.push((candidate, stamp));
        if found || dir.join(".git").exists() {
            break;
        }
    }
    Some(fingerprint)
}

#[derive(Debug, Clone)]
struct CachedSettings {
    resolved: ResolvedSettings,
    project_fingerprint: Option<Vec<(PathBuf, Option<ConfigFileStamp>)>>,
    /// The resolved environment/global file may live outside the project walk.
    source_fingerprint: Option<(PathBuf, Option<ConfigFileStamp>)>,
}

impl CachedSettings {
    fn new(resolved: ResolvedSettings, anchor: &Path, source: Option<PathBuf>) -> Self {
        let source_fingerprint = source.map(|path| {
            let stamp = config_file_stamp(&path);
            (path, stamp)
        });
        Self {
            resolved,
            project_fingerprint: project_config_fingerprint(anchor),
            source_fingerprint,
        }
    }

    fn is_fresh(&self, anchor: &Path) -> bool {
        self.project_fingerprint == project_config_fingerprint(anchor)
            && self
                .source_fingerprint
                .as_ref()
                .is_none_or(|(path, stamp)| *stamp == config_file_stamp(path))
    }
}

impl ResolvedSettings {
    /// Resolution from a discovered config (when `present`), else from the editor
    /// settings, applying the file-wins rule. The `exclude` filter is left
    /// exclude-nothing here; [`resolve_settings`] compiles and installs the real
    /// one (it holds the config's root directory).
    ///
    /// [`resolve_settings`]: GlobalState::resolve_settings
    fn from_config(config: &Config, present: bool, editor: &EditorSettings) -> Self {
        if present {
            let (sentence_lang, sentence_no_break) = resolve_owned(
                config.format.lang.as_deref(),
                &config.format.no_break_abbreviations,
            );
            Self {
                style: FormatStyle::from(&config.format),
                wrap_override: config.format.wrap.map(Into::into),
                config_present: true,
                lint: config.lint.clone(),
                exclude: ExcludeFilter::none(),
                sentence_lang,
                sentence_no_break,
                build: config.build.clone(),
                declarations: Arc::new(config.resolved_declarations()),
            }
        } else {
            Self::from_editor(editor)
        }
    }

    /// Editor-settings-only resolution: width knobs over the built-in defaults, no
    /// configured wrap, the full default rule set, and no exclude filter.
    fn from_editor(editor: &EditorSettings) -> Self {
        Self {
            style: editor.to_format_style(),
            wrap_override: None,
            config_present: false,
            lint: LintConfig::default(),
            exclude: ExcludeFilter::none(),
            sentence_lang: SentenceLanguage::default(),
            sentence_no_break: Vec::new(),
            build: BuildConfig::default(),
            declarations: Arc::new(ResolvedDeclarations::default()),
        }
    }

    /// The active lint-rule set this config implies (unknown ids are dropped; the
    /// CLI surfaces them, the LSP has no good channel to yet).
    fn rule_selection(&self) -> RuleSelection {
        RuleSelection::resolve(self.lint.select.as_deref(), &self.lint.ignore).0
    }
}

impl GlobalState {
    /// Resolve (and cache) the [`ResolvedSettings`] for `uri`'s document: discover a
    /// `badness.toml` from the document's anchor directory (its parent), falling back
    /// to the global user config (`~/.config/badness/config.toml`), then to the
    /// editor settings when neither is found. Cached by anchor dir; cache hits
    /// stat the small ancestor candidate set and the resolved source, but avoid
    /// rereading, parsing, and rebuilding derived settings while it is unchanged.
    ///
    /// A non-`file` buffer (untitled) or a directory-less / unreadable / malformed
    /// config resolves to the editor settings and is **not** cached, so fixing a
    /// broken `badness.toml` takes effect on the next request without a restart.
    fn resolve_settings(&mut self, uri: &Uri) -> ResolvedSettings {
        let Some(anchor) = uri_to_fs_path(uri).and_then(|p| p.parent().map(Path::to_path_buf))
        else {
            return ResolvedSettings::from_editor(&self.editor_settings);
        };
        if let Some(cached) = self
            .config_cache
            .get(&anchor)
            .filter(|cached| cached.is_fresh(&anchor))
        {
            return cached.resolved.clone();
        }
        let (resolved, source_path) = match Config::resolve(None, false, &anchor) {
            Ok((config, source)) => {
                let present = source.path().is_some();
                let source_path = source.path().map(Path::to_path_buf);
                let mut resolved =
                    ResolvedSettings::from_config(&config, present, &self.editor_settings);
                if present {
                    // Compile the sibling-discovery exclude filter, rooted at the
                    // config's directory — or at the document's directory for the
                    // global user config (the same `ConfigSource::exclude_root` rule
                    // as the CLI's `build_exclude_filter`; the LSP contributes no
                    // `--exclude`). A malformed pattern leaves the exclude-nothing
                    // default rather than failing resolution — there is no good
                    // channel to report it.
                    let root = source.exclude_root(&anchor);
                    if let Ok(filter) = ExcludeFilter::new(root, &config.exclude_patterns(&[])) {
                        resolved.exclude = filter;
                    }
                    // `[build] root` is documented relative to the config's own
                    // directory, and the consumers (forward search) only ever see
                    // the resolved `BuildConfig`. Absolutize once, here, where that
                    // directory is still in hand. `pdf-dir` is deliberately left
                    // alone: it resolves against the *root document's* directory,
                    // which is not known until the root is.
                    if let Some(build_root) =
                        resolved.build.root.as_ref().filter(|p| p.is_relative())
                    {
                        resolved.build.root = Some(root.join(build_root));
                    }
                }
                (resolved, source_path)
            }
            // No good channel to report a bad/unreadable config; fall back without
            // caching so a fix is picked up next time.
            Err(_) => return ResolvedSettings::from_editor(&self.editor_settings),
        };
        self.config_cache.insert(
            anchor.clone(),
            CachedSettings::new(resolved.clone(), &anchor, source_path),
        );
        resolved
    }

    /// Republish the declarations governing `uri` to the worker's salsa input
    /// when they differ from what it holds.
    ///
    /// The input is a project-wide **singleton**, so any job that reads a tree
    /// must find the cell holding *its own* document's block — and since a job
    /// rides the same FIFO channel, sending the write ahead of it needs no
    /// separate handshake. Requests are covered wholesale by
    /// [`publish_declarations_for_request`], which calls this from the dispatcher
    /// rather than from each handler; the notification sites that publish ahead
    /// of an `Edit` job go through
    /// [`analysis_settings`](Self::analysis_settings), which wants the settings
    /// anyway.
    ///
    /// A session holding two workspaces with *different* blocks therefore
    /// rewrites the input as attention crosses between them, reparsing the world
    /// each time — the accepted cost of a single project-wide input (see
    /// [`DeclarationsInput`](crate::incremental::DeclarationsInput)). Declaring
    /// nothing, the overwhelming default, never writes it at all.
    fn publish_declarations(&mut self, uri: &Uri, job_tx: &Sender<WorkerJob>) {
        let declarations = self.resolve_settings(uri).declarations;
        self.publish_resolved_declarations(declarations, job_tx);
    }

    /// [`resolve_settings`](Self::resolve_settings) for a notification whose work
    /// *parses* — `didOpen`, `didChange`, a relint sweep — additionally
    /// republishing the document's declarations ahead of the job it precedes.
    ///
    /// Requests do not need this: the dispatcher publishes for every one of them
    /// ([`publish_declarations_for_request`]), so a handler is free to resolve
    /// settings however it likes, or not at all. Resolving *once* matters here
    /// and not there — `didChange` runs per keystroke, and `resolve_settings`
    /// hands back a clone of the whole settings record.
    fn analysis_settings(&mut self, uri: &Uri, job_tx: &Sender<WorkerJob>) -> ResolvedSettings {
        let resolved = self.resolve_settings(uri);
        self.publish_resolved_declarations(Arc::clone(&resolved.declarations), job_tx);
        resolved
    }

    /// The write half of [`publish_declarations`](Self::publish_declarations),
    /// over declarations already resolved.
    fn publish_resolved_declarations(
        &mut self,
        declarations: Arc<ResolvedDeclarations>,
        job_tx: &Sender<WorkerJob>,
    ) {
        // `resolve_settings` hands back a clone of a cached value, so the common
        // case is the same allocation it returned last time and the pointer
        // check settles it without walking two signature databases.
        if Arc::ptr_eq(&declarations, &self.declarations) || declarations == self.declarations {
            return;
        }
        self.declarations = Arc::clone(&declarations);
        let _ = job_tx.send(WorkerJob::Declarations { declarations });
    }
}

/// Publish the declarations governing the document a request names, before the
/// request is routed to its handler.
///
/// **One insertion point, not a call per handler.** Nearly every request reads a
/// tree, the salsa input carrying the declarations is a project-wide singleton,
/// and a per-handler call is a rule the next handler forgets — leaving that one
/// feature parsing under whichever document was last analyzed. Reading
/// `textDocument.uri` straight off the params covers every `textDocument/*`
/// request, including ones not yet written.
///
/// A request that names no document (`workspace/symbol`) rides whatever the last
/// one published, and a request whose job reads no tree (`forwardSearch`) pays at
/// most one redundant write in a multi-workspace session — cheaper than the
/// method allowlist that would avoid it, and unable to go stale.
///
/// `workspace/executeCommand` is the one shape a plain `params.textDocument`
/// read misses: `changeEnvironment` carries its document one level down, in the
/// first command argument. It reads a tree like any other, so the extractor
/// knows both spellings.
fn publish_declarations_for_request(
    state: &mut GlobalState,
    req: &Request,
    job_tx: &Sender<WorkerJob>,
) {
    let document_uri = |value: &serde_json::Value| {
        value
            .get("textDocument")
            .and_then(|doc| doc.get("uri"))
            .and_then(serde_json::Value::as_str)
            .and_then(|uri| uri.parse::<Uri>().ok())
    };
    let Some(uri) = document_uri(&req.params).or_else(|| {
        req.params
            .get("arguments")
            .and_then(|args| args.get(0))
            .and_then(document_uri)
    }) else {
        return;
    };
    state.publish_declarations(&uri, job_tx);
}

/// A job from the main loop to the worker thread.
///
/// Every variant carrying a document buffer carries it as an
/// [`Arc<TextBuffer>`], never a `String`: the main loop is on the keystroke
/// path, so capturing a buffer for a job must not copy it, and the read job at
/// the far end wants the same [`LineIndex`] the buffer already holds.
enum WorkerJob {
    /// A buffer edit (from `didOpen` or `didChange`): write the full text into the
    /// db, then (re)analyze diagnostics.
    Edit {
        uri: Uri,
        path: PathBuf,
        text: Arc<TextBuffer>,
        version: i32,
        kind: FileKind,
        /// The document's resolved lint-rule selection, applied to the analyze.
        rules: RuleSelection,
        /// The document's resolved exclude filter, applied to sibling discovery
        /// ([`Worker::seed_dir`]). Built on the main side because the worker holds
        /// no config; exclude-nothing when no `badness.toml` governs.
        exclude: ExcludeFilter,
        /// The exact transform from the text the db currently holds to `text`, for
        /// the incremental reparse to splice (`AGENTS.md` decision #6). [`None`]
        /// when the text arrived by a route carrying no edits — a `didOpen`, a
        /// re-lint sweep — which clears the chain rather than leaving one that no
        /// longer describes how this text was reached.
        ///
        /// Only ever a hint: a stale or missing chain costs a full parse and
        /// nothing else, because [`reparse_edits`](crate::parser::reparse_edits)
        /// rejects any chain that does not land on exactly this text.
        edits: Option<Vec<Edit>>,
    },
    /// The project's declarations changed (or arrived): write them into the
    /// worker's salsa input, invalidating every parse that read the old ones.
    ///
    /// Sent by [`GlobalState::analysis_settings`] ahead of the parse-bearing job
    /// it governs, and only when the value actually differs from what the worker
    /// holds — the write reparses the whole database.
    Declarations {
        declarations: Arc<ResolvedDeclarations>,
    },
    /// `didClose`: evict the file from the db. Diagnostics are cleared directly by
    /// the main loop.
    Close { path: PathBuf },
    /// A `workspace/didChangeWatchedFiles` event for a **non-open** `.tex`/`.bib`
    /// project file: re-read it from disk (or evict it on delete) and re-lint every
    /// open document, since a sibling's labels/cites may have changed. The main loop
    /// has already confirmed the path is not an open editor buffer (whose overlay text
    /// is authoritative), so this path deliberately re-reads disk.
    WatchedChange { path: PathBuf, deleted: bool },
    /// A formatting request: format on the read pool and reply to `id`.
    Format {
        id: RequestId,
        path: PathBuf,
        text: Arc<TextBuffer>,
        style: FormatStyle,
        kind: FileKind,
        /// The `sentence`/`semantic` language and merged no-break abbreviations,
        /// resolved from the document's config (see [`ResolvedSettings`]).
        sentence_lang: SentenceLanguage,
        sentence_no_break: Vec<String>,
    },
    /// A range-formatting request: like [`WorkerJob::Format`] but bounded to the
    /// editor selection (expanded to whole document-level blocks on the read pool).
    RangeFormat {
        id: RequestId,
        path: PathBuf,
        text: Arc<TextBuffer>,
        style: FormatStyle,
        kind: FileKind,
        range: Range,
        sentence_lang: SentenceLanguage,
        sentence_no_break: Vec<String>,
    },
    /// An on-type-formatting request (`textDocument/onTypeFormatting`): the user
    /// typed `}`. Re-indents the containing top-level block, but only when the `}`
    /// structurally closes a multi-line group or an `\end{…}` (see
    /// [`compute_on_type_format`]); otherwise no edits. `position` is the cursor
    /// just after the typed `}`.
    OnTypeFormat {
        id: RequestId,
        path: PathBuf,
        text: Arc<TextBuffer>,
        style: FormatStyle,
        kind: FileKind,
        position: Position,
        sentence_lang: SentenceLanguage,
        sentence_no_break: Vec<String>,
    },
    /// A document-symbol request: build the outline on the read pool and reply to
    /// `id`. Cross-file only for the `.aux` enrichment (resolved numbers); the
    /// database snapshot carries project membership, and `build` locates the aux
    /// files.
    Symbols {
        id: RequestId,
        path: PathBuf,
        text: Arc<TextBuffer>,
        kind: FileKind,
        build: BuildConfig,
    },
    /// A `workspace/symbol` request: aggregate every tracked file's outline on the
    /// read pool and reply to `id` with the matches for `query`. The database
    /// snapshot supplies the whole project's membership.
    WorkspaceSymbols { id: RequestId, query: String },
    /// A folding-range request: compute foldable regions on the read pool and reply
    /// to `id`. Single-file like [`Symbols`](Self::Symbols), with no project snapshot.
    FoldingRange {
        id: RequestId,
        path: PathBuf,
        text: Arc<TextBuffer>,
        kind: FileKind,
    },
    /// A selection-range request: for each cursor `positions`, compute the nested
    /// "expand selection" chain from the CST ancestor walk on the read pool and reply
    /// to `id`. Single-file and positional like [`FoldingRange`](Self::FoldingRange),
    /// with no project snapshot.
    SelectionRange {
        id: RequestId,
        path: PathBuf,
        text: Arc<TextBuffer>,
        kind: FileKind,
        positions: Vec<Position>,
    },
    /// A document-link request: build clickable include/package/bib/graphics links
    /// on the read pool and reply to `id`. Single-file and positional (it bypasses
    /// the range-free project graph); `path` supplies the base directory that
    /// relative targets resolve and existence-check against. `texmf` gates the
    /// installed-tree fallback (a system `\usepackage{amsmath}` → its real source);
    /// the read pool builds/consults the index so the tree walk stays off the main loop.
    DocumentLink {
        id: RequestId,
        path: PathBuf,
        text: Arc<TextBuffer>,
        kind: FileKind,
        texmf: TexmfConfig,
    },
    /// A completion request: classify the cursor and build candidates on the read
    /// pool and reply to `id`. Carries the `uri` (the salsa-key path is derived from
    /// it) so file-path completion can read the document's on-disk directory, and the
    /// `[texmf]` settings gating the installed-set completion tier.
    Completion {
        id: RequestId,
        uri: Uri,
        text: Arc<TextBuffer>,
        position: Position,
        texmf: TexmfConfig,
    },
    /// A completion-resolve request: attach lazy signature/citation detail to a
    /// highlighted item on the read pool and reply to `id`. The item's `data`
    /// payload is self-contained, so no document buffer is needed; the cross-file
    /// lookup reads membership from the database snapshot.
    ResolveCompletion {
        id: RequestId,
        // Boxed: a `CompletionItem` is large and would bloat every `WorkerJob`.
        item: Box<CompletionItem>,
    },
    /// A hover request: describe the command/environment signature or `\cite` entry
    /// under the cursor on the read pool and reply to `id`. Cross-file (the signature
    /// scope folds in loaded packages, and a `\cite` resolves against the project
    /// bibliography) through the membership in the database snapshot.
    Hover {
        id: RequestId,
        path: PathBuf,
        text: Arc<TextBuffer>,
        position: Position,
        /// `[build]` settings for the label-number lookup (a `\label`/`\ref` hover
        /// reads the compile's `.aux`).
        build: BuildConfig,
    },
    /// A `textDocument/forwardSearch` request: resolve the cursor file's document
    /// root and that root's compiled PDF, then hand `(%f, %p, %l)` to the
    /// configured viewer. Cross-file — the root scan runs the salsa
    /// `file_is_document_root` query over the label namespace—and none of it can
    /// run on the main loop, which holds no database.
    ForwardSearch {
        id: RequestId,
        path: PathBuf,
        /// 1-based, as SyncTeX and every viewer count.
        line: u32,
        /// `[build]` settings locating the PDF, with `root` already absolutized.
        build: BuildConfig,
        executable: String,
        args: Vec<String>,
    },
    /// A signature-help request: describe the command/environment whose argument
    /// the cursor is typing in on the read pool and reply to `id`. The signature
    /// scope folds in loaded packages from the database snapshot's project.
    SignatureHelp {
        id: RequestId,
        path: PathBuf,
        text: Arc<TextBuffer>,
        position: Position,
    },
    /// A go-to-definition request: resolve the `\ref`/`\cite` under the cursor to
    /// its `\label`/bib entry on the read pool and reply to `id`. `texmf` gates the
    /// file-target fallback (an include/package argument jumps to its resolved
    /// source, installed-tree-aware).
    GotoDefinition {
        id: RequestId,
        path: PathBuf,
        text: Arc<TextBuffer>,
        position: Position,
        texmf: TexmfConfig,
    },
    /// A find-references request: enumerate every `\ref`/`\cite` use of the
    /// label/key under the cursor on the read pool and reply to `id`. It is
    /// cross-file and invokable from a definition site.
    References {
        id: RequestId,
        path: PathBuf,
        text: Arc<TextBuffer>,
        position: Position,
        include_declaration: bool,
    },
    /// A `documentHighlight` request: shade the cross-reference key under the cursor
    /// and every same-key occurrence in the *same* buffer. Single-file (the
    /// lightweight cousin of [`References`](Self::References)); dispatched to the read
    /// pool like the others to keep the threading model uniform.
    DocumentHighlight {
        id: RequestId,
        path: PathBuf,
        text: Arc<TextBuffer>,
        position: Position,
    },
    /// A `prepareRename` request: confirm the cursor sits on a renameable label/cite
    /// key and reply with that key's range + placeholder. The cursor target comes
    /// from one parse; command/environment names additionally use the project-wide
    /// user-definition gate.
    PrepareRename {
        id: RequestId,
        path: PathBuf,
        text: Arc<TextBuffer>,
        position: Position,
    },
    /// A `rename` request: build the project-wide [`WorkspaceEdit`] renaming the
    /// label/cite key under the cursor and every referencing command. Cross-file
    /// scope comes from the database snapshot.
    Rename {
        id: RequestId,
        path: PathBuf,
        text: Arc<TextBuffer>,
        position: Position,
        new_name: String,
    },
    /// A `changeEnvironment` execute-command: rewrite the enclosing environment's
    /// `\begin`/`\end` name pair around the cursor to `new_name`. Single-file and
    /// purely syntactic (the parser already pairs the delimiters), dispatched to
    /// the read pool like the others; `uri` keys the resulting edit. Answers via
    /// [`Outbound::ApplyEdit`] (or an error response when no environment encloses
    /// the cursor).
    ChangeEnvironment {
        id: RequestId,
        uri: Uri,
        path: PathBuf,
        text: Arc<TextBuffer>,
        position: Position,
        new_name: String,
    },
    /// A `textDocument/diagnostic` pull request: compute diagnostics **on demand**
    /// off a fresh snapshot and reply to `id`. Carries the live `text` only as the
    /// cancellation fallback's source—currency comes from the
    /// FIFO `job_tx`: the preceding `didChange`'s `Edit` upserts before this job is
    /// handled, so the snapshot is already current (no debounce, no staleness).
    Diagnostic {
        id: RequestId,
        path: PathBuf,
        text: Arc<TextBuffer>,
        kind: FileKind,
        previous_result_id: Option<String>,
        /// The document's resolved lint-rule selection, applied to the report.
        rules: RuleSelection,
    },
    /// A `textDocument/codeAction` request: re-lint the buffer off a fresh snapshot
    /// and reply with quick-fixes plus any syntax-aware refactoring at `range`.
    /// Cross-file state comes from the database snapshot; `uri` is needed to key
    /// the resulting [`WorkspaceEdit`].
    CodeAction {
        id: RequestId,
        uri: Uri,
        path: PathBuf,
        text: Arc<TextBuffer>,
        kind: FileKind,
        range: Range,
        /// Action kinds requested by the client. A parent kind such as `refactor`
        /// admits its descendants, including `refactor.rewrite`.
        only: Option<Vec<CodeActionKind>>,
        /// The document's resolved lint-rule selection, applied to the findings.
        rules: RuleSelection,
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
    /// Answer an execute-command by *pushing* `edit` to the client — a
    /// `workspace/applyEdit` server→client request (allocated a fresh request id by
    /// the main loop), followed by a `null` response to the originating request
    /// `id`. The client's applyEdit response is fire-and-forget.
    ApplyEdit {
        id: RequestId,
        label: String,
        edit: WorkspaceEdit,
    },
    /// Project membership grew (the worker discovered on-disk siblings), so the
    /// cross-file resolution may have changed for *every* open document. Re-lint
    /// them all.
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

/// Read the client's diagnostic capabilities from the `initialize` params, as
/// `(supports_pull, supports_refresh)`. Pointer-walks the JSON (like
/// [`EditorSettings::from_client_value`]) rather than deserializing the whole
/// `ClientCapabilities`: pull support is the mere presence of
/// `capabilities.textDocument.diagnostic`; refresh support is
/// `capabilities.workspace.diagnostic.refreshSupport == true`.
fn client_diagnostic_support(init_params: &serde_json::Value) -> (bool, bool) {
    let caps = init_params.get("capabilities");
    let supports_pull = caps
        .and_then(|c| c.get("textDocument"))
        .and_then(|t| t.get("diagnostic"))
        .is_some();
    let supports_refresh = caps
        .and_then(|c| c.get("workspace"))
        .and_then(|w| w.get("diagnostic"))
        .and_then(|d| d.get("refreshSupport"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    (supports_pull, supports_refresh)
}

/// Whether the client supports dynamically registered file watchers, i.e.
/// `capabilities.workspace.didChangeWatchedFiles.dynamicRegistration == true`.
/// Pointer-walks the JSON like [`client_diagnostic_support`]. When `false` we skip
/// registration and fall back to seed-on-open: on-disk edits to non-open includes go
/// unnoticed until something re-seeds the directory.
fn client_watched_files_support(init_params: &serde_json::Value) -> bool {
    init_params
        .get("capabilities")
        .and_then(|c| c.get("workspace"))
        .and_then(|w| w.get("didChangeWatchedFiles"))
        .and_then(|d| d.get("dynamicRegistration"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Whether the client supports `window/showDocument`, i.e.
/// `capabilities.window.showDocument.support == true`. Pointer-walks the JSON
/// like [`client_diagnostic_support`].
///
/// This is the whole of inverse search's client contract, so the IPC listener is
/// bound only when it holds: a server that cannot reveal the position has no
/// business advertising itself to viewers and stealing the request from one that
/// can.
fn client_show_document_support(init_params: &serde_json::Value) -> bool {
    init_params
        .get("capabilities")
        .and_then(|c| c.get("window"))
        .and_then(|w| w.get("showDocument"))
        .and_then(|s| s.get("support"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// The workspace this server owns, as filesystem paths: the `workspaceFolders`,
/// falling back to the deprecated `rootUri`. Empty when the client opened a bare
/// file, which an inverse-search client reads as "will take anything".
fn workspace_roots(init_params: &serde_json::Value) -> Vec<PathBuf> {
    let folders = init_params
        .get("workspaceFolders")
        .and_then(serde_json::Value::as_array)
        .map(|folders| {
            folders
                .iter()
                .filter_map(|folder| folder.get("uri"))
                .filter_map(serde_json::Value::as_str)
                .filter_map(|uri| uri.parse::<Uri>().ok())
                .filter_map(|uri| uri_to_fs_path(&uri))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !folders.is_empty() {
        return folders;
    }
    init_params
        .get("rootUri")
        .and_then(serde_json::Value::as_str)
        .and_then(|uri| uri.parse::<Uri>().ok())
        .and_then(|uri| uri_to_fs_path(&uri))
        .into_iter()
        .collect()
}

/// A bound inverse-search listener and the thread parked in its accept loop.
struct IpcHandle {
    listener: Arc<crate::ipc::Listener>,
    thread: std::thread::JoinHandle<()>,
}

/// One accepted inverse-search request, on its way to the main loop.
struct IpcMessage {
    request: crate::ipc::InverseSearchRequest,
    responder: crate::ipc::Responder,
}

/// Bind the inverse-search socket and park a thread on its accept loop, forwarding
/// each request to the main loop over `ipc_tx`.
///
/// `None` when the socket cannot be bound. Inverse search is a convenience, so a
/// server that cannot listen still serves everything else — the viewer-side
/// command is what reports the absence, and it says so in terms the user can act
/// on.
fn spawn_ipc_listener(
    settings: &EditorSettings,
    roots: Vec<PathBuf>,
    ipc_tx: Sender<IpcMessage>,
) -> Option<IpcHandle> {
    let dir = settings
        .forward_search
        .ipc_dir
        .clone()
        .unwrap_or_else(crate::ipc::ipc_dir);
    let listener = Arc::new(crate::ipc::Listener::bind_in(&dir, roots)?);
    let accept = Arc::clone(&listener);
    let thread = std::thread::Builder::new()
        .name("badness-lsp-ipc".to_owned())
        .spawn(move || {
            while let Some((request, responder)) = accept.accept_one() {
                if ipc_tx.send(IpcMessage { request, responder }).is_err() {
                    break;
                }
            }
        })
        .ok()?;
    Some(IpcHandle { listener, thread })
}

/// Build the inverse-search channel, or a receiver that can never become ready.
///
/// A disconnected receiver is always ready in `select!`; leaving one installed
/// when the client cannot show documents—or when binding the listener fails—
/// turns the main loop into a busy spin. `never()` preserves the disabled arm's
/// intended behavior without a dummy sender that could obscure listener failure.
fn ipc_channel(
    enabled: bool,
    settings: &EditorSettings,
    roots: Vec<PathBuf>,
) -> (Option<IpcHandle>, Receiver<IpcMessage>) {
    if !enabled {
        return (None, never());
    }
    let (ipc_tx, ipc_rx) = unbounded();
    match spawn_ipc_listener(settings, roots, ipc_tx) {
        Some(ipc) => (Some(ipc), ipc_rx),
        None => (None, never()),
    }
}

/// The blocking message loop. Owns [`GlobalState`]; spawns the worker thread and
/// the read pool, then shuttles messages between the client and the workers.
/// `encoding` is the position encoding [`serve`] negotiated (and advertised) at
/// `initialize`.
fn main_loop(
    connection: Connection,
    init_params: serde_json::Value,
    encoding: PositionEncoding,
) -> Result<(), DynError> {
    let editor_settings = init_params
        .get("initializationOptions")
        .map(EditorSettings::from_client_value)
        .unwrap_or_default();
    let (supports_pull_diagnostics, supports_diagnostic_refresh) =
        client_diagnostic_support(&init_params);
    let supports_dynamic_watchers = client_watched_files_support(&init_params);
    let mut state = GlobalState {
        documents: HashMap::new(),
        editor_settings,
        config_cache: HashMap::new(),
        // Matches the worker's freshly-built database, which starts out declaring
        // nothing; the mirror is only ever compared, never read for a parse.
        declarations: Arc::new(ResolvedDeclarations::default()),
        supports_pull_diagnostics,
        supports_diagnostic_refresh,
        supports_dynamic_watchers,
        next_request_id: 1,
        position_encoding: encoding,
        workspace_roots: workspace_roots(&init_params),
    };

    // Register on-disk watchers now: `lsp-server`'s `Connection::initialize` already
    // consumed the client's `initialized` notification, so we never see it as a
    // notification — the post-handshake point here is the LSP-legal place to fire
    // dynamic registrations.
    register_file_watchers(&connection, &mut state);

    // Inverse search, gated on the one client capability it needs. A client that
    // cannot `window/showDocument` never binds a socket, which is also why the
    // default test client is unaffected by any of this.
    let (ipc, ipc_rx) = ipc_channel(
        client_show_document_support(&init_params),
        &state.editor_settings,
        workspace_roots(&init_params),
    );

    let read_pool = TaskPool::new("badness-lsp-read", read_pool_size());
    let (job_tx, job_rx) = unbounded::<WorkerJob>();
    let (out_tx, out_rx) = unbounded::<Outbound>();
    let worker = spawn_worker(job_rx, out_tx, read_pool.spawner(), encoding);

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
                        // Ahead of the handler, so the declarations governing the
                        // named document reach the worker before whatever job it
                        // sends on this same channel.
                        publish_declarations_for_request(&mut state, &req, &job_tx);
                        match req.method.as_str() {
                            Formatting::METHOD => {
                                on_formatting(&connection, &mut state, &job_tx, req)
                            }
                            RangeFormatting::METHOD => {
                                on_range_formatting(&connection, &mut state, &job_tx, req)
                            }
                            OnTypeFormatting::METHOD => {
                                on_type_formatting(&connection, &mut state, &job_tx, req)
                            }
                            DocumentSymbolRequest::METHOD => {
                                on_document_symbol(&connection, &mut state, &job_tx, req)
                            }
                            WorkspaceSymbolRequest::METHOD => {
                                on_workspace_symbol(&connection, &job_tx, req)
                            }
                            Completion::METHOD => {
                                on_completion(&connection, &mut state, &job_tx, req)
                            }
                            ResolveCompletionItem::METHOD => {
                                on_completion_resolve(&connection, &job_tx, req)
                            }
                            HoverRequest::METHOD => {
                                on_hover(&connection, &mut state, &job_tx, req)
                            }
                            ForwardSearchRequest::METHOD => {
                                on_forward_search(&connection, &mut state, &job_tx, req)
                            }
                            SignatureHelpRequest::METHOD => {
                                on_signature_help(&connection, &mut state, &job_tx, req)
                            }
                            GotoDefinition::METHOD => {
                                on_goto_definition(&connection, &mut state, &job_tx, req)
                            }
                            References::METHOD => on_references(&connection, &state, &job_tx, req),
                            DocumentHighlightRequest::METHOD => {
                                on_document_highlight(&connection, &state, &job_tx, req)
                            }
                            PrepareRenameRequest::METHOD => {
                                on_prepare_rename(&connection, &state, &job_tx, req)
                            }
                            Rename::METHOD => on_rename(&connection, &state, &job_tx, req),
                            FoldingRangeRequest::METHOD => {
                                on_folding_range(&connection, &state, &job_tx, req)
                            }
                            SelectionRangeRequest::METHOD => {
                                on_selection_range(&connection, &state, &job_tx, req)
                            }
                            DocumentLinkRequest::METHOD => {
                                on_document_link(&connection, &mut state, &job_tx, req)
                            }
                            CodeActionRequest::METHOD => {
                                on_code_action(&connection, &mut state, &job_tx, req)
                            }
                            ExecuteCommand::METHOD => {
                                on_execute_command(&connection, &state, &job_tx, req)
                            }
                            DocumentDiagnosticRequest::METHOD => {
                                on_document_diagnostic(&connection, &mut state, &job_tx, req)
                            }
                            _ => respond_unhandled(&connection, req),
                        }
                    }
                    Message::Notification(not) => {
                        on_notification(&connection, &mut state, &job_tx, not);
                    }
                    // Server-initiated requests (watcher registration,
                    // `workspace/applyEdit`) are fire-and-forget, so the client's
                    // response needs no action.
                    Message::Response(_) => {}
                }
            }
            recv(out_rx) -> outbound => {
                let Ok(outbound) = outbound else { continue };
                forward_outbound(&connection, &mut state, &job_tx, outbound);
            }
            recv(ipc_rx) -> msg => {
                let Ok(msg) = msg else { continue };
                on_inverse_search(&connection, &mut state, msg);
            }
        }
    }

    // The accept thread is parked in a blocking `accept`, which dropping a
    // channel cannot wake, and `serve` runs *in-process* in the LSP test binary —
    // so a detached listener would hold a bound socket for the whole run. Dial
    // ourselves once, join, and let `Listener`'s `Drop` unlink both nodes.
    if let Some(ipc) = ipc {
        ipc.listener.wake();
        let _ = ipc.thread.join();
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
            let text = Arc::new(TextBuffer::new(doc.text, state.position_encoding));
            state.documents.insert(
                uri.clone(),
                Document {
                    text: text.clone(),
                    version: doc.version,
                },
            );
            let path = uri_to_path(&uri);
            let kind = file_kind_for(&path);
            let resolved = state.analysis_settings(&uri, job_tx);
            let _ = job_tx.send(WorkerJob::Edit {
                path,
                uri,
                text,
                version: doc.version,
                kind,
                rules: resolved.rule_selection(),
                exclude: resolved.exclude,
                // A whole buffer arriving fresh: no transform to describe.
                edits: None,
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
            let edits = apply_content_changes(&mut doc.text, params.content_changes);
            doc.version = version;
            let text = doc.text.clone();
            let path = uri_to_path(&uri);
            let kind = file_kind_for(&path);
            let resolved = state.analysis_settings(&uri, job_tx);
            let _ = job_tx.send(WorkerJob::Edit {
                path,
                uri,
                text,
                version,
                kind,
                rules: resolved.rule_selection(),
                exclude: resolved.exclude,
                edits,
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
            // In pull mode there is nothing to clear — the client drops a closed
            // file's diagnostics itself by ceasing to pull — and we never push.
            if !state.supports_pull_diagnostics {
                send_diagnostics(connection, uri, Vec::new(), None);
            }
        }
        DidChangeConfiguration::METHOD => {
            if let Ok(params) =
                not.extract::<DidChangeConfigurationParams>(DidChangeConfiguration::METHOD)
            {
                state.editor_settings = EditorSettings::from_client_value(&params.settings);
                // Drop cached resolutions so the new fallback is picked up on the
                // next request. A discovered `badness.toml` still wins, so docs in a
                // configured workspace are unaffected.
                state.config_cache.clear();
            }
        }
        DidChangeWatchedFiles::METHOD => {
            if let Ok(params) =
                not.extract::<DidChangeWatchedFilesParams>(DidChangeWatchedFiles::METHOD)
            {
                on_watched_files_change(connection, state, job_tx, params);
            }
        }
        _ => {}
    }
}

/// The id under which we register the watched-files capability, reused to deregister.
const WATCHED_FILES_REGISTRATION_ID: &str = "badness-watched-files";

/// Dynamically register file watchers for the project's on-disk leaves
/// (`**/*.{tex,bib}`) and the config file (`badness.toml`), so out-of-editor edits to
/// non-open includes reanalyze open documents. Called once right after the initialize
/// handshake. A no-op when the client lacks
/// `didChangeWatchedFiles.dynamicRegistration` (we then rely on seed-on-open). The
/// client's response is fire-and-forget — the main loop ignores it.
fn register_file_watchers(connection: &Connection, state: &mut GlobalState) {
    if !state.supports_dynamic_watchers {
        return;
    }
    let options = DidChangeWatchedFilesRegistrationOptions {
        watchers: vec![
            FileSystemWatcher {
                glob_pattern: GlobPattern::String("**/*.{tex,bib}".to_owned()),
                kind: None,
            },
            FileSystemWatcher {
                glob_pattern: GlobPattern::String("**/badness.toml".to_owned()),
                kind: None,
            },
        ],
    };
    let registration = Registration {
        id: WATCHED_FILES_REGISTRATION_ID.to_owned(),
        method: DidChangeWatchedFiles::METHOD.to_owned(),
        register_options: serde_json::to_value(options).ok(),
    };
    let params = RegistrationParams {
        registrations: vec![registration],
    };
    let Ok(params) = serde_json::to_value(params) else {
        return;
    };
    let id = state.next_request_id;
    state.next_request_id += 1;
    let _ = connection.sender.send(Message::Request(Request {
        id: RequestId::from(id),
        method: RegisterCapability::METHOD.to_owned(),
        params,
    }));
}

/// Handle a `workspace/didChangeWatchedFiles` batch. For each event on a **non-open**
/// file (an open buffer's overlay text is authoritative, so it is skipped — `didChange`
/// keeps it current): a `badness.toml` change clears the config cache and re-lints open
/// docs; a `.tex`/`.bib` change is forwarded to the worker to re-read/evict and re-lint.
fn on_watched_files_change(
    connection: &Connection,
    state: &mut GlobalState,
    job_tx: &Sender<WorkerJob>,
    params: DidChangeWatchedFilesParams,
) {
    let mut config_changed = false;
    for event in params.changes {
        let path = uri_to_path(&event.uri);
        // An open buffer's truth is the editor overlay, not disk — leave it to
        // `didChange`. Compare by normalized path, since a watcher URI may be encoded
        // differently than the `didOpen` URI.
        if state.documents.keys().any(|open| uri_to_path(open) == path) {
            continue;
        }
        if path.file_name().is_some_and(|name| name == "badness.toml") {
            config_changed = true;
        } else {
            let _ = job_tx.send(WorkerJob::WatchedChange {
                path,
                deleted: event.typ == FileChangeType::DELETED,
            });
        }
    }
    if config_changed {
        // A discovered `badness.toml` changed on disk: drop cached resolutions so the
        // next analyze re-reads it, then re-lint open docs (mirrors
        // `didChangeConfiguration`, plus the relint a fresh config implies).
        state.config_cache.clear();
        relint_all_open(connection, state, job_tx);
    }
}

/// Apply a batch of `didChange` content changes to `buffer`, in order, and report
/// the transform as a chain of [`Edit`]s. A change with no range replaces the
/// whole buffer; a ranged change splices via the buffer's (encoding-aware)
/// [`LineIndex`].
///
/// Each change yields a *new* [`TextBuffer`], because each mutation shifts later
/// offsets and so invalidates the index the next change resolves against —
/// and because a job that captured the buffer before this notification must keep
/// seeing the version it captured. The usual batch is one change, so the usual
/// keystroke builds one index and rebuilds the text once.
///
/// The returned chain is the incremental reparse's only input (`AGENTS.md`
/// decision #6: there is deliberately no whole-text `diff_edit` fallback inside
/// `parsed_document`), and it is expressed the way that consumer reads it — each
/// edit against the text its predecessors produced, so
/// `apply_edits(old, &chain)` reproduces `buffer` exactly. The offsets are the
/// *clamped* ones the splice actually used, never the raw client positions, so
/// the chain describes the transform the buffer took rather than the one the
/// client asked for.
///
/// [`None`] means an unknown transform: a range-less whole-buffer replacement
/// somewhere in the batch, which is a ~100% edit window a cost guard would
/// decline anyway. The whole batch degrades, because a chain has to describe the
/// entire step from the old text to the new one or it describes nothing.
///
/// `pub` so `benches/keystroke.rs` can time the real splice rather than a
/// re-implementation of it — the write phase it measures is exactly this call
/// plus [`upsert_file`](crate::incremental::IncrementalDatabase::upsert_file)
/// and the stage that follows it.
#[must_use = "the edit chain is the incremental reparse's only input; dropping it \
              silently costs a full parse per keystroke"]
pub fn apply_content_changes(
    buffer: &mut Arc<TextBuffer>,
    changes: Vec<TextDocumentContentChangeEvent>,
) -> Option<Vec<Edit>> {
    let mut edits = Some(Vec::with_capacity(changes.len()));
    for change in changes {
        let next = match change.range {
            None => {
                edits = None;
                TextBuffer::new(change.text, buffer.encoding())
            }
            Some(range) => {
                let idx = buffer.line_index();
                let start = idx.offset_at(range.start.line, range.start.character);
                let end = idx.offset_at(range.end.line, range.end.character);
                // Guard against a degenerate (start > end) range from a misbehaving
                // client: clamp rather than panic on the splice.
                let (start, end) = (start.min(end), start.max(end));
                let next = buffer.with_replacement(start..end, &change.text);
                // Recorded after the splice borrowed `change.text`, so the insert
                // moves rather than cloning on every keystroke. `offset_at` answers
                // in bounds and on a char boundary of this text, which is exactly
                // `Edit::fits`, so the chain is well-formed by construction.
                if let Some(edits) = edits.as_mut() {
                    edits.push(Edit {
                        range: start..end,
                        insert: change.text,
                    });
                }
                next
            }
        };
        *buffer = Arc::new(next);
    }
    edits
}

/// `textDocument/formatting`: build a format job for the worker, or reply `null`
/// when the document is unknown.
fn on_formatting(
    connection: &Connection,
    state: &mut GlobalState,
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
    if !state.documents.contains_key(&uri) {
        // Unknown document: nothing to format.
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    }
    let resolved = state.resolve_settings(&uri);
    let mut style = resolved.style;
    // A discovered `badness.toml` wins outright; only when none
    // governs does the request's `tab_size` override the indent width.
    if !resolved.config_present && params.options.tab_size > 0 {
        style.indent_width = params.options.tab_size as usize;
    }
    let path = uri_to_path(&uri);
    let kind = file_kind_for(&path);
    // `wrap` is decided per request: a configured `wrap` wins, else the default
    // (`reflow`, the same for every file kind).
    style.wrap = resolved.wrap_override.unwrap_or_default();
    let text = state.documents[&uri].text.clone();
    let _ = job_tx.send(WorkerJob::Format {
        id,
        path,
        text,
        style,
        kind,
        sentence_lang: resolved.sentence_lang,
        sentence_no_break: resolved.sentence_no_break,
    });
}

/// `textDocument/rangeFormatting`: build a range-format job for the worker, or
/// reply `null` when the document is unknown. Mirrors [`on_formatting`]; the only
/// extra input is the selection `range`, resolved against the buffer on the read
/// pool.
fn on_range_formatting(
    connection: &Connection,
    state: &mut GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let params = match req.extract::<DocumentRangeFormattingParams>(RangeFormatting::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid range formatting params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    let uri = params.text_document.uri;
    if !state.documents.contains_key(&uri) {
        // Unknown document: nothing to format.
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    }
    let resolved = state.resolve_settings(&uri);
    let mut style = resolved.style;
    if !resolved.config_present && params.options.tab_size > 0 {
        style.indent_width = params.options.tab_size as usize;
    }
    let path = uri_to_path(&uri);
    let kind = file_kind_for(&path);
    style.wrap = resolved.wrap_override.unwrap_or_default();
    let text = state.documents[&uri].text.clone();
    let _ = job_tx.send(WorkerJob::RangeFormat {
        id,
        path,
        text,
        style,
        kind,
        range: params.range,
        sentence_lang: resolved.sentence_lang,
        sentence_no_break: resolved.sentence_no_break,
    });
}

/// Handle `textDocument/onTypeFormatting`. The client fires this only on a
/// registered trigger (`}`); we re-check the character and dispatch a read-pool
/// job that re-indents the containing block when the `}` structurally closes a
/// multi-line construct. Mirrors [`on_range_formatting`]'s settings resolution.
fn on_type_formatting(
    connection: &Connection,
    state: &mut GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let params = match req.extract::<DocumentOnTypeFormattingParams>(OnTypeFormatting::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid on-type formatting params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    // We only re-indent on `}`. Any other trigger is a no-op (reply `null`).
    let uri = params.text_document_position.text_document.uri;
    if params.ch != "}" || !state.documents.contains_key(&uri) {
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    }
    let resolved = state.resolve_settings(&uri);
    let mut style = resolved.style;
    if !resolved.config_present && params.options.tab_size > 0 {
        style.indent_width = params.options.tab_size as usize;
    }
    let path = uri_to_path(&uri);
    let kind = file_kind_for(&path);
    style.wrap = resolved.wrap_override.unwrap_or_default();
    let text = state.documents[&uri].text.clone();
    let _ = job_tx.send(WorkerJob::OnTypeFormat {
        id,
        path,
        text,
        style,
        kind,
        position: params.text_document_position.position,
        sentence_lang: resolved.sentence_lang,
        sentence_no_break: resolved.sentence_no_break,
    });
}

/// `textDocument/documentSymbol`: build an outline job for the worker, or reply
/// `null` when the document is unknown.
fn on_document_symbol(
    connection: &Connection,
    state: &mut GlobalState,
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
    let text = doc.text.clone();
    // Resolve `[build]` for the `.aux` number enrichment, like hover.
    let build = state.resolve_settings(&uri).build;
    let _ = job_tx.send(WorkerJob::Symbols {
        id,
        path,
        text,
        kind,
        build,
    });
}

/// `workspace/symbol`: forward the query to the worker, which scans every tracked
/// project file. Unlike [`on_document_symbol`], it is not tied to an open buffer.
fn on_workspace_symbol(connection: &Connection, job_tx: &Sender<WorkerJob>, req: Request) {
    let id = req.id.clone();
    let params = match req.extract::<WorkspaceSymbolParams>(WorkspaceSymbolRequest::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid workspace/symbol params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };
    let _ = job_tx.send(WorkerJob::WorkspaceSymbols {
        id,
        query: params.query,
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

/// `textDocument/selectionRange`: build a selection-range job for the worker, or reply
/// `null` when the document is unknown. Modeled on [`on_folding_range`], plus the
/// cursor `positions` the expand-selection chains are computed at.
fn on_selection_range(
    connection: &Connection,
    state: &GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let params = match req.extract::<SelectionRangeParams>(SelectionRangeRequest::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid selectionRange params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    let uri = params.text_document.uri;
    let Some(doc) = state.documents.get(&uri) else {
        // Unknown document: no ranges.
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    };
    let path = uri_to_path(&uri);
    let kind = file_kind_for(&path);
    let _ = job_tx.send(WorkerJob::SelectionRange {
        id,
        path,
        text: doc.text.clone(),
        kind,
        positions: params.positions,
    });
}

/// `textDocument/documentLink`: dispatch a document-link job to the worker. Replies
/// `null` for an unknown document; otherwise the read pool resolves the links.
/// Single-file (with no project snapshot), modeled on [`on_folding_range`].
fn on_document_link(
    connection: &Connection,
    state: &mut GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let params = match req.extract::<DocumentLinkParams>(DocumentLinkRequest::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid documentLink params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    let uri = params.text_document.uri;
    let Some(doc) = state.documents.get(&uri) else {
        // Unknown document: no links.
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    };
    let text = doc.text.clone();
    let path = uri_to_path(&uri);
    let kind = file_kind_for(&path);
    // `texmf` is editor (machine) configuration, not resolved per document
    // (session-stable; the index is first-wins).
    let texmf = state.editor_settings.texmf.clone();
    let _ = job_tx.send(WorkerJob::DocumentLink {
        id,
        path,
        text,
        kind,
        texmf,
    });
}

/// `textDocument/diagnostic`: build an on-demand diagnostic job for the worker.
///
/// Always replies with a *report* (never `null`): an empty full report when the
/// client is push-only (it should not be pulling) or the document is unknown,
/// otherwise a [`WorkerJob::Diagnostic`] that computes off a fresh snapshot. The
/// snapshot is current because the preceding edit's `Edit` job sits ahead of this
/// one on the FIFO `job_tx` (see [`WorkerJob::Diagnostic`]).
fn on_document_diagnostic(
    connection: &Connection,
    state: &mut GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let params = match req.extract::<DocumentDiagnosticParams>(DocumentDiagnosticRequest::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid diagnostic params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    let uri = params.text_document.uri;
    // A push-only client should not be pulling; an unknown document has no buffer.
    // Either way, answer with an empty full report rather than leaving the request
    // hanging or replying `null`.
    if !state.supports_pull_diagnostics {
        reply_empty_diagnostic_report(connection, id);
        return;
    }
    let Some(doc) = state.documents.get(&uri) else {
        reply_empty_diagnostic_report(connection, id);
        return;
    };
    let text = doc.text.clone();
    let path = uri_to_path(&uri);
    let kind = file_kind_for(&path);
    let rules = state.resolve_settings(&uri).rule_selection();
    let _ = job_tx.send(WorkerJob::Diagnostic {
        id,
        path,
        text,
        kind,
        previous_result_id: params.previous_result_id,
        rules,
    });
}

/// Reply to a `textDocument/diagnostic` request with an empty *full* report. Used
/// when there is nothing to compute (push-only client, unknown buffer) — the pull
/// protocol requires a report, so `null` is not an option.
fn reply_empty_diagnostic_report(connection: &Connection, id: RequestId) {
    let report = DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(
        RelatedFullDocumentDiagnosticReport::default(),
    ));
    let value = serde_json::to_value(report).unwrap_or(serde_json::Value::Null);
    let _ = connection
        .sender
        .send(Message::Response(Response::new_ok(id, value)));
}

/// `textDocument/completion`: build a completion job for the worker, or reply
/// `null` when the document is unknown.
fn on_completion(
    connection: &Connection,
    state: &mut GlobalState,
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
    let text = doc.text.clone();
    // The editor's `texmf` settings gate the installed-set completion tier
    // (session-stable).
    let texmf = state.editor_settings.texmf.clone();
    let _ = job_tx.send(WorkerJob::Completion {
        id,
        uri,
        text,
        position,
        texmf,
    });
}

/// `completionItem/resolve`: dispatch a resolve job for the worker. The item's
/// `data` is self-contained (it carries everything needed to recompute detail),
/// so there is no document to look up — only invalid params short-circuit here.
fn on_completion_resolve(connection: &Connection, job_tx: &Sender<WorkerJob>, req: Request) {
    let id = req.id.clone();
    let item = match req.extract::<CompletionItem>(ResolveCompletionItem::METHOD) {
        Ok((_, item)) => item,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid completion item".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };
    let _ = job_tx.send(WorkerJob::ResolveCompletion {
        id,
        item: Box::new(item),
    });
}

/// `textDocument/hover`: build a hover job for the worker, or reply `null` when the
/// document is unknown. A `.bib` cursor is not rejected — `compute_hover` simply finds
/// nothing there today (no bib-field hover yet), so it returns `null` on its own.
fn on_hover(
    connection: &Connection,
    state: &mut GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let params = match req.extract::<HoverParams>(HoverRequest::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid hover params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    let uri = params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    let path = uri_to_path(&uri);
    let Some(doc) = state.documents.get(&uri) else {
        // Unknown document: nothing to describe.
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    };
    let text = doc.text.clone();
    // Resolve `[build]` for the label-number lookup (a `\label`/`\ref` hover reads
    // the compile's `.aux`), like go-to-def resolves `[texmf]`.
    let build = state.resolve_settings(&uri).build;
    let _ = job_tx.send(WorkerJob::Hover {
        id,
        path,
        text,
        position,
        build,
    });
}

/// `textDocument/forwardSearch`: launch the configured PDF viewer at the cursor's
/// source position.
///
/// Ordered so the cheap rejections never cost a database snapshot: an
/// unconfigured viewer and a pathless buffer are both answered here, and only a
/// request that can actually reach a viewer becomes a worker job.
///
/// The document need *not* be open — the database may know it as a seeded
/// sibling, and the namespace scan falls back to the file itself either way.
///
/// One deliberate divergence from texlab, which answers every forward search with
/// a status and never a JSON-RPC error: malformed params is a *client* bug, not a
/// search outcome, and the four statuses describe semantic results. A client that
/// sends a well-formed request can never see this error.
fn on_forward_search(
    connection: &Connection,
    state: &mut GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let reply = |status: ForwardSearchStatus| {
        let result = serde_json::to_value(status.result()).unwrap_or(serde_json::Value::Null);
        let _ = connection
            .sender
            .send(Message::Response(Response::new_ok(id.clone(), result)));
    };
    let params = match req.extract::<TextDocumentPositionParams>(ForwardSearchRequest::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid forwardSearch params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    let Some((executable, args)) = state.editor_settings.forward_search.viewer() else {
        reply(ForwardSearchStatus::Unconfigured);
        return;
    };
    let (executable, args) = (executable.to_owned(), args.to_vec());

    let uri = params.text_document.uri;
    // An untitled buffer has no path, so it was never compiled into anything.
    let Some(path) = uri_to_fs_path(&uri) else {
        reply(ForwardSearchStatus::Failure);
        return;
    };
    // SyncTeX's granularity is the line, so `position.character` is ignored.
    let line = params.position.line + 1;
    let build = state.resolve_settings(&uri).build;
    let _ = job_tx.send(WorkerJob::ForwardSearch {
        id,
        path,
        line,
        build,
        executable,
        args,
    });
}

/// `textDocument/signatureHelp`: build a signature-help job for the worker, or
/// reply `null` when the document is unknown. The request's `context` is ignored:
/// help is recomputed statelessly per request, and with exactly one signature per
/// reply `is_retrigger`/`active_signature_help` add nothing.
fn on_signature_help(
    connection: &Connection,
    state: &mut GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let params = match req.extract::<SignatureHelpParams>(SignatureHelpRequest::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid signature help params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    let uri = params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    let path = uri_to_path(&uri);
    let Some(doc) = state.documents.get(&uri) else {
        // Unknown document: nothing to describe.
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    };
    let text = doc.text.clone();
    let _ = job_tx.send(WorkerJob::SignatureHelp {
        id,
        path,
        text,
        position,
    });
}

/// `textDocument/codeAction`: build a code-action job for the worker, or reply with
/// an empty action list when the document is unknown. Surfaces linter autofixes as
/// quick-fixes and syntax-aware LaTeX refactorings; a `.bib` cursor is handled too
/// (its bib-lint fixes are surfaced the same way).
fn on_code_action(
    connection: &Connection,
    state: &mut GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let params = match req.extract::<CodeActionParams>(CodeActionRequest::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid codeAction params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    let uri = params.text_document.uri;
    let range = params.range;
    let only = params.context.only;
    let Some(doc) = state.documents.get(&uri) else {
        // Unknown document: no actions.
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    };
    let text = doc.text.clone();
    let path = uri_to_path(&uri);
    let kind = file_kind_for(&path);
    let rules = state.resolve_settings(&uri).rule_selection();
    let _ = job_tx.send(WorkerJob::CodeAction {
        id,
        uri,
        path,
        text,
        kind,
        range,
        only,
        rules,
    });
}

/// `textDocument/definition`: build a go-to-definition job for the worker, or reply
/// `null` when the document is unknown or is a `.bib` (cite/ref sites live in
/// `.tex`, so a `.bib` cursor has nothing to jump *from*).
fn on_goto_definition(
    connection: &Connection,
    state: &mut GlobalState,
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
    let text = doc.text.clone();
    // The editor's `texmf` settings gate the file-target fallback (an include/package
    // argument jumps to its resolved source, TEXMF-aware like document links).
    let texmf = state.editor_settings.texmf.clone();
    let _ = job_tx.send(WorkerJob::GotoDefinition {
        id,
        path,
        text,
        position,
        texmf,
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

/// `textDocument/documentHighlight`: build a document-highlight job for the worker,
/// or reply `null` when the document is unknown. Single-file, so no project membership
/// is captured — the worker shades the key under the cursor against the cursor buffer
/// alone.
fn on_document_highlight(
    connection: &Connection,
    state: &GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let params = match req.extract::<DocumentHighlightParams>(DocumentHighlightRequest::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid document-highlight params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    let uri = params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    let path = uri_to_path(&uri);
    let Some(doc) = state.documents.get(&uri) else {
        // Unknown document: nothing to highlight.
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    };
    let _ = job_tx.send(WorkerJob::DocumentHighlight {
        id,
        path,
        text: doc.text.clone(),
        position,
    });
}

/// `textDocument/prepareRename`: build a prepare-rename job, or reply `null` when
/// the document is unknown. The worker decides whether the cursor sits on a
/// renameable key (and returns its range + placeholder) or declines with `null`.
fn on_prepare_rename(
    connection: &Connection,
    state: &GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let params = match req.extract::<TextDocumentPositionParams>(PrepareRenameRequest::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid prepareRename params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    let uri = params.text_document.uri;
    let position = params.position;
    let path = uri_to_path(&uri);
    let Some(doc) = state.documents.get(&uri) else {
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    };
    let _ = job_tx.send(WorkerJob::PrepareRename {
        id,
        path,
        text: doc.text.clone(),
        position,
    });
}

/// `textDocument/rename`: build a rename job, or reply `null` when the document is
/// unknown. The worker resolves the key under the cursor and answers with a
/// project-wide [`WorkspaceEdit`] (or `null` when the rename is declined).
fn on_rename(
    connection: &Connection,
    state: &GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let params = match req.extract::<RenameParams>(Rename::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            let resp = Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                "invalid rename params".to_owned(),
            );
            let _ = connection.sender.send(Message::Response(resp));
            return;
        }
    };

    let uri = params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;
    let new_name = params.new_name;
    let path = uri_to_path(&uri);
    let Some(doc) = state.documents.get(&uri) else {
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    };
    let _ = job_tx.send(WorkerJob::Rename {
        id,
        path,
        text: doc.text.clone(),
        position,
        new_name,
    });
}

/// `workspace/executeCommand`: route the change-environment refactor (either
/// command id) to the worker. The single argument is `RenameParams`-shaped —
/// texlab's wire format, so texlab clients need no adaptation. Unlike the
/// document-keyed handlers, failures reply with an *error* (not `null`): a command
/// is an explicit user action, so a silent no-op would read as a bug.
fn on_execute_command(
    connection: &Connection,
    state: &GlobalState,
    job_tx: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let respond_err = |code: ErrorCode, message: String| {
        let resp = Response::new_err(id.clone(), code as i32, message);
        let _ = connection.sender.send(Message::Response(resp));
    };
    let params = match req.extract::<ExecuteCommandParams>(ExecuteCommand::METHOD) {
        Ok((_, params)) => params,
        Err(_) => {
            respond_err(
                ErrorCode::InvalidParams,
                "invalid executeCommand params".to_owned(),
            );
            return;
        }
    };
    match params.command.as_str() {
        CHANGE_ENVIRONMENT_COMMAND | CHANGE_ENVIRONMENT_COMMAND_TEXLAB => {
            let Some(args) = params
                .arguments
                .into_iter()
                .next()
                .and_then(|arg| serde_json::from_value::<RenameParams>(arg).ok())
            else {
                respond_err(
                    ErrorCode::InvalidParams,
                    "changeEnvironment expects one {textDocument, position, newName} argument"
                        .to_owned(),
                );
                return;
            };
            let new_name = args.new_name;
            if !is_valid_key(&new_name) {
                respond_err(
                    ErrorCode::InvalidParams,
                    format!("`{new_name}` is not a valid environment name"),
                );
                return;
            }
            let uri = args.text_document_position.text_document.uri;
            let position = args.text_document_position.position;
            let Some(doc) = state.documents.get(&uri) else {
                respond_err(
                    ErrorCode::InvalidParams,
                    format!("document is not open: {}", uri.as_str()),
                );
                return;
            };
            let path = uri_to_path(&uri);
            let _ = job_tx.send(WorkerJob::ChangeEnvironment {
                id,
                uri,
                path,
                text: doc.text.clone(),
                position,
                new_name,
            });
        }
        other => respond_err(
            ErrorCode::InvalidParams,
            format!("unknown workspace command: {other}"),
        ),
    }
}

/// Forward a worker result to the client. Diagnostics are version-gated: a result
/// is sent only when its document is still open at exactly that version, so a
/// stale (superseded or post-close) analyze never repaints squiggles.
fn forward_outbound(
    connection: &Connection,
    state: &mut GlobalState,
    job_tx: &Sender<WorkerJob>,
    outbound: Outbound,
) {
    match outbound {
        Outbound::Diagnostics {
            uri,
            version,
            diags,
        } => {
            // Pull and push are mutually exclusive: a pull-capable client is served
            // exclusively via `textDocument/diagnostic`, so drop the push (the
            // analyze still ran, warming the salsa memos the pull reads).
            if state.supports_pull_diagnostics {
                return;
            }
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
        Outbound::ApplyEdit { id, label, edit } => {
            let params = ApplyWorkspaceEditParams {
                label: Some(label),
                edit,
            };
            if let Ok(params) = serde_json::to_value(params) {
                let request_id = state.next_request_id;
                state.next_request_id += 1;
                let _ = connection.sender.send(Message::Request(Request {
                    id: RequestId::from(request_id),
                    method: ApplyWorkspaceEdit::METHOD.to_owned(),
                    params,
                }));
            }
            let _ = connection.sender.send(Message::Response(Response::new_ok(
                id,
                serde_json::Value::Null,
            )));
        }
        Outbound::RelintAll => relint_all_open(connection, state, job_tx),
    }
}

/// Answer a viewer's inverse search: reveal `msg`'s source position in the editor.
///
/// Acceptance first, because the client is fanning out across every listening
/// server and a wrong "yes" strands the request here. A file counts as ours when
/// it is open, when it sits under one of our workspace roots, or when we have no
/// roots at all (a client that opened a bare file takes anything).
///
/// The responder is answered *before* the editor is told, in that order
/// deliberately: the viewer-side command blocks on the ack, and the integration
/// test drives both ends from one process.
fn on_inverse_search(connection: &Connection, state: &mut GlobalState, msg: IpcMessage) {
    let IpcMessage { request, responder } = msg;
    let Some(uri) = path_to_uri(&request.path) else {
        responder.reject("not a representable path");
        return;
    };
    if !state.documents.contains_key(&uri) && !state.owns_path(&request.path) {
        responder.reject("outside this server's workspace");
        return;
    }
    responder.accept();

    // The wire is 1-based (SyncTeX's convention, and every viewer's); LSP is
    // 0-based. A viewer reporting line 0 would be out of contract, so saturate
    // rather than wrap.
    let position = Position::new(request.line.saturating_sub(1), request.character);
    let params = ShowDocumentParams {
        uri,
        external: Some(false),
        take_focus: Some(true),
        selection: Some(Range {
            start: position,
            end: position,
        }),
    };
    let Ok(params) = serde_json::to_value(params) else {
        return;
    };
    // Fire-and-forget, like `workspace/applyEdit`: the client's `{success}` reply
    // is swallowed by the main loop's `Message::Response` arm, since there is
    // nothing we would do differently on a `false`.
    let id = state.next_request_id;
    state.next_request_id += 1;
    let _ = connection.sender.send(Message::Request(Request {
        id: RequestId::from(id),
        method: ShowDocument::METHOD.to_owned(),
        params,
    }));
}

/// Re-lint every open document, because cross-file resolution may have changed for all
/// of them (a project member's content changed, membership grew, or the governing
/// config changed). A pull client learns this by re-pulling: nudge it with
/// `workspace/diagnostic/refresh`. A push client gets a fresh analyze re-queued per
/// open document at its current version. Shared by [`Outbound::RelintAll`] and the
/// `badness.toml` watched-change path.
fn relint_all_open(connection: &Connection, state: &mut GlobalState, job_tx: &Sender<WorkerJob>) {
    if state.supports_pull_diagnostics {
        if state.supports_diagnostic_refresh {
            let id = state.next_request_id;
            state.next_request_id += 1;
            let _ = connection.sender.send(Message::Request(Request {
                id: RequestId::from(id),
                method: WorkspaceDiagnosticRefresh::METHOD.to_owned(),
                params: serde_json::Value::Null,
            }));
        }
        return;
    }
    // Push mode: re-queue a fresh analyze for every open document at its current
    // version. The worker coalesces per-URI, so this is cheap; salsa memos make the
    // actual recompute incremental. A re-lint of a doc in an already-seeded directory
    // discovers no new members, so it can't re-trigger `RelintAll` (no loop). Snapshot
    // the buffers first so the per-document `resolve_settings` (`&mut self`) doesn't
    // alias the `documents` borrow.
    let snapshot: Vec<(Uri, Arc<TextBuffer>, i32)> = state
        .documents
        .iter()
        .map(|(uri, doc)| (uri.clone(), doc.text.clone(), doc.version))
        .collect();
    for (uri, text, version) in snapshot {
        let path = uri_to_path(&uri);
        let kind = file_kind_for(&path);
        let resolved = state.analysis_settings(&uri, job_tx);
        let _ = job_tx.send(WorkerJob::Edit {
            uri,
            path,
            text,
            version,
            kind,
            rules: resolved.rule_selection(),
            exclude: resolved.exclude,
            // The same text at the same version: nothing moved, so there is no
            // transform to describe. Clearing costs nothing — the base already is
            // this text, so `parsed_document` answers from it without a chain.
            edits: None,
        });
    }
}

// ---------------------------------------------------------------------------
// Worker thread (sole database writer).
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
    /// The document's resolved lint-rule selection, applied to the analyze.
    rules: RuleSelection,
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
    encoding: PositionEncoding,
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
                encoding,
                inflight: None,
                pending: HashMap::new(),
                seeded_dirs: HashSet::new(),
                bib_lookups: HashMap::new(),
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
    /// The position encoding negotiated at `initialize`, threaded into every
    /// read job so its `LineIndex` conversions count columns in the negotiated
    /// unit (see [`negotiate_position_encoding`]).
    encoding: PositionEncoding,
    /// The single in-flight analyze, if any. At most one runs at a time: the
    /// write-phase needs exclusive `&mut db`, and salsa cancellation is global, so
    /// a second concurrent analyze couldn't be cancelled selectively.
    inflight: Option<InflightAnalyze>,
    /// Coalesced analyze queue: the latest pending request per URI.
    pending: HashMap<Uri, AnalyzeRequest>,
    /// Directories already walked for on-disk `.tex`/`.bib` siblings, so each is
    /// seeded at most once (the membership-discovery hot-path guard).
    seeded_dirs: HashSet<PathBuf>,
    /// Cached bibliography search-path lookups, including misses. The server's
    /// inherited environment is fixed for its lifetime, and local file creation
    /// reaches the database through watched-file events, so repeating a failed
    /// `kpsewhich` process on every keystroke would buy nothing.
    bib_lookups: HashMap<PathBuf, Option<PathBuf>>,
}

/// Load bibliography resources referenced by tracked LaTeX files but located
/// outside ordinary sibling discovery. `resolve` owns all environment/filesystem
/// search policy; the database receives only explicit files and aliases, keeping
/// salsa queries deterministic.
fn seed_bibliographies_with(
    db: &mut IncrementalDatabase,
    lookups: &mut HashMap<PathBuf, Option<PathBuf>>,
    mut resolve: impl FnMut(&Path, Option<&Path>) -> Option<PathBuf>,
) -> bool {
    let mut grew = false;
    let tracked = db.tracked_files();
    for (source_path, source_file) in tracked {
        if !file_kind_or_tex(&source_path).is_latex() {
            continue;
        }
        let targets = file_cite_facts(db, source_file).bib_targets.clone();
        let base_dir = source_path.parent();
        for target in targets {
            let BibTarget::Path(requested) = target else {
                continue;
            };
            if db.lookup_file(&requested).is_some() {
                continue;
            }
            if let Some(actual) = db.bibliography_alias(&requested).map(Path::to_path_buf)
                && db.lookup_file(&actual).is_some()
            {
                continue;
            }

            let actual = match lookups.get(&requested) {
                Some(cached) => cached.clone(),
                None => {
                    let found = resolve(&requested, base_dir);
                    lookups.insert(requested.clone(), found.clone());
                    found
                }
            };
            let Some(actual) = actual else {
                continue;
            };

            let already_tracked = db.lookup_file(&actual).is_some();
            if !already_tracked {
                let Ok(text) = std::fs::read_to_string(&actual) else {
                    lookups.insert(requested, None);
                    continue;
                };
                let file = db.upsert_file(&actual, text);
                db.reparse_stage_edits(file, None);
                grew = true;
            }
            if actual != requested {
                grew |= db.set_bibliography_alias(&requested, &actual);
            }
        }
    }
    grew
}

impl Worker {
    fn run(&mut self, job_rx: &Receiver<WorkerJob>, done_rx: &Receiver<AnalyzeDone>) {
        loop {
            select! {
                recv(job_rx) -> job => {
                    let Ok(job) = job else { break };  // main dropped `job_tx`
                    self.handle_job_guarded(job);
                    while let Ok(j) = job_rx.try_recv() {
                        self.handle_job_guarded(j);
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

    /// Run [`handle_job`](Self::handle_job), catching any panic so one bad job
    /// can't silently kill the single write-phase worker thread — which would
    /// leave the server a zombie (the main loop keeps running, but every
    /// `job_tx.send` then no-ops, so no further diagnostics, formatting, or
    /// edits reach the db). Mirrors the read pool's per-job isolation
    /// (`task_pool.rs`). Salsa `Cancelled` never unwinds this far: the writer
    /// owns the db exclusively, so its writes are never cancelled.
    fn handle_job_guarded(&mut self, job: WorkerJob) {
        let guarded = std::panic::AssertUnwindSafe(|| self.handle_job(job));
        if let Err(panic) = std::panic::catch_unwind(guarded) {
            let msg = panic
                .downcast_ref::<&'static str>()
                .copied()
                .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("<non-string panic payload>");
            log::error!("LSP worker thread caught panic while handling job: {msg}");
        }
    }

    fn handle_job(&mut self, job: WorkerJob) {
        let enc = self.encoding;
        match job {
            WorkerJob::Edit {
                uri,
                path,
                text,
                version,
                kind,
                rules,
                exclude,
                edits,
            } => {
                // Write-phase: push the live buffer into the db. Cheap — the parse
                // is a lazy salsa query deferred to the analyze. Acquiring `&mut
                // db` blocks until any outstanding read snapshot drops (single
                // writer), which is how a fresher edit preempts an in-flight read.
                let file = self.db.upsert_file(&path, text.text_arc());
                // Stage the transform for the incremental reparse, **after** the
                // write and never before: that `&mut db` is what proves no analyze
                // is reading. A chain staged ahead of the text it describes could be
                // peeked by an in-flight `parsed_document`, which would fail to
                // verify it, full-parse, and then drain it — losing the edit for
                // good. Staged unconditionally, including when `upsert_file` skipped
                // its write: the chain is anchored at the reparse *base*, not at the
                // db text, and a buffer that round-trips back to what salsa holds
                // still took a transform to get there.
                self.db.reparse_stage_edits(file, edits);
                // Lazily pull the rest of the project off disk so cross-file rules
                // can fire. If this grows the member set, every open document's
                // resolution may have changed — re-lint them all.
                let mut membership_grew = self.seed_dir(&path, &exclude);
                membership_grew |= seed_bibliographies_with(
                    &mut self.db,
                    &mut self.bib_lookups,
                    crate::project::bibliography::resolve_bibliography_file,
                );
                if membership_grew {
                    let _ = self.out_tx.send(Outbound::RelintAll);
                }
                self.enqueue(AnalyzeRequest {
                    uri,
                    path,
                    version,
                    kind,
                    rules,
                });
            }
            WorkerJob::Declarations { declarations } => {
                // `set_declarations` no-ops on an unchanged value, so this is safe
                // to send defensively; the main loop's own mirror keeps it rare.
                self.db.set_declarations((*declarations).clone());
            }
            WorkerJob::Close { path } => {
                self.db.remove_file(&path);
            }
            WorkerJob::WatchedChange { path, deleted } => {
                if self.apply_watched_change(&path, deleted) {
                    let _ = self.out_tx.send(Outbound::RelintAll);
                }
            }
            WorkerJob::Format {
                id,
                path,
                text,
                style,
                kind,
                sentence_lang,
                sentence_no_break,
            } => {
                // Format reads run on the read pool against a snapshot, concurrent
                // with the analyze slot (they are id-bound responses, not coalesced).
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    let sentence =
                        SentenceOptions::from_resolved(sentence_lang, &sentence_no_break);
                    run_format(&snapshot, id, &path, &text, style, kind, sentence, &out_tx)
                });
            }
            WorkerJob::RangeFormat {
                id,
                path,
                text,
                style,
                kind,
                range,
                sentence_lang,
                sentence_no_break,
            } => {
                // Range formatting runs on the read pool against a snapshot, exactly
                // like `Format` (an id-bound response, not coalesced).
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    let sentence =
                        SentenceOptions::from_resolved(sentence_lang, &sentence_no_break);
                    run_range_format(
                        &snapshot, id, &path, &text, style, kind, range, sentence, &out_tx,
                    )
                });
            }
            WorkerJob::OnTypeFormat {
                id,
                path,
                text,
                style,
                kind,
                position,
                sentence_lang,
                sentence_no_break,
            } => {
                // On-type formatting reads on the read pool against a snapshot,
                // exactly like `RangeFormat` (an id-bound response, not coalesced).
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    let sentence =
                        SentenceOptions::from_resolved(sentence_lang, &sentence_no_break);
                    run_on_type_format(
                        &snapshot, id, &path, &text, style, kind, position, sentence, &out_tx,
                    )
                });
            }
            WorkerJob::Symbols {
                id,
                path,
                text,
                kind,
                build,
            } => {
                // Symbol reads, like formatting, run on the read pool against a
                // snapshot (id-bound responses, not coalesced).
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner
                    .spawn(move || run_symbols(&snapshot, id, &path, &text, kind, &build, &out_tx));
            }
            WorkerJob::WorkspaceSymbols { id, query } => {
                // Workspace symbols scan every file in the database snapshot.
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner
                    .spawn(move || run_workspace_symbols(&snapshot, id, &query, enc, &out_tx));
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
            WorkerJob::SelectionRange {
                id,
                path,
                text,
                kind,
                positions,
            } => {
                // Selection ranges run on the read pool against a snapshot, like
                // folding (single-file, id-bound responses).
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_selection_range(&snapshot, id, &path, &text, kind, &positions, &out_tx)
                });
            }
            WorkerJob::DocumentLink {
                id,
                path,
                text,
                kind,
                texmf,
            } => {
                // Document links run on the read pool against a snapshot, like
                // folding (single-file, id-bound responses). Resolution is positional
                // and disk-aware, so no project membership snapshot is needed. The
                // TEXMF index is built/consulted here (off the main loop).
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_document_link(&snapshot, id, &path, &text, kind, &texmf, &out_tx)
                });
            }
            WorkerJob::Completion {
                id,
                uri,
                text,
                position,
                texmf,
            } => {
                // Completion reads run on the read pool against a snapshot, like
                // formatting/symbols (id-bound responses, not coalesced). The TEXMF
                // index is built/consulted on the read pool, off the main loop.
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_completion(&snapshot, id, &uri, &text, position, &texmf, &out_tx)
                });
            }
            WorkerJob::ResolveCompletion { id, item } => {
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner
                    .spawn(move || run_completion_resolve(&snapshot, id, *item, &out_tx));
            }
            WorkerJob::Hover {
                id,
                path,
                text,
                position,
                build,
            } => {
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_hover(&snapshot, id, &path, &text, position, &build, &out_tx)
                });
            }
            WorkerJob::ForwardSearch {
                id,
                path,
                line,
                build,
                executable,
                args,
            } => {
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_forward_search(
                        &snapshot,
                        id,
                        &path,
                        line,
                        &build,
                        &executable,
                        &args,
                        &out_tx,
                    )
                });
            }
            WorkerJob::SignatureHelp {
                id,
                path,
                text,
                position,
            } => {
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_signature_help(&snapshot, id, &path, &text, position, enc, &out_tx)
                });
            }
            WorkerJob::GotoDefinition {
                id,
                path,
                text,
                position,
                texmf,
            } => {
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_goto_definition(&snapshot, id, &path, &text, position, &texmf, enc, &out_tx)
                });
            }
            WorkerJob::References {
                id,
                path,
                text,
                position,
                include_declaration,
            } => {
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_references(
                        &snapshot,
                        id,
                        &path,
                        &text,
                        position,
                        include_declaration,
                        enc,
                        &out_tx,
                    )
                });
            }
            WorkerJob::DocumentHighlight {
                id,
                path,
                text,
                position,
            } => {
                // Single-file like prepareRename: no project membership, just a db
                // snapshot to reach the cached model when the buffer is current.
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_document_highlight(&snapshot, id, &path, &text, position, &out_tx)
                });
            }
            WorkerJob::PrepareRename {
                id,
                path,
                text,
                position,
            } => {
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_prepare_rename(&snapshot, id, &path, &text, position, &out_tx)
                });
            }
            WorkerJob::Rename {
                id,
                path,
                text,
                position,
                new_name,
            } => {
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_rename(
                        &snapshot, id, &path, &text, position, &new_name, enc, &out_tx,
                    )
                });
            }
            WorkerJob::ChangeEnvironment {
                id,
                uri,
                path,
                text,
                position,
                new_name,
            } => {
                // Single-file like prepareRename: only the cursor buffer's tree is
                // read, so no membership snapshot is needed.
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_change_environment(
                        &snapshot, id, &uri, &path, &text, position, &new_name, &out_tx,
                    )
                });
            }
            WorkerJob::Diagnostic {
                id,
                path,
                text,
                kind,
                previous_result_id,
                rules,
            } => {
                // On-demand pull is a free, id-bound read—not the coalesced analyze
                // slot—so it never blocks or supersedes the push analyze.
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_document_diagnostic(
                        &snapshot,
                        id,
                        &path,
                        &text,
                        kind,
                        previous_result_id,
                        &rules,
                        enc,
                        &out_tx,
                    )
                });
            }
            WorkerJob::CodeAction {
                id,
                uri,
                path,
                text,
                kind,
                range,
                only,
                rules,
            } => {
                // On-demand re-lint, like the pull-diagnostics path, runs against a
                // snapshot on the read pool.
                let snapshot = self.db.snapshot();
                let out_tx = self.out_tx.clone();
                self.read_spawner.spawn(move || {
                    run_code_action(
                        &snapshot,
                        id,
                        &uri,
                        &path,
                        &text,
                        kind,
                        range,
                        only.as_deref(),
                        &rules,
                        enc,
                        &out_tx,
                    )
                });
            }
        }
    }

    /// Walk the active file's directory once for `.tex`/`.bib` siblings, reading
    /// and upserting any not already tracked, so the cross-file resolvers see the
    /// whole project. Returns whether the member set grew.
    ///
    /// Skips unsaved/synthetic buffers (whose path isn't a real file) and the
    /// filesystem root, so we never walk `/`. A sibling that is already tracked —
    /// an open buffer, or one seeded earlier — keeps its live text (we never read
    /// it back from disk). Each directory is walked at most once (`seeded_dirs`).
    ///
    /// `exclude` is the document's resolved [`ExcludeFilter`] (built on the main
    /// side, where the config lives, and threaded through [`WorkerJob::Edit`]), so
    /// a `badness.toml` `exclude`/`extend-exclude` prunes the same siblings here as
    /// it does for the CLI. It is exclude-nothing when no config governs.
    fn seed_dir(&mut self, path: &Path, exclude: &ExcludeFilter) -> bool {
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
        // A discovered `badness.toml` governs sibling discovery here too: the
        // document's resolved exclude filter is built on the main side (where the
        // config lives) and threaded in via `WorkerJob::Edit`, so the same
        // `exclude`/`extend-exclude` that scope the CLI's walk prune these siblings.
        let Ok(files) = collect_lint_files(&[dir], exclude) else {
            return false;
        };
        let mut grew = false;
        for (sibling, _kind) in files {
            if self.db.lookup_file(&sibling).is_some() {
                continue; // open buffer or already seeded — keep its live text
            }
            if let Ok(text) = std::fs::read_to_string(&sibling) {
                let file = self.db.upsert_file(&sibling, text);
                // A whole file off disk: no transform to describe. Pairing every
                // `upsert_file` with a stage is what keeps the rule exceptionless —
                // and clearing a chain a fresh file never had is a no-op, entry
                // included.
                self.db.reparse_stage_edits(file, None);
                grew = true;
            }
        }
        grew
    }

    /// Apply an on-disk change to a non-open `.tex`/`.bib` file (from a watched-files
    /// event): re-read and re-upsert it, or evict it on delete. Returns `true` when the
    /// db actually changed, so the caller can re-lint open documents only when it
    /// matters.
    ///
    /// Scoped to known projects: a change is acted on only when the file is already a
    /// tracked member, or it is a freshly-created sibling in a directory we have already
    /// seeded. The broad `**/*.{tex,bib}` glob also matches files in directories with no
    /// open document, and re-linting every open buffer for those would be pure waste.
    ///
    /// Unlike [`seed_dir`](Self::seed_dir), this deliberately re-reads a tracked file:
    /// the seed path keeps a tracked file's text precisely to avoid clobbering a live
    /// buffer, but the main loop has already excluded open buffers before dispatching
    /// here, so the file's truth is the disk.
    fn apply_watched_change(&mut self, path: &Path, deleted: bool) -> bool {
        let tracked = self.db.lookup_file(path);
        let in_seeded_dir = path
            .parent()
            .is_some_and(|dir| self.seeded_dirs.contains(dir));
        if tracked.is_none() && !in_seeded_dir {
            return false; // not part of any assembled project
        }
        if deleted {
            return self.db.remove_file(path).is_some();
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            return false; // unreadable (e.g. a delete racing the event) — leave as-is
        };
        // Skip the relint when the content is identical to what we already track
        // (a `touch` or a metadata-only event), so we don't re-lint every open doc for
        // nothing. `upsert_file` itself also no-ops the salsa write on equal text.
        if tracked.is_some_and(|file| self.db.text_is_current(file, &text)) {
            return false;
        }
        let file = self.db.upsert_file(path, text);
        // A disk re-read carries no edits, so any chain this file holds describes a
        // transform out of a text it no longer has. Drop it.
        self.db.reparse_stage_edits(file, None);
        true
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
        let enc = self.encoding;
        let snapshot = self.db.snapshot();
        let out_tx = self.out_tx.clone();
        let done_tx = self.done_tx.clone();
        let AnalyzeRequest {
            uri,
            path,
            version,
            kind,
            rules,
        } = req;
        self.inflight = Some(InflightAnalyze {
            uri: uri.clone(),
            version,
        });
        self.read_spawner.spawn(move || {
            let result = salsa::Cancelled::catch(AssertUnwindSafe(|| match kind {
                FileKind::Tex
                | FileKind::CodeTex
                | FileKind::Sty
                | FileKind::Cls
                | FileKind::Dtx
                | FileKind::Ins => analyze_tex(&snapshot, &path, &rules, enc),
                FileKind::Bib => analyze_bib(&snapshot, &path, &rules, enc),
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
/// resolution from the project membership carried by the snapshot.
/// `resolved_labels` / `resolved_citations` drive `undefined-ref`, the cross-file
/// branch of `duplicate-label`, and `undefined-citation`. Their gates (closed,
/// rooted namespace) keep a bare fragment opened alone from being flagged.
fn analyze_tex(
    snapshot: &Analysis,
    path: &Path,
    rules: &RuleSelection,
    enc: PositionEncoding,
) -> Option<Vec<Diagnostic>> {
    let file = snapshot.lookup_file(path)?;
    // The file's normalized identity, which keys the cross-file resolvers.
    let lint_path = snapshot.file_path(file).to_path_buf();
    let idx = LineIndex::with_encoding(snapshot.file_text(file), enc);
    let mut diags: Vec<Diagnostic> = snapshot
        .parse_diagnostics(file)
        .iter()
        .map(|d| Diagnostic {
            range: byte_range_to_lsp(&idx, d.start, d.end),
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("badness".to_owned()),
            message: d.message.clone(),
            ..Default::default()
        })
        .collect();
    let root = snapshot.parsed_tree(file);
    let model = snapshot.semantic_model(file);
    let packages = snapshot.resolve_package_options();
    let (resolution, citations) = snapshot.resolve_project();
    for d in lint_document(
        &lint_path,
        &root,
        model,
        Some(resolution),
        Some(citations),
        Some(packages),
    ) {
        if rules.is_active(d.rule) {
            diags.push(lint_to_lsp(&idx, d, true, &lint_path));
        }
    }
    Some(diags)
}

/// Compute diagnostics for a `.bib` file off the snapshot: bib parse diagnostics
/// plus bib lint-rule findings over the cached bib tree + model. The bib linter
/// has no cross-file resolution argument (no bib rule is cross-file-sensitive
/// yet).
fn analyze_bib(
    snapshot: &Analysis,
    path: &Path,
    rules: &RuleSelection,
    enc: PositionEncoding,
) -> Option<Vec<Diagnostic>> {
    let file = snapshot.lookup_file(path)?;
    let idx = LineIndex::with_encoding(snapshot.file_text(file), enc);
    let mut diags: Vec<Diagnostic> = snapshot
        .bib_parse_diagnostics(file)
        .iter()
        .map(|d| Diagnostic {
            range: byte_range_to_lsp(&idx, d.start, d.end),
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("badness".to_owned()),
            message: d.message.clone(),
            ..Default::default()
        })
        .collect();
    let root = snapshot.parsed_bib_tree(file);
    let model = snapshot.bib_semantic_model(file);
    for d in crate::bib::linter::lint_document(path, &root, model) {
        if rules.is_active(d.rule) {
            diags.push(lint_to_lsp(&idx, d, false, path));
        }
    }
    Some(diags)
}

/// Map a linter [`crate::linter::Diagnostic`] (shared by the LaTeX and BibTeX
/// linters) onto an LSP [`Diagnostic`].
/// Base URL of the published LaTeX linter-rules reference; each rule is a
/// heading whose mdBook anchor is the rule id (`#deprecated-command`), so
/// `{BASE}#{rule}` deep-links the rule's docs.
const LATEX_RULES_DOC_URL: &str = "https://badness.dev/reference/linter-rules.html";

/// Convert a linter finding into an LSP diagnostic. `link_docs` attaches a
/// `code_description` pointing at the rule's entry in the published reference;
/// only the LaTeX rules are catalogued there today, so the bib arms pass `false`
/// (their `code` still carries the rule id, just without a doc link).
fn lint_to_lsp(
    idx: &LineIndex,
    d: crate::linter::Diagnostic,
    link_docs: bool,
    self_path: &Path,
) -> Diagnostic {
    let code_description = link_docs
        .then(|| format!("{LATEX_RULES_DOC_URL}#{}", d.rule).parse().ok())
        .flatten()
        .map(|href| CodeDescription { href });
    let related_information = lint_related_to_lsp(idx, self_path, &d.related);
    Diagnostic {
        range: byte_range_to_lsp(idx, d.start, d.end),
        severity: Some(severity_to_lsp(d.severity)),
        code: Some(NumberOrString::String(d.rule.to_owned())),
        code_description,
        source: Some("badness".to_owned()),
        message: d.message,
        related_information,
        tags: lint_diagnostic_tags(d.rule),
        ..Default::default()
    }
}

/// Turn a finding's [`RelatedInfo`](crate::linter::RelatedInfo) secondary
/// locations into LSP `DiagnosticRelatedInformation`, the clickable "see also"
/// links (e.g. the first definition behind a `duplicate-label`). Returns `None`
/// when there are none, so the field stays absent for the common case.
///
/// A secondary in the *current* file (`self_path`) resolves its range against
/// `idx`; one in another file is **file-level** — a `0..0` byte range
/// maps to the document start regardless of encoding, so we need neither that
/// file's text nor its line index. An entry whose path cannot form a `file://`
/// URI is skipped (mirrors [`location_for`]).
fn lint_related_to_lsp(
    idx: &LineIndex,
    self_path: &Path,
    related: &[crate::linter::RelatedInfo],
) -> Option<Vec<DiagnosticRelatedInformation>> {
    if related.is_empty() {
        return None;
    }
    let items: Vec<DiagnosticRelatedInformation> = related
        .iter()
        .filter_map(|ri| {
            let range = if ri.path == self_path {
                byte_range_to_lsp(idx, ri.start, ri.end)
            } else {
                // File-level link: `0..0` at the document start.
                Range::default()
            };
            Some(DiagnosticRelatedInformation {
                location: Location {
                    uri: path_to_uri(&ri.path)?,
                    range,
                },
                message: ri.message.clone(),
            })
        })
        .collect();
    (!items.is_empty()).then_some(items)
}

/// Map a lint rule id onto the LSP diagnostic tags editors render specially:
/// `Unnecessary` dims/greys the span (dead code), `Deprecated` strikes it
/// through. Keyed on the stable rule id rather than a field on
/// [`crate::linter::Diagnostic`], so it stays a purely presentational LSP concern
/// (the CLI renderer is untouched). Returns `None` for rules with no tag.
fn lint_diagnostic_tags(rule: &str) -> Option<Vec<DiagnosticTag>> {
    match rule {
        // A label defined but never referenced is a dead definition.
        "unreferenced-label" => Some(vec![DiagnosticTag::UNNECESSARY]),
        // Commands/environments superseded by a modern LaTeX equivalent.
        "deprecated-command" | "obsolete-environment" | "primitive-command" => {
            Some(vec![DiagnosticTag::DEPRECATED])
        }
        _ => None,
    }
}

/// Compute a `textDocument/diagnostic` pull report on the read pool and reply.
///
/// Reuses the same per-file diagnostics the push path computes, then derives a
/// content-addressed `result_id` and returns either a `full` report (with items)
/// or, when `previous_result_id` matches, an `unchanged` report. `related_documents`
/// is always `None`: cross-file rules fire in the file that *holds* the reference,
/// so a single file's report is self-contained (the dependency is expressed by
/// `inter_file_dependencies`, not by foreign-file diagnostics).
#[allow(clippy::too_many_arguments)]
fn run_document_diagnostic(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &TextBuffer,
    kind: FileKind,
    previous_result_id: Option<String>,
    rules: &RuleSelection,
    enc: PositionEncoding,
    out_tx: &Sender<Outbound>,
) {
    let items = compute_diagnostics(snapshot, path, text, kind, rules, enc);
    let result_id = result_id_for(&items);
    let report = if previous_result_id.as_deref() == Some(result_id.as_str()) {
        DocumentDiagnosticReport::Unchanged(RelatedUnchangedDocumentDiagnosticReport {
            related_documents: None,
            unchanged_document_diagnostic_report: UnchangedDocumentDiagnosticReport { result_id },
        })
    } else {
        DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
            related_documents: None,
            full_document_diagnostic_report: FullDocumentDiagnosticReport {
                result_id: Some(result_id),
                items,
            },
        })
    };
    let value = serde_json::to_value(DocumentDiagnosticReportResult::Report(report))
        .unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, value)));
}

/// The diagnostics for a pull, computed **on demand**.
///
/// Fast path: reuse the snapshot's salsa-cached parse, model, and cross-file
/// resolution via [`analyze_tex`]/[`analyze_bib`]. The snapshot already reflects the
/// pulled buffer (the preceding `Edit` upserted ahead of this job on the FIFO
/// channel). On a racing write (`salsa::Cancelled`) or a missing file, fall back to a
/// single-file recompute from the captured `text` ([`fallback_diagnostics`]) so the
/// reply stays current — never a stale or empty flash (the bug panache fixed).
fn compute_diagnostics(
    snapshot: &Analysis,
    path: &Path,
    text: &TextBuffer,
    kind: FileKind,
    rules: &RuleSelection,
    enc: PositionEncoding,
) -> Vec<Diagnostic> {
    let cached = salsa::Cancelled::catch(AssertUnwindSafe(|| match kind {
        FileKind::Tex
        | FileKind::CodeTex
        | FileKind::Sty
        | FileKind::Cls
        | FileKind::Dtx
        | FileKind::Ins => analyze_tex(snapshot, path, rules, enc),
        FileKind::Bib => analyze_bib(snapshot, path, rules, enc),
    }));
    match cached {
        Ok(Some(items)) => items,
        // `Ok(None)` = file not in the snapshot; `Err` = cancelled by a racing edit.
        // Either way recompute from the captured buffer (single-file: cross-file
        // findings, if any, arrive on the client's next pull after the edit settles).
        Ok(None) | Err(_) => fallback_diagnostics(path, text, kind, rules, snapshot.declarations()),
    }
}

/// Single-file diagnostics computed directly from `text`, bypassing the salsa cache.
/// The cancellation/cache-miss fallback for a pull — parse diagnostics plus
/// node-shape lint findings, with no cross-file resolution (`None` resolvers).
fn fallback_diagnostics(
    path: &Path,
    text: &TextBuffer,
    kind: FileKind,
    rules: &RuleSelection,
    declared: &ResolvedDeclarations,
) -> Vec<Diagnostic> {
    let idx = text.line_index();
    let mut diags: Vec<Diagnostic> = Vec::new();
    match kind {
        FileKind::Tex
        | FileKind::CodeTex
        | FileKind::Sty
        | FileKind::Cls
        | FileKind::Dtx
        | FileKind::Ins => {
            let parsed = parse_with_declarations(text, kind.lex_config(), declared);
            for err in &parsed.errors {
                diags.push(Diagnostic {
                    range: byte_range_to_lsp(&idx, err.start, err.end),
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("badness".to_owned()),
                    message: err.message.clone(),
                    ..Default::default()
                });
            }
            let root = parsed.syntax();
            let model = SemanticModel::build_with_declarations(&root, declared);
            for d in lint_document(path, &root, &model, None, None, None) {
                if rules.is_active(d.rule) {
                    diags.push(lint_to_lsp(&idx, d, true, path));
                }
            }
        }
        FileKind::Bib => {
            let parsed = bib_parse(text);
            for err in &parsed.errors {
                diags.push(Diagnostic {
                    range: byte_range_to_lsp(&idx, err.start, err.end),
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("badness".to_owned()),
                    message: err.message.clone(),
                    ..Default::default()
                });
            }
            let root = parsed.syntax();
            let model = BibModel::build(&root);
            for d in crate::bib::linter::lint_document(path, &root, &model) {
                if rules.is_active(d.rule) {
                    diags.push(lint_to_lsp(&idx, d, false, path));
                }
            }
        }
    }
    diags
}

/// Compute a `textDocument/codeAction` reply on the read pool: re-lint the buffer,
/// surface each fix-carrying finding overlapping `range` as a quick-fix, and add
/// any conservative syntax-aware refactoring at the cursor.
///
/// Reuses the same on-demand lint the pull-diagnostics path runs (cached off the
/// snapshot, single-file fallback on a racing write), but keeps the **raw** linter
/// findings — with byte ranges and fixes — that the LSP diagnostic conversion drops.
#[allow(clippy::too_many_arguments)]
fn run_code_action(
    snapshot: &Analysis,
    id: RequestId,
    uri: &Uri,
    path: &Path,
    text: &TextBuffer,
    kind: FileKind,
    range: Range,
    only: Option<&[CodeActionKind]>,
    rules: &RuleSelection,
    enc: PositionEncoding,
    out_tx: &Sender<Outbound>,
) {
    let findings = compute_lint_findings(snapshot, path, text, kind, rules);
    // Only the LaTeX rules are catalogued in the published reference, so the
    // echoed diagnostic on a bib quick-fix carries no doc link (mirrors
    // `analyze_bib`/`lint_to_lsp`).
    let link_docs = !matches!(kind, FileKind::Bib);
    // Resolve a cross-file fix's foreign target to its `(uri, text)` from the
    // snapshot, so a quick-fix can carry edits in files other than this buffer.
    let resolve = |p: &Path| -> Option<(Uri, String)> {
        let file = snapshot.lookup_file(p)?;
        let uri = path_to_uri(p)?;
        Some((uri, snapshot.file_text(file).to_string()))
    };
    let mut actions = code_action::code_actions_for_range(
        &findings, text, uri, path, range, enc, link_docs, &resolve,
    );
    if !matches!(kind, FileKind::Bib) {
        let parsed = parse_with_declarations(text, kind.lex_config(), snapshot.declarations());
        let root = SyntaxNode::new_root(parsed.green);
        actions.extend(code_action::table_column_actions(&root, text, uri, range));
    }
    actions.retain(|action| match action {
        CodeActionOrCommand::CodeAction(action) => action
            .kind
            .as_ref()
            .is_none_or(|kind| code_action_kind_requested(kind, only)),
        CodeActionOrCommand::Command(_) => only.is_none(),
    });
    let value = serde_json::to_value(actions).unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, value)));
}

fn code_action_kind_requested(kind: &CodeActionKind, only: Option<&[CodeActionKind]>) -> bool {
    only.is_none_or(|requested| {
        requested.iter().any(|parent| {
            kind.as_str() == parent.as_str()
                || kind
                    .as_str()
                    .strip_prefix(parent.as_str())
                    .is_some_and(|suffix| suffix.starts_with('.'))
        })
    })
}

/// The raw linter findings (byte ranges + fixes) for a pull/code-action, computed
/// **on demand**. The fix-carrying analog of [`compute_diagnostics`]: fast path off
/// the snapshot's salsa cache, single-file recompute on a racing write or cache miss.
fn compute_lint_findings(
    snapshot: &Analysis,
    path: &Path,
    text: &str,
    kind: FileKind,
    rules: &RuleSelection,
) -> Vec<crate::linter::Diagnostic> {
    let cached = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        lint_findings(snapshot, path, kind, rules)
    }));
    match cached {
        Ok(Some(items)) => items,
        Ok(None) | Err(_) => {
            fallback_lint_findings(path, text, kind, rules, snapshot.declarations())
        }
    }
}

/// Run the linter over the snapshot's cached tree + model, returning the raw
/// findings (with their fixes). The lint half of [`analyze_tex`]/[`analyze_bib`]
/// without the LSP conversion, so code actions can read each finding's `fix`.
fn lint_findings(
    snapshot: &Analysis,
    path: &Path,
    kind: FileKind,
    rules: &RuleSelection,
) -> Option<Vec<crate::linter::Diagnostic>> {
    let file = snapshot.lookup_file(path)?;
    let findings = match kind {
        FileKind::Tex
        | FileKind::CodeTex
        | FileKind::Sty
        | FileKind::Cls
        | FileKind::Dtx
        | FileKind::Ins => {
            let lint_path = snapshot.file_path(file).to_path_buf();
            let root = snapshot.parsed_tree(file);
            let model = snapshot.semantic_model(file);
            let packages = snapshot.resolve_package_options();
            let (resolution, citations) = snapshot.resolve_project();
            lint_document(
                &lint_path,
                &root,
                model,
                Some(resolution),
                Some(citations),
                Some(packages),
            )
        }
        FileKind::Bib => {
            let root = snapshot.parsed_bib_tree(file);
            let model = snapshot.bib_semantic_model(file);
            crate::bib::linter::lint_document(path, &root, model)
        }
    };
    Some(retain_active(findings, rules))
}

/// Single-file raw findings computed directly from `text`, bypassing the salsa
/// cache — the cancellation/cache-miss fallback for [`compute_lint_findings`] (no
/// cross-file resolution, mirroring [`fallback_diagnostics`]).
fn fallback_lint_findings(
    path: &Path,
    text: &str,
    kind: FileKind,
    rules: &RuleSelection,
    declared: &ResolvedDeclarations,
) -> Vec<crate::linter::Diagnostic> {
    let findings = match kind {
        FileKind::Tex
        | FileKind::CodeTex
        | FileKind::Sty
        | FileKind::Cls
        | FileKind::Dtx
        | FileKind::Ins => {
            let parsed = parse_with_declarations(text, kind.lex_config(), declared);
            let root = parsed.syntax();
            let model = SemanticModel::build_with_declarations(&root, declared);
            lint_document(path, &root, &model, None, None, None)
        }
        FileKind::Bib => {
            let parsed = bib_parse(text);
            let root = parsed.syntax();
            let model = BibModel::build(&root);
            crate::bib::linter::lint_document(path, &root, &model)
        }
    };
    retain_active(findings, rules)
}

/// Parse and analyze a captured LaTeX buffer under the snapshot's declarations.
/// Read-job fallbacks use this when the cached file is absent or stale, so they
/// cannot silently lose configured ref/cite aliases.
fn fallback_tex_model(snapshot: &Analysis, path: &Path, text: &str) -> (SyntaxNode, SemanticModel) {
    let declared = snapshot.declarations();
    let root = SyntaxNode::new_root(
        parse_with_declarations(text, file_kind_or_tex(path).lex_config(), declared).green,
    );
    let model = SemanticModel::build_with_declarations(&root, declared);
    (root, model)
}

/// Drop findings whose rule the config deselected (parse diagnostics always
/// survive — see [`RuleSelection::is_active`]). The raw-findings analog of the
/// inline `is_active` filter in [`analyze_tex`]/[`analyze_bib`], shared by the
/// code-action paths. Mirrors the CLI's `diagnostics.retain(|d| rules.is_active(..))`.
fn retain_active(
    mut findings: Vec<crate::linter::Diagnostic>,
    rules: &RuleSelection,
) -> Vec<crate::linter::Diagnostic> {
    findings.retain(|d| rules.is_active(d.rule));
    findings
}

/// Derive a stable, content-addressed `result_id` from a diagnostic set, so a
/// re-pull with no change reports `unchanged`. Hashes the JSON encoding because
/// [`Diagnostic`] is not `Hash`; the encoding is order-stable (serde field order +
/// deterministic diagnostic ordering), so identical diagnostics hash identically.
/// Mirrors panache's `result_id_for`.
fn result_id_for(items: &[Diagnostic]) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(items)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish().to_string()
}

/// Format the buffer behind a [`WorkerJob::Format`] on the read pool and reply.
///
/// Fast path: reuse the snapshot's cached tree (no reparse). On a racing write
/// (`salsa::Cancelled`), a stale snapshot (`!text_is_current`), or a cache miss,
/// recompute from the captured `text` via [`format_with_style`] (which itself
/// guards parse errors) so the client always gets a correct response.
#[allow(clippy::too_many_arguments)]
fn run_format(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &TextBuffer,
    style: FormatStyle,
    kind: FileKind,
    sentence: SentenceOptions<'_>,
    out_tx: &Sender<Outbound>,
) {
    let result = match compute_format(snapshot, path, text, style, kind, sentence) {
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
    text: &TextBuffer,
    style: FormatStyle,
    kind: FileKind,
    sentence: SentenceOptions<'_>,
) -> Option<TextEdit> {
    // `Some(Some(s))` = formatted; `Some(None)` = clean refusal (parse/format
    // error); `None` = cache miss / stale snapshot (fall back to the captured text).
    let cached = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let file = snapshot.lookup_file(path)?;
        if !snapshot.text_is_current(file, text) {
            return None;
        }
        match kind {
            FileKind::Tex
            | FileKind::CodeTex
            | FileKind::Sty
            | FileKind::Cls
            | FileKind::Dtx
            | FileKind::Ins => {
                if !snapshot.parse_diagnostics(file).is_empty() {
                    return Some(None);
                }
                // The cached tree was already parsed with the file's flavor (the
                // salsa `parsed_document` query flavors by path), so this needs no
                // flavor. The merged signature scope folds in the file's loaded
                // local packages (those tracked as project members).
                let root = snapshot.parsed_tree(file);
                let sigs = snapshot.scope_signatures(file);
                Some(format_node_with_signatures_sentence(&root, style, sigs, sentence).ok())
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
            FileKind::Tex
            | FileKind::CodeTex
            | FileKind::Sty
            | FileKind::Cls
            | FileKind::Dtx
            | FileKind::Ins => format_with_declarations_sentence(
                text,
                style,
                kind.lex_config(),
                sentence,
                snapshot.declarations(),
            )
            .ok(),
            FileKind::Bib => bib_format_with_style(text, style).ok(),
        },
    }?;

    if formatted == text.text() {
        return None;
    }
    let idx = text.line_index();
    let (end_line, end_col) = idx.position(text.len());
    Some(TextEdit {
        range: Range {
            start: Position::new(0, 0),
            end: Position::new(end_line, end_col),
        },
        new_text: formatted,
    })
}

/// Range-format the buffer behind a [`WorkerJob::RangeFormat`] on the read pool and
/// reply with the (possibly empty) edit array, or `null` on refusal / unknown
/// buffer. Mirrors [`run_format`]'s cancellation/fallback contract.
#[allow(clippy::too_many_arguments)]
fn run_range_format(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &TextBuffer,
    style: FormatStyle,
    kind: FileKind,
    range: Range,
    sentence: SentenceOptions<'_>,
    out_tx: &Sender<Outbound>,
) {
    let result = match compute_range_format(snapshot, path, text, style, kind, range, sentence) {
        Some(edits) => serde_json::to_value(edits).unwrap_or(serde_json::Value::Null),
        None => serde_json::Value::Null,
    };
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Produce the minimal edits that range-format `sel_range`, `Some(vec![])` for a
/// no-op, or `None` for a refusal (parse errors), an unsupported kind, or a
/// selection touching no block. LaTeX-only for now; BibTeX returns `None`.
///
/// The whole document is formatted with an emission filter so only the in-range
/// document-level blocks are laid out (see [`format_node_range_with_signatures`]); the
/// formatted fragment is then diffed against the original block slice so the edits
/// are minimal. Shares [`compute_format`]'s salsa fast-path / reparse-fallback
/// shape.
#[allow(clippy::too_many_arguments)]
fn compute_range_format(
    snapshot: &Analysis,
    path: &Path,
    text: &TextBuffer,
    style: FormatStyle,
    kind: FileKind,
    sel_range: Range,
    sentence: SentenceOptions<'_>,
) -> Option<Vec<TextEdit>> {
    // Range formatting is LaTeX-only for now; bib falls back to no edits.
    match kind {
        FileKind::Tex
        | FileKind::CodeTex
        | FileKind::Sty
        | FileKind::Cls
        | FileKind::Dtx
        | FileKind::Ins => {}
        FileKind::Bib => return None,
    }

    let idx = text.line_index();
    let start = idx.offset_at(sel_range.start.line, sel_range.start.character);
    let end = idx.offset_at(sel_range.end.line, sel_range.end.character);
    let (lo, hi) = (start.min(end), start.max(end));
    let sel = TextRange::new(
        TextSize::new(lo.min(u32::MAX as usize) as u32),
        TextSize::new(hi.min(u32::MAX as usize) as u32),
    );

    // `Some(Some(e))` = computed edits (possibly empty); `Some(None)` = clean
    // refusal (parse errors); `None` = cache miss / stale snapshot / cancellation
    // → reparse from the captured text.
    let cached = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let file = snapshot.lookup_file(path)?;
        if !snapshot.text_is_current(file, text) {
            return None;
        }
        if !snapshot.parse_diagnostics(file).is_empty() {
            return Some(None);
        }
        let root = snapshot.parsed_tree(file);
        let sigs = snapshot.scope_signatures(file);
        Some(Some(range_edits_for_root(
            &root, text, &idx, sel, style, sigs, sentence,
        )))
    }));

    match cached {
        Ok(Some(Some(edits))) => edits,
        Ok(Some(None)) => None,
        Ok(None) | Err(_) => {
            let declared = snapshot.declarations();
            let parsed = parse_with_declarations(text, kind.lex_config(), declared);
            if !parsed.errors.is_empty() {
                return None;
            }
            range_edits_for_root(
                &parsed.syntax(),
                text,
                &idx,
                sel,
                style,
                &declared_scope(declared),
                sentence,
            )
        }
    }
}

/// Expand `sel` to top-level-block boundaries, format those blocks, and diff the
/// result against the original slice into minimal edits. `None` when the selection
/// touches no block or the formatter refuses; `Some(vec![])` when already
/// formatted.
#[allow(clippy::too_many_arguments)]
fn range_edits_for_root(
    root: &SyntaxNode,
    text: &str,
    idx: &LineIndex,
    sel: TextRange,
    style: FormatStyle,
    external: &SignatureDb,
    sentence: SentenceOptions<'_>,
) -> Option<Vec<TextEdit>> {
    let block_range = expand_to_document_blocks(root, sel)?;
    let fragment =
        format_node_range_with_signatures_sentence(root, style, external, block_range, sentence)
            .ok()?;
    let base = usize::from(block_range.start());
    let end = usize::from(block_range.end());
    if fragment == text[base..end] {
        return Some(Vec::new());
    }
    Some(diff_to_edits(idx, text, block_range, &fragment))
}

/// On-type-format the buffer behind a [`WorkerJob::OnTypeFormat`] on the read pool
/// and reply with the (possibly empty) edit array, or `null` on refusal / unknown
/// buffer. Mirrors [`run_range_format`]'s cancellation/fallback contract.
#[allow(clippy::too_many_arguments)]
fn run_on_type_format(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &TextBuffer,
    style: FormatStyle,
    kind: FileKind,
    position: Position,
    sentence: SentenceOptions<'_>,
    out_tx: &Sender<Outbound>,
) {
    let result = match compute_on_type_format(snapshot, path, text, style, kind, position, sentence)
    {
        Some(edits) => serde_json::to_value(edits).unwrap_or(serde_json::Value::Null),
        None => serde_json::Value::Null,
    };
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Produce the minimal edits that re-indent the block around a just-typed `}`, an
/// empty vec when nothing should change (the `}` doesn't close a multi-line
/// construct, or the block is already formatted), or `None` on refusal (parse
/// errors), an unsupported kind, or a cursor touching no block. LaTeX-only.
///
/// This is [`compute_range_format`] with an **empty selection** at the cursor and
/// a [`closes_multiline_construct`] guard: the guard decides *whether* to fire,
/// and the shared range machinery ([`range_edits_for_root`]) computes the edit
/// against the containing top-level block. Shares the salsa fast-path / reparse-
/// fallback shape.
#[allow(clippy::too_many_arguments)]
fn compute_on_type_format(
    snapshot: &Analysis,
    path: &Path,
    text: &TextBuffer,
    style: FormatStyle,
    kind: FileKind,
    position: Position,
    sentence: SentenceOptions<'_>,
) -> Option<Vec<TextEdit>> {
    match kind {
        FileKind::Tex
        | FileKind::CodeTex
        | FileKind::Sty
        | FileKind::Cls
        | FileKind::Dtx
        | FileKind::Ins => {}
        FileKind::Bib => return None,
    }

    let idx = text.line_index();
    let offset = idx.offset_at(position.line, position.character);
    let off = offset.min(u32::MAX as usize) as u32;
    let sel = TextRange::empty(TextSize::new(off));

    // Nesting mirrors `compute_range_format`: `Some(Some(e))` = computed edits;
    // `Some(None)` = clean refusal (parse errors); `None` = cache miss / stale
    // snapshot / cancellation → reparse from the captured text.
    let cached = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let file = snapshot.lookup_file(path)?;
        if !snapshot.text_is_current(file, text) {
            return None;
        }
        if !snapshot.parse_diagnostics(file).is_empty() {
            return Some(None);
        }
        let root = snapshot.parsed_tree(file);
        if !closes_multiline_construct(&root, text, off) {
            return Some(Some(Some(Vec::new())));
        }
        let sigs = snapshot.scope_signatures(file);
        Some(Some(range_edits_for_root(
            &root, text, &idx, sel, style, sigs, sentence,
        )))
    }));

    match cached {
        Ok(Some(Some(edits))) => edits,
        Ok(Some(None)) => None,
        Ok(None) | Err(_) => {
            let declared = snapshot.declarations();
            let parsed = parse_with_declarations(text, kind.lex_config(), declared);
            if !parsed.errors.is_empty() {
                return None;
            }
            let root = parsed.syntax();
            if !closes_multiline_construct(&root, text, off) {
                return Some(Vec::new());
            }
            range_edits_for_root(
                &root,
                text,
                &idx,
                sel,
                style,
                &declared_scope(declared),
                sentence,
            )
        }
    }
}

/// Decide whether a `}` typed at byte `offset` (cursor just past the brace)
/// structurally closes a *multi-line* construct that warrants a re-indent: a plain
/// multi-line group, or an `\end{…}` terminating a multi-line environment. A `}`
/// that closes an inline group (e.g. `\textbf{x}`) or that *opens* an environment
/// (`\begin{…}`) returns `false`.
fn closes_multiline_construct(root: &SyntaxNode, text: &str, offset: u32) -> bool {
    let Some(brace) = root.token_at_offset(TextSize::new(offset)).left_biased() else {
        return false;
    };
    if brace.kind() != SyntaxKind::R_BRACE {
        return false;
    }
    // The node this brace closes: a `GROUP`, or the `NAME_GROUP` of a
    // `\begin`/`\end`.
    let Some(close_node) = brace.parent() else {
        return false;
    };
    // The structural unit to (potentially) re-indent.
    let unit = match close_node.parent() {
        // Closing `\end{…}` → re-indent the whole environment.
        Some(p) if p.kind() == SyntaxKind::END => p
            .parent()
            .filter(|e| e.kind() == SyntaxKind::ENVIRONMENT)
            .unwrap_or(p),
        // This `}` *opens* an environment; nothing is closed yet.
        Some(p) if p.kind() == SyntaxKind::BEGIN => return false,
        // A plain group (command body, brace group, …).
        _ => close_node,
    };
    let range = unit.text_range();
    let (lo, hi) = (usize::from(range.start()), usize::from(range.end()));
    text.get(lo..hi).is_some_and(|s| s.contains('\n'))
}

/// Build the document-symbol outline for a [`WorkerJob::Symbols`] on the read pool
/// and reply with a nested [`DocumentSymbolResponse`].
///
/// Fast path: reuse the snapshot's cached tree. On a racing write
/// (`salsa::Cancelled`), a stale snapshot (`!text_is_current`), or a cache miss,
/// reparse the captured `text` directly. Best-effort — unlike formatting, a parse
/// error does *not* suppress the outline (the tree is error-tolerant).
#[allow(clippy::too_many_arguments)]
fn run_symbols(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &TextBuffer,
    kind: FileKind,
    build: &BuildConfig,
    out_tx: &Sender<Outbound>,
) {
    let symbols = match kind {
        FileKind::Tex
        | FileKind::CodeTex
        | FileKind::Sty
        | FileKind::Cls
        | FileKind::Dtx
        | FileKind::Ins => compute_symbols(snapshot, path, text, kind, build),
        FileKind::Bib => compute_bib_symbols(snapshot, path, text),
    };
    let result = serde_json::to_value(DocumentSymbolResponse::Nested(symbols))
        .unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Compute the LaTeX outline for `text`, preferring the snapshot's cached tree and
/// falling back to a direct reparse when it is unavailable or stale. When the
/// project has been compiled, the outline is enriched with the `.aux`'s resolved
/// numbers (see [`to_document_symbol`]).
fn compute_symbols(
    snapshot: &Analysis,
    path: &Path,
    text: &TextBuffer,
    kind: FileKind,
    build: &BuildConfig,
) -> Vec<DocumentSymbol> {
    let idx = text.line_index();
    let cached = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let file = snapshot.lookup_file(path)?;
        if !snapshot.text_is_current(file, text) {
            return None;
        }
        Some(outline(&snapshot.parsed_tree(file)))
    }));
    let items = match cached {
        Ok(Some(items)) => items,
        // Cache miss, stale snapshot, or a cancelled read: reparse the buffer. Flavor
        // by file kind so a `.dtx`'s docstrip vocabulary (and thus its documented
        // macros) still surfaces off the fallback path.
        Ok(None) | Err(_) => outline(&SyntaxNode::new_root(
            parse_with_declarations(text, kind.lex_config(), snapshot.declarations()).green,
        )),
    };
    let aux = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let (resolution, _) = snapshot.resolve_project();
        document_aux(snapshot, resolution, path, build)
    }))
    // A cancelled read degrades to the numberless outline this round.
    .unwrap_or_default();
    let mut toc_cursor = 0;
    items
        .iter()
        .map(|item| to_document_symbol(item, &idx, aux.as_ref(), &mut toc_cursor))
        .collect()
}

/// Compute the BibTeX outline (a flat entry list) for `text`, preferring the
/// snapshot's cached bib model and falling back to a direct reparse when it is
/// unavailable or stale.
fn compute_bib_symbols(snapshot: &Analysis, path: &Path, text: &TextBuffer) -> Vec<DocumentSymbol> {
    let idx = text.line_index();
    let cached = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let file = snapshot.lookup_file(path)?;
        if !snapshot.text_is_current(file, text) {
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
        .map(|item| bib_to_document_symbol(item, &idx))
        .collect()
}

/// Aggregate every tracked LaTeX file's outline into a flat `workspace/symbol`
/// reply, keeping only entries whose name contains `query` (case-insensitive; an
/// empty query matches everything). `.bib` members are skipped — their cite keys
/// are reachable via go-to-def/references. A cancelled per-file read omits that
/// file rather than failing the whole response.
fn run_workspace_symbols(
    snapshot: &Analysis,
    id: RequestId,
    query: &str,
    enc: PositionEncoding,
    out_tx: &Sender<Outbound>,
) {
    let needle = query.to_ascii_lowercase();
    let mut symbols = Vec::new();
    for member in snapshot.project_members() {
        if member.kind == FileKind::Bib {
            continue;
        }
        let collected = salsa::Cancelled::catch(AssertUnwindSafe(|| {
            let text = snapshot.file_text(member.file);
            let idx = LineIndex::with_encoding(text, enc);
            let items = outline(&snapshot.parsed_tree(member.file));
            // Group the picker by file via `container_name`.
            let container = member.path.file_stem().and_then(|s| s.to_str());
            let mut file_symbols = Vec::new();
            collect_workspace_symbols(
                &items,
                &member.path,
                &idx,
                &needle,
                container,
                &mut file_symbols,
            );
            file_symbols
        }));
        if let Ok(mut file_symbols) = collected {
            symbols.append(&mut file_symbols);
        }
    }
    let result = serde_json::to_value(WorkspaceSymbolResponse::Nested(symbols))
        .unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Recursively flatten an [`OutlineItem`] tree into [`WorkspaceSymbol`]s, keeping
/// entries whose name contains `needle`. Children are always visited so a matching
/// label nested under a non-matching section still surfaces.
fn collect_workspace_symbols(
    items: &[OutlineItem],
    path: &Path,
    idx: &LineIndex,
    needle: &str,
    container: Option<&str>,
    out: &mut Vec<WorkspaceSymbol>,
) {
    for item in items {
        let matches = needle.is_empty() || item.name.to_ascii_lowercase().contains(needle);
        if matches && let Some(location) = location_for(path, idx, item.selection_range) {
            out.push(WorkspaceSymbol {
                name: item.name.clone(),
                kind: outline_symbol_kind(item.kind),
                tags: None,
                container_name: container.map(str::to_owned),
                location: OneOf::Left(location),
                data: None,
            });
        }
        // Always recurse: a matching label can nest under a non-matching section.
        collect_workspace_symbols(&item.children, path, idx, needle, container, out);
    }
}

/// Compute folding ranges for a [`WorkerJob::FoldingRange`] on the read pool and
/// reply with a `Vec<FoldingRange>`. Same snapshot fast-path / reparse fallback as
/// [`run_symbols`].
fn run_folding(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &TextBuffer,
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
    text: &TextBuffer,
    kind: FileKind,
) -> Vec<FoldingRange> {
    if kind == FileKind::Bib {
        return Vec::new();
    }
    let idx = text.line_index();
    let cached = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let file = snapshot.lookup_file(path)?;
        if !snapshot.text_is_current(file, text) {
            return None;
        }
        Some(folding::folding_ranges(&snapshot.parsed_tree(file), &idx))
    }));
    match cached {
        Ok(Some(ranges)) => ranges,
        // Cache miss, stale snapshot, or a cancelled read: reparse the buffer.
        Ok(None) | Err(_) => {
            folding::folding_ranges(&SyntaxNode::new_root(parse(text).green), &idx)
        }
    }
}

/// Compute selection ranges for a [`WorkerJob::SelectionRange`] on the read pool and
/// reply with a `Vec<SelectionRange>` (one chain per input position). Mirrors
/// [`run_folding`].
#[allow(clippy::too_many_arguments)]
fn run_selection_range(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &TextBuffer,
    kind: FileKind,
    positions: &[Position],
    out_tx: &Sender<Outbound>,
) {
    let ranges = compute_selection_range(snapshot, path, text, kind, positions);
    let result = serde_json::to_value(ranges).unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Compute the LaTeX expand-selection chains for each cursor in `positions`, preferring
/// the snapshot's cached tree and falling back to a direct reparse when it is
/// unavailable or stale. `.bib` files have no LaTeX structure (the LaTeX parser does not
/// apply), so they yield an empty range per position. Mirrors [`compute_folding`].
fn compute_selection_range(
    snapshot: &Analysis,
    path: &Path,
    text: &TextBuffer,
    kind: FileKind,
    positions: &[Position],
) -> Vec<SelectionRange> {
    if kind == FileKind::Bib {
        return positions
            .iter()
            .map(|&pos| SelectionRange {
                range: Range::new(pos, pos),
                parent: None,
            })
            .collect();
    }
    let idx = text.line_index();
    let cached = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let file = snapshot.lookup_file(path)?;
        if !snapshot.text_is_current(file, text) {
            return None;
        }
        Some(selection_range::selection_ranges(
            &snapshot.parsed_tree(file),
            &idx,
            positions,
        ))
    }));
    match cached {
        Ok(Some(ranges)) => ranges,
        // Cache miss, stale snapshot, or a cancelled read: reparse the buffer.
        Ok(None) | Err(_) => selection_range::selection_ranges(
            &SyntaxNode::new_root(parse(text).green),
            &idx,
            positions,
        ),
    }
}

/// Compute document links for a [`WorkerJob::DocumentLink`] on the read pool and
/// reply to `id`. Mirrors [`run_folding`].
#[allow(clippy::too_many_arguments)]
fn run_document_link(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &TextBuffer,
    kind: FileKind,
    texmf: &TexmfConfig,
    out_tx: &Sender<Outbound>,
) {
    let links = compute_document_link(snapshot, path, text, kind, texmf);
    let result = serde_json::to_value(links).unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Compute the clickable links in `text`, preferring the snapshot's cached tree and
/// falling back to a direct reparse when it is unavailable or stale. `.bib` files
/// have no include structure (the LaTeX parser does not apply), so they instead get
/// external `doi`/`url` field links via [`crate::bib::document_link`]. Each resolved,
/// on-disk target is mapped to an LSP [`DocumentLink`] via the shared [`lsp_range`] +
/// [`path_to_uri`]; a target whose path cannot form a URI is dropped.
fn compute_document_link(
    snapshot: &Analysis,
    path: &Path,
    text: &TextBuffer,
    kind: FileKind,
    texmf: &TexmfConfig,
) -> Vec<DocumentLink> {
    if kind == FileKind::Bib {
        return compute_bib_document_link(snapshot, path, text);
    }
    let idx = text.line_index();
    let base_dir = path.parent();
    // The installed-tree index for the system-package fallback. Built lazily on first
    // use (this read-pool thread), empty when scanning is disabled — either way the
    // local `base_dir` resolution runs first and unchanged.
    let index = crate::project::texmf::global_index(texmf);
    let targets = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let cached = snapshot
            .lookup_file(path)
            .filter(|&file| snapshot.text_is_current(file, text));
        match cached {
            Some(file) => {
                document_link::document_links(&snapshot.parsed_tree(file), base_dir, index)
            }
            // Cache miss or stale snapshot: reparse the buffer.
            None => document_link::document_links(
                &SyntaxNode::new_root(parse(text).green),
                base_dir,
                index,
            ),
        }
    }))
    // A cancelled read yields no links this round; the client re-requests.
    .unwrap_or_default();

    targets
        .into_iter()
        .filter_map(|target| {
            Some(DocumentLink {
                range: lsp_range(&idx, target.range),
                target: Some(path_to_uri(&target.target)?),
                tooltip: None,
                data: None,
            })
        })
        .collect()
}

/// Document links for a `.bib` file: the `doi`/`url` field values turned into
/// clickable external links (see [`crate::bib::document_link`]). Prefers the
/// snapshot's cached bib tree, reparsing on a cache miss or stale buffer; a target
/// string that does not parse as a URI is dropped.
fn compute_bib_document_link(
    snapshot: &Analysis,
    path: &Path,
    text: &TextBuffer,
) -> Vec<DocumentLink> {
    let idx = text.line_index();
    let links = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let cached = snapshot
            .lookup_file(path)
            .filter(|&file| snapshot.text_is_current(file, text));
        match cached {
            Some(file) => {
                crate::bib::document_link::document_links(&snapshot.parsed_bib_tree(file))
            }
            None => crate::bib::document_link::document_links(&crate::bib::parse(text).syntax()),
        }
    }))
    .unwrap_or_default();

    links
        .into_iter()
        .filter_map(|link| {
            Some(DocumentLink {
                range: lsp_range(&idx, link.range),
                target: Some(link.target.parse::<Uri>().ok()?),
                tooltip: None,
                data: None,
            })
        })
        .collect()
}

/// `path`'s label namespace — its include-graph component — falling back to just
/// `path` for a standalone or untracked file, so a caller always has something to
/// scan.
fn namespace_of<'a>(resolution: &'a ResolvedLabels, path: &'a Path) -> Vec<&'a Path> {
    let members = resolution.namespace_members(path);
    if members.is_empty() {
        vec![path]
    } else {
        members
    }
}

/// The document root of `namespace`: its first member carrying a
/// `\documentclass` or `\begin{document}` ([`project::labels::is_document_root`],
/// via the salsa `file_is_document_root` firewall). `None` when no member is a
/// root — an uncompilable fragment, or a project whose root the server has not
/// loaded (the db seeds one directory at a time, so a child in `chapters/` may
/// never have seen `../main.tex`; that is what `[build] root` is for).
///
/// The single place the "which file did the compiler run on?" question is
/// answered, shared by `.aux` resolution ([`document_aux`]) and PDF resolution
/// ([`forward_search::pdf_path`]).
///
/// [`project::labels::is_document_root`]: crate::project::labels::is_document_root
fn root_document_of<'a>(snapshot: &Analysis, namespace: &[&'a Path]) -> Option<&'a Path> {
    namespace.iter().copied().find(|p| {
        snapshot
            .lookup_file(p)
            .is_some_and(|f| snapshot.file_is_document_root(f))
    })
}

/// The merged `.aux` facts for `path`'s label namespace: the aux root is the
/// namespace's document root's directory (where the compiler ran), falling back
/// to `path`'s own; an unknown/untracked file still checks its sibling `.aux`.
/// `None` when the project was never compiled. Shared by label hover (numbers in
/// the preview) and `documentSymbol` (numbers in the outline).
fn document_aux(
    snapshot: &Analysis,
    resolution: &ResolvedLabels,
    path: &Path,
    build: &BuildConfig,
) -> Option<AuxData> {
    let namespace = namespace_of(resolution, path);
    let root_dir = root_document_of(snapshot, &namespace)
        .and_then(Path::parent)
        .or_else(|| path.parent())
        .unwrap_or(Path::new(""));
    crate::project::aux::aux_data_for(&namespace, root_dir, build.aux_dir.as_deref())
}

/// Convert an [`OutlineItem`] tree into an LSP [`DocumentSymbol`], mapping byte
/// ranges through the (encoding-aware) [`LineIndex`].
/// Map an [`OutlineSymbol`] to its LSP [`SymbolKind`]. Shared by the per-file
/// `documentSymbol` ([`to_document_symbol`]) and project-wide `workspace/symbol`
/// ([`run_workspace_symbols`]) outputs so the two never drift.
fn outline_symbol_kind(kind: OutlineSymbol) -> SymbolKind {
    match kind {
        OutlineSymbol::Section => SymbolKind::MODULE,
        OutlineSymbol::Frame => SymbolKind::CLASS,
        OutlineSymbol::Float => SymbolKind::OBJECT,
        OutlineSymbol::Theorem => SymbolKind::CLASS,
        OutlineSymbol::Label => SymbolKind::CONSTANT,
        OutlineSymbol::Macro => SymbolKind::FUNCTION,
        OutlineSymbol::Environment => SymbolKind::INTERFACE,
    }
}

/// Enrichment from the compile's `.aux` (when one exists): a section name gets its
/// toc number prefixed (`"1.2 Intro"`), a label its `\newlabel` number as
/// `detail`, and a float/theorem its child label's number as `detail`. Section
/// matching consumes toc entries in document order (`toc_cursor`) against
/// whitespace-normalized titles, so a macro-heavy title that fails to match
/// degrades to its plain, numberless name.
#[allow(deprecated)] // `DocumentSymbol::deprecated` is a required struct field.
fn to_document_symbol(
    item: &OutlineItem,
    idx: &LineIndex,
    aux: Option<&AuxData>,
    toc_cursor: &mut usize,
) -> DocumentSymbol {
    let kind = outline_symbol_kind(item.kind);
    let range = item.range;
    let selection = item.selection_range;
    let mut name = item.name.clone();
    let mut detail = None;
    if let Some(aux) = aux {
        match item.kind {
            OutlineSymbol::Section => {
                if let Some(number) = next_toc_number(aux, &item.name, toc_cursor) {
                    name = format!("{number} {name}");
                }
            }
            OutlineSymbol::Label => {
                detail = aux.labels.get(item.name.as_str()).cloned();
            }
            OutlineSymbol::Float | OutlineSymbol::Theorem => {
                // The float's own number is exactly its child label's.
                detail = item
                    .children
                    .iter()
                    .find(|c| c.kind == OutlineSymbol::Label)
                    .and_then(|label| aux.labels.get(label.name.as_str()).cloned());
            }
            OutlineSymbol::Frame | OutlineSymbol::Macro | OutlineSymbol::Environment => {}
        }
    }
    let children: Vec<DocumentSymbol> = item
        .children
        .iter()
        .map(|child| to_document_symbol(child, idx, aux, toc_cursor))
        .collect();
    DocumentSymbol {
        name,
        detail,
        kind,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(idx, range.start().into(), range.end().into()),
        selection_range: byte_range_to_lsp(idx, selection.start().into(), selection.end().into()),
        children: (!children.is_empty()).then_some(children),
    }
}

/// The number of the next toc entry matching `title`, consuming entries up to and
/// including the match. Titles compare with all whitespace stripped: TeX pads the
/// written form (`\textsc  {Intro}`) where the CST title has none.
fn next_toc_number(aux: &AuxData, title: &str, cursor: &mut usize) -> Option<String> {
    let want = normalize_toc_title(title);
    if want.is_empty() {
        return None;
    }
    let (offset, entry) = aux.toc[(*cursor).min(aux.toc.len())..]
        .iter()
        .enumerate()
        .find(|(_, e)| e.number.is_some() && normalize_toc_title(&e.title) == want)?;
    *cursor += offset + 1;
    entry.number.clone()
}

/// Strip all whitespace for toc-title comparison.
fn normalize_toc_title(title: &str) -> String {
    title.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Convert a flat [`BibOutlineItem`] into an LSP [`DocumentSymbol`]. Bib entries
/// have no nesting, so there are never children; the cite key is the name and the
/// entry type the detail.
#[allow(deprecated)] // `DocumentSymbol::deprecated` is a required struct field.
fn bib_to_document_symbol(item: &BibOutlineItem, idx: &LineIndex) -> DocumentSymbol {
    let range = item.range;
    let selection = item.selection_range;
    DocumentSymbol {
        name: item.name.clone(),
        detail: Some(item.detail.clone()),
        kind: SymbolKind::CONSTANT,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(idx, range.start().into(), range.end().into()),
        selection_range: byte_range_to_lsp(idx, selection.start().into(), selection.end().into()),
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
#[allow(clippy::too_many_arguments)]
fn run_completion(
    snapshot: &Analysis,
    id: RequestId,
    uri: &Uri,
    text: &TextBuffer,
    position: Position,
    texmf: &TexmfConfig,
    out_tx: &Sender<Outbound>,
) {
    // The salsa-key path is derived from the URI (the same mapping `on_completion` uses).
    let path = uri_to_path(uri);
    // The `[texmf]` config is threaded down; the installed-tree index is resolved
    // *only* when the cursor is in a package/class argument (see
    // `build_completion_items`), so a command/label completion never pays the walk.
    let items = compute_completion(snapshot, uri, &path, text, position, texmf);
    // `is_incomplete`: command/label/key universes are prefix-filtered server-side, so
    // the client re-queries as the typed prefix narrows.
    let result = serde_json::to_value(CompletionResponse::List(CompletionList {
        is_incomplete: true,
        items,
    }))
    .unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Resolve a highlighted [`CompletionItem`] on the read pool, attaching lazy
/// signature/citation detail (see [`completion_resolve`]) and replying to `id`. A
/// racing write (snapshot cancellation) replies with the item unchanged — the
/// client keeps the un-enriched item, never an error.
fn run_completion_resolve(
    snapshot: &Analysis,
    id: RequestId,
    item: CompletionItem,
    out_tx: &Sender<Outbound>,
) {
    let resolved = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        completion_resolve::resolve(snapshot, item.clone())
    }))
    .unwrap_or(item);
    let result = serde_json::to_value(resolved).unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Compute completion items at `position`. A `.bib` cursor goes through the bib
/// classifier; a `.tex` cursor through the LaTeX one, preferring the snapshot's cached
/// tree/queries and falling back to a direct reparse when unavailable or stale.
#[allow(clippy::too_many_arguments)]
fn compute_completion(
    snapshot: &Analysis,
    uri: &Uri,
    path: &Path,
    text: &TextBuffer,
    position: Position,
    texmf: &TexmfConfig,
) -> Vec<CompletionItem> {
    let idx = text.line_index();
    let offset = idx.offset_at(position.line, position.character);

    if file_kind_for(path) == FileKind::Bib {
        return compute_bib_completion(text, offset);
    }
    compute_tex_completion(snapshot, uri, path, text, offset, texmf)
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
/// cite/glossary-key context whose candidates need cross-file facts (resolved
/// against the snapshot, like a file-path read).
enum TexCompletion {
    Items(Vec<CompletionItem>),
    Cite { lint_path: PathBuf },
    Glossary { prefix: String, lint_path: PathBuf },
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
    texmf: &TexmfConfig,
) -> Vec<CompletionItem> {
    // Classify off the cached tree when current; reparse on stale/miss. A cancelled
    // read also falls back to a reparse (`unwrap_or_else`).
    let resolved = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        if let Some(file) = snapshot.lookup_file(path)
            && snapshot.text_is_current(file, text)
        {
            let root = snapshot.parsed_tree(file);
            let ctx = crate::completion::classify_context_with_declarations(
                &root,
                offset,
                snapshot.declarations(),
            );
            return match ctx {
                // Citations are not prefix-filtered server-side: the full namespace is
                // returned and the client filters by `filterText` (key + title +
                // authors), so a title word surfaces its entry.
                CompletionContext::CitationKey { .. } => TexCompletion::Cite {
                    lint_path: snapshot.file_path(file).to_path_buf(),
                },
                CompletionContext::GlossaryKey { prefix } => TexCompletion::Glossary {
                    prefix,
                    lint_path: snapshot.file_path(file).to_path_buf(),
                },
                _ => TexCompletion::Items(build_completion_items(
                    &ctx,
                    // The merged scope folds in loaded local packages' macros.
                    snapshot.scope_signatures(file),
                    snapshot.semantic_model(file),
                    snapshot.declarations(),
                    uri,
                    texmf,
                )),
            };
        }
        reparse_tex_completion(text, offset, uri, path, texmf, snapshot.declarations())
    }))
    .unwrap_or_else(|_| {
        reparse_tex_completion(text, offset, uri, path, texmf, snapshot.declarations())
    });

    match resolved {
        TexCompletion::Items(items) => items,
        TexCompletion::Cite { lint_path } => {
            // Cross-file resolve against the db snapshot; a racing write yields none.
            salsa::Cancelled::catch(AssertUnwindSafe(|| {
                let (_, citations) = snapshot.resolve_project();
                cite_completion_items(snapshot, citations, &lint_path)
            }))
            .unwrap_or_default()
        }
        TexCompletion::Glossary { prefix, lint_path } => {
            salsa::Cancelled::catch(AssertUnwindSafe(|| {
                // The glossary key namespace is the same include-graph component
                // the label resolver computes; only membership is consumed here.
                let (labels, _) = snapshot.resolve_project();
                glossary_completion_items(snapshot, labels, &lint_path, &prefix)
            }))
            .unwrap_or_default()
        }
    }
}

/// Classify a `.tex` cursor off a fresh parse (the snapshot-free fallback). For a
/// `\cite` context this still defers resolution to the snapshot, keying off `path`.
fn reparse_tex_completion(
    text: &str,
    offset: usize,
    uri: &Uri,
    path: &Path,
    texmf: &TexmfConfig,
    declared: &ResolvedDeclarations,
) -> TexCompletion {
    let root = SyntaxNode::new_root(
        parse_with_declarations(text, file_kind_or_tex(path).lex_config(), declared).green,
    );
    let ctx = crate::completion::classify_context_with_declarations(&root, offset, declared);
    match ctx {
        CompletionContext::CitationKey { .. } => TexCompletion::Cite {
            lint_path: path.to_path_buf(),
        },
        CompletionContext::GlossaryKey { prefix } => TexCompletion::Glossary {
            prefix,
            lint_path: path.to_path_buf(),
        },
        _ => {
            let sigs = crate::semantic::scan_definitions(&root);
            let model = SemanticModel::build_with_declarations(&root, declared);
            TexCompletion::Items(build_completion_items(
                &ctx, &sigs, &model, declared, uri, texmf,
            ))
        }
    }
}

/// Cite-key candidates: every entry in the citing file's bibliography namespace,
/// deduped by folded key (first definer wins). The list is *not* prefix-filtered —
/// each item carries a `filterText` of key + title + authors, so the client filters
/// on any of those fields (LaTeX Workshop's `citation.filterText`). Mirrors
/// [`resolve_citation_locations`] but collects all entries rather than matching a target.
fn cite_completion_items(
    snapshot: &Analysis,
    citations: &ResolvedCitations,
    lint_path: &Path,
) -> Vec<CompletionItem> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut items: Vec<CompletionItem> = Vec::new();
    for bib_path in citations.bib_definers(lint_path) {
        let Some(file) = snapshot.lookup_file(bib_path) else {
            continue;
        };
        for entry in snapshot.bib_semantic_model(file).entries() {
            // Dedup case-insensitively (BibTeX folds key case); the first definer wins.
            if !seen.insert(entry.key.to_lowercase()) {
                continue;
            }
            items.push(CompletionItem {
                // Carry the citing file + key so `completionItem/resolve` can re-walk
                // the bibliography namespace and attach the entry card lazily.
                data: completion_resolve::CompletionResolveData::Citation {
                    lint_path: lint_path.to_path_buf(),
                    key: entry.key.to_string(),
                }
                .into_value(),
                label: entry.key.to_string(),
                filter_text: Some(citation_filter_text(entry)),
                // A deterministic tiebreak within the client's match-score bucket,
                // preserving the old alphabetical-by-key order.
                sort_text: Some(entry.key.to_lowercase()),
                kind: Some(CompletionItemKind::REFERENCE),
                ..Default::default()
            });
        }
    }
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

/// The `filterText` for a citation item: key, then title, then authors, space-joined
/// and truncated to 128 chars on a char boundary. The key comes first so it always
/// survives the cap (VS Code truncates `filterText` at 128 chars).
fn citation_filter_text(entry: &crate::bib::semantic::Entry) -> String {
    let mut text = entry.key.to_string();
    for extra in [entry.title.as_ref(), entry.authors.as_ref()]
        .into_iter()
        .flatten()
    {
        text.push(' ');
        text.push_str(extra);
    }
    if text.len() > 128 {
        let end = text
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|&i| i <= 128)
            .last()
            .unwrap_or(0);
        text.truncate(end);
    }
    text
}

/// Glossary/acronym key candidates: every `\newglossaryentry`/`\newacronym` key
/// defined in the completing file's namespace (its include-graph component,
/// [`ResolvedLabels::namespace_members`]), prefix-filtered and deduped. The
/// glossary analog of [`cite_completion_items`]; unlike BibTeX keys, glossary
/// keys are case-sensitive, so the prefix filter is exact.
fn glossary_completion_items(
    snapshot: &Analysis,
    labels: &ResolvedLabels,
    lint_path: &Path,
    prefix: &str,
) -> Vec<CompletionItem> {
    let mut keys: Vec<SmolStr> = Vec::new();
    for member in labels.namespace_members(lint_path) {
        let Some(file) = snapshot.lookup_file(member) else {
            continue;
        };
        keys.extend(
            snapshot
                .file_glossary_keys(file)
                .iter()
                .filter(|key| key.starts_with(prefix))
                .cloned(),
        );
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

/// Describe the command/environment, `\cite` key, or `\label`/`\ref` key under the
/// cursor and reply with a [`Hover`] (or `null` when nothing resolves).
#[allow(clippy::too_many_arguments)]
fn run_hover(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &TextBuffer,
    position: Position,
    build: &BuildConfig,
    out_tx: &Sender<Outbound>,
) {
    let result = hover::compute_hover(snapshot, path, text, position, build)
        .and_then(|hover| serde_json::to_value(hover).ok())
        .unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Locate the cursor file's compiled PDF, launch the configured viewer at
/// `line`, and reply with the resulting status.
///
/// The document root is the first of: `[build] root`, the label namespace's
/// `\documentclass`/`\begin{document}` member ([`root_document_of`]), or the file
/// itself. `%f` stays the *cursor's* file throughout — SyncTeX indexes per input
/// file — while `%p` is the *root's* PDF, which is the whole reason the root
/// matters here.
///
/// The PDF must exist. Without that check a stale or never-built project would
/// launch a viewer onto nothing, and the client would have no way to say so;
/// with it, `Failure` is a signal the editor can turn into "build the document
/// first". Reading one directory entry is the same class of environment access
/// the `.aux` freshness check already performs.
#[allow(clippy::too_many_arguments)]
fn run_forward_search(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    line: u32,
    build: &BuildConfig,
    executable: &str,
    args: &[String],
    out_tx: &Sender<Outbound>,
) {
    // A cancelled read leaves the root unresolved; fall back to the cursor's own
    // file rather than failing the request, since a single-file project resolves
    // to exactly that anyway.
    let root = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let (resolution, _) = snapshot.resolve_project();
        root_document_of(snapshot, &namespace_of(resolution, path)).map(Path::to_path_buf)
    }))
    .ok()
    .flatten();
    let root = build
        .root
        .clone()
        .or(root)
        .unwrap_or_else(|| path.to_path_buf());

    let status = match forward_search::pdf_path(&root, build) {
        Some(pdf) if pdf.is_file() => {
            let target = forward_search::SearchTarget {
                tex: path.to_path_buf(),
                pdf,
                line,
            };
            forward_search::spawn_viewer(executable, args, &target)
        }
        Some(pdf) => {
            log::info!(
                "forward search: no PDF at {} (has the document been compiled?)",
                pdf.display()
            );
            ForwardSearchStatus::Failure
        }
        None => ForwardSearchStatus::Failure,
    };
    let result = serde_json::to_value(status.result()).unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Describe the command/environment whose argument the cursor is typing in and
/// reply with a `SignatureHelp` (or `null` when nothing resolves).
#[allow(clippy::too_many_arguments)]
fn run_signature_help(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &TextBuffer,
    position: Position,
    enc: PositionEncoding,
    out_tx: &Sender<Outbound>,
) {
    let result = signature_help::compute_signature_help(snapshot, path, text, position, enc)
        .and_then(|help| serde_json::to_value(help).ok())
        .unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Resolve the `\ref`/`\cite` under the cursor and reply with the matching
/// definition [`Location`]s (always an array — empty when nothing resolves).
#[allow(clippy::too_many_arguments)]
fn run_goto_definition(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &TextBuffer,
    position: Position,
    texmf: &TexmfConfig,
    enc: PositionEncoding,
    out_tx: &Sender<Outbound>,
) {
    let locations = compute_goto_definition(snapshot, path, text, position, texmf, enc);
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
    text: &TextBuffer,
    position: Position,
    include_declaration: bool,
    enc: PositionEncoding,
    out_tx: &Sender<Outbound>,
) {
    let locations = compute_references(snapshot, path, text, position, include_declaration, enc);
    let result = serde_json::to_value(locations).unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Resolve the label/cite key under the cursor and reply with its key-token range +
/// placeholder, or `null` when the cursor isn't on a renameable key. The narrow
/// `key_range` (not the whole-command range) is what anchors the client's rename UI.
/// Resolve the cross-reference key under the cursor and reply with every same-key
/// occurrence in the buffer as `DocumentHighlight`s (an empty array when nothing
/// resolves), serialized to the read pool's outbound channel.
fn run_document_highlight(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &TextBuffer,
    position: Position,
    out_tx: &Sender<Outbound>,
) {
    let highlights = compute_document_highlight(snapshot, path, text, position);
    let result = serde_json::to_value(highlights).unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

#[allow(clippy::too_many_arguments)]
fn run_prepare_rename(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &TextBuffer,
    position: Position,
    out_tx: &Sender<Outbound>,
) {
    let result = compute_prepare_rename(snapshot, path, text, position)
        .map(|(range, placeholder)| {
            serde_json::to_value(PrepareRenameResponse::RangeWithPlaceholder { range, placeholder })
                .unwrap_or(serde_json::Value::Null)
        })
        .unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Resolve the label/cite key under the cursor and reply with the project-wide
/// [`WorkspaceEdit`] renaming it (definition and every referencing command), or
/// `null` when nothing resolves or the new name is rejected.
#[allow(clippy::too_many_arguments)]
fn run_rename(
    snapshot: &Analysis,
    id: RequestId,
    path: &Path,
    text: &TextBuffer,
    position: Position,
    new_name: &str,
    enc: PositionEncoding,
    out_tx: &Sender<Outbound>,
) {
    let result = compute_rename(snapshot, path, text, position, new_name, enc)
        .and_then(|edit| serde_json::to_value(edit).ok())
        .unwrap_or(serde_json::Value::Null);
    let _ = out_tx.send(Outbound::Response(Response::new_ok(id, result)));
}

/// Answer a `changeEnvironment` execute-command: push the begin/end name rewrite
/// via [`Outbound::ApplyEdit`], or reply with an error when no environment
/// encloses the cursor (an executed command should say why it did nothing, unlike
/// the `null`-on-miss document requests).
#[allow(clippy::too_many_arguments)]
fn run_change_environment(
    snapshot: &Analysis,
    id: RequestId,
    uri: &Uri,
    path: &Path,
    text: &TextBuffer,
    position: Position,
    new_name: &str,
    out_tx: &Sender<Outbound>,
) {
    match compute_change_environment(snapshot, path, text, position) {
        Some((old_name, ranges)) => {
            let idx = text.line_index();
            let mut changes = HashMap::new();
            for range in ranges {
                push_edit(&mut changes, uri, &idx, range, new_name);
            }
            let edit = WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            };
            let label = format!("change environment: {old_name} -> {new_name}");
            let _ = out_tx.send(Outbound::ApplyEdit { id, label, edit });
        }
        None => {
            let _ = out_tx.send(Outbound::Response(Response::new_err(
                id,
                ErrorCode::RequestFailed as i32,
                "no environment around the cursor".to_owned(),
            )));
        }
    }
}

/// Compute the change-environment target at `position`: the enclosing
/// environment's current name plus the name spans to rewrite. Reads the cached
/// tree when current, else a fresh parse (the same guard as
/// [`compute_document_highlight`]). `None` for a `.bib` buffer (no environments)
/// or when [`environment_change_target`] declines.
fn compute_change_environment(
    snapshot: &Analysis,
    path: &Path,
    text: &TextBuffer,
    position: Position,
) -> Option<(String, Vec<TextRange>)> {
    if file_kind_for(path) == FileKind::Bib {
        return None;
    }
    let idx = text.line_index();
    let offset = idx.offset_at(position.line, position.character);

    let computed = salsa::Cancelled::catch(AssertUnwindSafe(|| match snapshot.lookup_file(path) {
        Some(file) if snapshot.text_is_current(file, text) => {
            environment_change_target(&snapshot.parsed_tree(file), offset)
        }
        _ => environment_change_target(&SyntaxNode::new_root(parse(text).green), offset),
    }));
    computed.ok().flatten()
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

/// The renameable key under the cursor: which name(s) to rewrite project-wide
/// ([`target`](Self::target)), the precise key-token span the cursor sits on (for
/// the `prepareRename` range), and the current key text as the rename placeholder.
#[derive(Debug)]
struct RenameTarget {
    target: CursorTarget,
    span: TextRange,
    placeholder: SmolStr,
}

/// Compute the definition locations for a go-to-definition at `position`, preferring
/// the snapshot's cached model and falling back to a fresh parse when it is stale or
/// uncached. Cross-file resolution always runs against the db snapshot's resolvers
/// (`resolved_labels`/`resolved_citations`).
fn compute_goto_definition(
    snapshot: &Analysis,
    path: &Path,
    text: &TextBuffer,
    position: Position,
    texmf: &TexmfConfig,
    enc: PositionEncoding,
) -> Vec<Location> {
    let idx = text.line_index();
    let offset = idx.offset_at(position.line, position.character);
    let base_dir = path.parent();

    let cached = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        // Find the reference under the cursor (off the cached model when current,
        // else a fresh parse), then resolve cross-file against the db snapshot. The
        // parsed `root` is also kept for the file-target fallback below.
        let (root, target, lint_path) = match snapshot.lookup_file(path) {
            Some(file) if snapshot.text_is_current(file, text) => (
                snapshot.parsed_tree(file),
                reference_under_cursor(snapshot.semantic_model(file), offset),
                snapshot.file_path(file).to_path_buf(),
            ),
            _ => {
                let (root, model) = fallback_tex_model(snapshot, path, text);
                let target = reference_under_cursor(&model, offset);
                (root, target, path.to_path_buf())
            }
        };
        if let Some(target) = target {
            let (resolution, citations) = snapshot.resolve_project();
            return match target {
                CursorTarget::Labels(names) => {
                    resolve_label_locations(snapshot, resolution, &lint_path, &names, enc)
                }
                CursorTarget::Citations(names) => {
                    resolve_citation_locations(snapshot, citations, &lint_path, &names, enc)
                }
            };
        }
        // Not a `\ref`/`\cite`: a user command/environment name jumps to its
        // definition sites (`\newcommand`/`\def`/xparse, `\newenvironment`) across
        // the macro namespace. A name with no project definition (a built-in) falls
        // through to the file-target tier below.
        let sites = scan_definition_sites(&root);
        if let Some(target) = name_refs::name_target_under_cursor(&root, offset, &sites) {
            let (resolution, _) = snapshot.resolve_project();
            let packages = snapshot.package_graph();
            let defs = name_definition_sites(snapshot, resolution, packages, &lint_path, &target);
            if !defs.is_empty() {
                return defs
                    .into_iter()
                    .filter_map(|(def_path, range)| {
                        let file = snapshot.lookup_file(&def_path)?;
                        let text = snapshot.file_text(file);
                        let idx = LineIndex::with_encoding(text, enc);
                        location_for(&def_path, &idx, range)
                    })
                    .collect();
            }
        }
        // Fall back to a file-referencing argument
        // (include/package/class/bib/graphics) under the cursor, jumping to the
        // resolved on-disk target — the same resolution the document-link path uses,
        // so a system package resolves through the TEXMF index too.
        file_target_under_cursor(&root, base_dir, offset, texmf)
    }));
    cached.unwrap_or_default()
}

/// The go-to-definition file target under `offset`: the document link whose argument
/// span covers the cursor, mapped to a whole-file [`Location`]. Reuses
/// [`document_link::document_links`] (disk-aware and TEXMF-aware), so every command it
/// resolves—`\input`, `\usepackage`, `\includegraphics`, …—becomes navigable. Empty
/// when the cursor is not on a resolvable file argument.
fn file_target_under_cursor(
    root: &SyntaxNode,
    base_dir: Option<&Path>,
    offset: usize,
    texmf: &TexmfConfig,
) -> Vec<Location> {
    let at = TextSize::new(offset as u32);
    let index = crate::project::texmf::global_index(texmf);
    document_link::document_links(root, base_dir, index)
        .into_iter()
        .find(|link| link.range.contains_inclusive(at))
        .and_then(|link| path_to_uri(&link.target))
        .map(|uri| {
            vec![Location {
                uri,
                range: Range::default(),
            }]
        })
        .unwrap_or_default()
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
/// `\ref`/`\cite` use sites across the namespace, falling back to command/
/// environment *name* occurrences ([`name_reference_locations`]) when the cursor
/// is not on a key. The cursor's own buffer is read off the cached tree when
/// current, else a fresh parse. `include_declaration` appends the
/// `\label`/`@entry`/definition-site occurrence to the results.
#[allow(clippy::too_many_arguments)]
fn compute_references(
    snapshot: &Analysis,
    path: &Path,
    text: &TextBuffer,
    position: Position,
    include_declaration: bool,
    enc: PositionEncoding,
) -> Vec<Location> {
    let idx = text.line_index();
    let offset = idx.offset_at(position.line, position.character);

    let computed = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let (resolution, citations) = snapshot.resolve_project();

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
                location_for(&origin, &idx, key_range)
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
                enc,
            );
        }

        // `.tex` origin: a `\ref`/`\cite` use *or* a `\label` definition. The parsed
        // `root` is kept for the command/environment name fallback below.
        let (root, target, origin) = match snapshot.lookup_file(path) {
            Some(file) if snapshot.text_is_current(file, text) => (
                snapshot.parsed_tree(file),
                references_target_under_cursor(snapshot.semantic_model(file), offset),
                snapshot.file_path(file).to_path_buf(),
            ),
            _ => {
                let (root, model) = fallback_tex_model(snapshot, path, text);
                let target = references_target_under_cursor(&model, offset);
                (root, target, path.to_path_buf())
            }
        };
        if let Some(target) = target {
            return match target {
                CursorTarget::Labels(names) => reference_label_locations(
                    snapshot,
                    resolution,
                    &origin,
                    &names,
                    include_declaration,
                    enc,
                ),
                CursorTarget::Citations(names) => reference_citation_locations(
                    snapshot,
                    citations,
                    &origin,
                    FileKind::Tex,
                    &names,
                    include_declaration,
                    None,
                    enc,
                ),
            };
        }
        // Not a key: a command or environment *name* under the cursor — a pure
        // occurrence search across the macro namespace, ungated (built-ins included;
        // only *rename* is gated to user-defined names).
        let sites = scan_definition_sites(&root);
        let Some(target) = name_refs::name_target_under_cursor(&root, offset, &sites) else {
            return Vec::new();
        };
        let packages = snapshot.package_graph();
        name_reference_locations(
            snapshot,
            resolution,
            packages,
            &origin,
            &target,
            include_declaration,
            enc,
        )
    }));
    computed.unwrap_or_default()
}

/// Every occurrence of a command/environment name across `origin`'s macro
/// namespace — the name-based tier of [`compute_references`]. Command occurrences
/// are full `\name` token ranges (definition-site names included: the `\mycmd` in
/// `\newcommand{\mycmd}` is itself a `CONTROL_WORD`, filtered out by range equality
/// against its [`DefSite`] unless `include_declaration`). Environment occurrences
/// are `\begin`/`\end` name spans; their `\newenvironment{name}` definition names
/// are invisible to that walk, so `include_declaration` *adds* them.
///
/// [`DefSite`]: crate::semantic::DefSite
fn name_reference_locations(
    snapshot: &Analysis,
    resolution: &ResolvedLabels,
    packages: &PackageGraph,
    origin: &Path,
    target: &NameTarget,
    include_declaration: bool,
    enc: PositionEncoding,
) -> Vec<Location> {
    let mut locations = Vec::new();
    for member in name_refs::macro_namespace(resolution, packages, origin) {
        let Some(file) = snapshot.lookup_file(&member) else {
            continue;
        };
        let text = snapshot.file_text(file);
        let idx = LineIndex::with_encoding(text, enc);
        let root = snapshot.parsed_tree(file);
        let sites = scan_definition_sites(&root);
        match target.kind {
            NameKind::Command => {
                let def_ranges: HashSet<TextRange> = sites
                    .iter()
                    .filter(|s| s.kind == DefSiteKind::Command && s.name == target.name)
                    .map(|s| s.name_range)
                    .collect();
                for range in name_refs::command_occurrences(&root, &target.name) {
                    if include_declaration || !def_ranges.contains(&range) {
                        locations.push(location_for(&member, &idx, range));
                    }
                }
            }
            NameKind::Environment => {
                for range in name_refs::environment_occurrences(&root, &target.name) {
                    locations.push(location_for(&member, &idx, range));
                }
                if include_declaration {
                    for site in sites
                        .iter()
                        .filter(|s| s.kind == DefSiteKind::Environment && s.name == target.name)
                    {
                        locations.push(location_for(&member, &idx, site.name_range));
                    }
                }
            }
        }
    }
    dedup_locations(locations)
}

/// The matching [`DefSite`]s of `target` across `origin`'s macro namespace, as
/// `(file, name span)` pairs in member order. Serves three consumers: the
/// user-defined rename gate (non-empty = defined in the project), goto-definition
/// (each pair becomes a [`Location`]), and environment rename (each name span is
/// rewritten alongside the `\begin`/`\end` occurrences).
///
/// [`DefSite`]: crate::semantic::DefSite
fn name_definition_sites(
    snapshot: &Analysis,
    resolution: &ResolvedLabels,
    packages: &PackageGraph,
    origin: &Path,
    target: &NameTarget,
) -> Vec<(PathBuf, TextRange)> {
    let want = match target.kind {
        NameKind::Command => DefSiteKind::Command,
        NameKind::Environment => DefSiteKind::Environment,
    };
    let mut out = Vec::new();
    for member in name_refs::macro_namespace(resolution, packages, origin) {
        let Some(file) = snapshot.lookup_file(&member) else {
            continue;
        };
        let root = snapshot.parsed_tree(file);
        for site in scan_definition_sites(&root) {
            if site.kind == want && site.name == target.name {
                out.push((member.clone(), site.name_range));
            }
        }
    }
    out
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

/// The renameable key whose **key-token** range (not the whole-command range)
/// covers `offset`: a `\ref`/`\cite` use or a `\label` definition. Keyed on
/// `key_range` so the cursor must sit on the key itself — a position on the command
/// word, the braces, or a sibling key in a `\cref{a,b}` resolves to `None`, which is
/// what makes `prepareRename` decline outside a key. Precedence mirrors
/// [`reference_under_cursor`] (refs, then citations, then label defs); the spans are
/// disjoint, so at most one matches.
fn rename_target_under_cursor(model: &SemanticModel, offset: usize) -> Option<RenameTarget> {
    let at = TextSize::new(offset as u32);
    if let Some(r) = model
        .refs()
        .iter()
        .find(|r| r.key_range.contains_inclusive(at))
    {
        return Some(RenameTarget {
            target: CursorTarget::Labels(vec![r.name.clone()]),
            span: r.key_range,
            placeholder: r.name.clone(),
        });
    }
    if let Some(c) = model
        .citations()
        .iter()
        .find(|c| c.key_range.contains_inclusive(at))
    {
        return Some(RenameTarget {
            target: CursorTarget::Citations(vec![c.name.clone()]),
            span: c.key_range,
            placeholder: c.name.clone(),
        });
    }
    let label = model
        .labels()
        .iter()
        .find(|l| l.key_range.contains_inclusive(at))?;
    Some(RenameTarget {
        target: CursorTarget::Labels(vec![label.name.clone()]),
        span: label.key_range,
        placeholder: label.name.clone(),
    })
}

/// The name spans to highlight when the cursor at byte `offset` sits on a
/// `\begin{env}` or `\end{env}` delimiter: both paired names of the enclosing
/// `ENVIRONMENT` (or just the begin's when the environment is unclosed). Purely
/// syntactic — the parser already pairs begin/end structurally. Empty when the
/// cursor isn't inside a `BEGIN`/`END` node (a cursor in the body walks up to the
/// `ENVIRONMENT` without passing through `BEGIN`/`END`, so it resolves to nothing).
/// A stray `\end` (no `ENVIRONMENT` parent) self-highlights.
fn environment_pair_ranges(root: &SyntaxNode, offset: usize) -> Vec<TextRange> {
    let at = TextSize::new(offset.min(u32::MAX as usize) as u32);
    let (left, right) = match root.token_at_offset(at) {
        rowan::TokenAtOffset::None => return Vec::new(),
        rowan::TokenAtOffset::Single(t) => (Some(t.clone()), Some(t)),
        rowan::TokenAtOffset::Between(l, r) => (Some(l), Some(r)),
    };
    let delimiter = [left, right].into_iter().flatten().find_map(|token| {
        token
            .parent_ancestors()
            .find(|n| matches!(n.kind(), SyntaxKind::BEGIN | SyntaxKind::END))
    });
    let Some(delimiter) = delimiter else {
        return Vec::new();
    };
    match delimiter.parent() {
        Some(env) if env.kind() == SyntaxKind::ENVIRONMENT => env
            .children()
            .filter(|c| matches!(c.kind(), SyntaxKind::BEGIN | SyntaxKind::END))
            .filter_map(|c| crate::ast::environment_name_range(&c))
            .collect(),
        // A stray `\end` with no open environment: highlight it alone.
        _ => crate::ast::environment_name_range(&delimiter)
            .into_iter()
            .collect(),
    }
}

/// The change-environment target at byte `offset`: the *innermost* `ENVIRONMENT`
/// node containing the cursor (anywhere in the body or on either delimiter — the
/// refactor names the environment "around the cursor", unlike
/// [`environment_pair_ranges`]'s delimiter-only gate), as its current begin name
/// plus the name spans to rewrite. The parser's structural pairing is
/// authoritative: an unclosed environment rewrites just its `\begin` name, and a
/// mismatched-but-paired `\end` is rewritten too (making the pair consistent).
/// Correctness-only (tenet #1): when any paired delimiter's name is not a plain
/// token run (so a textual rewrite could corrupt it), decline the whole edit
/// rather than rewrite half a pair.
fn environment_change_target(root: &SyntaxNode, offset: usize) -> Option<(String, Vec<TextRange>)> {
    use crate::ast::AstNode;

    let at = TextSize::new(offset.min(u32::MAX as usize) as u32);
    let (left, right) = match root.token_at_offset(at) {
        rowan::TokenAtOffset::None => return None,
        rowan::TokenAtOffset::Single(t) => (Some(t.clone()), Some(t)),
        rowan::TokenAtOffset::Between(l, r) => (Some(l), Some(r)),
    };
    let env = [left, right].into_iter().flatten().find_map(|token| {
        token
            .parent_ancestors()
            .find_map(crate::ast::Environment::cast)
    })?;
    let begin = env.begin()?;
    let old_name = begin.name()?;
    let ranges = env
        .syntax()
        .children()
        .filter(|child| matches!(child.kind(), SyntaxKind::BEGIN | SyntaxKind::END))
        .map(|child| crate::ast::environment_name_range(&child))
        .collect::<Option<Vec<_>>>()?;
    (!ranges.is_empty()).then_some((old_name, ranges))
}

/// Compute the `prepareRename` range + placeholder at `position`: the key-token span
/// under the cursor and its current text. Reads the cached model when current, else a
/// fresh parse (the same guard as [`compute_references`]); a `.bib` cursor resolves
/// to its `@entry` key. `None` when the cursor isn't on a renameable key.
/// Compute the document highlights for `position`. Two cases, tried in order:
///
/// - **Cross-reference key** under the cursor (a `\ref`/`\cite` use or a `\label`
///   definition): every same-key occurrence in the *same* buffer. Single-file, so no
///   project resolution — the `\label` definition shades as
///   [`DocumentHighlightKind::WRITE`] and every `\ref`/`\cite` use as
///   [`DocumentHighlightKind::READ`]. Strict key gating (via
///   [`rename_target_under_cursor`]): a cursor on the command word, the braces, or a
///   sibling key in `\cref{a,b}` highlights nothing for that key.
/// - **Environment delimiter** under the cursor (a `\begin{env}`/`\end{env}`): the
///   matching pair's name spans, shaded [`DocumentHighlightKind::TEXT`] (via
///   [`environment_pair_ranges`]).
///
/// The two positions are disjoint (a key sits in a command `GROUP`, a name in a
/// `BEGIN`/`END` `NAME_GROUP`), so key-first ordering is behavior-preserving.
/// `.bib` buffers yield no highlights (an `@entry` key has no in-file uses).
fn compute_document_highlight(
    snapshot: &Analysis,
    path: &Path,
    text: &TextBuffer,
    position: Position,
) -> Vec<DocumentHighlight> {
    let idx = text.line_index();
    let offset = idx.offset_at(position.line, position.character);

    let computed = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        if file_kind_for(path) == FileKind::Bib {
            return Vec::new();
        }
        let highlight = |range: TextRange, kind: DocumentHighlightKind| DocumentHighlight {
            range: lsp_range(&idx, range),
            kind: Some(kind),
        };
        let collect = |root: &SyntaxNode, model: &SemanticModel| -> Vec<DocumentHighlight> {
            // A cross-reference key takes precedence when the cursor is on one.
            if let Some(target) = rename_target_under_cursor(model, offset) {
                return match &target.target {
                    CursorTarget::Labels(_) => {
                        let name = &target.placeholder;
                        let defs = model
                            .labels()
                            .iter()
                            .filter(|l| &l.name == name)
                            .map(|l| highlight(l.key_range, DocumentHighlightKind::WRITE));
                        let uses = model
                            .refs()
                            .iter()
                            .filter(|r| &r.name == name)
                            .map(|r| highlight(r.key_range, DocumentHighlightKind::READ));
                        defs.chain(uses).collect()
                    }
                    CursorTarget::Citations(_) => {
                        let name = &target.placeholder;
                        model
                            .citations()
                            .iter()
                            .filter(|c| &c.name == name)
                            .map(|c| highlight(c.key_range, DocumentHighlightKind::READ))
                            .collect()
                    }
                };
            }
            // Otherwise, a `\begin`/`\end` delimiter pair.
            environment_pair_ranges(root, offset)
                .into_iter()
                .map(|range| highlight(range, DocumentHighlightKind::TEXT))
                .collect()
        };
        match snapshot.lookup_file(path) {
            Some(file) if snapshot.text_is_current(file, text) => {
                collect(&snapshot.parsed_tree(file), snapshot.semantic_model(file))
            }
            _ => {
                let (root, model) = fallback_tex_model(snapshot, path, text);
                collect(&root, &model)
            }
        }
    }));
    computed.unwrap_or_default()
}

fn compute_prepare_rename(
    snapshot: &Analysis,
    path: &Path,
    text: &TextBuffer,
    position: Position,
) -> Option<(Range, String)> {
    let idx = text.line_index();
    let offset = idx.offset_at(position.line, position.character);

    let computed = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        // `.bib` origin: the `@entry` key under the cursor.
        if file_kind_for(path) == FileKind::Bib {
            let (key, key_range) = bib_entry_under_cursor(snapshot, path, text, offset)?;
            return Some((lsp_range(&idx, key_range), key.to_string()));
        }
        // `.tex` origin: a `\ref`/`\cite` use or a `\label` definition. The parsed
        // `root` is kept for the command/environment name fallback below.
        let (root, target) = match snapshot.lookup_file(path) {
            Some(file) if snapshot.text_is_current(file, text) => (
                snapshot.parsed_tree(file),
                rename_target_under_cursor(snapshot.semantic_model(file), offset),
            ),
            _ => {
                let (root, model) = fallback_tex_model(snapshot, path, text);
                let target = rename_target_under_cursor(&model, offset);
                (root, target)
            }
        };
        if let Some(target) = target {
            return Some((lsp_range(&idx, target.span), target.placeholder.to_string()));
        }
        // Not a key: a command or environment name, gated to user-defined names
        // (a project definition site must exist — renaming `\textbf` or
        // `verbatim` over a partial namespace view is a footgun).
        let sites = scan_definition_sites(&root);
        let target = name_refs::name_target_under_cursor(&root, offset, &sites)?;
        if !name_rename_allowed(snapshot, path, &sites, &target) {
            return None;
        }
        Some((lsp_range(&idx, target.span), target.name.to_string()))
    }));
    computed.ok().flatten()
}

/// The user-defined rename gate: `target` is renameable when a matching
/// definition site exists in `origin`'s macro namespace, or — for an
/// untracked/stale cursor buffer the namespace walk cannot see — in the buffer's
/// own scanned sites (`own_sites`, the conservative degradation). This is
/// deliberately the *user tier* only: built-in and CWL names never pass, so
/// `\alpha` and `verbatim` decline. References stay ungated.
fn name_rename_allowed(
    snapshot: &Analysis,
    origin: &Path,
    own_sites: &[crate::semantic::DefSite],
    target: &NameTarget,
) -> bool {
    let want = match target.kind {
        NameKind::Command => DefSiteKind::Command,
        NameKind::Environment => DefSiteKind::Environment,
    };
    if own_sites
        .iter()
        .any(|site| site.kind == want && site.name == target.name)
    {
        return true;
    }
    let (resolution, _) = snapshot.resolve_project();
    let packages = snapshot.package_graph();
    !name_definition_sites(snapshot, resolution, packages, origin, target).is_empty()
}

/// Compute the [`WorkspaceEdit`] renaming the key — or, in the name-based fallback
/// tier, the user-defined command/environment name — under the cursor to `new_name`
/// across its namespace — the write mirror of [`compute_references`]. Rewrites only
/// the per-key `key_range` of each occurrence (so a sibling key in `\cref{a,b}` is
/// untouched), always including the definition. Best-effort: every occurrence in the
/// *visible* namespace is rewritten (an unresolved/dynamic `\input` may hide a use we
/// cannot see). `None` when `new_name` is not syntactically safe for the target
/// ([`is_valid_key`], or [`is_valid_command_name`] for a command), or nothing
/// resolves.
fn compute_rename(
    snapshot: &Analysis,
    path: &Path,
    text: &TextBuffer,
    position: Position,
    new_name: &str,
    enc: PositionEncoding,
) -> Option<WorkspaceEdit> {
    let idx = text.line_index();
    let offset = idx.offset_at(position.line, position.character);

    let changes = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        let (resolution, citations) = snapshot.resolve_project();

        // `.bib` origin: the `@entry` key under the cursor → its `\cite` uses + the
        // entry itself.
        if file_kind_for(path) == FileKind::Bib {
            if !is_valid_key(new_name) {
                return HashMap::new();
            }
            let Some((key, _)) = bib_entry_under_cursor(snapshot, path, text, offset) else {
                return HashMap::new();
            };
            let origin = snapshot
                .lookup_file(path)
                .map(|file| snapshot.file_path(file).to_path_buf())
                .unwrap_or_else(|| path.to_path_buf());
            return rename_citation_edits(
                snapshot,
                citations,
                &origin,
                FileKind::Bib,
                &[key],
                new_name,
                enc,
            );
        }

        // `.tex` origin: a `\ref`/`\cite` use or a `\label` definition. The parsed
        // `root` is kept for the command/environment name fallback below.
        let (root, target, origin) = match snapshot.lookup_file(path) {
            Some(file) if snapshot.text_is_current(file, text) => (
                snapshot.parsed_tree(file),
                rename_target_under_cursor(snapshot.semantic_model(file), offset),
                snapshot.file_path(file).to_path_buf(),
            ),
            _ => {
                let (root, model) = fallback_tex_model(snapshot, path, text);
                let target = rename_target_under_cursor(&model, offset);
                (root, target, path.to_path_buf())
            }
        };
        if let Some(target) = target {
            if !is_valid_key(new_name) {
                return HashMap::new();
            }
            return match target.target {
                CursorTarget::Labels(names) => {
                    rename_label_edits(snapshot, resolution, &origin, &names, new_name, enc)
                }
                CursorTarget::Citations(names) => rename_citation_edits(
                    snapshot,
                    citations,
                    &origin,
                    FileKind::Tex,
                    &names,
                    new_name,
                    enc,
                ),
            };
        }
        // Not a key: a command or environment name, gated to user-defined names
        // like `compute_prepare_rename` (a client may skip prepareRename).
        let sites = scan_definition_sites(&root);
        let Some(target) = name_refs::name_target_under_cursor(&root, offset, &sites) else {
            return HashMap::new();
        };
        if !name_rename_allowed(snapshot, path, &sites, &target) {
            return HashMap::new();
        }
        let packages = snapshot.package_graph();
        match target.kind {
            NameKind::Command => {
                // The placeholder is the bare name, but a typed `\newname` is
                // accepted too — strip one leading backslash so both agree.
                let bare = new_name.strip_prefix('\\').unwrap_or(new_name);
                if !is_valid_command_name(bare, &target.name) {
                    return HashMap::new();
                }
                rename_command_edits(
                    snapshot,
                    resolution,
                    packages,
                    &origin,
                    &target.name,
                    bare,
                    enc,
                )
            }
            NameKind::Environment => {
                if !is_valid_key(new_name) {
                    return HashMap::new();
                }
                rename_environment_edits(
                    snapshot, resolution, packages, &origin, &target, new_name, enc,
                )
            }
        }
    }))
    .unwrap_or_default();
    finalize_rename(changes)
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
        Some(file) if snapshot.text_is_current(file, text) => {
            find(snapshot.bib_semantic_model(file))
        }
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
    enc: PositionEncoding,
) -> Vec<Location> {
    let mut locations = Vec::new();
    for member in resolution.namespace_members(origin) {
        let Some(file) = snapshot.lookup_file(member) else {
            continue;
        };
        let text = snapshot.file_text(file);
        let idx = LineIndex::with_encoding(text, enc);
        let model = snapshot.semantic_model(file);
        for r in model.refs() {
            if names.contains(&r.name) {
                locations.push(location_for(member, &idx, r.range));
            }
        }
        if include_declaration {
            for label in model.labels() {
                if names.contains(&label.name) {
                    locations.push(location_for(member, &idx, label.range));
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
    enc: PositionEncoding,
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
        let idx = LineIndex::with_encoding(text, enc);
        for c in snapshot.semantic_model(file).citations() {
            if names.iter().any(|n| n.eq_ignore_ascii_case(&c.name)) {
                locations.push(location_for(member, &idx, c.range));
            }
        }
    }
    let mut locations = dedup_locations(locations);
    if include_declaration {
        match kind {
            FileKind::Bib => locations.extend(decl_for_bib),
            _ => locations.extend(resolve_citation_locations(
                snapshot, citations, origin, names, enc,
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
    enc: PositionEncoding,
) -> Vec<Location> {
    let mut locations = Vec::new();
    for name in names {
        for def_path in resolution.definers(lint_path, name) {
            let Some(file) = snapshot.lookup_file(def_path) else {
                continue;
            };
            let text = snapshot.file_text(file);
            let idx = LineIndex::with_encoding(text, enc);
            for label in snapshot.semantic_model(file).labels() {
                if &label.name == name {
                    locations.push(location_for(def_path, &idx, label.range));
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
    enc: PositionEncoding,
) -> Vec<Location> {
    let mut locations = Vec::new();
    for bib_path in citations.bib_definers(lint_path) {
        let Some(file) = snapshot.lookup_file(bib_path) else {
            continue;
        };
        let text = snapshot.file_text(file);
        let idx = LineIndex::with_encoding(text, enc);
        for entry in snapshot.bib_semantic_model(file).entries() {
            if names.iter().any(|n| n.eq_ignore_ascii_case(&entry.key)) {
                locations.push(location_for(bib_path, &idx, entry.key_range));
            }
        }
    }
    dedup_locations(locations)
}

/// Build an LSP [`Location`] from a definer file's path and a byte range in its
/// text. A path that cannot form a `file://` URI yields `None` (skipped).
fn location_for(path: &Path, idx: &LineIndex, range: TextRange) -> Option<Location> {
    Some(Location {
        uri: path_to_uri(path)?,
        range: byte_range_to_lsp(idx, usize::from(range.start()), usize::from(range.end())),
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

/// Convert a byte [`TextRange`] to an LSP [`Range`] via `idx`.
fn lsp_range(idx: &LineIndex, range: TextRange) -> Range {
    byte_range_to_lsp(idx, usize::from(range.start()), usize::from(range.end()))
}

/// Every `\ref`-family use of `names` across `origin`'s label namespace, plus every
/// `\label` definition, each rewritten to `new_name` at its precise `key_range`. The
/// rename mirror of [`reference_label_locations`] — `TextEdit`s grouped by URI
/// instead of `Location`s, and the definition is *always* included (a rename rewrites
/// the def, unlike find-references' optional declaration).
fn rename_label_edits(
    snapshot: &Analysis,
    resolution: &ResolvedLabels,
    origin: &Path,
    names: &[SmolStr],
    new_name: &str,
    enc: PositionEncoding,
) -> HashMap<Uri, Vec<TextEdit>> {
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for member in resolution.namespace_members(origin) {
        let Some(file) = snapshot.lookup_file(member) else {
            continue;
        };
        let Some(uri) = path_to_uri(member) else {
            continue;
        };
        let text = snapshot.file_text(file);
        let idx = LineIndex::with_encoding(text, enc);
        let model = snapshot.semantic_model(file);
        for r in model.refs() {
            if names.contains(&r.name) {
                push_edit(&mut changes, &uri, &idx, r.key_range, new_name);
            }
        }
        for label in model.labels() {
            if names.contains(&label.name) {
                push_edit(&mut changes, &uri, &idx, label.key_range, new_name);
            }
        }
    }
    changes
}

/// Every `\cite`-family use of `names` across `origin`'s citation namespace, plus the
/// bibliography `@entry` keys, rewritten to `new_name` at each precise `key_range`.
/// The rename mirror of [`reference_citation_locations`]: `.tex` use sites come from
/// `bib_citers` (a `.bib` origin) or `namespace_members` (a `.tex` origin); the
/// definition sites are the origin bib itself (`.bib` origin) or `bib_definers` (a
/// `.tex` origin). Matching is case-insensitive, as BibTeX folds key case.
fn rename_citation_edits(
    snapshot: &Analysis,
    citations: &ResolvedCitations,
    origin: &Path,
    kind: FileKind,
    names: &[SmolStr],
    new_name: &str,
    enc: PositionEncoding,
) -> HashMap<Uri, Vec<TextEdit>> {
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    let tex_members = if kind == FileKind::Bib {
        citations.bib_citers(origin)
    } else {
        citations.namespace_members(origin)
    };
    for member in tex_members {
        let Some(file) = snapshot.lookup_file(member) else {
            continue;
        };
        let Some(uri) = path_to_uri(member) else {
            continue;
        };
        let text = snapshot.file_text(file);
        let idx = LineIndex::with_encoding(text, enc);
        for c in snapshot.semantic_model(file).citations() {
            if names.iter().any(|n| n.eq_ignore_ascii_case(&c.name)) {
                push_edit(&mut changes, &uri, &idx, c.key_range, new_name);
            }
        }
    }
    match kind {
        // From a `.bib` cursor, rewrite the entry in the origin bibliography itself.
        FileKind::Bib => push_bib_entry_edits(snapshot, &mut changes, origin, names, new_name, enc),
        _ => {
            for bib_path in citations.bib_definers(origin) {
                push_bib_entry_edits(snapshot, &mut changes, bib_path, names, new_name, enc);
            }
        }
    }
    changes
}

/// Every `\name` occurrence across `origin`'s macro namespace rewritten to the bare
/// `new_name` — the rename mirror of the command arm of
/// [`name_reference_locations`]. Each edit rewrites the token's name span *behind*
/// the backslash ([`name_refs::strip_backslash`]), leaving the `\` byte untouched.
/// Definition-site names (`\newcommand{\name}`, `\def\name`) are themselves
/// `CONTROL_WORD` tokens, so the occurrence walk rewrites them too — no separate
/// definition pass. Letter-globbing safety is by construction: the old token only
/// lexed as `\name` because the next byte is a non-letter, and
/// [`is_valid_command_name`] keeps the new name letters-only, so the new control
/// word ends at the same boundary (`\foo bar`, `\foo{x}`, `\foo\bar`, `\foo*` all
/// stay correct).
fn rename_command_edits(
    snapshot: &Analysis,
    resolution: &ResolvedLabels,
    packages: &PackageGraph,
    origin: &Path,
    name: &str,
    new_name: &str,
    enc: PositionEncoding,
) -> HashMap<Uri, Vec<TextEdit>> {
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for member in name_refs::macro_namespace(resolution, packages, origin) {
        let Some(file) = snapshot.lookup_file(&member) else {
            continue;
        };
        let Some(uri) = path_to_uri(&member) else {
            continue;
        };
        let text = snapshot.file_text(file);
        let idx = LineIndex::with_encoding(text, enc);
        let root = snapshot.parsed_tree(file);
        for range in name_refs::command_occurrences(&root, name) {
            push_edit(
                &mut changes,
                &uri,
                &idx,
                name_refs::strip_backslash(range),
                new_name,
            );
        }
    }
    changes
}

/// Every `\begin{name}`/`\end{name}` occurrence across `origin`'s macro namespace,
/// plus every `\newenvironment{name}`-family definition name, rewritten to
/// `new_name` — the rename mirror of the environment arm of
/// [`name_reference_locations`]. Name-based, not pair-based: an unbalanced
/// `\begin` is still renamed. The definition names come from
/// [`name_definition_sites`], since the `\begin`/`\end` walk cannot see them.
fn rename_environment_edits(
    snapshot: &Analysis,
    resolution: &ResolvedLabels,
    packages: &PackageGraph,
    origin: &Path,
    target: &NameTarget,
    new_name: &str,
    enc: PositionEncoding,
) -> HashMap<Uri, Vec<TextEdit>> {
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for member in name_refs::macro_namespace(resolution, packages, origin) {
        let Some(file) = snapshot.lookup_file(&member) else {
            continue;
        };
        let Some(uri) = path_to_uri(&member) else {
            continue;
        };
        let text = snapshot.file_text(file);
        let idx = LineIndex::with_encoding(text, enc);
        let root = snapshot.parsed_tree(file);
        for range in name_refs::environment_occurrences(&root, &target.name) {
            push_edit(&mut changes, &uri, &idx, range, new_name);
        }
    }
    for (def_path, name_range) in
        name_definition_sites(snapshot, resolution, packages, origin, target)
    {
        let Some(file) = snapshot.lookup_file(&def_path) else {
            continue;
        };
        let Some(uri) = path_to_uri(&def_path) else {
            continue;
        };
        let text = snapshot.file_text(file);
        let idx = LineIndex::with_encoding(text, enc);
        push_edit(&mut changes, &uri, &idx, name_range, new_name);
    }
    changes
}

/// Push the `@entry` key edits for `names` in the bibliography at `bib_path` (case-
/// insensitive match), rewriting each `key_range` to `new_name`.
fn push_bib_entry_edits(
    snapshot: &Analysis,
    changes: &mut HashMap<Uri, Vec<TextEdit>>,
    bib_path: &Path,
    names: &[SmolStr],
    new_name: &str,
    enc: PositionEncoding,
) {
    let Some(file) = snapshot.lookup_file(bib_path) else {
        return;
    };
    let Some(uri) = path_to_uri(bib_path) else {
        return;
    };
    let text = snapshot.file_text(file);
    let idx = LineIndex::with_encoding(text, enc);
    for entry in snapshot.bib_semantic_model(file).entries() {
        if names.iter().any(|n| n.eq_ignore_ascii_case(&entry.key)) {
            push_edit(changes, &uri, &idx, entry.key_range, new_name);
        }
    }
}

/// Append a `key_range → new_name` [`TextEdit`] to `uri`'s edit list.
fn push_edit(
    changes: &mut HashMap<Uri, Vec<TextEdit>>,
    uri: &Uri,
    idx: &LineIndex,
    range: TextRange,
    new_name: &str,
) {
    changes.entry(uri.clone()).or_default().push(TextEdit {
        range: lsp_range(idx, range),
        new_text: new_name.to_owned(),
    });
}

/// Sort and dedup each file's edits, drop empty files, and wrap the rest in a
/// [`WorkspaceEdit`]. `None` when nothing is left to rewrite (so the handler replies
/// `null`).
fn finalize_rename(mut changes: HashMap<Uri, Vec<TextEdit>>) -> Option<WorkspaceEdit> {
    changes.retain(|_, edits| {
        edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
        edits.dedup();
        !edits.is_empty()
    });
    (!changes.is_empty()).then(|| WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    })
}

/// Whether `new_name` is a safe replacement key: non-empty after trimming and free of
/// characters that would break the surface syntax or the comma key-list split (so an
/// applied rename can never introduce a parse/format error). Conservative — a few
/// exotic-but-legal key characters are rejected rather than risk a corrupt edit.
fn is_valid_key(new_name: &str) -> bool {
    !new_name.trim().is_empty()
        && !new_name.chars().any(|c| {
            matches!(
                c,
                '{' | '}' | '%' | '\\' | ',' | '#' | '~' | '$' | '^' | '&' | '\n' | '\r'
            )
        })
}

/// Whether `new_name` is a safe replacement *command* name (leading `\` already
/// stripped): non-empty ASCII letters, plus `@`/`_`/`:` only when `old_name`
/// already used that character. The old token lexing as one `CONTROL_WORD` proves
/// every occurrence site is inside the right letter-mode region (`\makeatletter`
/// for `@`, expl3 for `_`/`:`), so the new name re-lexes identically there — while
/// a plain name never gains `@`, which would mis-lex in ordinary text. Letters-only
/// also preserves the token boundary at every occurrence (a control word ends at
/// the first non-letter, exactly where the old one did).
fn is_valid_command_name(new_name: &str, old_name: &str) -> bool {
    !new_name.is_empty()
        && new_name.chars().all(|c| {
            c.is_ascii_alphabetic() || (matches!(c, '@' | '_' | ':') && old_name.contains(c))
        })
}

/// Turn a classified [`CompletionContext`] into LSP items. Name/label contexts go
/// through the pure [`crate::completion::candidates`]; a file-path context reads
/// the document's directory off disk (see [`file_completion_items`]).
fn build_completion_items(
    ctx: &CompletionContext,
    sigs: &SignatureDb,
    model: &SemanticModel,
    declared: &ResolvedDeclarations,
    uri: &Uri,
    texmf: &TexmfConfig,
) -> Vec<CompletionItem> {
    match ctx {
        CompletionContext::FilePath { prefix, kind } => file_completion_items(uri, prefix, *kind),
        CompletionContext::PackageName { prefix, kind } => {
            // Resolve the installed-tree index lazily, here, so only a package/class
            // completion pays for the (first-time) tree walk.
            let index = crate::project::texmf::global_index(texmf);
            package_completion_items(uri, prefix, *kind, sigs, model, index)
        }
        CompletionContext::None => Vec::new(),
        _ => {
            // The document path keys the scope-first signature lookup that
            // `completionItem/resolve` repeats; unsaved buffers have none.
            let file = uri_to_fs_path(uri);
            crate::completion::candidates_with_declarations(ctx, sigs, model, declared)
                .into_iter()
                .map(|candidate| candidate_to_item(candidate, file.as_deref()))
                .collect()
        }
    }
}

/// Map a neutral [`CompletionCandidate`] onto an `lsp_types::CompletionItem`. A
/// command/environment carries resolve `data` (its name + originating `file`) so
/// its signature can be attached lazily; a label carries none.
fn candidate_to_item(candidate: CompletionCandidate, file: Option<&Path>) -> CompletionItem {
    let kind = match candidate.kind {
        CandidateKind::Command => CompletionItemKind::FUNCTION,
        CandidateKind::Environment => CompletionItemKind::CLASS,
        CandidateKind::Label => CompletionItemKind::REFERENCE,
        CandidateKind::Package => CompletionItemKind::MODULE,
        CandidateKind::Color => CompletionItemKind::COLOR,
        CandidateKind::ColorModel => CompletionItemKind::ENUM_MEMBER,
        CandidateKind::TikzLibrary => CompletionItemKind::MODULE,
        CandidateKind::ArgumentEnum => CompletionItemKind::ENUM_MEMBER,
    };
    let data = file.and_then(|file| {
        let payload = match candidate.kind {
            CandidateKind::Command => completion_resolve::CompletionResolveData::Command {
                name: candidate.label.clone(),
                file: file.to_path_buf(),
            },
            CandidateKind::Environment => completion_resolve::CompletionResolveData::Environment {
                name: candidate.label.clone(),
                file: file.to_path_buf(),
            },
            // A package/class name carries no resolvable signature (yet); a future
            // description payload would attach here. Colors and TikZ libraries are
            // likewise static labels with nothing to resolve lazily.
            CandidateKind::Label
            | CandidateKind::Package
            | CandidateKind::Color
            | CandidateKind::ColorModel
            | CandidateKind::TikzLibrary
            | CandidateKind::ArgumentEnum => return None,
        };
        payload.into_value()
    });
    CompletionItem {
        label: candidate.label,
        kind: Some(kind),
        insert_text: candidate.insert_text,
        insert_text_format: candidate.snippet.then_some(InsertTextFormat::SNIPPET),
        data,
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

/// Package/class name candidates for `\usepackage`/`\documentclass`, in three tiers
/// of decreasing relevance: local `.sty`/`.cls` files in the document directory, then
/// the **installed set** from the TEXMF index (`texmf`), then the baked name list
/// ([`crate::semantic::completion::package_names`], all of CTAN in rank order). Files/installed
/// names are offered as their **stem** (`\usepackage` takes a name, not a filename),
/// so `amsmath.sty` becomes `amsmath`; a name already emitted by an earlier tier is
/// dropped. Every item is enriched with the CTAN one-line description
/// ([`package_metadata`](crate::semantic::completion::package_metadata)) as `detail`,
/// and `sortText` is assigned by final position so the client preserves the tiering
/// instead of re-sorting alphabetically. An empty `texmf` simply skips the middle
/// tier (the pre-index behavior).
fn package_completion_items(
    uri: &Uri,
    prefix: &str,
    kind: FileArgKind,
    sigs: &SignatureDb,
    model: &SemanticModel,
    texmf: &TexmfIndex,
) -> Vec<CompletionItem> {
    let mut seen = std::collections::HashSet::new();
    let mut items: Vec<CompletionItem> = Vec::new();
    // Tier 1: local files (offered as stems).
    for file_item in file_completion_items(uri, prefix, kind) {
        // A directory can't be a package/class *name*; only files, as stems.
        if file_item.kind != Some(CompletionItemKind::FILE) {
            continue;
        }
        let stem = file_stem(&file_item.label);
        if seen.insert(stem.clone()) {
            items.push(CompletionItem {
                label: stem,
                kind: Some(CompletionItemKind::MODULE),
                ..Default::default()
            });
        }
    }
    // Tier 2: the installed set (what the user actually has), prefix-filtered here
    // (the baked tier filters inside `candidates`).
    let installed = match kind {
        FileArgKind::Class => texmf.cls_stems(),
        _ => texmf.sty_stems(),
    };
    for stem in installed.iter().filter(|s| s.starts_with(prefix)) {
        if seen.insert(stem.clone()) {
            items.push(CompletionItem {
                label: stem.clone(),
                kind: Some(CompletionItemKind::MODULE),
                ..Default::default()
            });
        }
    }
    // Tier 3: the baked all-of-CTAN name list.
    let ctx = CompletionContext::PackageName {
        prefix: prefix.to_string(),
        kind,
    };
    let file = uri_to_fs_path(uri);
    for candidate in crate::completion::candidates(&ctx, sigs, model) {
        if seen.contains(&candidate.label) {
            continue;
        }
        items.push(candidate_to_item(candidate, file.as_deref()));
    }
    for (i, item) in items.iter_mut().enumerate() {
        // Attach the CTAN description as detail (when this stem has metadata and no
        // tier already set one).
        if item.detail.is_none()
            && let Some(meta) = crate::semantic::completion::package_metadata(&item.label)
        {
            item.detail = meta.desc.map(str::to_string);
        }
        item.sort_text = Some(format!("{i:06}"));
    }
    items
}

/// The stem of a filename label (`amsmath.sty` -> `amsmath`); unchanged if no dot.
fn file_stem(label: &str) -> String {
    label
        .rsplit_once('.')
        .map(|(stem, _)| stem.to_string())
        .unwrap_or_else(|| label.to_string())
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
    Some(PathBuf::from(native_separators(path).as_ref()))
}

/// Rewrite a decoded URI path's `/` separators to the platform's.
///
/// A URI always spells separators `/`; a Windows filesystem path spells them
/// `\`. `Path` compares and hashes by component, so the two forms are already
/// interchangeable as *keys* — but the spelling leaks wherever a decoded path is
/// rendered back to text. Forward search is where that bites: `%f` comes off the
/// document URI while `%p` is built from a root discovered on disk, so a viewer
/// received one of each. Normalize at the one decode point so they cannot
/// disagree.
///
/// No-op off Windows, where `\` is an ordinary filename byte.
#[cfg(windows)]
fn native_separators(path: &str) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Owned(path.replace('/', "\\"))
}

#[cfg(not(windows))]
fn native_separators(path: &str) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Borrowed(path)
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

/// Convert a byte range into an LSP range via the (encoding-aware) [`LineIndex`].
fn byte_range_to_lsp(idx: &LineIndex, start: usize, end: usize) -> Range {
    let (sl, sc) = idx.position(start);
    let (el, ec) = idx.position(end);
    Range {
        start: Position::new(sl, sc),
        end: Position::new(el, ec),
    }
}

/// Expand a selection to whole document-level-block boundaries: the cover of every
/// `ROOT` child *node* overlapping `sel`, except that a canonical `document`
/// environment exposes its direct body nodes as document-level blocks. Its body is
/// formatter-defined to sit flush at the root indentation, so those nodes have the
/// same independent layout context as root children. Other environments remain
/// indivisible: their specialized list, alignment, math, and indentation layouts
/// need the complete environment.
///
/// A partial selection always pulls in the whole structural units it touches.
/// Child-node iteration naturally skips inter-block trivia.
/// Returns `None` when the selection touches no block (e.g. a cursor in blank space
/// between blocks), meaning there is nothing to format.
fn expand_to_document_blocks(root: &SyntaxNode, sel: TextRange) -> Option<TextRange> {
    let mut acc: Option<TextRange> = None;
    for child in root.children() {
        let r = child.text_range();
        // A cursor (empty selection) hits the block whose range contains it
        // (touch-inclusive, so a cursor at a block edge still selects it); a
        // non-empty selection hits any block it genuinely overlaps.
        let hit = if sel.is_empty() {
            r.contains_inclusive(sel.start())
        } else {
            sel.start() < r.end() && r.start() < sel.end()
        };
        if !hit {
            continue;
        }
        if document_body_contains(&child, sel)
            && let Some(body) = cover_overlapping_children(&child, sel)
        {
            acc = Some(acc.map_or(body, |a| a.cover(body)));
        } else {
            acc = Some(acc.map_or(r, |a| a.cover(r)));
        }
    }
    acc
}

/// Whether `range` lies wholly between a canonical `document` environment's
/// delimiters. Only this built-in no-indent environment is transparent here;
/// custom and specialized environments keep their full structural context.
fn document_body_contains(node: &SyntaxNode, range: TextRange) -> bool {
    let Some(environment) = Environment::cast(node.clone()) else {
        return false;
    };
    if environment.name().as_deref() != Some("document") {
        return false;
    }
    let (Some(begin), Some(end)) = (environment.begin(), environment.end()) else {
        return false;
    };
    range.start() >= begin.syntax().text_range().end()
        && range.end() <= end.syntax().text_range().start()
}

fn cover_overlapping_children(container: &SyntaxNode, sel: TextRange) -> Option<TextRange> {
    container
        .children()
        .filter(|child| !matches!(child.kind(), SyntaxKind::BEGIN | SyntaxKind::END))
        .filter_map(|child| {
            let range = child.text_range();
            let hit = if sel.is_empty() {
                range.contains_inclusive(sel.start())
            } else {
                sel.start() < range.end() && range.start() < sel.end()
            };
            hit.then_some(range)
        })
        .reduce(TextRange::cover)
}

/// Diff the formatted `fragment` against the original `text[block_range]` slice and
/// emit one [`TextEdit`] per changed line hunk, mapped back into document
/// coordinates. A line-level LCS keeps the edits minimal (better editor
/// undo/cursor behavior than one wholesale block replacement). For a pathologically
/// large block the `O(n*m)` table is skipped in favor of a single replace.
fn diff_to_edits(
    idx: &LineIndex,
    text: &str,
    block_range: TextRange,
    fragment: &str,
) -> Vec<TextEdit> {
    let base = usize::from(block_range.start());
    let end = usize::from(block_range.end());
    let original = &text[base..end];

    // Lines keep their trailing `\n` (`split_inclusive`), so equality compares whole
    // lines and the byte offsets stay exact.
    let a: Vec<&str> = original.split_inclusive('\n').collect();
    let b: Vec<&str> = fragment.split_inclusive('\n').collect();
    let (n, m) = (a.len(), b.len());

    // Safety valve: cap the LCS table so a huge block cannot blow up; fall back to
    // one wholesale replace of the block range.
    if n.saturating_mul(m) > 4_000_000 {
        return vec![TextEdit {
            range: byte_range_to_lsp(idx, base, end),
            new_text: fragment.to_owned(),
        }];
    }

    // lcs[i][j] = length of the longest common subsequence of a[i..] and b[j..].
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    // Walk the table, coalescing each run of deletes/inserts into one replace edit.
    let mut edits = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let mut a_off = base; // byte offset of a[i] within `text`
    let mut del_start = base;
    let mut del_end = base;
    let mut ins = String::new();
    let mut in_hunk = false;
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            if in_hunk {
                edits.push(TextEdit {
                    range: byte_range_to_lsp(idx, del_start, del_end),
                    new_text: std::mem::take(&mut ins),
                });
                in_hunk = false;
            }
            a_off += a[i].len();
            i += 1;
            j += 1;
        } else if j == m || (i < n && lcs[i + 1][j] >= lcs[i][j + 1]) {
            // delete a[i]
            if !in_hunk {
                del_start = a_off;
                in_hunk = true;
            }
            a_off += a[i].len();
            del_end = a_off;
            i += 1;
        } else {
            // insert b[j]
            if !in_hunk {
                del_start = a_off;
                del_end = a_off;
                in_hunk = true;
            }
            ins.push_str(b[j]);
            j += 1;
        }
    }
    if in_hunk {
        edits.push(TextEdit {
            range: byte_range_to_lsp(idx, del_start, del_end),
            new_text: ins,
        });
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bibliography_seeding_publishes_external_path_alias() {
        let project = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let main_path = project.path().join("main.tex");
        let actual_bib = external.path().join("shared.bib");
        std::fs::write(&actual_bib, "@article{present, title={Present}}\n").unwrap();

        let mut db = IncrementalDatabase::default();
        db.upsert_file(
            &main_path,
            "\\documentclass{article}\n\\bibliography{shared}\n\\cite{present}\n".to_string(),
        );
        let requested = project.path().join("shared.bib");
        let mut lookups = HashMap::new();
        let grew = seed_bibliographies_with(&mut db, &mut lookups, |path, base| {
            assert_eq!(path, requested);
            assert_eq!(base, Some(project.path()));
            Some(actual_bib.clone())
        });

        assert!(grew);
        let citations = crate::project::resolved_citations(&db);
        assert!(citations.is_defined(&main_path, "present"));
        assert!(citations.is_closed(&main_path));
        assert_eq!(citations.bib_definers(&main_path), &[actual_bib]);

        assert!(!seed_bibliographies_with(
            &mut db,
            &mut lookups,
            |_, _| panic!("a published alias must not be looked up again")
        ));
    }

    fn uri(s: &str) -> Uri {
        s.parse().unwrap()
    }

    #[test]
    fn requested_code_action_kind_admits_descendants_only() {
        assert!(code_action_kind_requested(
            &CodeActionKind::REFACTOR_REWRITE,
            None
        ));
        assert!(code_action_kind_requested(
            &CodeActionKind::REFACTOR_REWRITE,
            Some(std::slice::from_ref(&CodeActionKind::REFACTOR))
        ));
        assert!(!code_action_kind_requested(
            &CodeActionKind::REFACTOR_REWRITE,
            Some(std::slice::from_ref(&CodeActionKind::QUICKFIX))
        ));
    }

    #[test]
    fn disabled_ipc_receiver_stays_pending() {
        let (_ipc, rx) = ipc_channel(false, &EditorSettings::default(), Vec::new());
        assert!(matches!(
            rx.try_recv(),
            Err(crossbeam_channel::TryRecvError::Empty)
        ));
    }

    #[test]
    fn lint_tags_map_dead_and_deprecated_rules() {
        // A dead label definition dims (Unnecessary).
        assert_eq!(
            lint_diagnostic_tags("unreferenced-label"),
            Some(vec![DiagnosticTag::UNNECESSARY])
        );
        // Superseded commands/environments strike through (Deprecated).
        for rule in [
            "deprecated-command",
            "obsolete-environment",
            "primitive-command",
        ] {
            assert_eq!(
                lint_diagnostic_tags(rule),
                Some(vec![DiagnosticTag::DEPRECATED]),
                "rule {rule}"
            );
        }
        // An ordinary lint carries no tag.
        assert_eq!(lint_diagnostic_tags("straight-quotes"), None);
    }

    #[test]
    fn lint_to_lsp_links_documented_rules_only() {
        let d = || crate::linter::Diagnostic {
            rule: "deprecated-command",
            severity: crate::linter::Severity::Warning,
            path: PathBuf::from("x.tex"),
            start: 0,
            end: 3,
            message: "use bfseries".to_owned(),
            fix: None,
            related: Vec::new(),
        };
        let idx = LineIndex::with_encoding("\\bf x", PositionEncoding::Utf16);

        // LaTeX arm (link_docs = true): the code deep-links the rule's reference
        // anchor, and the tag still rides along.
        let latex = lint_to_lsp(&idx, d(), true, Path::new("x.tex"));
        assert_eq!(
            latex.code_description.map(|c| c.href.to_string()),
            Some("https://badness.dev/reference/linter-rules.html#deprecated-command".to_owned())
        );
        assert_eq!(latex.tags, Some(vec![DiagnosticTag::DEPRECATED]));

        // Bib arm (link_docs = false): the rule id is still the `code`, but with
        // no doc link (bib rules aren't catalogued yet).
        let bib = lint_to_lsp(&idx, d(), false, Path::new("x.tex"));
        assert!(bib.code_description.is_none());
        assert_eq!(
            bib.code,
            Some(NumberOrString::String("deprecated-command".to_owned()))
        );
    }

    #[test]
    fn lint_to_lsp_builds_related_information() {
        // A same-file secondary resolves its range against the current index; a
        // cross-file one is a file-level `0..0` link to the other document.
        let text = "\\label{a}\\label{a}\n";
        let idx = LineIndex::with_encoding(text, PositionEncoding::Utf16);
        let d = crate::linter::Diagnostic {
            rule: "duplicate-label",
            severity: crate::linter::Severity::Warning,
            path: PathBuf::from("/p/main.tex"),
            start: 9,
            end: 18,
            message: "label `a` is defined more than once".to_owned(),
            fix: None,
            related: vec![
                crate::linter::RelatedInfo {
                    path: PathBuf::from("/p/main.tex"),
                    start: 7,
                    end: 8,
                    message: "first definition of `a`".to_owned(),
                },
                crate::linter::RelatedInfo {
                    path: PathBuf::from("/p/other.tex"),
                    start: 0,
                    end: 0,
                    message: "other definition of `a`".to_owned(),
                },
            ],
        };
        let lsp = lint_to_lsp(&idx, d, true, Path::new("/p/main.tex"));
        let related = lsp.related_information.expect("related present");
        assert_eq!(related.len(), 2);

        // Same-file: real range (line 0, cols 7..8), self URI.
        assert_eq!(related[0].message, "first definition of `a`");
        assert_eq!(related[0].location.uri, uri("file:///p/main.tex"));
        assert_eq!(related[0].location.range.start, Position::new(0, 7));
        assert_eq!(related[0].location.range.end, Position::new(0, 8));

        // Cross-file: file-level `0..0` at the other document's start.
        assert_eq!(related[1].message, "other definition of `a`");
        assert_eq!(related[1].location.uri, uri("file:///p/other.tex"));
        assert_eq!(related[1].location.range, Range::default());
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
    fn uri_to_fs_path_spells_separators_natively() {
        // `Path` compares by component, so the assertions above pass either way;
        // this pins the *spelling*, which is what a viewer sees in `%f`.
        let path = uri_to_fs_path(&uri("file:///C:/Users/me/main.tex")).expect("a path");
        let expected = if cfg!(windows) {
            "C:\\Users\\me\\main.tex"
        } else {
            "C:/Users/me/main.tex"
        };
        assert_eq!(path.display().to_string(), expected);
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

    /// A ranged `didChange` content change, spelled the way a client sends it.
    fn ranged(start: (u32, u32), end: (u32, u32), text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position::new(start.0, start.1),
                end: Position::new(end.0, end.1),
            }),
            range_length: None,
            text: text.to_owned(),
        }
    }

    /// A range-less content change: the whole-buffer replacement.
    fn whole(text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: text.to_owned(),
        }
    }

    /// Apply `changes` to `old` and assert the reported chain is *exactly* the
    /// transform that happened — the one property `parsed_document`'s replay
    /// leans on (`reparse_edits` rejects a chain landing anywhere else, so a
    /// violation costs a full parse per keystroke and is otherwise invisible).
    ///
    /// Returns the new text and the chain, for the caller's own assertions.
    fn assert_chain_round_trips(
        old: &str,
        encoding: PositionEncoding,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) -> (String, Option<Vec<Edit>>) {
        let mut buffer = Arc::new(TextBuffer::new(old, encoding));
        let edits = apply_content_changes(&mut buffer, changes);
        if let Some(edits) = edits.as_deref() {
            assert_eq!(
                crate::parser::try_apply_edits(old, edits).as_deref(),
                Some(buffer.text()),
                "chain {edits:?} does not reproduce the buffer from {old:?}",
            );
        }
        (buffer.text().to_owned(), edits)
    }

    #[test]
    fn apply_content_changes_splices_ranged_edit() {
        // Replace "world" with "there" in "hello world".
        let (text, edits) = assert_chain_round_trips(
            "hello world\n",
            PositionEncoding::Utf16,
            vec![ranged((0, 6), (0, 11), "there")],
        );
        assert_eq!(text, "hello there\n");
        assert_eq!(
            edits,
            Some(vec![Edit {
                range: 6..11,
                insert: "there".to_owned(),
            }])
        );
    }

    #[test]
    fn apply_content_changes_full_replace_on_no_range() {
        let (text, edits) =
            assert_chain_round_trips("old", PositionEncoding::Utf16, vec![whole("new")]);
        assert_eq!(text, "new");
        // A whole-buffer replacement is an unknown transform: a ~100% window the
        // reparse would decline, so the chain degrades rather than describing it.
        assert_eq!(edits, None);
    }

    #[test]
    fn apply_content_changes_reports_an_insert_and_a_delete() {
        let (text, edits) = assert_chain_round_trips(
            "\\section{Hi}\n",
            PositionEncoding::Utf16,
            vec![ranged((0, 10), (0, 10), "z")],
        );
        assert_eq!(text, "\\section{Hzi}\n");
        assert_eq!(edits.unwrap()[0].insert, "z");

        let (text, edits) = assert_chain_round_trips(
            "\\section{Hzi}\n",
            PositionEncoding::Utf16,
            vec![ranged((0, 10), (0, 11), "")],
        );
        assert_eq!(text, "\\section{Hi}\n");
        assert_eq!(
            edits,
            Some(vec![Edit {
                range: 10..11,
                insert: String::new(),
            }])
        );
    }

    /// Every change after the first is expressed against the text its
    /// predecessors produced — the shape `apply_edits` folds, and the reason the
    /// chain is a `Vec` rather than a single spanning edit.
    #[test]
    fn apply_content_changes_chains_a_multi_change_batch() {
        let (text, edits) = assert_chain_round_trips(
            "ab\n",
            PositionEncoding::Utf16,
            vec![
                ranged((0, 1), (0, 1), "XYZ"),
                // Against "aXYZb\n", not against "ab\n".
                ranged((0, 4), (0, 5), "-"),
            ],
        );
        assert_eq!(text, "aXYZ-\n");
        assert_eq!(
            edits,
            Some(vec![
                Edit {
                    range: 1..1,
                    insert: "XYZ".to_owned(),
                },
                Edit {
                    range: 4..5,
                    insert: "-".to_owned(),
                },
            ])
        );
    }

    /// One unknown transform poisons the whole batch: a chain has to describe the
    /// entire step from the old text to the new one, and the ranged changes after
    /// a full replace are expressed against a text the base never held.
    #[test]
    fn apply_content_changes_degrades_a_batch_mixing_a_full_replace() {
        let (text, edits) = assert_chain_round_trips(
            "old\n",
            PositionEncoding::Utf16,
            vec![whole("new\n"), ranged((0, 3), (0, 3), "!")],
        );
        assert_eq!(text, "new!\n");
        assert_eq!(edits, None);
    }

    #[test]
    fn apply_content_changes_reports_an_empty_batch_as_an_empty_chain() {
        let (text, edits) = assert_chain_round_trips("x\n", PositionEncoding::Utf16, vec![]);
        assert_eq!(text, "x\n");
        // Staging this appends nothing, which is right: nothing happened.
        assert_eq!(edits, Some(Vec::new()));
    }

    /// The chain carries the *clamped* offsets, so it describes the splice that
    /// happened rather than the one a misbehaving client asked for.
    #[test]
    fn apply_content_changes_reports_the_clamped_range() {
        let (text, edits) = assert_chain_round_trips(
            "hello\n",
            PositionEncoding::Utf16,
            vec![ranged((0, 4), (0, 1), "EY")],
        );
        assert_eq!(text, "hEYo\n");
        assert_eq!(
            edits,
            Some(vec![Edit {
                range: 1..4,
                insert: "EY".to_owned(),
            }])
        );
    }

    /// CRLF is the hazard `TODO.md` records panache losing its whole feature to:
    /// nothing here may assume a one-byte line terminator. The `\r` is a line's
    /// content as far as byte offsets go, so a column at the line's end lands
    /// before it.
    #[test]
    fn apply_content_changes_reports_offsets_across_crlf() {
        let (text, edits) = assert_chain_round_trips(
            "ab\r\ncd\r\n",
            PositionEncoding::Utf16,
            vec![ranged((1, 1), (1, 1), "X")],
        );
        assert_eq!(text, "ab\r\ncXd\r\n");
        assert_eq!(
            edits,
            Some(vec![Edit {
                range: 5..5,
                insert: "X".to_owned(),
            }])
        );
    }

    /// `offset_at` is the only thing between the chain and a mid-codepoint slice:
    /// a UTF-16 column counts units, and `\alpha` beside a literal astral char is
    /// ordinary LaTeX. Checked in both negotiated encodings.
    #[test]
    fn apply_content_changes_reports_char_boundary_offsets() {
        for encoding in [PositionEncoding::Utf16, PositionEncoding::Utf8] {
            // "a𝕏b": the astral char is 4 bytes, 2 UTF-16 units.
            let column = if encoding == PositionEncoding::Utf16 {
                3
            } else {
                5
            };
            let (text, edits) = assert_chain_round_trips(
                "a𝕏b\n",
                encoding,
                vec![ranged((0, column), (0, column), "!")],
            );
            assert_eq!(text, "a𝕏!b\n", "{encoding:?}");
            assert_eq!(
                edits,
                Some(vec![Edit {
                    range: 5..5,
                    insert: "!".to_owned(),
                }]),
                "{encoding:?}",
            );
        }
    }

    /// An edit that lands *on* a `\r\n` is where a patched line table is most
    /// likely to go wrong, because the two bytes are one terminator and the edit
    /// reads across the seam: deleting the `\n` leaves a bare `\r` that still
    /// breaks, and both directions change the line count without either byte
    /// moving. `TODO.md` records panache losing this whole feature on
    /// Windows-authored files, so it is worth its own case rather than trusting
    /// the unit oracle.
    ///
    /// Every test in this module is a patch oracle too — tests build in debug, so
    /// `with_replacement`'s `debug_assert` rescans on each splice — but only this
    /// one puts a CRLF under it.
    #[test]
    fn apply_content_changes_edits_a_crlf_terminator() {
        // Deleting a whole `\r\n` joins two lines. The pair can only be addressed
        // as a whole: column 5 is the end of the visible line and `(1, 0)` is the
        // start of the next, so there is no position *between* the `\r` and the
        // `\n` — which is what `offset_at` promises and why the seam always moves
        // as a unit from the client's side.
        let (text, edits) = assert_chain_round_trips(
            "alpha\r\nbeta\r\n",
            PositionEncoding::Utf16,
            vec![ranged((0, 5), (1, 0), "")],
        );
        assert_eq!(text, "alphabeta\r\n");
        assert_eq!(
            edits,
            Some(vec![Edit {
                range: 5..7,
                insert: String::new(),
            }])
        );

        // An insert just before the `\r` leaves the terminator whole.
        let (text, _) = assert_chain_round_trips(
            "alpha\r\nbeta\r\n",
            PositionEncoding::Utf16,
            vec![ranged((0, 5), (0, 5), "X")],
        );
        assert_eq!(text, "alphaX\r\nbeta\r\n");

        // An inserted `\r` in the same place does not: it breaks on its own, so
        // the document gains a line without either byte of the original pair
        // moving. This is the shape a table carrying its boundary verdict across
        // an edit gets wrong.
        let (text, _) = assert_chain_round_trips(
            "alpha\r\nbeta\r\n",
            PositionEncoding::Utf16,
            vec![ranged((0, 5), (0, 5), "\r")],
        );
        assert_eq!(text, "alpha\r\r\nbeta\r\n");

        // A second change resolving against the first: line 1 is only reachable
        // if the patched table shifted, so this fails loudly on a stale one.
        let (text, _) = assert_chain_round_trips(
            "alpha\r\nbeta\r\n",
            PositionEncoding::Utf16,
            vec![
                ranged((0, 5), (0, 5), "\r\nmid"),
                ranged((2, 0), (2, 4), "BETA"),
            ],
        );
        assert_eq!(text, "alpha\r\nmid\r\nBETA\r\n");
    }

    /// An edit beside an astral char that also adds a line: the wide-line flags
    /// have to splice *and* the joined lines have to be re-derived, together. The
    /// positions in the second change are only meaningful if both happened.
    #[test]
    fn apply_content_changes_edits_beside_a_wide_char() {
        let (text, _) = assert_chain_round_trips(
            "a𝕏b\nplain\n",
            PositionEncoding::Utf16,
            vec![
                // After `a𝕏` — one UTF-16 unit for `a`, two for the astral char.
                ranged((0, 3), (0, 3), "\nnew"),
                ranged((2, 0), (2, 5), "PLAIN"),
            ],
        );
        assert_eq!(text, "a𝕏\nnewb\nPLAIN\n");
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

    #[test]
    fn editor_settings_texmf() {
        let value = serde_json::json!({
            "texmf": { "enabled": false, "roots": ["/opt/texmf"], "useKpsewhich": false }
        });
        let s = EditorSettings::from_client_value(&value);
        assert!(!s.texmf.enabled);
        assert!(!s.texmf.use_kpsewhich);
        assert_eq!(s.texmf.roots, vec![PathBuf::from("/opt/texmf")]);
        // Omitted entirely: the defaults (enabled, kpsewhich discovery).
        let s = EditorSettings::from_client_value(&serde_json::json!({ "lineWidth": 80 }));
        assert!(s.texmf.enabled);
        assert!(s.texmf.use_kpsewhich);
        assert!(s.texmf.roots.is_empty());
    }

    /// A bare [`GlobalState`] with the given editor settings and an empty cache, for
    /// exercising [`GlobalState::resolve_settings`].
    fn state_with_editor(editor: EditorSettings) -> GlobalState {
        GlobalState {
            documents: HashMap::new(),
            editor_settings: editor,
            config_cache: HashMap::new(),
            declarations: Arc::new(ResolvedDeclarations::default()),
            supports_pull_diagnostics: false,
            supports_diagnostic_refresh: false,
            supports_dynamic_watchers: false,
            next_request_id: 1,
            position_encoding: PositionEncoding::Utf16,
            workspace_roots: Vec::new(),
        }
    }

    /// A `file://` URI for `main.tex` inside `dir`.
    fn file_uri_in(dir: &Path) -> Uri {
        // Go through `path_to_uri` so the URI is well-formed on Windows too,
        // where `dir.display()` yields `C:\…` (backslashes, no leading slash).
        path_to_uri(&dir.join("main.tex")).expect("file uri")
    }

    #[test]
    fn resolve_settings_prefers_file_config_over_editor() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("badness.toml"),
            "[format]\nline-width = 100\nindent-width = 8\n",
        )
        .expect("write config");
        let mut state = state_with_editor(EditorSettings {
            line_width: Some(40),
            indent_width: Some(3),
            ..Default::default()
        });
        let resolved = state.resolve_settings(&file_uri_in(dir.path()));
        assert!(resolved.config_present);
        assert_eq!(resolved.style.line_width, 100);
        assert_eq!(resolved.style.indent_width, 8);
    }

    #[test]
    fn resolve_settings_falls_back_to_editor_without_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = state_with_editor(EditorSettings {
            line_width: Some(40),
            indent_width: None,
            ..Default::default()
        });
        let resolved = state.resolve_settings(&file_uri_in(dir.path()));
        assert!(!resolved.config_present);
        assert_eq!(resolved.style.line_width, 40);
        // Unset editor knob keeps the built-in default.
        assert_eq!(
            resolved.style.indent_width,
            FormatStyle::default().indent_width
        );
        assert!(resolved.wrap_override.is_none());
    }

    #[test]
    fn resolve_settings_wrap_override_from_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("badness.toml"),
            "[format]\nwrap = \"preserve\"\n",
        )
        .expect("write config");
        let mut state = state_with_editor(EditorSettings::default());
        let resolved = state.resolve_settings(&file_uri_in(dir.path()));
        assert_eq!(resolved.wrap_override, Some(WrapMode::Preserve));
    }

    #[test]
    fn resolve_settings_stable_wrap_from_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("badness.toml"),
            "[format]\nline-width = 100\nwrap = \"stable\"\n",
        )
        .expect("write config");
        let mut state = state_with_editor(EditorSettings::default());
        let resolved = state.resolve_settings(&file_uri_in(dir.path()));
        assert_eq!(resolved.wrap_override, Some(WrapMode::Stable));
    }

    #[test]
    fn resolve_settings_applies_lint_selection() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("badness.toml"),
            "[lint]\nselect = [\"duplicate-label\"]\n",
        )
        .expect("write config");
        let mut state = state_with_editor(EditorSettings::default());
        let rules = state
            .resolve_settings(&file_uri_in(dir.path()))
            .rule_selection();
        assert!(rules.is_active("duplicate-label"));
        assert!(!rules.is_active("deprecated-command"));
        // Parse diagnostics are never filtered out.
        assert!(rules.is_active("parse"));
    }

    #[test]
    fn resolve_settings_builds_exclude_filter_for_sibling_discovery() {
        // The resolved exclude filter is what `Worker::seed_dir` feeds to
        // `collect_lint_files`, so verify it prunes a configured directory while
        // keeping a normal sibling — the whole point of plumbing config into the
        // worker.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("badness.toml"), "exclude = [\"vendor/\"]\n")
            .expect("write config");
        std::fs::write(dir.path().join("main.tex"), "").expect("write main");
        std::fs::create_dir(dir.path().join("vendor")).expect("mkdir vendor");
        std::fs::write(dir.path().join("vendor").join("lib.tex"), "").expect("write lib");

        let mut state = state_with_editor(EditorSettings::default());
        let resolved = state.resolve_settings(&file_uri_in(dir.path()));

        let files =
            collect_lint_files(&[dir.path().to_path_buf()], &resolved.exclude).expect("collect");
        let names: Vec<_> = files
            .iter()
            .map(|(p, _)| p.strip_prefix(dir.path()).unwrap_or(p).to_path_buf())
            .collect();
        assert!(names.contains(&PathBuf::from("main.tex")));
        assert!(
            !names.iter().any(|p| p.starts_with("vendor")),
            "excluded sibling should be pruned, got {names:?}"
        );
    }

    #[test]
    fn resolve_settings_without_config_excludes_nothing() {
        // The editor-fallback path keeps the historical unfiltered walk: a
        // `vendor/` sibling is still discovered when no `badness.toml` governs.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("main.tex"), "").expect("write main");
        std::fs::create_dir(dir.path().join("vendor")).expect("mkdir vendor");
        std::fs::write(dir.path().join("vendor").join("lib.tex"), "").expect("write lib");

        let mut state = state_with_editor(EditorSettings::default());
        let resolved = state.resolve_settings(&file_uri_in(dir.path()));
        assert!(!resolved.config_present);

        let files =
            collect_lint_files(&[dir.path().to_path_buf()], &resolved.exclude).expect("collect");
        let names: Vec<_> = files
            .iter()
            .map(|(p, _)| p.strip_prefix(dir.path()).unwrap_or(p).to_path_buf())
            .collect();
        assert!(names.contains(&PathBuf::from("main.tex")));
        assert!(names.contains(&PathBuf::from("vendor/lib.tex")));
    }

    #[test]
    fn resolve_settings_caches_by_anchor_until_cleared() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = state_with_editor(EditorSettings {
            line_width: Some(40),
            indent_width: None,
            ..Default::default()
        });
        let uri = file_uri_in(dir.path());
        assert_eq!(state.resolve_settings(&uri).style.line_width, 40);
        // A later editor change is masked by the cache until it is cleared (the
        // `didChangeConfiguration` handler clears it).
        state.editor_settings.line_width = Some(72);
        assert_eq!(state.resolve_settings(&uri).style.line_width, 40);
        state.config_cache.clear();
        assert_eq!(state.resolve_settings(&uri).style.line_width, 72);
    }

    #[test]
    fn resolve_settings_detects_nearer_config_creation_and_deletion() {
        let repo = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(repo.path().join(".git")).expect("create git boundary");
        std::fs::write(
            repo.path().join("badness.toml"),
            "[format]\nline-width = 60\n",
        )
        .expect("write root config");
        let nested = repo.path().join("chapters");
        std::fs::create_dir(&nested).expect("create nested dir");
        let uri = file_uri_in(&nested);
        let mut state = state_with_editor(EditorSettings::default());

        assert_eq!(state.resolve_settings(&uri).style.line_width, 60);

        let nearer = nested.join("badness.toml");
        std::fs::write(&nearer, "[format]\nline-width = 40\n").expect("write nearer config");
        assert_eq!(state.resolve_settings(&uri).style.line_width, 40);

        std::fs::remove_file(nearer).expect("remove nearer config");
        assert_eq!(state.resolve_settings(&uri).style.line_width, 60);
    }

    #[test]
    fn resolve_settings_untitled_uses_editor_fallback_uncached() {
        let mut state = state_with_editor(EditorSettings {
            line_width: Some(55),
            indent_width: None,
            ..Default::default()
        });
        let resolved = state.resolve_settings(&uri("untitled:Untitled-1"));
        assert!(!resolved.config_present);
        assert_eq!(resolved.style.line_width, 55);
        // A non-file buffer never joins the anchor-dir cache.
        assert!(state.config_cache.is_empty());
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

    #[test]
    fn package_completion_surfaces_installed_set_and_ctan_detail() {
        use crate::completion::FileArgKind;
        use crate::semantic::signature::SignatureDb;

        // A tree with an installed package that is *not* in the baked CTAN list.
        let tree = tempfile::tempdir().unwrap();
        let sty = tree.path().join("tex/latex/zzlocalpkg/zzlocalpkg.sty");
        std::fs::create_dir_all(sty.parent().unwrap()).unwrap();
        std::fs::write(&sty, "").unwrap();
        let texmf = TexmfIndex::build_from_roots(&[tree.path().to_path_buf()]);

        let sigs = SignatureDb::default();
        let root = SyntaxNode::new_root(parse("").green);
        let model = SemanticModel::build(&root);
        let doc = uri("file:///proj/main.tex");

        // The installed-set tier surfaces the local install; an empty index does not.
        let installed =
            package_completion_items(&doc, "zzlocal", FileArgKind::Package, &sigs, &model, &texmf);
        assert!(installed.iter().any(|i| i.label == "zzlocalpkg"));
        assert!(
            package_completion_items(
                &doc,
                "zzlocal",
                FileArgKind::Package,
                &sigs,
                &model,
                &TexmfIndex::default()
            )
            .is_empty()
        );

        // A baked CTAN name is enriched with its shipped description as `detail`.
        let baked = package_completion_items(
            &doc,
            "amsmath",
            FileArgKind::Package,
            &sigs,
            &model,
            &TexmfIndex::default(),
        );
        let amsmath = baked
            .iter()
            .find(|i| i.label == "amsmath")
            .expect("amsmath from the baked list");
        assert!(
            amsmath.detail.as_deref().is_some_and(|d| d.contains("AMS")),
            "detail: {:?}",
            amsmath.detail
        );
    }
}
