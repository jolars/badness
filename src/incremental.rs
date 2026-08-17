//! Salsa-backed incremental layer: file text → parse tree.
//!
//! The CST is cached as a `rowan::GreenNode` (Arc-backed, `Send + Sync`) rather
//! than a `SyntaxNode` (which holds non-`Send` cursor state and is neither
//! `Eq` nor `salsa::SalsaValue`). Callers materialize a fresh cursor via
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
use crate::declarations::ResolvedDeclarations;
use crate::file_discovery::file_kind_or_tex;
use crate::parser::{
    Edit, LexConfig, ParseCtx, ReparseBase, ReparseTier, SyntaxError,
    parse_with_declarations_resolved, reparse_edits,
};
use crate::project::citations::document_cite_names;
use crate::project::labels::{
    document_glossary_keys, document_label_names, document_ref_names, is_document_root,
};
use crate::project::options::resolved_package_options;
use crate::project::{
    BibTarget, IncludeEdgeKey, PackageEdgeKey, PackageOptionFacts, Project, ProjectMember,
    ResolvedCitations, ResolvedLabels, ResolvedPackageOptions, collect_bib_resource_targets,
    collect_include_edge_keys, collect_package_edge_keys, package_graph, package_option_facts,
    resolved_citations, resolved_labels,
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
    ///
    /// Constructed at [`Durability::HIGH`](salsa::Durability::HIGH) (it is never
    /// `set_`), while `text` keeps the `LOW` default because it mutates per
    /// keystroke. Salsa's per-field revision tracking already keeps a path-only
    /// query from re-running on a text edit; the `HIGH` marking adds the coarse
    /// global short-circuit that starts to matter once a genuinely
    /// rarely-changing input (config, package metadata) is promoted into salsa —
    /// such inputs must likewise be constructed `HIGH`/`MEDIUM`, or every
    /// keystroke's `LOW` write would invalidate them.
    #[returns(ref)]
    pub path: PathBuf,
    /// The file's current text, as a shared handle rather than a `String`.
    ///
    /// A language-server keystroke moves this text through several hands — the
    /// live buffer, the worker job, the salsa cell, every in-flight read job —
    /// and each of them only ever reads it. `Arc<str>` makes all but the first
    /// of those a refcount bump, and gives the staleness guards
    /// ([`text_is_current`](IncrementalDatabase::text_is_current)) a pointer
    /// comparison in front of the `O(N)` content compare.
    #[returns(ref)]
    pub text: Arc<str>,
}

/// The project's [`ResolvedDeclarations`] as a salsa input: the one non-text
/// value the parse is allowed to read (`AGENTS.md` decision #12).
///
/// A **singleton** — one cell per database, not one per file — because a
/// declaration block is a property of the project, and because both readers
/// ([`parsed_document`] and [`scope_signatures`]) want it without threading it
/// through every caller of `parsed_tree_root`. The language server keys its
/// config resolution per anchor directory, so a session holding two workspaces
/// with *different* declaration blocks writes this cell whenever the active
/// document crosses between them; that reparses the world, exactly as editing
/// `badness.toml` does. Declaring nothing is the overwhelmingly common case and
/// costs nothing — [`IncrementalDatabase::set_declarations`] skips the write
/// when the value is unchanged, so an undeclaring session never touches the
/// cell after construction.
///
/// Constructed (and always written) at [`Durability::HIGH`](salsa::Durability::HIGH):
/// left at the `LOW` default, every keystroke's revision bump would invalidate
/// every parse in the database.
#[salsa::input(singleton)]
pub struct DeclarationsInput {
    #[returns(ref)]
    pub declarations: ResolvedDeclarations,
}

