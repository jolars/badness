//! Salsa-backed incremental layer: file text → parse tree.
//!
//! The CST is cached as a `rowan::GreenNode` (Arc-backed, `Send + Sync`) rather
//! than a `SyntaxNode` (which holds non-`Send` cursor state and is neither
//! `Eq` nor `salsa::Update`). Callers materialize a fresh cursor via
//! [`parsed_tree_root`] — a cheap atomic clone — so each consumer gets its own
//! tree without leaking the salsa cell.
//!
//! This is the Phase 3 foundation (TODO.md): the salsa harness only. The
//! per-file semantic-model query, the cross-file firewall queries, and the
//! project graph that the sibling project `arity` layers on top of this same
//! harness arrive with later Phase 3 items, once their consumers (linter, LSP)
//! and the `semantic`/`project` modules exist. Keep this file close to arity's
//! `incremental.rs` so the eventual shared-crate extraction stays a mechanical
//! lift.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use salsa::Setter;
use smol_str::SmolStr;

use crate::parser::parse;
use crate::project::labels::{document_label_names, is_document_root};
use crate::project::{IncludeEdgeKey, collect_include_edge_keys};
use crate::semantic::{SemanticModel, SignatureDb, scan_definitions};
use crate::syntax::SyntaxNode;

#[salsa::input]
pub struct SourceFile {
    /// The path this file was tracked under. Set once at creation and never
    /// mutated, so path-keyed queries (which later items will add) don't re-run
    /// on a text edit. In-memory files (see [`IncrementalDatabase::add_file`])
    /// get a unique synthetic path so they never collide.
    #[returns(ref)]
    pub path: PathBuf,
    #[returns(ref)]
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueryKind {
    ParsedDocument,
    /// A file's per-file label/reference model ([`semantic_model`]).
    SemanticModel,
    /// A file's scanned `\newcommand`/`\newenvironment`/xparse signatures
    /// ([`document_signatures`]).
    DocumentSignatures,
    /// A file's range-free inclusion edges ([`include_edges`]).
    IncludeEdges,
    /// A file's sorted, distinct label-name set ([`file_labels`]) — the firewall
    /// the cross-file label resolver consumes.
    FileLabels,
    /// Whether a file is a document root ([`file_is_document_root`]).
    FileIsDocumentRoot,
    /// The cross-file inclusion graph ([`crate::project::project_graph`]); a
    /// project-level query, not keyed on a single file.
    ProjectGraph,
    /// The cross-file label resolution ([`crate::project::resolved_labels`]); a
    /// project-level query, not keyed on a single file.
    ResolvedLabels,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct QueryLogEntry {
    pub kind: QueryKind,
    /// The per-file query subject, or `None` for project-level queries (none
    /// exist yet; the field mirrors arity so later items slot in mechanically).
    pub file: Option<SourceFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseDiagnosticData {
    pub message: String,
    pub start: usize,
    pub end: usize,
}

/// A cached parse: the green tree plus parse diagnostics, computed once per
/// `(db, file)`.
///
/// The `GreenNode` is not `Eq`/`salsa::Update`, so [`parsed_document`] is
/// `no_eq, unsafe(non_update_types)`: salsa never compares parse outputs and
/// relies purely on input (text) change detection to invalidate. That is sound
/// because the tree is a pure function of the text.
#[derive(Debug, Clone)]
pub struct ParsedDocument {
    pub green: rowan::GreenNode,
    pub diagnostics: Vec<ParseDiagnosticData>,
}

#[salsa::db]
pub trait IncrementalDb: salsa::Database {
    fn record_query(&self, entry: QueryLogEntry);
}

#[salsa::tracked(returns(ref), no_eq, unsafe(non_update_types))]
pub fn parsed_document(db: &dyn IncrementalDb, file: SourceFile) -> ParsedDocument {
    db.record_query(QueryLogEntry {
        kind: QueryKind::ParsedDocument,
        file: Some(file),
    });

    let parsed = parse(file.text(db).as_str());
    let diagnostics = parsed
        .errors
        .into_iter()
        .map(|error| ParseDiagnosticData {
            message: error.message,
            start: error.start,
            end: error.end,
        })
        .collect();

    ParsedDocument {
        green: parsed.green,
        diagnostics,
    }
}

/// The parse diagnostics for `file` (empty when the file parses cleanly).
pub fn parse_diagnostics(db: &dyn IncrementalDb, file: SourceFile) -> &[ParseDiagnosticData] {
    &parsed_document(db, file).diagnostics
}

/// Materialize the cached parse for `file` as a fresh `SyntaxNode` cursor.
pub fn parsed_tree_root(db: &dyn IncrementalDb, file: SourceFile) -> SyntaxNode {
    SyntaxNode::new_root(parsed_document(db, file).green.clone())
}

/// The per-file label/reference model, built on the cached parse tree.
///
/// Unlike [`parsed_document`], this query is **not** `no_eq`: [`SemanticModel`]
/// *is* `Eq`, so salsa compares outputs and **backdates** when an edit leaves
/// the model unchanged (e.g. a prose edit that touches no `\label`/`\ref`),
/// keeping any downstream query from re-running. (`parsed_document` must be
/// `no_eq` only because its `GreenNode` is neither `Eq` nor `salsa::Update`, so
/// salsa cannot compare parses and falls back to text-input change detection.)
/// This is the same firewall [`include_edges`] uses; the future cross-file label
/// resolver is its first consumer.
#[salsa::tracked(returns(ref))]
pub fn semantic_model(db: &dyn IncrementalDb, file: SourceFile) -> SemanticModel {
    db.record_query(QueryLogEntry {
        kind: QueryKind::SemanticModel,
        file: Some(file),
    });
    SemanticModel::build(&parsed_tree_root(db, file))
}

/// The file's scanned user-definition signatures — `\newcommand`,
/// `\newenvironment`, and the xparse `\NewDocument…` family
/// ([`crate::semantic::scan_definitions`]) — built on the cached parse tree.
///
/// Like [`semantic_model`] (and unlike [`parsed_document`]) this is **not**
/// `no_eq`: [`SignatureDb`] is `Eq`, so salsa backdates when an edit defines no
/// new command/environment (e.g. a prose or `\ref` edit), keeping completion's
/// consumer from re-running. Its first consumer is the language server's
/// completion request, which unions these scanned names with the built-in DB.
#[salsa::tracked(returns(ref))]
pub fn document_signatures(db: &dyn IncrementalDb, file: SourceFile) -> SignatureDb {
    db.record_query(QueryLogEntry {
        kind: QueryKind::DocumentSignatures,
        file: Some(file),
    });
    scan_definitions(&parsed_tree_root(db, file))
}

/// The file's inclusion edges, range-free
/// ([`crate::project::collect_include_edge_keys`]), as a tracked query. Resolves
/// relative targets against the file's own directory (`path.parent()`); the path
/// is an input field set once, so this re-runs only on a text edit and backdates
/// when the edges are unchanged — the firewall that keeps a body edit from
/// rebuilding the cross-file [`crate::project::project_graph`].
#[salsa::tracked(returns(ref))]
pub fn include_edges(db: &dyn IncrementalDb, file: SourceFile) -> Vec<IncludeEdgeKey> {
    db.record_query(QueryLogEntry {
        kind: QueryKind::IncludeEdges,
        file: Some(file),
    });
    let root = parsed_tree_root(db, file);
    collect_include_edge_keys(&root, file.path(db).parent())
}

/// The file's distinct `\label` names, sorted — a range-free, ref-free
/// projection of [`semantic_model`].
///
/// This is the per-file firewall the cross-file
/// [`crate::project::resolved_labels`] resolver consumes (the LaTeX analog of
/// arity's `file_exports`). Stripping ranges and refs means a prose edit, or a
/// `\ref` edit, or a body edit that shifts a `\label`'s offset, leaves this
/// `Vec` *equal* — salsa backdates and the project-level union is not rebuilt.
/// Unlike [`project_graph`](crate::project::project_graph) it is **not** `no_eq`:
/// `Vec<SmolStr>` is `Eq`, which is exactly what makes the firewall hold (same
/// reasoning as [`semantic_model`]).
#[salsa::tracked(returns(ref))]
pub fn file_labels(db: &dyn IncrementalDb, file: SourceFile) -> Vec<SmolStr> {
    db.record_query(QueryLogEntry {
        kind: QueryKind::FileLabels,
        file: Some(file),
    });
    document_label_names(semantic_model(db, file))
}

/// Whether `file` looks like a document *root* — it carries a `\documentclass`
/// or a `\begin{document}`. The cross-file `undefined-ref` lint only fires
/// inside a namespace that contains a root, so a bare chapter fragment opened
/// alone (whose labels live in the main document) is never flagged.
///
/// A cheap `bool` projection of the parse tree, `Eq` for the same firewall
/// reason as [`file_labels`]: it changes only when a `\documentclass` /
/// `\begin{document}` is added or removed, so ordinary edits backdate.
#[salsa::tracked(returns(ref))]
pub fn file_is_document_root(db: &dyn IncrementalDb, file: SourceFile) -> bool {
    db.record_query(QueryLogEntry {
        kind: QueryKind::FileIsDocumentRoot,
        file: Some(file),
    });
    is_document_root(&parsed_tree_root(db, file))
}

#[salsa::db]
pub struct IncrementalDatabase {
    storage: salsa::Storage<Self>,
    query_log: Arc<Mutex<Vec<QueryLogEntry>>>,
    /// Path → input mapping, so repeated edits to the same path reuse the same
    /// `SourceFile` input (and thus its cached queries) instead of creating a
    /// fresh one each time. Seeds the cross-file project graph (later items).
    files: Arc<Mutex<HashMap<PathBuf, SourceFile>>>,
}

impl Default for IncrementalDatabase {
    fn default() -> Self {
        Self {
            storage: salsa::Storage::new(None),
            query_log: Arc::new(Mutex::new(Vec::new())),
            files: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// Cloning yields a second handle onto the *same* salsa storage (a cheap
/// `Arc`-bump of the shared `Zalsa`, plus the shared path→input map and query
/// log). This is how the language server runs read-only queries off the lint
/// thread: the owner mints a short-lived clone, hands it to a worker, and the
/// clone is dropped promptly. Salsa is single-writer — a clone outstanding when
/// the owner performs a write blocks that write until the clone drops (and trips
/// `salsa::Cancelled` in any read still in flight), so clones must never be held
/// across a write or parked long-term.
impl Clone for IncrementalDatabase {
    fn clone(&self) -> Self {
        Self {
            storage: self.storage.clone(),
            query_log: Arc::clone(&self.query_log),
            files: Arc::clone(&self.files),
        }
    }
}

impl std::fmt::Debug for IncrementalDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncrementalDatabase")
            .finish_non_exhaustive()
    }
}

/// Monotonic counter minting unique synthetic paths for in-memory documents, so
/// two of them never alias in a path-keyed query. Unique-within-process is
/// sufficient; this sidesteps a `uuid` dependency.
static MEM_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

impl IncrementalDatabase {
    /// Track an in-memory document with no on-disk path. Each call mints a
    /// unique synthetic path. Used by tests and one-shot single-file checks; the
    /// LSP/CLI use [`upsert_file`](Self::upsert_file) with the real path.
    pub fn add_file(&self, text: impl Into<String>) -> SourceFile {
        let n = MEM_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(format!("<mem>/{n}.tex"));
        SourceFile::new(self, path, text.into())
    }

    pub fn set_file_text(&mut self, file: SourceFile, text: impl Into<String>) {
        file.set_text(self).to(text.into());
    }

    /// Insert or update the input for `path`, reusing the existing `SourceFile`
    /// when one is already tracked. The hot path for editor buffers: a keystroke
    /// updates the text of an existing input so unchanged downstream queries stay
    /// cached.
    pub fn upsert_file(&mut self, path: &Path, text: String) -> SourceFile {
        let existing = self
            .files
            .lock()
            .expect("file cache mutex poisoned")
            .get(path)
            .copied();
        match existing {
            Some(file) => {
                // Skip the write when the text is unchanged: setting an input
                // unconditionally bumps the revision and would re-run every
                // downstream query (a sibling file re-read on each keystroke).
                if file.text(self) != &text {
                    file.set_text(self).to(text);
                }
                file
            }
            None => {
                let file = SourceFile::new(self, path.to_path_buf(), text);
                self.files
                    .lock()
                    .expect("file cache mutex poisoned")
                    .insert(path.to_path_buf(), file);
                file
            }
        }
    }

    /// The `SourceFile` input currently tracked for `path`, if any. Read-only:
    /// unlike [`upsert_file`](Self::upsert_file) it never inserts, so it is safe
    /// to call on a shared clone (the language server's read path uses it to find
    /// the cached parse for the buffer under the cursor).
    pub fn lookup_file(&self, path: &Path) -> Option<SourceFile> {
        self.files
            .lock()
            .expect("file cache mutex poisoned")
            .get(path)
            .copied()
    }

    /// Stop tracking `path`, returning the `SourceFile` it was mapped to (if
    /// any). Best-effort eviction for the language server's `didClose`: salsa has
    /// no true input delete, so the input cell and its query memos linger in
    /// storage as unreachable garbage; dropping the map entry is what releases the
    /// strong handle and lets a later `didOpen` mint a *fresh* input rather than
    /// reusing the closed one.
    ///
    /// Caveat: a closed file that another open document `\input`s is no longer
    /// resolvable by path until it is reopened. That is acceptable today — there
    /// is no cross-file label resolver yet (see TODO.md), and [`include_edges`]
    /// re-resolves targets from disk.
    pub fn remove_file(&mut self, path: &Path) -> Option<SourceFile> {
        self.files
            .lock()
            .expect("file cache mutex poisoned")
            .remove(path)
    }

    /// The text currently tracked for `file`.
    pub fn file_text(&self, file: SourceFile) -> &str {
        file.text(self)
    }

    /// The path `file` is tracked under.
    pub fn file_path(&self, file: SourceFile) -> &Path {
        file.path(self)
    }

    /// Parse diagnostics for `file` (empty when it parses cleanly).
    pub fn parse_diagnostics(&self, file: SourceFile) -> &[ParseDiagnosticData] {
        parse_diagnostics(self, file)
    }

    /// A fresh `SyntaxNode` over the cached parse tree.
    pub fn parsed_tree(&self, file: SourceFile) -> SyntaxNode {
        parsed_tree_root(self, file)
    }

    /// The file's range-free inclusion edges.
    pub fn include_edges(&self, file: SourceFile) -> &[IncludeEdgeKey] {
        include_edges(self, file)
    }

    /// The file's per-file label/reference model.
    pub fn semantic_model(&self, file: SourceFile) -> &SemanticModel {
        semantic_model(self, file)
    }

    /// The file's scanned user-definition signatures.
    pub fn document_signatures(&self, file: SourceFile) -> &SignatureDb {
        document_signatures(self, file)
    }

    /// The file's distinct, sorted `\label` names (the firewall feeding the
    /// cross-file resolver).
    pub fn file_labels(&self, file: SourceFile) -> &[SmolStr] {
        file_labels(self, file)
    }

    /// Whether `file` carries a `\documentclass` / `\begin{document}`.
    pub fn file_is_document_root(&self, file: SourceFile) -> bool {
        *file_is_document_root(self, file)
    }

    pub fn clear_query_log(&self) {
        self.query_log
            .lock()
            .expect("query log mutex poisoned")
            .clear();
    }

    pub fn query_log(&self) -> Vec<QueryLogEntry> {
        self.query_log
            .lock()
            .expect("query log mutex poisoned")
            .clone()
    }

    /// Mint a read-only [`Analysis`] snapshot: a short-lived db clone wrapped so
    /// callers can only *read*. Drop it promptly — an outstanding clone blocks
    /// the next write (salsa is single-writer; see the [`Clone`] impl).
    pub fn snapshot(&self) -> Analysis {
        Analysis(self.clone())
    }
}

/// A read-only handle onto the incremental database, à la rust-analyzer's
/// `Analysis` (vs. its writer `AnalysisHost`). Wraps a short-lived clone of the
/// worker thread's [`IncrementalDatabase`] and exposes *only* read queries, so a
/// read job cannot call `upsert_file` / salsa setters — the single-writer
/// invariant is encoded in the type system rather than left to convention.
///
/// Handed to the language server's read jobs (formatting, the parse-diagnostics
/// read-phase); the `&mut`-capable [`IncrementalDatabase`] stays private to the
/// worker thread.
pub struct Analysis(IncrementalDatabase);

impl Analysis {
    /// The `SourceFile` input currently tracked for `path`, if any.
    pub fn lookup_file(&self, path: &Path) -> Option<SourceFile> {
        self.0.lookup_file(path)
    }

    /// The text currently tracked for `file`.
    pub fn file_text(&self, file: SourceFile) -> &str {
        self.0.file_text(file)
    }

    /// Parse diagnostics for `file` (empty when it parses cleanly).
    pub fn parse_diagnostics(&self, file: SourceFile) -> &[ParseDiagnosticData] {
        self.0.parse_diagnostics(file)
    }

    /// A fresh `SyntaxNode` over the cached parse tree.
    pub fn parsed_tree(&self, file: SourceFile) -> SyntaxNode {
        self.0.parsed_tree(file)
    }

    /// The file's per-file label/reference model (for lint rules).
    pub fn semantic_model(&self, file: SourceFile) -> &SemanticModel {
        self.0.semantic_model(file)
    }

    /// The file's scanned user-definition signatures (for completion).
    pub fn document_signatures(&self, file: SourceFile) -> &SignatureDb {
        self.0.document_signatures(file)
    }
}

#[salsa::db]
impl salsa::Database for IncrementalDatabase {}

#[salsa::db]
impl IncrementalDb for IncrementalDatabase {
    fn record_query(&self, entry: QueryLogEntry) {
        self.query_log
            .lock()
            .expect("query log mutex poisoned")
            .push(entry);
    }
}
