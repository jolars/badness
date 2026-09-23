//! Plan file moves against one project snapshot; the client owns filesystem writes.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use lsp_types::{
    DocumentChangeOperation, DocumentChanges, FileOperationFilter, FileOperationPattern,
    FileOperationPatternKind, FileOperationRegistrationOptions,
    OptionalVersionedTextDocumentIdentifier, RenameFile, RenameFileOptions, RenameFilesParams,
    ResourceOp, TextDocumentEdit, WorkspaceFileOperationsServerCapabilities,
};

use super::file_reference::{FileReference, SourceLoad, file_references};
use super::*;
use crate::file_discovery::FileDiscoveryError;
use crate::incremental::normalize_path;

#[derive(Clone, Copy, Default)]
pub(super) struct Capabilities {
    pub resource_rename: bool,
    pub versioned: bool,
    pub will_rename: bool,
    pub did_rename: bool,
}

impl Capabilities {
    pub fn from_params(params: &serde_json::Value) -> Self {
        let workspace = &params["capabilities"]["workspace"];
        Self {
            resource_rename: workspace["workspaceEdit"]["resourceOperations"]
                .as_array()
                .is_some_and(|ops| ops.iter().any(|op| op == "rename")),
            versioned: workspace["workspaceEdit"]["documentChanges"] == true,
            will_rename: workspace["fileOperations"]["willRename"] == true,
            did_rename: workspace["fileOperations"]["didRename"] == true,
        }
    }

    pub fn server(self) -> WorkspaceFileOperationsServerCapabilities {
        let options = || FileOperationRegistrationOptions {
            filters: vec![
                FileOperationFilter {
                    scheme: Some("file".into()),
                    pattern: FileOperationPattern {
                        glob: "**/*.{tex,sty,cls,dtx,ins}".into(),
                        matches: Some(FileOperationPatternKind::File),
                        options: Some(lsp_types::FileOperationPatternOptions {
                            ignore_case: Some(true),
                        }),
                    },
                },
                FileOperationFilter {
                    scheme: Some("file".into()),
                    pattern: FileOperationPattern {
                        glob: "**".into(),
                        matches: Some(FileOperationPatternKind::Folder),
                        options: None,
                    },
                },
            ],
        };
        WorkspaceFileOperationsServerCapabilities {
            will_rename: self.will_rename.then(options),
            did_rename: self.did_rename.then(options),
            ..Default::default()
        }
    }
}

#[derive(Clone)]
pub(super) struct OpenFile {
    uri: Uri,
    path: PathBuf,
    text: Arc<TextBuffer>,
    version: i32,
}

pub(super) struct Context {
    roots: Vec<PathBuf>,
    open: Vec<OpenFile>,
    capabilities: Capabilities,
    encoding: PositionEncoding,
    files: HashMap<PathBuf, Arc<ResolvedDeclarations>>,
    declarations: Arc<ResolvedDeclarations>,
}

pub(super) fn settings_uri(path: &Path) -> Option<Uri> {
    if path.is_dir() {
        path_to_uri(&path.join("__badness_rename__.tex"))
    } else {
        path_to_uri(path)
    }
}

impl Context {
    pub fn capture(state: &mut GlobalState, anchor: &Path) -> Self {
        let settings = settings_uri(anchor)
            .map(|uri| state.resolve_settings(&uri))
            .unwrap_or_else(|| ResolvedSettings::from_editor(&state.editor_settings));
        let roots = if state.workspace_roots.is_empty() {
            anchor
                .parent()
                .filter(|p| p.parent().is_some())
                .map(Path::to_path_buf)
                .into_iter()
                .collect()
        } else {
            state.workspace_roots.clone()
        };
        Self {
            roots: roots
                .into_iter()
                .map(|root| normalize_path(&root))
                .collect(),
            open: open_files(state),
            capabilities: state.file_rename_capabilities,
            encoding: state.position_encoding,
            files: HashMap::new(),
            declarations: settings.declarations,
        }
    }

    fn uri(&self, path: &Path) -> Option<Uri> {
        self.open
            .iter()
            .find(|doc| doc.path == path)
            .map(|doc| doc.uri.clone())
            .or_else(|| path_to_uri(path))
    }

    fn contains(&self, path: &Path) -> bool {
        self.roots
            .iter()
            .any(|root| path.starts_with(root) && !has_symlink_ancestor(path, root))
    }

    fn normalize(&self, path: &Path) -> Result<PathBuf, String> {
        let root = self
            .roots
            .iter()
            .find(|root| path.starts_with(root))
            .map_or(Path::new(""), PathBuf::as_path);
        // `link/..` follows the link before visiting its parent. Collapsing it
        // first would hide the traversal and could select an unrelated file.
        if has_symlink_ancestor(path, root) {
            return Err("File paths must not pass through a symbolic link.".into());
        }
        Ok(normalize_path(path))
    }

    fn discover_files(&mut self, state: &mut GlobalState) -> Result<(), String> {
        let mut paths: HashSet<_> = self.open.iter().map(|doc| doc.path.clone()).collect();
        // A nested config can replace its parent's excludes. Defer exclusion
        // until each source's governing project is known.
        for root in &self.roots {
            let files = collect_lint_files(std::slice::from_ref(root), &ExcludeFilter::none())
                .map_err(|error| match error {
                    FileDiscoveryError::WalkError { path, message } => format!(
                        "Cannot discover workspace files in {}: {message}",
                        path.display()
                    ),
                    FileDiscoveryError::UnsupportedLintFilePath { path } => format!(
                        "Cannot discover workspace files: unsupported root {}.",
                        path.display()
                    ),
                })?;
            paths.extend(files.into_iter().map(|(path, _)| normalize_path(&path)));
        }
        // Workspace roots can contain several projects. Resolve each source's
        // own config before filtering or parsing it, including live buffers.
        self.files = paths
            .into_iter()
            .filter(|path| self.contains(path))
            .filter_map(|path| {
                let settings = state.resolve_settings(&path_to_uri(&path)?);
                (!settings
                    .exclude
                    .with_force_exclude(true)
                    .force_excludes(&path))
                .then_some((path, settings.declarations))
            })
            .collect();
        Ok(())
    }

    pub fn seed(&self, db: &mut IncrementalDatabase) -> Result<(), String> {
        for path in self.files.keys() {
            if self.open.iter().any(|doc| &doc.path == path) {
                continue;
            }
            let text = std::fs::read_to_string(path).map_err(|error| {
                format!("Cannot read workspace source {}: {error}", path.display())
            })?;
            let file = db.upsert_file(path, text);
            db.reparse_stage_edits(file, None);
        }
        Ok(())
    }
}

fn open_files(state: &GlobalState) -> Vec<OpenFile> {
    state
        .documents
        .iter()
        .filter_map(|(uri, doc)| {
            Some(OpenFile {
                uri: uri.clone(),
                path: normalize_path(&uri_to_fs_path(uri)?),
                text: doc.text.clone(),
                version: doc.version,
            })
        })
        .collect()
}