/// The project's declarations as seen from inside a query, registering the salsa
/// dependency that makes an edit to `badness.toml` invalidate this parse.
///
/// Free function rather than an [`IncrementalDb`] method so the read stays a
/// plain field read on the singleton input — a trait method returning the value
/// could be implemented without touching salsa at all, and would then silently
/// drop the dependency.
fn declarations_of(db: &dyn IncrementalDb) -> &ResolvedDeclarations {
    DeclarationsInput::get(db).declarations(db)
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
    /// A file's sorted, distinct `\ref`-key set ([`file_refs`]) — the reference
    /// firewall the cross-file label resolver consumes for `unreferenced-label`.
    FileRefs,
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
    /// A `.sty` file's statically-declared option surface
    /// ([`file_package_option_facts`]) — the firewall the cross-file
    /// package-option resolver consumes.
    FilePackageOptionFacts,
    /// The cross-file package-option model
    /// ([`crate::project::options::resolved_package_options`]); a project-level
    /// query, not keyed on a single file.
    ResolvedPackageOptions,
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
/// The `GreenNode` is not `Eq`/`salsa::SalsaValue`, so [`parsed_document`] is
/// `no_eq, unsafe(non_salsa_values)`: salsa never compares parse outputs and
/// relies purely on input (text) change detection to invalidate. That is sound
/// because the tree is a pure function of the text.
#[derive(Debug, Clone)]
pub struct ParsedDocument {
    pub green: rowan::GreenNode,
    pub diagnostics: Vec<ParseDiagnosticData>,
}

/// A cached `.bib` parse: the green tree plus parse diagnostics. The bib analog
/// of [`ParsedDocument`], `no_eq, unsafe(non_salsa_values)` for the identical
/// reason — `rowan::GreenNode` is neither `Eq` nor `salsa::SalsaValue`, so
/// [`parsed_bib_document`] relies purely on text-input change detection to
/// invalidate.
#[derive(Debug, Clone)]
pub struct ParsedBibDocument {
    pub green: rowan::GreenNode,
    pub diagnostics: Vec<ParseDiagnosticData>,
}

/// The previous parse an incremental reparse splices against, plus the inputs it
/// was produced under.
///
/// All four inputs are carried, not just the text, because a base is only usable
/// for a parse that would have used the same ones: `config` is fixed per file by
/// its extension, `declared` changes when `badness.toml` does, and `ctx` is the
/// scanned context the tree was parsed with (a tier that relexes a fragment must
/// use the same one, or a `\newcommand` the scan found makes the fragment's tokens
/// disagree with the tree's).
///
/// `text` is an `Arc<str>` shared with the tracked input and the live editor
/// buffer, so storing a base costs a refcount bump rather than a copy.
#[derive(Debug, Clone)]
pub struct PrevParse {
    pub text: Arc<str>,
    pub green: rowan::GreenNode,
    pub errors: Vec<SyntaxError>,
    pub ctx: ParseCtx,
    pub config: LexConfig,
    pub declared: ResolvedDeclarations,
}

impl PrevParse {
    /// Whether this base already *is* the parse being asked for — same text, same
    /// inputs. The tree can then be handed back whole.
    fn is_current(
        &self,
        text: &Arc<str>,
        config: LexConfig,
        declared: &ResolvedDeclarations,
    ) -> bool {
        self.config == config
            && &self.declared == declared
            // `ptr_eq` is the free half: the language server hands the query the
            // same allocation it wrote. The content compare behind it is what makes
            // the guard correct for an equal-but-distinct text (a disk re-read).
            && (Arc::ptr_eq(&self.text, text) || *self.text == **text)
    }

    /// Borrow this base in the shape the parser's reparse entry points take.
    fn as_reparse_base<'a>(&'a self, declared: &'a ResolvedDeclarations) -> ReparseBase<'a> {
        ReparseBase {
            text: &self.text,
            green: &self.green,
            errors: &self.errors,
            ctx: &self.ctx,
            config: self.config,
            declared,
        }
    }
}

/// How many files may hold a reparse base at once.
const MAX_REPARSE_BASES: usize = 64;
/// How many edits may queue for one file before the chain is abandoned.
const MAX_CHAIN_EDITS: usize = 16;
/// How many inserted bytes may queue for one file before the chain is abandoned.
const MAX_CHAIN_INSERT_BYTES: usize = 64 * 1024;

#[salsa::db]
pub trait IncrementalDb: salsa::Database {
    fn record_query(&self, entry: QueryLogEntry);

    /// The reparse base for `file`, if one is cached.
    ///
    /// This and the four methods below are the **side channel**: mutable state read
    /// from inside an otherwise-pure tracked query. That is sound only because of
    /// the reparse's governing invariant — `parsed_document` returns exactly what
    /// `parse(text)` would whatever this cache holds, so a cold, stale, or evicted
    /// cache costs a full parse and nothing else. Nothing here is a salsa input, and
    /// nothing here may become one: a base that invalidated on write would defeat
    /// the point, and one that did not would be a lie to the dependency graph.
    ///
    /// Default-implemented so a database without a cache simply always full-parses.
    /// That keeps the query total for bare test databases and for any future
    /// non-editor host, without either having to opt out.
    fn reparse_prev(&self, _file: SourceFile) -> Option<Arc<PrevParse>> {
        None
    }

    /// Append `edits` to `file`'s pending chain, or clear it when `None`.
    ///
    /// `None` means "the text changed by a route carrying no edits" — a disk
    /// reload, a whole-buffer replacement, a sweep. Clearing rather than keeping is
    /// what makes the chain self-healing: a chain that no longer describes how the
    /// current text was reached can never describe it again.
    fn reparse_stage_edits(&self, _file: SourceFile, _edits: Option<Vec<Edit>>) {}

    /// Peek at `file`'s pending chain without draining it.
    ///
    /// Peek rather than take, because the query that reads this may be cancelled
    /// before it stores a result; draining here would lose the chain for the retry.
    /// [`reparse_store`](Self::reparse_store) drains the prefix that was consumed.
    fn reparse_pending_edits(&self, _file: SourceFile) -> Vec<Edit> {
        Vec::new()
    }

    /// Install `prev` as `file`'s base and drop the first `consumed` pending edits.
    ///
    /// `tier` says whether a splice actually landed, which is what promotes the
    /// entry to the cache's *hot* class.
    fn reparse_store(
        &self,
        _file: SourceFile,
        _prev: PrevParse,
        _tier: Option<ReparseTier>,
        _consumed: usize,
    ) {
    }

    /// Drop `file`'s base and chain outright.
    ///
    /// For the case where the buffer the base describes is *gone* rather than
    /// merely changed — a `didClose`, a revert to disk.
    fn reparse_evict(&self, _file: SourceFile) {}
}

#[salsa::tracked(returns(ref), no_eq, unsafe(non_salsa_values))]
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
    // The project's declarations seed the parse context, so a declared `\bea`
    // pairs here exactly as it does on the CLI's `parse_with_declarations` path.
    // Reading them registers a `HIGH`-durability dependency: editing
    // `badness.toml` reparses every file, and nothing else does.
    let declared = declarations_of(db);
    let text = file.text(db);

    // The side channel (see `IncrementalDb::reparse_prev`). Everything below is a
    // hint: each branch produces exactly what a full parse of `text` would, so a
    // cold or stale cache costs a parse and nothing else.
    let staged = db.reparse_pending_edits(file);
    let prev = db.reparse_prev(file);

    // The base already *is* this parse. Reached when salsa re-executes after
    // evicting the memo, or when a write set the text back to what it was. Store
    // nothing and leave the chain anchored to the base: its net effect on this text
    // is a no-op, so a later appended edit still describes the whole transform.
    if let Some(prev) = prev
        .as_ref()
        .filter(|prev| prev.is_current(text, config, declared))
    {
        return ParsedDocument {
            green: prev.green.clone(),
            diagnostics: to_diagnostics(&prev.errors),
        };
    }

    // Replay the staged chain. Deliberately the *only* incremental route: there is
    // no whole-text `diff_edit` fallback here, because re-deriving an edit the
    // language server already handed us costs more than the reparse it feeds (fatou
    // measured ~200 us of a ~500 us keystroke at 1 MB). A text that changed by a
    // route carrying no edits is a disk reload or a whole-buffer replace — both
    // shapes a cost guard would decline anyway — so it simply full-parses.
    let reparsed = prev
        .as_ref()
        .and_then(|prev| reparse_edits(&prev.as_reparse_base(declared), &staged, text));

    let tier = reparsed.as_ref().map(|r| r.tier);
    let (green, errors) = match reparsed {
        Some(r) => (r.green, r.errors),
        None => {
            let (parsed, ctx) = parse_with_declarations_resolved(text, config, declared);
            // Carrying the scanned context out of the parse is what lets the next
            // reparse relex a fragment under the same one.
            let (green, errors) = (parsed.green, parsed.errors);
            db.reparse_store(
                file,
                PrevParse {
                    text: text.clone(),
                    green: green.clone(),
                    errors: errors.clone(),
                    ctx,
                    config,
                    declared: declared.clone(),
                },
                tier,
                staged.len(),
            );
            return ParsedDocument {
                diagnostics: to_diagnostics(&errors),
                green,
            };
        }
    };

    // A splice keeps the base's context: the tiers only admit edits that cannot
    // change what the definition scan found, so the context the tree was parsed
    // under is still the one it holds.
    let ctx = prev
        .as_ref()
        .map(|prev| prev.ctx.clone())
        .unwrap_or_default();
    // Stored last, after every fallible step, so a panic or a salsa cancellation
    // can never leave a base whose text and tree disagree. The drain is by
    // *consumed prefix count* rather than "clear all": a stage can land between the
    // peek above and this store, and it must survive.
    db.reparse_store(
        file,
        PrevParse {
            text: text.clone(),
            green: green.clone(),
            errors: errors.clone(),
            ctx,
            config,
            declared: declared.clone(),
        },
        tier,
        staged.len(),
    );

    ParsedDocument {
        green,
        diagnostics: to_diagnostics(&errors),
    }
}

