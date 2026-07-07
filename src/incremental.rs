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
//! project graph that layers on top of this same harness arrive with later
//! Phase 3 items, once their consumers (linter, LSP) and the `semantic`/`project`
//! modules exist.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use salsa::Setter;
use smol_str::SmolStr;

use crate::bib::semantic::Model as BibModel;
use crate::bib::syntax::SyntaxNode as BibSyntaxNode;
use crate::file_discovery::file_kind_or_tex;
use crate::parser::parse_with_flavor;
use crate::project::citations::document_cite_names;
use crate::project::labels::{document_glossary_keys, document_label_names, is_document_root};
use crate::project::{
    BibTarget, IncludeEdgeKey, PackageEdgeKey, Project, ProjectMember, ResolvedCitations,
    ResolvedLabels, collect_bib_resource_targets, collect_include_edge_keys,
    collect_package_edge_keys, package_graph, resolved_citations, resolved_labels,
};
use crate::semantic::{
    DocAssociation, SemanticModel, SignatureDb, doc_associations as build_doc_associations,
    scan_definitions,
};
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
    /// A `.dtx` file's documentation↔code associations ([`doc_associations`]).
    DocAssociations,
    /// A file's range-free inclusion edges ([`include_edges`]).
    IncludeEdges,
    /// A file's range-free package/class load edges ([`package_edges`]).
    PackageEdges,
    /// A file's sorted, distinct label-name set ([`file_labels`]) — the firewall
    /// the cross-file label resolver consumes.
    FileLabels,
    /// A file's sorted, distinct glossary/acronym key set
    /// ([`file_glossary_keys`]) — the firewall glossary key completion consumes.
    FileGlossaryKeys,
    /// Whether a file is a document root ([`file_is_document_root`]).
    FileIsDocumentRoot,
    /// The cross-file inclusion graph ([`crate::project::project_graph`]); a
    /// project-level query, not keyed on a single file.
    ProjectGraph,
    /// The cross-file package-load graph ([`crate::project::package_graph`]); a
    /// project-level query, not keyed on a single file.
    PackageGraph,
    /// A file's merged signature scope — its own definitions plus those of its
    /// transitively loaded local packages ([`scope_signatures`]).
    ScopeSignatures,
    /// The cross-file label resolution ([`crate::project::resolved_labels`]); a
    /// project-level query, not keyed on a single file.
    ResolvedLabels,
    /// A `.bib` file's parse tree ([`parsed_bib_document`]).
    ParsedBibDocument,
    /// A `.bib` file's per-file entry / cite-key / `@string` model
    /// ([`bib_semantic_model`]).
    BibSemanticModel,
    /// A `.bib` file's sorted, distinct cite-key set ([`file_cite_names`]) — the
    /// firewall the cross-file citation resolver consumes.
    FileCiteNames,
    /// A `.tex` file's bibliography-resource targets + `\nocite{*}` flag
    /// ([`file_cite_facts`]) — the per-file citation firewall.
    FileCiteFacts,
    /// The cross-file citation resolution ([`crate::project::resolved_citations`]);
    /// a project-level query, not keyed on a single file.
    ResolvedCitations,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct QueryLogEntry {
    pub kind: QueryKind,
    /// The per-file query subject, or `None` for project-level queries (none
    /// exist yet; the field is reserved so later items slot in mechanically).
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

/// A cached `.bib` parse: the green tree plus parse diagnostics. The bib analog
/// of [`ParsedDocument`], `no_eq, unsafe(non_update_types)` for the identical
/// reason — `rowan::GreenNode` is neither `Eq` nor `salsa::Update`, so
/// [`parsed_bib_document`] relies purely on text-input change detection to
/// invalidate.
#[derive(Debug, Clone)]
pub struct ParsedBibDocument {
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

    // Parse with the config implied by the file's extension: a `.sty`/`.cls` is
    // loaded under an implicit `\makeatletter` (`LatexFlavor::Package`), so `@` is
    // a letter throughout, and a `.dtx` runs the docstrip mode.
    // `file_kind_or_tex` reads only the path name.
    let config = file_kind_or_tex(file.path(db)).lex_config();
    let parsed = parse_with_flavor(file.text(db).as_str(), config);
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

/// The file's merged **signature scope**: the scanned definitions of every package
/// it transitively loads (local `.sty`/`.cls` members of `project`), unioned in
/// load order, with the file's *own* [`document_signatures`] overlaid on top so a
/// document redefinition wins over any package. Built from the cross-file
/// [`package_graph`](crate::project::package_graph) and the per-file
/// [`document_signatures`] firewall.
///
/// Like [`document_signatures`] this is **not** `no_eq`: [`SignatureDb`] is `Eq`,
/// so it backdates when no definition-relevant edit occurred anywhere in the
/// loaded set. Its consumers are the formatter (package-defined arities/verbatim)
/// and completion. A name like `amsmath` with no sibling `amsmath.sty` simply
/// contributes nothing — resolution is local-only.
#[salsa::tracked(returns(ref))]
pub fn scope_signatures<'db>(
    db: &'db dyn IncrementalDb,
    project: Project<'db>,
    file: SourceFile,
) -> SignatureDb {
    db.record_query(QueryLogEntry {
        kind: QueryKind::ScopeSignatures,
        file: Some(file),
    });

    let graph = package_graph(db, project);
    // Map each member's path back to its tracked input, to fetch its scan.
    let by_path: HashMap<&Path, SourceFile> = project
        .members(db)
        .iter()
        .map(|member| (member.path.as_path(), member.file))
        .collect();

    let mut merged = SignatureDb::default();
    for loaded in graph.transitively_loaded(file.path(db)) {
        if let Some(&member) = by_path.get(loaded.as_path()) {
            merged.merge_from(document_signatures(db, member));
        }
    }
    // The document's own definitions are applied last, so they win over packages.
    merged.merge_from(document_signatures(db, file));
    merged
}

/// The file's `.dtx` documentation↔code associations
/// ([`crate::semantic::doc_associations`]) — each documented `macro`/`environment`
/// or `\DescribeMacro`/`\DescribeEnv` paired with the `macrocode` it brackets.
///
/// Like [`semantic_model`] (and unlike [`parsed_document`]) this is **not** `no_eq`:
/// `Vec<DocAssociation>` is `Eq`, so salsa backdates when an edit changes no
/// documented construct. The query runs on any file; a non-`.dtx` source simply
/// carries none of the ltxdoc vocabulary, so the result is empty.
#[salsa::tracked(returns(ref))]
pub fn doc_associations(db: &dyn IncrementalDb, file: SourceFile) -> Vec<DocAssociation> {
    db.record_query(QueryLogEntry {
        kind: QueryKind::DocAssociations,
        file: Some(file),
    });
    build_doc_associations(&parsed_tree_root(db, file))
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

/// The file's package/class load edges, range-free
/// ([`crate::project::collect_package_edge_keys`]), as a tracked query — the
/// load-graph analog of [`include_edges`]. Resolves relative `.sty`/`.cls` targets
/// against the file's own directory; backdates when the load edges are unchanged,
/// the firewall that keeps a body edit from rebuilding
/// [`crate::project::package_graph`].
#[salsa::tracked(returns(ref))]
pub fn package_edges(db: &dyn IncrementalDb, file: SourceFile) -> Vec<PackageEdgeKey> {
    db.record_query(QueryLogEntry {
        kind: QueryKind::PackageEdges,
        file: Some(file),
    });
    let root = parsed_tree_root(db, file);
    collect_package_edge_keys(&root, file.path(db).parent())
}

/// The file's distinct `\label` names, sorted — a range-free, ref-free
/// projection of [`semantic_model`].
///
/// This is the per-file firewall the cross-file
/// [`crate::project::resolved_labels`] resolver consumes. Stripping ranges and
/// refs means a prose edit, or a
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

/// The file's distinct glossary/acronym keys, sorted — a range-free projection
/// of [`semantic_model`], the glossary analog of [`file_labels`].
///
/// The per-file firewall glossary key completion consumes: a prose or `\gls`
/// edit leaves this `Vec` *equal*, so salsa backdates and the completion path's
/// per-member reads stay memoized. Cross-file union needs no dedicated resolver —
/// the namespace is the same include-graph component
/// [`crate::project::resolved_labels`] already computes, so the LSP layer walks
/// `namespace_members` and unions these per-file sets directly.
#[salsa::tracked(returns(ref))]
pub fn file_glossary_keys(db: &dyn IncrementalDb, file: SourceFile) -> Vec<SmolStr> {
    db.record_query(QueryLogEntry {
        kind: QueryKind::FileGlossaryKeys,
        file: Some(file),
    });
    document_glossary_keys(semantic_model(db, file))
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

/// A `.bib` file's cached parse: the green tree plus parse diagnostics. The bib
/// analog of [`parsed_document`].
///
/// `no_eq, unsafe(non_update_types)` for the same reason — `GreenNode` is neither
/// `Eq` nor `salsa::Update`, so salsa never compares parses and relies on
/// text-input change detection. The same [`SourceFile`] input feeds both this and
/// [`parsed_document`]: queries dispatch on the function, not the path, so a
/// buffer's `.bib`-ness is decided by which query the caller runs, not by the
/// input's synthetic extension.
#[salsa::tracked(returns(ref), no_eq, unsafe(non_update_types))]
pub fn parsed_bib_document(db: &dyn IncrementalDb, file: SourceFile) -> ParsedBibDocument {
    db.record_query(QueryLogEntry {
        kind: QueryKind::ParsedBibDocument,
        file: Some(file),
    });

    let parsed = crate::bib::parse(file.text(db).as_str());
    let diagnostics = parsed
        .errors
        .into_iter()
        .map(|error| ParseDiagnosticData {
            message: error.message,
            start: error.start,
            end: error.end,
        })
        .collect();

    ParsedBibDocument {
        green: parsed.green,
        diagnostics,
    }
}

/// The `.bib` parse diagnostics for `file` (empty when it parses cleanly).
pub fn bib_parse_diagnostics(db: &dyn IncrementalDb, file: SourceFile) -> &[ParseDiagnosticData] {
    &parsed_bib_document(db, file).diagnostics
}

/// Materialize the cached `.bib` parse for `file` as a fresh bib `SyntaxNode`.
pub fn parsed_bib_tree_root(db: &dyn IncrementalDb, file: SourceFile) -> BibSyntaxNode {
    BibSyntaxNode::new_root(parsed_bib_document(db, file).green.clone())
}

/// The per-file bib model (entries, `@string` defs/uses), built on the cached
/// `.bib` parse.
///
/// Like [`semantic_model`] and unlike [`parsed_bib_document`] this is **not**
/// `no_eq`: [`crate::bib::semantic::Model`] is `Eq`, so salsa backdates when an
/// edit leaves the model unchanged.
#[salsa::tracked(returns(ref))]
pub fn bib_semantic_model(db: &dyn IncrementalDb, file: SourceFile) -> BibModel {
    db.record_query(QueryLogEntry {
        kind: QueryKind::BibSemanticModel,
        file: Some(file),
    });
    BibModel::build(&parsed_bib_tree_root(db, file))
}

/// A `.bib` file's distinct cite keys, sorted — a range-free projection of
/// [`bib_semantic_model`].
///
/// The per-file firewall the cross-file [`crate::project::resolved_citations`]
/// resolver consumes (the bib analog of [`file_labels`]). Stripping ranges means
/// an edit that shifts a `@entry`'s offset, or touches a field but not a key,
/// leaves this `Vec` *equal* — salsa backdates and the project-level union is not
/// rebuilt. Like [`file_labels`] it is **not** `no_eq`: `Vec<SmolStr>` is `Eq`,
/// which is what makes the firewall hold.
#[salsa::tracked(returns(ref))]
pub fn file_cite_names(db: &dyn IncrementalDb, file: SourceFile) -> Vec<SmolStr> {
    db.record_query(QueryLogEntry {
        kind: QueryKind::FileCiteNames,
        file: Some(file),
    });
    document_cite_names(bib_semantic_model(db, file))
}

/// A `.tex` file's citation facts: its bibliography-resource targets
/// (`\bibliography`/`\addbibresource`) and whether it carries a `\nocite{*}`
/// wildcard. The per-file firewall feeding [`crate::project::resolved_citations`]
/// on the `.tex` side (the document-root flag reuses [`file_is_document_root`]).
///
/// `Eq` for the same firewall reason as [`file_labels`]: a prose or `\cite` edit
/// changes neither the resource targets nor the wildcard, so it backdates and the
/// cross-file resolution memo holds. Resolves relative targets against the file's
/// own directory (`path.parent()`), like [`include_edges`].
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct FileCiteFacts {
    pub bib_targets: Vec<BibTarget>,
    pub nocite_all: bool,
}

#[salsa::tracked(returns(ref))]
pub fn file_cite_facts(db: &dyn IncrementalDb, file: SourceFile) -> FileCiteFacts {
    db.record_query(QueryLogEntry {
        kind: QueryKind::FileCiteFacts,
        file: Some(file),
    });
    let root = parsed_tree_root(db, file);
    FileCiteFacts {
        bib_targets: collect_bib_resource_targets(&root, file.path(db).parent()),
        nocite_all: semantic_model(db, file).has_wildcard_nocite(),
    }
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

/// Lexically normalize `path` for use as a deduplication key: absolutize it
/// (against the current directory, without touching the filesystem) and collapse
/// `.` / `..` segments. Purely textual — no symlink resolution, no existence
/// check — so it is stable for not-yet-saved buffers and never blocks on I/O.
/// `a.tex`, `./a.tex`, and a sibling resolved as `dir/../a.tex` all map to one
/// key, so the language server's `\input`-resolved siblings collapse onto the
/// same input as the buffer the editor opened.
pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir
                if matches!(out.components().next_back(), Some(Component::Normal(_))) =>
            {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
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
        let key = normalize_path(path);
        let existing = self
            .files
            .lock()
            .expect("file cache mutex poisoned")
            .get(&key)
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
                // Store the normalized key as the input's path so `\input`/bib
                // resolution (which joins onto `file.path(db).parent()`) lands in
                // the same normalized space as the member set.
                let file = SourceFile::new(self, key.clone(), text);
                self.files
                    .lock()
                    .expect("file cache mutex poisoned")
                    .insert(key, file);
                file
            }
        }
    }

    /// Every currently-tracked `(normalized path, input)` pair, sorted by path —
    /// the membership snapshot the language server interns a `Project` from.
    pub fn tracked_files(&self) -> Vec<(PathBuf, SourceFile)> {
        let mut files: Vec<(PathBuf, SourceFile)> = self
            .files
            .lock()
            .expect("file cache mutex poisoned")
            .iter()
            .map(|(path, &file)| (path.clone(), file))
            .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        files
    }

    /// The `SourceFile` input currently tracked for `path`, if any. Read-only:
    /// unlike [`upsert_file`](Self::upsert_file) it never inserts, so it is safe
    /// to call on a shared clone (the language server's read path uses it to find
    /// the cached parse for the buffer under the cursor).
    pub fn lookup_file(&self, path: &Path) -> Option<SourceFile> {
        self.files
            .lock()
            .expect("file cache mutex poisoned")
            .get(&normalize_path(path))
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
            .remove(&normalize_path(path))
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

    /// The file's `.dtx` documentation↔code associations.
    pub fn doc_associations(&self, file: SourceFile) -> &[DocAssociation] {
        doc_associations(self, file)
    }

    /// The file's distinct, sorted `\label` names (the firewall feeding the
    /// cross-file resolver).
    pub fn file_labels(&self, file: SourceFile) -> &[SmolStr] {
        file_labels(self, file)
    }

    /// The file's distinct, sorted glossary/acronym keys (the firewall feeding
    /// glossary key completion).
    pub fn file_glossary_keys(&self, file: SourceFile) -> &[SmolStr] {
        file_glossary_keys(self, file)
    }

    /// Whether `file` carries a `\documentclass` / `\begin{document}`.
    pub fn file_is_document_root(&self, file: SourceFile) -> bool {
        *file_is_document_root(self, file)
    }

    /// `.bib` parse diagnostics for `file` (empty when it parses cleanly).
    pub fn bib_parse_diagnostics(&self, file: SourceFile) -> &[ParseDiagnosticData] {
        bib_parse_diagnostics(self, file)
    }

    /// A fresh bib `SyntaxNode` over the cached `.bib` parse tree.
    pub fn parsed_bib_tree(&self, file: SourceFile) -> BibSyntaxNode {
        parsed_bib_tree_root(self, file)
    }

    /// The file's per-file bib model (entries, `@string` defs/uses).
    pub fn bib_semantic_model(&self, file: SourceFile) -> &BibModel {
        bib_semantic_model(self, file)
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

    /// The normalized path `file` is tracked under (its cross-file identity).
    pub fn file_path(&self, file: SourceFile) -> &Path {
        self.0.file_path(file)
    }

    /// Every currently-tracked `(normalized path, input)` pair, sorted by path.
    pub fn tracked_files(&self) -> Vec<(PathBuf, SourceFile)> {
        self.0.tracked_files()
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

    /// Whether `file` carries a `\documentclass` / `\begin{document}` — label
    /// hover's anchor for the aux root (the directory the compiler ran in).
    pub fn file_is_document_root(&self, file: SourceFile) -> bool {
        self.0.file_is_document_root(file)
    }

    /// The file's scanned user-definition signatures (for completion).
    pub fn document_signatures(&self, file: SourceFile) -> &SignatureDb {
        self.0.document_signatures(file)
    }

    /// The file's distinct, sorted glossary/acronym keys (for completion).
    pub fn file_glossary_keys(&self, file: SourceFile) -> &[SmolStr] {
        self.0.file_glossary_keys(file)
    }

    /// `.bib` parse diagnostics for `file` (empty when it parses cleanly).
    pub fn bib_parse_diagnostics(&self, file: SourceFile) -> &[ParseDiagnosticData] {
        self.0.bib_parse_diagnostics(file)
    }

    /// A fresh bib `SyntaxNode` over the cached `.bib` parse tree.
    pub fn parsed_bib_tree(&self, file: SourceFile) -> BibSyntaxNode {
        self.0.parsed_bib_tree(file)
    }

    /// The file's per-file bib model (entries, `@string` defs/uses).
    pub fn bib_semantic_model(&self, file: SourceFile) -> &BibModel {
        self.0.bib_semantic_model(file)
    }

    /// Intern `members` as a `Project` against this snapshot and resolve its
    /// cross-file label and citation models (the inputs the cross-file lint rules
    /// consume). The returned references borrow the snapshot's salsa storage, so
    /// they live as long as this `Analysis`. Interning takes `&db` and is safe on a
    /// read snapshot.
    pub fn resolve_project(
        &self,
        members: Vec<ProjectMember>,
    ) -> (&ResolvedLabels, &ResolvedCitations) {
        let project = Project::new(&self.0, members);
        (
            resolved_labels(&self.0, project),
            resolved_citations(&self.0, project),
        )
    }

    /// Intern `members` as a `Project` and compute `file`'s merged signature scope
    /// ([`scope_signatures`]): its own scanned definitions plus those of every
    /// package it transitively loads from the local member set. The formatter and
    /// completion consume this. Borrows the snapshot's storage.
    pub fn scope_signatures(&self, members: Vec<ProjectMember>, file: SourceFile) -> &SignatureDb {
        let project = Project::new(&self.0, members);
        scope_signatures(&self.0, project, file)
    }

    /// Intern `members` as a `Project` and resolve its package-load graph
    /// ([`package_graph`]): the `\usepackage`/`\documentclass` edges into local
    /// `.sty`/`.cls` members. Name-based references/rename walk it (in both
    /// directions) to extend the macro namespace past the include component.
    /// Borrows the snapshot's storage.
    pub fn package_graph(&self, members: Vec<ProjectMember>) -> &crate::project::PackageGraph {
        let project = Project::new(&self.0, members);
        package_graph(&self.0, project)
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