pub(super) enum Operation {
    Prepare {
        path: PathBuf,
        text: Arc<TextBuffer>,
        position: Position,
    },
    Rename {
        path: PathBuf,
        text: Arc<TextBuffer>,
        position: Position,
        new_name: String,
    },
    WillRename(RenameFilesParams),
}

pub(super) struct Job {
    pub id: RequestId,
    pub context: Context,
    pub operation: Operation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Move {
    old: PathBuf,
    new: PathBuf,
    directory: bool,
}

impl Move {
    fn destination_contains(&self, path: &Path) -> bool {
        path == self.new || (self.directory && path.starts_with(&self.new))
    }

    fn apply(&self, path: &Path) -> Option<PathBuf> {
        let tail = strip_existing_prefix(path, &self.old)?;
        if tail.as_os_str().is_empty() {
            Some(self.new.clone())
        } else {
            self.directory.then(|| self.new.join(tail))
        }
    }
}

fn strip_existing_prefix<'a>(path: &'a Path, prefix: &Path) -> Option<&'a Path> {
    if let Ok(tail) = path.strip_prefix(prefix) {
        return Some(tail);
    }
    // A reference may spell an existing path with different casing. Let the
    // filesystem identify the prefix, preserving the authored descendant path.
    // Comparing canonical paths also keeps distinct hard links separate.
    let mut components = path.components();
    let head: PathBuf = components
        .by_ref()
        .take(prefix.components().count())
        .collect();
    (head.canonicalize().ok()? == prefix.canonicalize().ok()?).then_some(components.as_path())
}

fn moved(path: &Path, moves: &[Move]) -> PathBuf {
    moves
        .iter()
        .find_map(|m| m.apply(path))
        .unwrap_or_else(|| path.to_path_buf())
}

pub(super) struct PendingMove {
    movement: Move,
    since: Instant,
}

pub(super) struct Reply {
    id: RequestId,
    open: Vec<OpenFile>,
    expected: Vec<(PathBuf, Arc<TextBuffer>)>,
    result: Result<(serde_json::Value, Vec<Move>), String>,
}

/// Keep discovery off the symbol-rename path and parsing off the writer thread.
pub(super) fn dispatch(snapshot: &Analysis, job: Job, out: &WorkerSender) {
    if !matches!(job.operation, Operation::WillRename(_)) {
        let result = salsa::Cancelled::catch(AssertUnwindSafe(|| {
            let (path, text, position) = match &job.operation {
                Operation::Rename {
                    path,
                    text,
                    position,
                    new_name,
                } => {
                    if let Some(edit) = compute_rename(
                        snapshot,
                        path,
                        text,
                        *position,
                        new_name,
                        job.context.encoding,
                    ) {
                        return Some(serde_json::to_value(edit).unwrap());
                    }
                    (path, text, position)
                }
                Operation::Prepare {
                    path,
                    text,
                    position,
                } => {
                    if let Some((range, placeholder)) =
                        compute_prepare_rename(snapshot, path, text, *position)
                    {
                        return Some(
                            serde_json::to_value(PrepareRenameResponse::RangeWithPlaceholder {
                                range,
                                placeholder,
                            })
                            .unwrap(),
                        );
                    }
                    (path, text, position)
                }
                Operation::WillRename(_) => unreachable!(),
            };
            cursor_reference(snapshot, &job.context, path, text, *position)
                .is_none()
                .then_some(serde_json::Value::Null)
        }));
        match result {
            Ok(Some(result)) => {
                out.respond(Response::new_ok(job.id, result));
                return;
            }
            Err(_) => {
                out.respond(Response::new_err(
                    job.id,
                    ErrorCode::ContentModified as i32,
                    "The project changed; try the rename again.".into(),
                ));
                return;
            }
            Ok(None) => {}
        }
    }
    out.send(Outbound::DiscoverFileRename(Box::new(job)));
}

pub(super) fn discover(
    connection: &Connection,
    state: &mut GlobalState,
    jobs: &Sender<WorkerJob>,
    mut job: Job,
) {
    if open_changed(state, &job.context.open) {
        let _ = connection.sender.send(Message::Response(Response::new_err(
            job.id,
            ErrorCode::ContentModified as i32,
            "The document changed; try the rename again.".into(),
        )));
        return;
    }
    // A newly opened document's overlay must not be overwritten by discovery.
    job.context.open = open_files(state);
    if let Err(message) = job.context.discover_files(state) {
        let _ = connection.sender.send(Message::Response(Response::new_err(
            job.id,
            ErrorCode::RequestFailed as i32,
            message,
        )));
        return;
    }
    // Other requests may publish a different project's singleton declaration
    // block while discovery is making its round trip through the main loop.
    state.publish_resolved_declarations(Arc::clone(&job.context.declarations), jobs);
    let _ = jobs.send(WorkerJob::PlanFileRename(Box::new(job)));
}

pub(super) fn run(snapshot: &Analysis, job: Job, out: &WorkerSender) {
    let Job {
        id,
        context,
        operation,
    } = job;
    let mut expected = Vec::new();
    let result = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        compute(snapshot, &context, operation, &mut expected)
    }))
    .unwrap_or_else(|_| Err("The project changed while planning the rename; try again.".into()));
    out.send(Outbound::FileRename(Box::new(Reply {
        id,
        open: context.open,
        expected,
        result,
    })));
}

fn tree(
    snapshot: &Analysis,
    path: &Path,
    text: &TextBuffer,
    declarations: &ResolvedDeclarations,
) -> SyntaxNode {
    match snapshot.lookup_file(path) {
        Some(file)
            if snapshot.text_is_current(file, text) && snapshot.declarations() == declarations =>
        {
            snapshot.parsed_tree(file)
        }
        _ => SyntaxNode::new_root(
            parse_with_declarations(text, file_kind_or_tex(path).lex_config(), declarations).green,
        ),
    }
}

fn source_file(path: &Path) -> bool {
    crate::file_discovery::lint_file_kind(path).is_some_and(FileKind::is_latex)
}