/// The parser's error currency, in the shape the rest of the crate consumes.
fn to_diagnostics(errors: &[SyntaxError]) -> Vec<ParseDiagnosticData> {
    errors
        .iter()
        .map(|error| ParseDiagnosticData {
            message: error.message.clone(),
            start: error.start,
            end: error.end,
        })
        .collect()
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
/// `no_eq` only because its `GreenNode` is neither `Eq` nor `salsa::SalsaValue`, so
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
/// [`document_signatures`] firewall, with the project's declarations
/// ([`DeclarationsInput`]) folded in last.
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
            // Tag each merged name with the package's file stem, so hover can
            // name the defining package (mirrors `semantic::load`).
            match loaded.file_stem().and_then(|s| s.to_str()) {
                Some(origin) => {
                    merged.merge_from_package(document_signatures(db, member), origin);
                }
                None => merged.merge_from(document_signatures(db, member)),
            }
        }
    }
    // The document's own definitions are applied last, so they win over packages
    // (and clear any package origin for a shadowed name).
    merged.merge_from(document_signatures(db, file));
    // Except the project's declarations, the top tier: a declaration is the user
    // explicitly correcting an inference. Same order as the disk-backed
    // `collect_package_signatures`, so the two scope builders cannot disagree.
    merged.merge_declarations(declarations_of(db));
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

