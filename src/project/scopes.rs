//! Document membership shared by label and citation resolution.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::ast::{command_name, environment_name};
use crate::incremental::{IncrementalDb, file_document_root};
use crate::project::graph::{IncludeGraph, project_graph, workspace_project};
use crate::project::include::IncludeKind;
use crate::syntax::{SyntaxKind, SyntaxNode};

/// Root markers found in a single source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, salsa::SalsaValue)]
pub enum DocumentRoot {
    None,
    Class,
    Body,
    Complete,
}

impl DocumentRoot {
    pub fn detect(root: &SyntaxNode) -> Self {
        let mut conditionals = None;
        let mut saw_literal_test = false;
        let mut class = false;
        let mut body = false;
        for node in root.descendants() {
            let marker = match node.kind() {
                SyntaxKind::BEGIN if environment_name(&node).as_deref() == Some("document") => {
                    Self::Body
                }
                SyntaxKind::COMMAND => match command_name(&node).as_deref() {
                    Some("documentclass") => Self::Class,
                    Some("iftrue" | "iffalse") => {
                        saw_literal_test = true;
                        continue;
                    }
                    _ => continue,
                },
                _ => continue,
            };
            if saw_literal_test
                && conditionals
                    .get_or_insert_with(|| {
                        crate::linter::conditional::ConditionalIndex::compute(root)
                    })
                    .is_inactive(node.text_range().start().into())
            {
                continue;
            }
            class |= marker == Self::Class;
            body |= marker == Self::Body;
            if class && body {
                return Self::Complete;
            }
        }
        match (class, body) {
            (true, _) => Self::Class,
            (_, true) => Self::Body,
            _ => Self::None,
        }
    }
}

impl From<bool> for DocumentRoot {
    fn from(rooted: bool) -> Self {
        if rooted { Self::Complete } else { Self::None }
    }
}

#[derive(Debug)]
pub struct DocumentScope {
    pub root: Option<PathBuf>,
    pub members: Vec<PathBuf>,
    pub closed: bool,
}

#[derive(Debug, Default)]
pub struct DocumentScopes {
    pub scopes: Vec<DocumentScope>,
    membership: HashMap<PathBuf, Vec<usize>>,
}