fn cursor_reference(
    snapshot: &Analysis,
    context: &Context,
    path: &Path,
    text: &TextBuffer,
    position: Position,
) -> Option<FileReference> {
    if !context.capabilities.resource_rename || file_kind_for(path) == FileKind::Bib {
        return None;
    }
    let offset = text
        .line_index()
        .offset_at(position.line, position.character);
    file_references(&tree(snapshot, path, text, &context.declarations))
        .into_iter()
        .find(|r| {
            r.kind == FileArgKind::TexSource && r.path.range.contains(TextSize::from(offset as u32))
        })
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CompilationContext {
    directory: PathBuf,
    new_directory: PathBuf,
    preamble_only: bool,
}

impl CompilationContext {
    fn includes(&self, reference: &FileReference) -> bool {
        !self.preamble_only || reference.in_preamble
    }
}

struct References {
    files: BTreeMap<PathBuf, Vec<FileReference>>,
    contexts: HashMap<PathBuf, BTreeSet<CompilationContext>>,
}

impl References {
    fn bases<'a>(
        &'a self,
        source: &Path,
        reference: &'a FileReference,
    ) -> impl Iterator<Item = &'a Path> {
        self.compilations(source, reference)
            .map(|compilation| compilation.directory.as_path())
    }

    fn compilations<'a>(
        &'a self,
        source: &Path,
        reference: &'a FileReference,
    ) -> impl Iterator<Item = &'a CompilationContext> {
        self.contexts
            .get(source)
            .into_iter()
            .flatten()
            .filter(|compilation| compilation.includes(reference))
    }

    fn collect(snapshot: &Analysis, context: &Context) -> Self {
        let files: BTreeMap<_, _> = snapshot
            .tracked_files()
            .into_iter()
            .filter(|(path, _)| {
                context.files.contains_key(path)
                    && context.contains(path)
                    && file_kind_for(path).is_latex()
            })
            .map(|(path, file)| {
                let root = tree(
                    snapshot,
                    &path,
                    snapshot.file_buffer(file, context.encoding),
                    &context.files[&path],
                );
                (path, file_references(&root))
            })
            .collect();
        let mut references = Self {
            files,
            contexts: HashMap::new(),
        };
        references.collect_contexts(context, &[]);
        references
    }

    fn collect_contexts(&mut self, context: &Context, moves: &[Move]) {
        self.contexts.clear();
        let mut pending = Vec::new();
        for path in self.files.keys() {
            let new_path = moved(path, moves);
            if let (Some(parent), Some(new_parent)) = (path.parent(), new_path.parent()) {
                let compilation = CompilationContext {
                    directory: parent.to_path_buf(),
                    new_directory: new_parent.to_path_buf(),
                    preamble_only: false,
                };
                self.contexts
                    .entry(path.clone())
                    .or_default()
                    .insert(compilation.clone());
                pending.push((path.clone(), compilation));
            }
        }
        // A file may be compiled alone or loaded by several documents. Carry
        // the candidate caller directories through literal source loads, also
        // retaining import directories. Subfiles load only the parent's preamble
        // in the child's context. A rename must agree in every applicable context.
        // Carry both directories because a single-file move changes the caller's
        // compilation base without moving the directory itself.
        while let Some((source, compilation)) = pending.pop() {
            let base = &compilation.directory;
            for reference in &self.files[&source] {
                if reference.source_load == SourceLoad::None || !compilation.includes(reference) {
                    continue;
                }
                let Some(target) = resolve_reference(context, reference, base)
                    .filter(|target| self.files.contains_key(target))
                else {
                    continue;
                };
                let preamble_only =
                    compilation.preamble_only || reference.source_load == SourceLoad::Preamble;
                let inherited = CompilationContext {
                    preamble_only,
                    ..compilation.clone()
                };
                let imported = reference.directory.as_ref().and_then(|_| {
                    let directory = context.normalize(&reference.base(base)).ok()?;
                    Some(CompilationContext {
                        new_directory: moved(&directory, moves),
                        directory,
                        preamble_only,
                    })
                });
                for inherited in std::iter::once(inherited).chain(imported) {
                    // Only existing directories can supply a search base. This
                    // also bounds cycles that grow a relative import prefix.
                    if inherited.directory.is_dir()
                        && self
                            .contexts
                            .entry(target.clone())
                            .or_default()
                            .insert(inherited.clone())
                    {
                        pending.push((target.clone(), inherited.clone()));
                    }
                }
            }
        }
    }
}

fn resolve_reference(context: &Context, reference: &FileReference, base: &Path) -> Option<PathBuf> {
    if !safe_literal(&reference.lookup_path(&reference.path.text))
        || reference
            .directory
            .as_ref()
            .is_some_and(|dir| !dir.text.is_empty() && !safe_literal(&dir.text))
    {
        return None;
    }
    context
        .normalize(&reference.resolve(Some(base), &TexmfIndex::default(), false)?)
        .ok()
}

fn cursor_target(
    snapshot: &Analysis,
    context: &Context,
    references: &References,
    path: &Path,
    text: &TextBuffer,
    position: Position,
) -> Option<(FileReference, PathBuf)> {
    let reference = cursor_reference(snapshot, context, path, text, position)?;
    let source = context.normalize(path).ok()?;
    let mut targets = references
        .bases(&source, &reference)
        .map(|base| resolve_reference(context, &reference, base));
    let target = targets.next()??;
    for other in targets {
        if other.as_ref() != Some(&target) {
            return None;
        }
    }
    (context.contains(&target) && source_file(&target) && !target.is_symlink())
        .then_some((reference, target))
}

fn compute(
    snapshot: &Analysis,
    context: &Context,
    operation: Operation,
    expected: &mut Vec<(PathBuf, Arc<TextBuffer>)>,
) -> Result<(serde_json::Value, Vec<Move>), String> {
    let mut references = References::collect(snapshot, context);
    let (mut moves, cursor) = match operation {
        Operation::Prepare {
            path,
            text,
            position,
        } => {
            let prepared = compute_prepare_rename(snapshot, &path, &text, position).or_else(|| {
                let (reference, _) =
                    cursor_target(snapshot, context, &references, &path, &text, position)?;
                Some((
                    lsp_range(&text.line_index(), reference.path.range),
                    reference.path.text,
                ))
            });
            return Ok((
                serde_json::to_value(prepared.map(|(range, placeholder)| {
                    PrepareRenameResponse::RangeWithPlaceholder { range, placeholder }
                }))
                .unwrap(),
                vec![],
            ));
        }
        Operation::Rename {
            path,
            text,
            position,
            new_name,
        } => {
            let Some((reference, old)) =
                cursor_target(snapshot, context, &references, &path, &text, position)
            else {
                return Ok((serde_json::Value::Null, vec![]));
            };
            if !safe_literal(&new_name) || new_name.trim() != new_name {
                return Err("Enter a literal file path without TeX commands or delimiters.".into());
            }
            let mut name = PathBuf::from(new_name);
            if name.extension().is_none() {
                name.set_extension(if file_kind_for(&old) == FileKind::CodeTex {
                    std::ffi::OsStr::new("code.tex")
                } else {
                    old.extension().unwrap_or_default()
                });
            }
            if name.extension() != old.extension() {
                return Err("File rename must preserve the source file's extension.".into());
            }
            let source = context.normalize(&path)?;
            let mut destinations = references
                .bases(&source, &reference)
                .map(|base| context.normalize(&reference.base(base).join(&name)));
            let new = destinations
                .next()
                .ok_or("The document has no filesystem directory.")??;
            for destination in destinations {
                if destination? != new {
                    return Err(
                        "The new file path is ambiguous across compilation contexts.".into(),
                    );
                }
            }
            (
                vec![Move {
                    old,
                    new,
                    directory: false,
                }],
                true,
            )
        }
        Operation::WillRename(params) => (parse_moves(params)?, false),
    };
    normalize_moves(context, &mut moves)?;
    // A returned proposal can be canceled. Plan each request against the current
    // text; pending moves are only evidence for filesystem reconciliation.
    let moves: Vec<_> = moves.into_iter().filter(|m| m.old != m.new).collect();
    if moves.is_empty() {
        return Ok((serde_json::Value::Null, vec![]));
    }
    validate_moves(context, &moves)?;
    references.collect_contexts(context, &moves);
    let edits = plan_edits(snapshot, context, &references, &moves, expected)?;
    let mut operations = Vec::new();
    for (path, edits) in &edits {
        let open = context.open.iter().find(|doc| &doc.path == path);
        let uri = open
            .map(|doc| doc.uri.clone())
            .or_else(|| path_to_uri(path))
            .ok_or("Cannot encode a file URI.")?;
        operations.push(DocumentChangeOperation::Edit(TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri,
                version: context
                    .capabilities
                    .versioned
                    .then(|| open.map(|doc| doc.version))
                    .flatten(),
            },
            edits: edits.iter().cloned().map(OneOf::Left).collect(),
        }));
    }
    if cursor {
        for movement in &moves {
            operations.push(DocumentChangeOperation::Op(ResourceOp::Rename(
                RenameFile {
                    old_uri: path_to_uri(&movement.old).ok_or("Cannot encode the source URI.")?,
                    new_uri: path_to_uri(&movement.new)
                        .ok_or("Cannot encode the destination URI.")?,
                    options: Some(RenameFileOptions {
                        overwrite: Some(false),
                        ignore_if_exists: Some(false),
                    }),
                    annotation_id: None,
                },
            )));
        }
    }
    let edit = if !cursor && !context.capabilities.versioned {
        WorkspaceEdit {
            changes: Some(
                edits
                    .into_iter()
                    .filter_map(|(p, edits)| Some((context.uri(&p)?, edits)))
                    .collect(),
            ),
            ..Default::default()
        }
    } else {
        WorkspaceEdit {
            document_changes: Some(DocumentChanges::Operations(operations)),
            ..Default::default()
        }
    };
    Ok((serde_json::to_value(edit).unwrap(), moves))
}