/// The file's distinct `\ref`-family key names, sorted — a range-free projection
/// of [`semantic_model`], the reference mirror of [`file_labels`].
///
/// This is the per-file firewall the cross-file
/// [`crate::project::resolved_labels`] resolver consumes for `unreferenced-label`
/// (a label with no reference anywhere in its namespace). Stripping ranges means
/// a prose edit, or a body edit that only shifts a `\ref`'s offset, leaves this
/// `Vec` *equal* — salsa backdates and the project union is not rebuilt. Adding or
/// removing a `\ref` key *does* change it, so the resolution rebuilds (that is
/// exactly the dependency `unreferenced-label` needs). `Eq` for the same firewall
/// reason as [`file_labels`].
#[salsa::tracked(returns(ref))]
pub fn file_refs(db: &dyn IncrementalDb, file: SourceFile) -> Vec<SmolStr> {
    db.record_query(QueryLogEntry {
        kind: QueryKind::FileRefs,
        file: Some(file),
    });
    document_ref_names(semantic_model(db, file))
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

/// A `.sty` file's statically-declared option surface
/// ([`crate::project::package_option_facts`]), or `None` for any other file
/// kind — the per-file firewall the cross-file package-option resolver
/// consumes. `Eq` for the same reason as [`file_labels`]: a body edit that
/// leaves the `\DeclareOption` set and the dynamic-processor signals unchanged
/// backdates, and the project-level model is not rebuilt.
#[salsa::tracked(returns(ref))]
pub fn file_package_option_facts(
    db: &dyn IncrementalDb,
    file: SourceFile,
) -> Option<PackageOptionFacts> {
    db.record_query(QueryLogEntry {
        kind: QueryKind::FilePackageOptionFacts,
        file: Some(file),
    });
    package_option_facts(
        file.path(db),
        &parsed_tree_root(db, file),
        semantic_model(db, file),
    )
}

/// A `.bib` file's cached parse: the green tree plus parse diagnostics. The bib
/// analog of [`parsed_document`].
///
/// `no_eq, unsafe(non_salsa_values)` for the same reason — `GreenNode` is neither
/// `Eq` nor `salsa::SalsaValue`, so salsa never compares parses and relies on
/// text-input change detection. The same [`SourceFile`] input feeds both this and
/// [`parsed_document`]: queries dispatch on the function, not the path, so a
/// buffer's `.bib`-ness is decided by which query the caller runs, not by the
/// input's synthetic extension.
#[salsa::tracked(returns(ref), no_eq, unsafe(non_salsa_values))]
pub fn parsed_bib_document(db: &dyn IncrementalDb, file: SourceFile) -> ParsedBibDocument {
    db.record_query(QueryLogEntry {
        kind: QueryKind::ParsedBibDocument,
        file: Some(file),
    });

    let parsed = crate::bib::parse(file.text(db));
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
#[derive(Debug, Clone, PartialEq, Eq, salsa::SalsaValue)]
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

/// One file's entry in the [`ReparseCache`].
#[derive(Default)]
struct FileReparseState {
    prev: Option<Arc<PrevParse>>,
    pending: Vec<Edit>,
    /// Logical timestamp of the last store, for LRU ordering.
    used: u64,
    /// Whether this entry has ever shown it benefits — an editor staged a real
    /// chain, or a splice actually landed. See [`ReparseCache::evict_if_full`].
    hot: bool,
}

/// Reparse bases and pending edit chains, keyed by file.
///
/// Base and chain live under **one** lock because a store must advance both
/// atomically: the store runs on whichever thread demanded the parse while a stage
/// runs on the language server's worker, and a drain that raced a stage would
/// either lose an edit or keep a stale one.
#[derive(Default)]
struct ReparseCache {
    files: HashMap<SourceFile, FileReparseState>,
    clock: u64,
}

impl ReparseCache {
    fn touch(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Drop entries until at most [`MAX_REPARSE_BASES`] remain, **cold ones first**.
    ///
    /// The class matters because most parses in this database are not editor
    /// keystrokes. A `package_graph` or `scope_signatures` sweep parses every
    /// workspace member, each storing a base it will never hit; under a plain LRU
    /// one project-wide query would cost every open buffer its base and turn the
    /// next keystroke in each of them into a full parse. An entry becomes hot only
    /// by demonstrating use, so a sweep can never evict a buffer being edited.
    fn evict_if_full(&mut self) {
        if self.files.len() <= MAX_REPARSE_BASES {
            return;
        }
        let mut stamps: Vec<(bool, u64, SourceFile)> = self
            .files
            .iter()
            .map(|(&file, state)| (state.hot, state.used, file))
            .collect();
        // Cold before hot, then least-recently-used first. Keyed rather than a bare
        // sort because `SourceFile` is a salsa handle with no `Ord` — and its
        // identity must not decide evictions anyway.
        stamps.sort_unstable_by_key(|&(hot, used, _)| (hot, used));
        let excess = self.files.len() - MAX_REPARSE_BASES;
        for (_, _, file) in stamps.into_iter().take(excess) {
            self.files.remove(&file);
        }
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
    /// The incremental-reparse side channel: previous parses to splice against,
    /// plus the edits staged since. Shared across database clones exactly as the
    /// path map is, so a read job off the worker thread sees the base the worker
    /// stored. Never a salsa input — see [`IncrementalDb::reparse_prev`].
    reparse_cache: Arc<Mutex<ReparseCache>>,
}

impl Default for IncrementalDatabase {
    fn default() -> Self {
        let db = Self {
            storage: salsa::Storage::new(None),
            query_log: Arc::new(Mutex::new(Vec::new())),
            files: Arc::new(Mutex::new(HashMap::new())),
            reparse_cache: Arc::new(Mutex::new(ReparseCache::default())),
        };
        // Create the declarations singleton eagerly, declaring nothing. Every
        // reader goes through `DeclarationsInput::get`, which panics on an
        // uncreated cell; creating it here — the type's only constructor — is
        // what makes that unconditional. A lazy `try_get`-with-fallback would be
        // worse than a panic: the fallback registers no dependency, so a parse
        // taken before the cell existed would never be invalidated by its
        // arrival.
        let _ = DeclarationsInput::builder(ResolvedDeclarations::default())
            .declarations_durability(salsa::Durability::HIGH)
            .new(&db);
        db
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
            reparse_cache: Arc::clone(&self.reparse_cache),
        }
    }
}

impl std::fmt::Debug for IncrementalDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncrementalDatabase")
            .finish_non_exhaustive()
    }
}

/// Recover a mutex guard even when the lock was poisoned by a panic in another
/// thread. The `files` and `query_log` mutexes each guard a plain map/vec that a
/// single access mutates atomically (one `insert`/`get`/`remove`/`push`), so a
/// panic can leave no half-updated invariant behind — taking the inner guard is
/// safe. This keeps a read-pool job that panics while holding one of these locks
/// from cascading into a poisoned mutex that would then kill the writer thread on
/// its next `.lock()` (see the language server's single-writer worker).
fn recover_poison<T>(err: std::sync::PoisonError<T>) -> T {
    err.into_inner()
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
    pub fn add_file(&self, text: impl Into<Arc<str>>) -> SourceFile {
        let n = MEM_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(format!("<mem>/{n}.tex"));
        SourceFile::builder(path, text.into())
            .path_durability(salsa::Durability::HIGH)
            .new(self)
    }

    pub fn set_file_text(&mut self, file: SourceFile, text: impl Into<Arc<str>>) {
        file.set_text(self).to(text.into());
    }

    /// The project's declarations as currently tracked.
    pub fn declarations(&self) -> &ResolvedDeclarations {
        DeclarationsInput::get(self).declarations(self)
    }

    /// Replace the project's declarations, invalidating every parse that read the
    /// old ones. Returns whether the write actually happened.
    ///
    /// Skipped when the value is unchanged, for the same reason
    /// [`upsert_file`](Self::upsert_file) skips an unchanged text: setting an
    /// input bumps the revision unconditionally, and here that would reparse the
    /// whole database on every job the language server dispatches.
    ///
    /// The durability is restated on the write. Salsa would inherit the field's
    /// existing one, so this is a guard rather than a requirement: it keeps the
    /// `HIGH` claim legible at the site that could silently demote it, which
    /// would cost every parse in the database its `HIGH`-durability standing and
    /// with it the global short-circuit that keeps a keystroke from reaching them
    /// at all.
    pub fn set_declarations(&mut self, declared: ResolvedDeclarations) -> bool {
        let input = DeclarationsInput::get(self);
        if input.declarations(self) == &declared {
            return false;
        }
        input
            .set_declarations(self)
            .with_durability(salsa::Durability::HIGH)
            .to(declared);
        true
    }

    /// Insert or update the input for `path`, reusing the existing `SourceFile`
    /// when one is already tracked. The hot path for editor buffers: a keystroke
    /// updates the text of an existing input so unchanged downstream queries stay
    /// cached.
    pub fn upsert_file(&mut self, path: &Path, text: impl Into<Arc<str>>) -> SourceFile {
        let key = normalize_path(path);
        let text = text.into();
        let existing = self
            .files
            .lock()
            .unwrap_or_else(recover_poison)
            .get(&key)
            .copied();
        match existing {
            Some(file) => {
                // Skip the write when the text is unchanged: setting an input
                // unconditionally bumps the revision and would re-run every
                // downstream query (a sibling file re-read on each keystroke).
                // Salsa's setter does no equality check of its own, so this
                // guard is the only thing standing between a redundant upsert
                // and a full reanalysis — hence the `ptr_eq` fast path *in front
                // of* the content compare, never instead of it: the language
                // server hands us the same allocation it already wrote, while a
                // re-read from disk is a fresh one that may still be equal.
                let unchanged = {
                    let tracked = file.text(self);
                    Arc::ptr_eq(tracked, &text) || **tracked == *text
                };
                if !unchanged {
                    file.set_text(self).to(text);
                }
                file
            }
            None => {
                // Store the normalized key as the input's path so `\input`/bib
                // resolution (which joins onto `file.path(db).parent()`) lands in
                // the same normalized space as the member set.
                let file = SourceFile::builder(key.clone(), text)
                    .path_durability(salsa::Durability::HIGH)
                    .new(self);
                self.files
                    .lock()
                    .unwrap_or_else(recover_poison)
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
            .unwrap_or_else(recover_poison)
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
            .unwrap_or_else(recover_poison)
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
        let removed = self
            .files
            .lock()
            .unwrap_or_else(recover_poison)
            .remove(&normalize_path(path));
        // The buffer this file's reparse base described is gone, and a later
        // `didOpen` mints a fresh input anyway, so the entry could never be hit
        // again — it would just occupy a slot until the LRU noticed.
        if let Some(file) = removed {
            self.reparse_evict(file);
        }
        removed
    }

    /// How many files currently hold a reparse base or a pending chain.
    ///
    /// Observability for tests and diagnostics only. Reuse is invisible in a
    /// query's *value* by construction — the whole contract is that a splice and a
    /// full parse agree — so the cache's own state is the only honest observable,
    /// and a test that only checked the value would prove nothing about it.
    pub fn reparse_cache_len(&self) -> usize {
        self.reparse_cache
            .lock()
            .unwrap_or_else(recover_poison)
            .files
            .len()
    }

    /// The text currently tracked for `file`.
    pub fn file_text(&self, file: SourceFile) -> &str {
        file.text(self)
    }

    /// Whether the text currently tracked for `file` *is* `text`.
    ///
    /// The language server asks this of every read job, to decide between the
    /// snapshot's cached parse and a reparse of the buffer it captured. Both
    /// sides of the comparison normally come from the same [`Arc<str>`] the
    /// buffer handed to [`upsert_file`](Self::upsert_file), so the fat-pointer
    /// test settles the common case without touching a byte; the content
    /// compare behind it is what keeps the answer correct for a text that
    /// arrived by another route (a disk re-read, a test).
    pub fn text_is_current(&self, file: SourceFile, text: &str) -> bool {
        let tracked: &str = file.text(self);
        let same_bytes = std::ptr::eq(tracked.as_ptr(), text.as_ptr());
        (same_bytes && tracked.len() == text.len()) || tracked == text
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

    /// The file's distinct, sorted `\ref`-family key names (the reference firewall
    /// feeding the cross-file resolver for `unreferenced-label`).
    pub fn file_refs(&self, file: SourceFile) -> &[SmolStr] {
        file_refs(self, file)
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
        self.query_log.lock().unwrap_or_else(recover_poison).clear();
    }

    pub fn query_log(&self) -> Vec<QueryLogEntry> {
        self.query_log.lock().unwrap_or_else(recover_poison).clone()
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

    /// Whether the text currently tracked for `file` *is* `text` — the read
    /// jobs' "is this snapshot still the buffer I captured?" guard. See
    /// [`IncrementalDatabase::text_is_current`].
    pub fn text_is_current(&self, file: SourceFile, text: &str) -> bool {
        self.0.text_is_current(file, text)
    }

    /// The normalized path `file` is tracked under (its cross-file identity).
    pub fn file_path(&self, file: SourceFile) -> &Path {
        self.0.file_path(file)
    }

    /// Every currently-tracked `(normalized path, input)` pair, sorted by path.
    pub fn tracked_files(&self) -> Vec<(PathBuf, SourceFile)> {
        self.0.tracked_files()
    }

    /// The project's declarations. The cached queries already carry them, so this
    /// is for the read jobs' *fallback* paths — the ones that reparse from the
    /// captured buffer when the snapshot is stale or a write cancels them, and
    /// which would otherwise answer declaration-blind.
    pub fn declarations(&self) -> &ResolvedDeclarations {
        self.0.declarations()
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

    /// Intern `members` as a `Project`, normalizing the key first so an
    /// unchanged membership always re-interns to the same id regardless of the
    /// order the caller assembled it in (see
    /// [`normalize_members`](crate::project::graph::normalize_members)). Every
    /// interning method below routes through this, keeping memo survival correct
    /// by construction.
    fn intern_project(&self, mut members: Vec<ProjectMember>) -> Project<'_> {
        crate::project::graph::normalize_members(&mut members);
        Project::new(&self.0, members)
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
        let project = self.intern_project(members);
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
        let project = self.intern_project(members);
        scope_signatures(&self.0, project, file)
    }

    /// Intern `members` as a `Project` and resolve its package-load graph
    /// ([`package_graph`]): the `\usepackage`/`\documentclass` edges into local
    /// `.sty`/`.cls` members. Name-based references/rename walk it (in both
    /// directions) to extend the macro namespace past the include component.
    /// Borrows the snapshot's storage.
    pub fn package_graph(&self, members: Vec<ProjectMember>) -> &crate::project::PackageGraph {
        let project = self.intern_project(members);
        package_graph(&self.0, project)
    }

    /// Intern `members` as a `Project` and resolve its package-option model
    /// ([`resolved_package_options`]): which options each analyzed `.sty`
    /// member statically declares. The `unknown-option` lint consumes this
    /// through `RuleContext`. Borrows the snapshot's storage.
    pub fn resolve_package_options(&self, members: Vec<ProjectMember>) -> &ResolvedPackageOptions {
        let project = self.intern_project(members);
        resolved_package_options(&self.0, project)
    }
}

#[salsa::db]
impl salsa::Database for IncrementalDatabase {}

#[salsa::db]
impl IncrementalDb for IncrementalDatabase {
    fn record_query(&self, entry: QueryLogEntry) {
        self.query_log
            .lock()
            .unwrap_or_else(recover_poison)
            .push(entry);
    }

    fn reparse_prev(&self, file: SourceFile) -> Option<Arc<PrevParse>> {
        self.reparse_cache
            .lock()
            .unwrap_or_else(recover_poison)
            .files
            .get(&file)
            .and_then(|state| state.prev.clone())
    }

    fn reparse_stage_edits(&self, file: SourceFile, edits: Option<Vec<Edit>>) {
        let mut cache = self.reparse_cache.lock().unwrap_or_else(recover_poison);

        let Some(edits) = edits else {
            // An unknown transform. Clear the chain, and deliberately do *not* mark
            // the entry hot: a sweep and a disk revert look identical from here, and
            // neither is evidence that a splice would pay off. A file with no entry
            // has no chain to clear, and must not gain one: the language server
            // pairs every `upsert_file` with a stage, and most of those writes are
            // project seeding, which would otherwise mint an entry per sibling.
            if let Some(state) = cache.files.get_mut(&file) {
                state.pending.clear();
            }
            return;
        };

        let state = cache.files.entry(file).or_default();
        state.hot = true;
        state.pending.extend(edits);

        // Budget the chain where it is *staged*, not where it is read. Under pull
        // diagnostics the worker stages an edit per keystroke and may demand no
        // parse for a long time, so a read-side budget would grow one edit per
        // keypress. Over budget, the chain is abandoned and the next parse is a full
        // one — the base is still good, only the shortcut is gone.
        let inserted: usize = state.pending.iter().map(|e| e.insert.len()).sum();
        if state.pending.len() > MAX_CHAIN_EDITS || inserted > MAX_CHAIN_INSERT_BYTES {
            state.pending.clear();
        }
    }

    fn reparse_pending_edits(&self, file: SourceFile) -> Vec<Edit> {
        self.reparse_cache
            .lock()
            .unwrap_or_else(recover_poison)
            .files
            .get(&file)
            .map(|state| state.pending.clone())
            .unwrap_or_default()
    }

    fn reparse_store(
        &self,
        file: SourceFile,
        prev: PrevParse,
        tier: Option<ReparseTier>,
        consumed: usize,
    ) {
        let mut cache = self.reparse_cache.lock().unwrap_or_else(recover_poison);
        let used = cache.touch();
        let state = cache.files.entry(file).or_default();

        state.prev = Some(Arc::new(prev));
        state.used = used;
        // A landed splice is the other way an entry earns its hot class.
        state.hot |= tier.is_some();
        // Drain the prefix the caller consumed, unconditionally — including when the
        // chain went unused. A chain kept back because it did not splice is stale
        // forever after: it describes a transform out of a text the base no longer
        // holds, so it would fail to verify on every later parse and poison them all.
        let consumed = consumed.min(state.pending.len());
        state.pending.drain(..consumed);

        cache.evict_if_full();
    }

    fn reparse_evict(&self, file: SourceFile) {
        self.reparse_cache
            .lock()
            .unwrap_or_else(recover_poison)
            .files
            .remove(&file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a base for `text` the way [`parsed_document`]'s full-parse branch does.
    fn base_for(text: &str) -> PrevParse {
        let text: Arc<str> = Arc::from(text);
        let declared = ResolvedDeclarations::default();
        let config = LexConfig::default();
        let (parse, ctx) = parse_with_declarations_resolved(&text, config, &declared);
        PrevParse {
            text,
            green: parse.green,
            errors: parse.errors,
            ctx,
            config,
            declared,
        }
    }

    /// The fast path's predicate. It has to be all three inputs, not just the text:
    /// the same bytes parse differently under a different `badness.toml` or a
    /// different file flavor, so a text-only check would hand back a stale tree the
    /// oracle never sees (this path does not splice, so nothing verifies it).
    #[test]
    fn a_base_is_current_only_for_the_inputs_it_was_parsed_under() {
        let base = base_for("\\section{Hi}\n");
        let declared = ResolvedDeclarations::default();
        let same: Arc<str> = Arc::from("\\section{Hi}\n");
        let other: Arc<str> = Arc::from("\\section{Ho}\n");

        // Equal but distinct allocations still count: a disk re-read is a fresh one.
        assert!(base.is_current(&same, LexConfig::default(), &declared));
        assert!(base.is_current(&base.text.clone(), LexConfig::default(), &declared));

        assert!(!base.is_current(&other, LexConfig::default(), &declared));
        assert!(
            !base.is_current(
                &same,
                LexConfig {
                    flavor: crate::parser::LatexFlavor::Package,
                    dtx: false,
                },
                &declared,
            ),
            "a `.sty` reads `@` as a letter, so the same bytes are a different parse"
        );
    }

    /// Eviction drops cold entries first, so a project-wide sweep cannot cost an
    /// edited buffer its base.
    #[test]
    fn eviction_prefers_cold_entries() {
        let mut db = IncrementalDatabase::default();
        let hot = db.upsert_file(Path::new("hot.tex"), "hot\n".to_owned());
        db.reparse_store(hot, base_for("hot\n"), Some(ReparseTier::Token), 0);

        for n in 0..MAX_REPARSE_BASES + 10 {
            let cold = db.upsert_file(Path::new(&format!("cold{n}.tex")), "cold\n".to_owned());
            db.reparse_store(cold, base_for("cold\n"), None, 0);
        }

        assert!(db.reparse_cache_len() <= MAX_REPARSE_BASES);
        assert!(
            db.reparse_prev(hot).is_some(),
            "the hot entry outlived every cold one"
        );
    }

    /// A store drains only the prefix its caller peeked. A stage that lands between
    /// the peek and the store describes an edit the parse never saw, and clearing
    /// wholesale would drop it.
    #[test]
    fn a_store_drains_only_the_prefix_it_consumed() {
        let mut db = IncrementalDatabase::default();
        let file = db.upsert_file(Path::new("a.tex"), "x\n".to_owned());

        db.reparse_stage_edits(
            file,
            Some(vec![Edit {
                range: 0..0,
                insert: "a".to_string(),
            }]),
        );
        let peeked = db.reparse_pending_edits(file).len();
        // The race: another stage arrives before the store.
        db.reparse_stage_edits(
            file,
            Some(vec![Edit {
                range: 0..0,
                insert: "b".to_string(),
            }]),
        );

        db.reparse_store(file, base_for("ax\n"), None, peeked);
        let left = db.reparse_pending_edits(file);
        assert_eq!(left.len(), 1, "the late stage must survive");
        assert_eq!(left[0].insert, "b");
    }

    /// A panic while holding the `files` lock poisons it, but the database must
    /// keep working afterward: `recover_poison` takes the inner guard instead of
    /// re-panicking, so a read-pool job's crash can't cascade into worker death.
    #[test]
    fn poisoned_files_lock_recovers() {
        let mut db = IncrementalDatabase::default();
        db.upsert_file(Path::new("a.tex"), "before".to_owned());

        // Poison the `files` mutex from another thread by panicking while its
        // guard is held.
        let files = Arc::clone(&db.files);
        let poisoned = std::thread::spawn(move || {
            let _guard = files.lock().expect("first lock is unpoisoned");
            panic!("boom while holding the files lock");
        })
        .join();
        assert!(poisoned.is_err(), "the helper thread should have panicked");
        assert!(db.files.is_poisoned(), "the lock should now be poisoned");

        // Every accessor must still work despite the poison, rather than
        // re-panicking on the `.expect(... poisoned)` the old code used.
        assert!(db.lookup_file(Path::new("a.tex")).is_some());
        db.upsert_file(Path::new("b.tex"), "after".to_owned());
        assert_eq!(db.tracked_files().len(), 2);
    }
}