impl DocumentScopes {
    pub fn build<'a, R: Into<DocumentRoot>>(
        graph: &IncludeGraph,
        files: impl IntoIterator<Item = (&'a Path, R)>,
    ) -> Arc<Self> {
        let files: BTreeMap<_, _> = files
            .into_iter()
            .map(|(path, root)| (path, root.into()))
            .collect();
        let mut parents = HashMap::<&Path, BTreeSet<Option<&Path>>>::new();
        let mut open = HashSet::new();
        for edge in graph.unresolved() {
            open.insert(edge.from.as_path());
            if edge.kind == IncludeKind::SubFilesParent {
                parents.entry(&edge.from).or_default().insert(None);
            }
        }
        for &path in files.keys() {
            for edge in graph.outgoing(path) {
                if !files.contains_key(edge.to.as_path()) {
                    open.insert(path);
                }
                if edge.kind == IncludeKind::SubFilesParent {
                    parents
                        .entry(path)
                        .or_default()
                        .insert(Some(edge.to.as_path()));
                }
            }
        }

        // An included class-only preamble belongs to the document that
        // supplies the body. A standalone class-only file can still be a root
        // while its body is being written.
        let body_seeds = files
            .iter()
            .filter_map(|(&path, &root)| {
                (matches!(root, DocumentRoot::Body | DocumentRoot::Complete)
                    && !parents.contains_key(path))
                .then_some(path)
            })
            .collect();
        let body_members = forward_members(graph, body_seeds, |path| files.contains_key(path));
        // A body-only input can get its preamble from a class-bearing caller.
        let class_seeds = files
            .iter()
            .filter_map(|(&path, &root)| {
                (root == DocumentRoot::Class
                    && !body_members.contains(path)
                    && !parents.contains_key(path))
                .then_some(path)
            })
            .collect();
        let class_members = forward_members(graph, class_seeds, |path| files.contains_key(path));
        // A markerless entry point can assemble a document from separate inputs.
        // Propagate each partial marker to its callers, stopping at complete
        // documents and subfiles, which already have their own root evidence.
        let callers_with = |marker| {
            let mut pending: Vec<_> = files
                .iter()
                .filter_map(|(&path, &kind)| {
                    (kind == marker && !parents.contains_key(path)).then_some(path)
                })
                .collect();
            let mut callers = HashSet::new();
            while let Some(path) = pending.pop() {
                if callers.insert(path) {
                    pending.extend(graph.source_callers(path).filter(|caller| {
                        files
                            .get(caller)
                            .is_some_and(|&kind| kind != DocumentRoot::Complete)
                            && !parents.contains_key(caller)
                    }));
                }
            }
            callers
        };
        let class_callers = callers_with(DocumentRoot::Class);
        let body_callers = callers_with(DocumentRoot::Body);
        let assembles_document =
            |path: &Path| class_callers.contains(path) && body_callers.contains(path);
        let split_roots: Vec<_> = class_callers
            .intersection(&body_callers)
            .copied()
            .filter(|path| {
                files[path] == DocumentRoot::None
                    && !graph.outgoing(path).iter().any(|edge| {
                        edge.kind != IncludeKind::SubFilesParent && assembles_document(&edge.to)
                    })
            })
            .collect();
        let split_members =
            forward_members(graph, split_roots.clone(), |path| files.contains_key(path));
        let split_roots: HashSet<_> = split_roots.into_iter().collect();
        let is_root = |path: &Path| match files[path] {
            DocumentRoot::Complete => true,
            DocumentRoot::Body => !class_members.contains(path) && !split_members.contains(path),
            DocumentRoot::Class => !body_members.contains(path) && !split_members.contains(path),
            DocumentRoot::None => split_roots.contains(path),
        };

        // Subfiles declarations assign chapters to their parent document even
        // when the parent has no explicit source edge to the chapter.
        let mut seeds = BTreeMap::<&Path, Vec<&Path>>::new();
        let mut incomplete = HashSet::new();
        for &path in files.keys() {
            if let Some(parent) = parents.get(path) {
                let mut next = parent;
                let mut seen = HashSet::from([path]);
                let owner = loop {
                    if next.len() != 1 {
                        break None;
                    }
                    let Some(Some(candidate)) = next.first() else {
                        break None;
                    };
                    if !seen.insert(candidate) || !files.contains_key(candidate) {
                        break None;
                    }
                    match parents.get(candidate) {
                        Some(chain) => next = chain,
                        None => break Some(*candidate),
                    }
                };
                if let Some(owner) = owner {
                    seeds.entry(owner).or_default().push(path);
                } else {
                    seeds.entry(path).or_default();
                    incomplete.insert(path);
                }
            } else if is_root(path) {
                seeds.entry(path).or_default();
            }
        }

        let mut index = Self::default();
        for (root, mut seeds) in seeds {
            // A declared parent can delegate its whole document to an input.
            // Determine completeness from the parent's source tree before
            // adding any subfiles associated with it.
            let has_document = is_root(root)
                || forward_members(graph, vec![root], |path| files.contains_key(path))
                    .iter()
                    .any(|path| matches!(files[path], DocumentRoot::Body | DocumentRoot::Complete));
            seeds.push(root);
            let members = forward_members(graph, seeds, |path| files.contains_key(path));
            let closed = has_document
                && !incomplete.contains(root)
                && members.iter().all(|path| !open.contains(path));
            index.add(Some(root), members, closed);
        }

        // Group the rootless files without walking backwards out of a shared
        // include into some other document. Include their forward dependencies
        // so editor operations can see every analyzed use of a shared key.
        for &start in files.keys() {
            if index.membership.contains_key(start) {
                continue;
            }
            let mut stack = vec![start];
            let mut members = BTreeSet::new();
            while let Some(path) = stack.pop() {
                if index.membership.contains_key(path) || !members.insert(path) {
                    continue;
                }
                stack.extend(
                    graph
                        .outgoing(path)
                        .iter()
                        .map(|edge| edge.to.as_path())
                        .chain(graph.included_by(path).iter().map(PathBuf::as_path))
                        .filter(|path| files.contains_key(path)),
                );
            }
            let members = forward_members(graph, members.into_iter().collect(), |path| {
                files.contains_key(path)
            });
            let closed = members.iter().all(|path| !open.contains(path));
            index.add(None, members, closed);
        }
        Arc::new(index)
    }

    fn add(&mut self, root: Option<&Path>, members: BTreeSet<&Path>, closed: bool) {
        let id = self.scopes.len();
        let members: Vec<_> = members.into_iter().map(Path::to_path_buf).collect();
        for member in &members {
            self.membership.entry(member.clone()).or_default().push(id);
        }
        self.scopes.push(DocumentScope {
            root: root.map(Path::to_path_buf),
            members,
            closed,
        });
    }

    pub fn containing(&self, path: &Path) -> &[usize] {
        self.membership.get(path).map_or(&[], Vec::as_slice)
    }

    pub fn members(&self, ids: impl IntoIterator<Item = usize>) -> Vec<&Path> {
        ids.into_iter()
            .flat_map(|id| self.scopes[id].members.iter().map(PathBuf::as_path))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

pub(crate) fn forward_members<'a>(
    graph: &'a IncludeGraph,
    mut stack: Vec<&'a Path>,
    contains: impl Fn(&Path) -> bool,
) -> BTreeSet<&'a Path> {
    let mut members = BTreeSet::new();
    while let Some(path) = stack.pop() {
        if !members.insert(path) {
            continue;
        }
        stack.extend(graph.outgoing(path).iter().filter_map(|edge| {
            (edge.kind != IncludeKind::SubFilesParent && contains(edge.to.as_path()))
                .then_some(edge.to.as_path())
        }));
    }
    members
}

#[salsa::tracked(returns(ref), no_eq, unsafe(non_salsa_values))]
pub(crate) fn document_scopes(db: &dyn IncrementalDb) -> Arc<DocumentScopes> {
    DocumentScopes::build(
        project_graph(db),
        workspace_project(db)
            .members
            .iter()
            .filter(|member| member.kind.is_latex())
            .map(|member| (member.path.as_path(), *file_document_root(db, member.file))),
    )
}