fn parse_moves(params: RenameFilesParams) -> Result<Vec<Move>, String> {
    params
        .files
        .into_iter()
        .map(|file| {
            let path = |raw: &str| {
                raw.parse::<Uri>()
                    .ok()
                    .and_then(|uri| uri_to_fs_path(&uri))
                    .ok_or_else(|| "File rename requires file: URIs.".to_owned())
            };
            let old = path(&file.old_uri)?;
            let new = path(&file.new_uri)?;
            let directory = old.is_dir() || (!old.exists() && new.is_dir());
            Ok(Move {
                old,
                new,
                directory,
            })
        })
        .collect()
}

fn normalize_moves(context: &Context, moves: &mut [Move]) -> Result<(), String> {
    for movement in moves {
        movement.old = context.normalize(&movement.old)?;
        movement.new = context.normalize(&movement.new)?;
    }
    Ok(())
}

fn has_symlink_ancestor(path: &Path, root: &Path) -> bool {
    path.ancestors()
        .take_while(|ancestor| *ancestor != root)
        .any(Path::is_symlink)
}

fn validate_moves(context: &Context, moves: &[Move]) -> Result<(), String> {
    for (i, movement) in moves.iter().enumerate() {
        let Move {
            old,
            new,
            directory,
        } = movement;
        let Some(root) = context
            .roots
            .iter()
            .find(|root| old.starts_with(root) && new.starts_with(root) && old != *root)
        else {
            return Err("Move destinations must remain inside the same workspace root.".into());
        };
        if has_symlink_ancestor(old, root) || has_symlink_ancestor(new, root) {
            return Err("File moves must not pass through a symbolic link.".into());
        }
        spelling(new.strip_prefix(root).unwrap())?;
        if !old.exists() || old.is_symlink() {
            return Err("The source must exist and must not be a symbolic link.".into());
        }
        if !directory
            && (!source_file(old)
                || old.extension() != new.extension()
                || file_kind_for(old) != file_kind_for(new))
        {
            return Err("File rename must preserve the LaTeX source file type.".into());
        }
        if new.symlink_metadata().is_ok()
            || context
                .open
                .iter()
                .any(|doc| movement.destination_contains(&doc.path))
        {
            return Err(format!("The destination already exists: {}", new.display()));
        }
        if *directory && new.starts_with(old) {
            return Err("A directory cannot be moved into itself.".into());
        }
        if new.ancestors().skip(1).any(|p| p.exists() && !p.is_dir()) {
            return Err("A destination parent is a file, not a directory.".into());
        }
        if moves[..i].iter().any(|m| {
            [(&m.old, old), (&m.new, new), (&m.old, new), (&m.new, old)]
                .iter()
                .any(|(a, b)| a.starts_with(b) || b.starts_with(a))
        }) {
            return Err("Overlapping or conflicting batch moves are not supported.".into());
        }
    }
    Ok(())
}

fn safe_literal(text: &str) -> bool {
    !text.is_empty()
        // TeX interprets a leading pipe as a shell command, even inside braces.
        && !text.starts_with('|')
        // TeX collapses consecutive spaces before scanning a file argument.
        && !text.contains("  ")
        && !text.chars().any(|c| {
            c.is_control()
                || matches!(
                    c,
                    '\\' | '{' | '}' | '%' | '#' | '$' | '^' | '~' | '&' | ',' | '[' | ']' | '"'
                )
        })
}

fn relative_path(target: &Path, base: &Path) -> PathBuf {
    let a: Vec<_> = target.components().collect();
    let b: Vec<_> = base.components().collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    if common == 0 {
        return target.to_path_buf();
    }
    let mut path = PathBuf::new();
    for _ in common..b.len() {
        path.push("..");
    }
    for component in &a[common..] {
        path.push(component.as_os_str());
    }
    if path.as_os_str().is_empty() {
        path.push(".");
    }
    path
}

fn spelling(path: &Path) -> Result<String, String> {
    let text = path.to_str().ok_or("The new path is not valid Unicode.")?;
    let text = if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.to_owned()
    };
    // LaTeX strips surrounding whitespace when scanning file arguments.
    (safe_literal(&text) && text.trim() == text)
        .then_some(text)
        .ok_or_else(|| "The new path cannot be written as a literal TeX file argument.".into())
}

fn virtual_exists(path: &Path, moves: &[Move]) -> bool {
    for movement in moves {
        if path == movement.new {
            return movement.old.is_file();
        }
        if movement.directory
            && let Ok(tail) = path.strip_prefix(&movement.new)
        {
            return movement.old.join(tail).is_file();
        }
    }
    moved(path, moves) == path && path.is_file()
}

fn resolve_virtual(
    raw: &str,
    base: &Path,
    reference: &FileReference,
    moves: &[Move],
) -> Option<PathBuf> {
    reference
        .candidates(raw)
        .into_iter()
        .map(|p| normalize_path(&base.join(p)))
        .find(|p| virtual_exists(p, moves))
}

fn path_text(
    target: &Path,
    base: &Path,
    original: &str,
    reference: &FileReference,
    moves: &[Move],
) -> Result<String, String> {
    let mut path = if Path::new(original).is_absolute() {
        target.to_path_buf()
    } else {
        relative_path(target, base)
    };
    if reference.appended_extension(original).is_some()
        || Path::new(original).extension().is_none()
        || reference
            .candidates(original)
            .first()
            .is_some_and(|candidate| candidate.extension() != Path::new(original).extension())
    {
        let stem = path.with_extension("");
        if let Ok(text) = spelling(&stem)
            && resolve_virtual(&text, base, reference, moves).as_deref() == Some(target)
        {
            path = stem;
        }
    }
    let text = spelling(&path)?;
    // These loaders remove spaces before looking up the file, so filesystem
    // resolution alone cannot prove that the replacement preserves its target.
    if text.contains(' ') && reference.strips_spaces() {
        return Err(format!(
            "The new path contains spaces, which \\{} cannot preserve.",
            reference.command
        ));
    }
    if resolve_virtual(&text, base, reference, moves).as_deref() != Some(target) {
        return Err(format!(
            "The new path cannot preserve the target of \\{}.",
            reference.command
        ));
    }
    Ok(text)
}

fn plan_edits(
    snapshot: &Analysis,
    context: &Context,
    references: &References,
    moves: &[Move],
    expected: &mut Vec<(PathBuf, Arc<TextBuffer>)>,
) -> Result<BTreeMap<PathBuf, Vec<TextEdit>>, String> {
    let mut changes = BTreeMap::new();
    let mut includes = Vec::new();
    let mut include_only = Vec::new();
    for (source, source_references) in &references.files {
        let file = snapshot.lookup_file(source).expect("indexed source");
        let text = snapshot.file_buffer(file, context.encoding);
        // Even an unedited source can gain references in a newly opened buffer,
        // invalidating the compilation contexts or the safety of moving it.
        expected.push((
            source.clone(),
            Arc::new(TextBuffer::new(text.text_arc(), context.encoding)),
        ));
        let new_source = moved(source, moves);
        let (Some(old_dir), Some(new_dir)) = (source.parent(), new_source.parent()) else {
            continue;
        };
        // Ordinary input/include commands keep TeX's compilation directory;
        // import and subfiles can establish other bases. The source's parent
        // alone cannot prove how a relative argument behaves after a move.
        if old_dir != new_dir
            && source_references.iter().any(|reference| {
                Path::new(&reference.path.text).is_relative()
                    && reference
                        .directory
                        .as_ref()
                        .is_none_or(|directory| Path::new(&directory.text).is_relative())
            })
        {
            return Err(format!(
                "Cannot move {} across directories: the compilation base of its relative file arguments is unknown.",
                source.display()
            ));
        }
        let mut edits = Vec::new();
        for reference in source_references {
            let mut agreed = None;
            for compilation in references.compilations(source, reference) {
                let candidate = reference_edits(context, text, reference, compilation, moves)?;
                if agreed
                    .as_ref()
                    .is_some_and(|previous| *previous != candidate)
                {
                    return Err(format!(
                        "Cannot update a relative file argument in {}: its compilation contexts disagree.",
                        source.display()
                    ));
                }
                agreed = Some(candidate);
            }
            let candidate = agreed.unwrap_or_default();
            let names = match reference.command.as_str() {
                "include" | "subfileinclude" => Some(&mut includes),
                "includeonly" => Some(&mut include_only),
                _ => None,
            };
            if let Some(names) = names {
                let range = lsp_range(&text.line_index(), reference.path.range);
                let name = candidate
                    .iter()
                    .find(|edit| edit.range == range)
                    .map_or(&reference.path.text, |edit| &edit.new_text);
                names.push((source, reference, name.clone()));
            }
            edits.extend(candidate);
        }
        if !edits.is_empty() {
            edits.sort_by_key(|edit| (edit.range.start.line, edit.range.start.character));
            changes.insert(source.clone(), edits);
        }
    }
    // LaTeX compares include-only names by spelling, stripping a trailing .tex
    // but keeping path components such as ./ intact. Check even unresolved
    // entries: a rename can turn a previously skipped chapter into a match.
    for (source, reference, name) in &includes {
        for (only_source, only_reference, only_name) in &include_only {
            let matched =
                include_name(&reference.path.text) == include_name(&only_reference.path.text);
            let matches = include_name(name) == include_name(only_name);
            if matched != matches
                && references.bases(source, reference).any(|base| {
                    references
                        .bases(only_source, only_reference)
                        .any(|only_base| base == only_base)
                })
            {
                return Err(format!(
                    "Cannot rename files: the spelling change would alter \\includeonly membership in {}.",
                    source.display()
                ));
            }
        }
    }
    Ok(changes)
}

fn include_name(text: &str) -> &str {
    text.strip_suffix(".tex").unwrap_or(text)
}

fn reference_edits(
    context: &Context,
    text: &TextBuffer,
    reference: &FileReference,
    compilation: &CompilationContext,
    moves: &[Move],
) -> Result<Vec<TextEdit>, String> {
    let old_dir = &compilation.directory;
    let Some(target) = resolve_reference(context, reference, old_dir) else {
        return Ok(Vec::new());
    };
    let target = moved(&target, moves);
    let new_dir = &compilation.new_directory;
    let mut edits = Vec::new();
    let base = if let Some(directory) = &reference.directory {
        // Import's first argument sets the imported document's base. Preserve
        // it unless that directory itself is being moved.
        let old_base = context.normalize(&reference.base(old_dir))?;
        let base = moved(&old_base, moves);
        if base != context.normalize(&reference.base(new_dir))? {
            let dir = if Path::new(&directory.text).is_absolute() {
                base.clone()
            } else {
                relative_path(&base, new_dir)
            };
            edits.push(TextEdit {
                range: lsp_range(&text.line_index(), directory.range),
                new_text: format!("{}/", spelling(&dir)?),
            });
        }
        base
    } else {
        new_dir.clone()
    };
    if resolve_virtual(&reference.path.text, &base, reference, moves).as_deref()
        != Some(target.as_path())
    {
        let replacement = path_text(&target, &base, &reference.path.text, reference, moves)?;
        if replacement != reference.path.text {
            edits.push(TextEdit {
                range: lsp_range(&text.line_index(), reference.path.range),
                new_text: replacement,
            });
        }
    }
    Ok(edits)
}

fn open_changed(state: &GlobalState, open: &[OpenFile]) -> bool {
    open.iter().any(|before| {
        state.documents.get(&before.uri).is_none_or(|now| {
            now.version != before.version
                || (!Arc::ptr_eq(&now.text, &before.text) && now.text.text() != before.text.text())
        })
    })
}

pub(super) fn deliver(connection: &Connection, state: &mut GlobalState, reply: Reply) {
    let Reply {
        id,
        open,
        expected,
        result,
    } = reply;
    let stale = open_changed(state, &open)
        // Destinations may acquire buffers after planning without receiving any
        // text edits, so neither captured buffers nor expected text cover them.
        || result.as_ref().is_ok_and(|(_, moves)| {
            state.documents.keys().any(|uri| {
                uri_to_fs_path(uri).is_some_and(|path| {
                    let path = normalize_path(&path);
                    moves.iter().any(|movement| movement.destination_contains(&path))
                })
            })
        })
        || expected.iter().any(|(path, text)| {
            state.documents.iter().any(|(uri, doc)| {
                uri_to_fs_path(uri).is_some_and(|p| normalize_path(&p) == *path)
                    && doc.text.text() != text.text()
            })
        });
    let response = if stale {
        Response::new_err(
            id,
            ErrorCode::ContentModified as i32,
            "The document changed while planning the rename; try again.".into(),
        )
    } else {
        match result {
            Ok((value, moves)) => {
                for movement in moves {
                    state
                        .pending_file_moves
                        .retain(|pending| pending.movement.old != movement.old);
                    state.pending_file_moves.push(PendingMove {
                        movement,
                        since: Instant::now(),
                    });
                }
                Response::new_ok(id, value)
            }
            Err(message) => Response::new_err(id, ErrorCode::RequestFailed as i32, message),
        }
    };
    let _ = connection.sender.send(Message::Response(response));
}

pub(super) fn reconcile(
    connection: &Connection,
    state: &mut GlobalState,
    jobs: &Sender<WorkerJob>,
) {
    let mut completed = Vec::new();
    state.pending_file_moves.retain(|pending| {
        if !pending.movement.old.exists() && pending.movement.new.exists() {
            completed.push(pending.movement.clone());
            false
        } else {
            pending.since.elapsed() < Duration::from_secs(300)
        }
    });
    if !completed.is_empty() {
        completed_moves(connection, state, jobs, completed);
    }
}

pub(super) fn did_rename(
    connection: &Connection,
    state: &mut GlobalState,
    jobs: &Sender<WorkerJob>,
    params: RenameFilesParams,
) {
    if let Ok(mut moves) = parse_moves(params)
        && let Some(first) = moves.first()
    {
        let context = Context::capture(state, &first.new);
        if normalize_moves(&context, &mut moves).is_err() {
            return;
        }
        completed_moves(connection, state, jobs, moves);
    }
}

fn completed_moves(
    connection: &Connection,
    state: &mut GlobalState,
    jobs: &Sender<WorkerJob>,
    moves: Vec<Move>,
) {
    state
        .pending_file_moves
        .retain(|pending| !moves.iter().any(|m| m.old == pending.movement.old));
    let mut replacements = Vec::new();
    for uri in state.documents.keys() {
        let Some(path) = uri_to_fs_path(uri).map(|p| normalize_path(&p)) else {
            continue;
        };
        let new = moved(&path, &moves);
        if new != path
            && let Some(new_uri) = path_to_uri(&new)
        {
            replacements.push((uri.clone(), new_uri));
        }
    }
    for (old, new) in replacements {
        if let Some(doc) = state.documents.remove(&old) {
            state.documents.entry(new).or_insert(doc);
            if !state.supports_pull_diagnostics {
                send_diagnostics(connection, old, Vec::new(), None);
            }
        }
    }
    state.config_cache.clear();
    let Some(anchor) = moves.first().map(|movement| movement.new.clone()) else {
        return;
    };
    let mut context = Box::new(Context::capture(state, &anchor));
    if let Err(error) = context.discover_files(state) {
        // The client has already moved the files. Reconcile known files even
        // when discovery cannot refresh the rest of the workspace.
        log::warn!("{error}");
    }
    let _ = jobs.send(WorkerJob::FilesRenamed { moves, context });
}

pub(super) fn update_database(worker: &mut Worker, moves: &[Move], context: &mut Context) {
    let files = worker.db.tracked_files();
    for (old, _) in files {
        let new = moved(&old, moves);
        if old == new {
            continue;
        }
        worker.db.remove_file(&old);
        let text = context
            .open
            .iter()
            .find(|doc| doc.path == new)
            .map(|doc| doc.text.text_arc())
            .or_else(|| std::fs::read_to_string(&new).ok().map(Arc::from));
        if let Some(text) = text {
            let file = worker.db.upsert_file(&new, text);
            worker.db.reparse_stage_edits(file, None);
        }
    }
    worker
        .pending
        .retain(|_, request| moved(&request.path, moves) == request.path);
    worker.seeded_dirs.clear();
    worker.bib_lookups.clear();
    // didClose may have evicted the old input before the rename notification.
    // Discovery must still find its new location, preserving any live overlays.
    if let Err(error) = context.seed(&mut worker.db) {
        log::warn!("{error}");
    }
    worker.out_tx.send(Outbound::RelintAll);
}

pub(super) fn will_rename(
    connection: &Connection,
    state: &mut GlobalState,
    jobs: &Sender<WorkerJob>,
    req: Request,
) {
    let id = req.id.clone();
    let Ok((_, params)) = req.extract::<RenameFilesParams>("workspace/willRenameFiles") else {
        let _ = connection.sender.send(Message::Response(Response::new_err(
            id,
            ErrorCode::InvalidParams as i32,
            "Invalid file rename parameters.".into(),
        )));
        return;
    };
    let Some(anchor) = params
        .files
        .first()
        .and_then(|f| f.old_uri.parse::<Uri>().ok())
        .and_then(|u| uri_to_fs_path(&u))
    else {
        let _ = connection.sender.send(Message::Response(Response::new_ok(
            id,
            serde_json::Value::Null,
        )));
        return;
    };
    let context = Context::capture(state, &anchor);
    let _ = jobs.send(WorkerJob::FileRename(Box::new(Job {
        id,
        context,
        operation: Operation::WillRename(params),
    })));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> GlobalState {
        GlobalState {
            documents: HashMap::new(),
            editor_settings: EditorSettings::default(),
            config_cache: HashMap::new(),
            declarations: Arc::new(ResolvedDeclarations::default()),
            supports_pull_diagnostics: false,
            supports_diagnostic_refresh: false,
            supports_dynamic_watchers: false,
            next_request_id: 1,
            position_encoding: PositionEncoding::Utf16,
            workspace_roots: Vec::new(),
            file_rename_capabilities: Capabilities::default(),
            pending_file_moves: Vec::new(),
        }
    }

    #[test]
    fn discovery_preserves_exclusions_in_other_workspaces() {
        let workspace = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let project = workspace.path().join("docs");
        std::fs::create_dir(&project).unwrap();
        for root in [project.as_path(), other.path()] {
            std::fs::write(root.join("badness.toml"), "exclude = ['/ignored.tex']\n").unwrap();
            std::fs::write(root.join("main.tex"), "\\input{foo}\n").unwrap();
            std::fs::write(root.join("ignored.tex"), "\\input{foo}\n").unwrap();
        }
        let mut state = state();
        state.workspace_roots = vec![workspace.path().to_owned(), other.path().to_owned()];
        let mut context = Context::capture(&mut state, &project.join("main.tex"));
        context.discover_files(&mut state).unwrap();
        context.seed(&mut IncrementalDatabase::default()).unwrap();
        for root in [project.as_path(), other.path()] {
            assert!(context.files.contains_key(&root.join("main.tex")));
            assert!(!context.files.contains_key(&root.join("ignored.tex")));
        }
    }

    #[cfg(unix)]
    #[test]
    fn discovery_preserves_exclusions_through_a_symlinked_workspace_parent() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("real");
        let project = parent.join("workspace/docs");
        std::fs::create_dir_all(project.join("vendor")).unwrap();
        std::fs::write(
            project.join("badness.toml"),
            "exclude = ['/ignored.tex', 'vendor/']\n",
        )
        .unwrap();
        for name in ["main.tex", "ignored.tex"] {
            std::fs::write(project.join(name), "\\input{foo}\n").unwrap();
        }
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&parent, &alias).unwrap();
        let workspace = alias.join("workspace");
        let docs = workspace.join("docs");
        let mut state = state();
        state.workspace_roots.push(workspace);
        for name in ["unsaved.tex", "vendor/unsaved.tex"] {
            state.documents.insert(
                path_to_uri(&docs.join(name)).unwrap(),
                Document {
                    text: Arc::new(TextBuffer::new("\\input{foo}\n", PositionEncoding::Utf16)),
                    version: 1,
                },
            );
        }
        let mut context = Context::capture(&mut state, &docs.join("main.tex"));
        context.discover_files(&mut state).unwrap();
        context.seed(&mut IncrementalDatabase::default()).unwrap();
        assert_eq!(context.files.len(), 2);
        assert!(context.files.contains_key(&docs.join("main.tex")));
        assert!(context.files.contains_key(&docs.join("unsaved.tex")));
    }

    #[test]
    fn discovery_restores_declarations_after_an_interleaved_request() {
        let declared = tempfile::tempdir().unwrap();
        let plain = tempfile::tempdir().unwrap();
        std::fs::write(
            declared.path().join("badness.toml"),
            "[environments.mycode]\nlike = 'lstlisting'\n",
        )
        .unwrap();
        let main = declared.path().join("main.tex");
        let source = "\\input{foo}\n\\begin{mycode}\n\\input{foo}\n\\end{mycode}\n";
        std::fs::write(&main, source).unwrap();
        std::fs::write(declared.path().join("foo.tex"), "text").unwrap();
        let uri = path_to_uri(&main).unwrap();
        let mut state = state();
        state.workspace_roots.push(declared.path().to_owned());
        let (jobs, incoming) = crossbeam_channel::unbounded();
        let mut db = IncrementalDatabase::default();
        let request = Request::new(
            1.into(),
            "workspace/willRenameFiles".into(),
            serde_json::json!({"files":[{
                "oldUri":path_to_uri(&declared.path().join("foo.tex")),
                "newUri":path_to_uri(&declared.path().join("bar.tex"))
            }]}),
        );
        publish_declarations_for_request(&mut state, &request, &jobs);
        let context = Context::capture(&mut state, &main);
        publish_declarations_for_request(
            &mut state,
            &Request::new(
                2.into(),
                "textDocument/documentSymbol".into(),
                serde_json::json!({"textDocument":{"uri":path_to_uri(&plain.path().join("main.tex"))}}),
            ),
            &jobs,
        );
        for job in incoming.try_iter() {
            if let WorkerJob::Declarations { declarations } = job {
                db.set_declarations((*declarations).clone());
            }
        }
        let (connection, _client) = Connection::memory();
        discover(
            &connection,
            &mut state,
            &jobs,
            Job {
                id: request.id,
                context,
                operation: Operation::WillRename(serde_json::from_value(request.params).unwrap()),
            },
        );
        let mut planned = false;
        for job in incoming.try_iter() {
            match job {
                WorkerJob::Declarations { declarations } => {
                    db.set_declarations((*declarations).clone());
                }
                WorkerJob::PlanFileRename(job) => {
                    job.context.seed(&mut db).unwrap();
                    let (edit, _) =
                        compute(&db.snapshot(), &job.context, job.operation, &mut Vec::new())
                            .unwrap();
                    let edits = edit["changes"][uri.as_str()].as_array().unwrap();
                    assert_eq!(edits.len(), 1, "protected examples must remain unchanged");
                    assert_eq!(edits[0]["range"]["start"]["line"], 0);
                    planned = true;
                }
                _ => panic!("unexpected discovery job"),
            }
        }
        assert!(planned);
    }

    #[test]
    fn a_stale_reply_never_offers_edits_or_tracks_a_move() {
        let mut state = state();
        let path = normalize_path(Path::new("main.tex"));
        let uri = path_to_uri(&path).unwrap();
        let before = Arc::new(TextBuffer::new("\\input{foo}", PositionEncoding::Utf16));
        state.documents.insert(
            uri.clone(),
            Document {
                text: before.clone(),
                version: 1,
            },
        );
        let captured = open_files(&state);
        state.documents.insert(
            uri,
            Document {
                text: Arc::new(TextBuffer::new("\\input{other}", PositionEncoding::Utf16)),
                version: 2,
            },
        );
        let (server, client) = Connection::memory();
        deliver(
            &server,
            &mut state,
            Reply {
                id: 1.into(),
                open: captured,
                expected: vec![(path.clone(), before)],
                result: Ok((
                    serde_json::json!({"documentChanges": []}),
                    vec![Move {
                        old: path.clone(),
                        new: path.with_file_name("renamed.tex"),
                        directory: false,
                    }],
                )),
            },
        );
        let Message::Response(response) = client.receiver.recv().unwrap() else {
            panic!("response")
        };
        assert_eq!(response.response_result.unwrap_err().code, -32801);
        assert!(state.pending_file_moves.is_empty());
    }

    #[test]
    fn newly_opened_buffers_are_checked_against_planned_disk_text() {
        for changed in [false, true] {
            let mut state = state();
            let path = normalize_path(Path::new("other.tex"));
            state.documents.insert(
                path_to_uri(&path).unwrap(),
                Document {
                    text: Arc::new(TextBuffer::new(
                        if changed { "unsaved" } else { "disk" },
                        PositionEncoding::Utf16,
                    )),
                    version: 1,
                },
            );
            let (server, client) = Connection::memory();
            deliver(
                &server,
                &mut state,
                Reply {
                    id: 1.into(),
                    open: vec![],
                    expected: vec![(
                        path,
                        Arc::new(TextBuffer::new("disk", PositionEncoding::Utf16)),
                    )],
                    result: Ok((serde_json::json!({"changes": {}}), vec![])),
                },
            );
            let Message::Response(response) = client.receiver.recv().unwrap() else {
                panic!("response")
            };
            assert_eq!(response.response_result.is_err(), changed);
        }
    }

    #[test]
    fn delivery_rechecks_sources_without_planned_edits() {
        for moved_source in [true, false] {
            for changed in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let old = dir.path().join("foo.tex");
                let new = dir.path().join("sub/foo.tex");
                let other = dir.path().join("other.tex");
                for path in [&old, &other] {
                    std::fs::write(path, "plain disk text").unwrap();
                }
                std::fs::write(dir.path().join("shared.tex"), "shared").unwrap();
                let mut state = state();
                state.workspace_roots.push(dir.path().to_owned());
                let mut context = Context::capture(&mut state, &old);
                context.discover_files(&mut state).unwrap();
                let mut db = IncrementalDatabase::default();
                context.seed(&mut db).unwrap();
                let operation = Operation::WillRename(
                    serde_json::from_value(serde_json::json!({"files":[{
                        "oldUri":path_to_uri(&old), "newUri":path_to_uri(&new)
                    }]}))
                    .unwrap(),
                );
                let mut expected = Vec::new();
                let result = compute(&db.snapshot(), &context, operation, &mut expected).unwrap();
                assert_eq!(result.0, serde_json::json!({"changes": {}}));
                assert_eq!(result.1.len(), 1);
                let opened = if moved_source { &old } else { &other };
                let text = if !changed {
                    "plain disk text"
                } else if moved_source {
                    "\\input{shared}"
                } else {
                    "\\input{foo}"
                };
                let (server, client) = Connection::memory();
                let (jobs, _incoming) = crossbeam_channel::unbounded();
                on_notification(
                    &server,
                    &mut state,
                    &jobs,
                    Notification::new(
                        "textDocument/didOpen".into(),
                        serde_json::json!({"textDocument": {
                            "uri":path_to_uri(opened), "languageId":"latex", "version":1,
                            "text":text
                        }}),
                    ),
                );
                deliver(
                    &server,
                    &mut state,
                    Reply {
                        id: 1.into(),
                        open: context.open,
                        expected,
                        result: Ok(result),
                    },
                );
                let Message::Response(response) = client.receiver.recv().unwrap() else {
                    panic!("response")
                };
                if changed {
                    assert_eq!(response.response_result.unwrap_err().code, -32801);
                    assert!(state.pending_file_moves.is_empty());
                } else {
                    assert!(response.response_result.is_ok());
                    assert_eq!(state.pending_file_moves.len(), 1);
                }
            }
        }
    }

    #[test]
    fn delivery_rechecks_newly_opened_destinations() {
        for (cursor, directory) in [(true, false), (false, false), (false, true)] {
            for occupied in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let main = dir.path().join("main.tex");
                let old = dir.path().join(if directory { "foo" } else { "foo.tex" });
                let new = dir.path().join(if directory { "bar" } else { "bar.tex" });
                let source = if directory {
                    "\\input{foo/child}\n"
                } else {
                    "\\input{foo}\n"
                };
                std::fs::write(&main, source).unwrap();
                if directory {
                    std::fs::create_dir(&old).unwrap();
                    std::fs::write(old.join("child.tex"), "source").unwrap();
                } else {
                    std::fs::write(&old, "source").unwrap();
                }
                let mut state = state();
                state.workspace_roots.push(dir.path().to_owned());
                state.file_rename_capabilities.resource_rename = true;
                let mut context = Context::capture(&mut state, &main);
                context.discover_files(&mut state).unwrap();
                let mut db = IncrementalDatabase::default();
                context.seed(&mut db).unwrap();
                let operation = if cursor {
                    Operation::Rename {
                        path: main,
                        text: Arc::new(TextBuffer::new(source, PositionEncoding::Utf16)),
                        position: Position::new(0, 8),
                        new_name: "bar".into(),
                    }
                } else {
                    Operation::WillRename(
                        serde_json::from_value(serde_json::json!({"files":[{
                            "oldUri":path_to_uri(&old), "newUri":path_to_uri(&new)
                        }]}))
                        .unwrap(),
                    )
                };
                let mut expected = Vec::new();
                let result = compute(&db.snapshot(), &context, operation, &mut expected).unwrap();
                assert_eq!(result.1.len(), 1);
                let opened = if !occupied {
                    dir.path().join("bar-other.tex")
                } else if directory {
                    new.join("unsaved.tex")
                } else {
                    new
                };
                let (server, client) = Connection::memory();
                let (jobs, _incoming) = crossbeam_channel::unbounded();
                on_notification(
                    &server,
                    &mut state,
                    &jobs,
                    Notification::new(
                        "textDocument/didOpen".into(),
                        serde_json::json!({
                            "textDocument": {
                                "uri":path_to_uri(&opened), "languageId":"latex", "version":1,
                                "text":"unsaved destination"
                            }
                        }),
                    ),
                );
                deliver(
                    &server,
                    &mut state,
                    Reply {
                        id: 1.into(),
                        open: context.open,
                        expected,
                        result: Ok(result),
                    },
                );
                let Message::Response(response) = client.receiver.recv().unwrap() else {
                    panic!("response")
                };
                if occupied {
                    assert_eq!(response.response_result.unwrap_err().code, -32801);
                    assert!(state.pending_file_moves.is_empty());
                } else {
                    assert!(response.response_result.is_ok());
                    assert_eq!(state.pending_file_moves.len(), 1);
                }
            }
        }
    }

    #[test]
    fn move_mapping_matches_existing_prefix_casing() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("chapters");
        std::fs::create_dir(&old).unwrap();
        let alternate = dir.path().join("Chapters");
        let movement = Move {
            old,
            new: dir.path().join("appendices"),
            directory: true,
        };
        let path = alternate.join("nested/Unwritten.tex");
        assert_eq!(
            movement.apply(&path),
            alternate
                .is_dir()
                .then(|| movement.new.join("nested/Unwritten.tex"))
        );
    }

    #[test]
    fn move_mapping_does_not_follow_hard_links() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("chapter.tex");
        let alias = dir.path().join("other.tex");
        std::fs::write(&old, "text").unwrap();
        std::fs::hard_link(&old, &alias).unwrap();
        let movement = Move {
            old,
            new: dir.path().join("appendix.tex"),
            directory: false,
        };
        assert_eq!(movement.apply(&alias), None);
    }

    #[test]
    fn move_mapping_uses_components_and_rejects_ambiguous_batches() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("part");
        std::fs::create_dir(&old).unwrap();
        std::fs::write(old.join("a.tex"), "").unwrap();
        let context = Context {
            roots: vec![dir.path().to_owned()],
            open: vec![],
            capabilities: Capabilities::default(),
            encoding: PositionEncoding::Utf16,
            files: HashMap::new(),
            declarations: Arc::new(ResolvedDeclarations::default()),
        };
        let movement = Move {
            old: old.clone(),
            new: dir.path().join("new"),
            directory: true,
        };
        assert_eq!(
            moved(&old.join("a.tex"), std::slice::from_ref(&movement)),
            dir.path().join("new/a.tex")
        );
        assert_eq!(
            moved(
                &dir.path().join("part-other/a.tex"),
                std::slice::from_ref(&movement)
            ),
            dir.path().join("part-other/a.tex")
        );
        let child = Move {
            old: old.join("a.tex"),
            new: dir.path().join("b.tex"),
            directory: false,
        };
        assert!(validate_moves(&context, &[movement, child]).is_err());
        assert!(
            validate_moves(
                &context,
                &[Move {
                    old: old.clone(),
                    new: old.join("nested"),
                    directory: true
                }]
            )
            .is_err()
        );
    }
}
