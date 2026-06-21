//! Cross-file label resolution: union the per-file [`\label`] sets across the
//! inclusion graph so a `\ref` can be resolved against the whole document, and a
//! key defined in two files of one document can be flagged as a duplicate.
//!
//! Layered like [`crate::project::graph`]: [`ResolvedLabels::build`] is the
//! **pure** algorithm (no salsa, no disk), and [`crate::project::resolved_labels`]
//! is a thin tracked wrapper. The CLI calls the pure builder directly (one-shot,
//! no salsa); the language server (eventually) uses the query. Both feed the same
//! data into the linter, so results match.
//!
//! **Namespace = undirected connected component of the include graph.** LaTeX
//! labels share one namespace per *compiled document*, but with no designated
//! main file ([`crate::project::project_graph`] passes `root: None`) the
//! root-free approximation is the connected component: a `main` and the chapters
//! it `\input`s form one namespace, while two unrelated documents in the same
//! directory stay separate and don't cross-contaminate. **Known limitation:** two
//! independent documents that share a common include (e.g. a `preamble.tex`) are
//! merged into one component, so a label defined in both is reported as a
//! cross-file duplicate even though they never co-compile.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use smol_str::SmolStr;

use crate::ast::{command_name, environment_name};
use crate::incremental::{
    IncrementalDb, QueryKind, QueryLogEntry, file_is_document_root, file_labels,
};
use crate::project::graph::{IncludeGraph, Project, project_graph};
use crate::semantic::SemanticModel;
use crate::syntax::{SyntaxKind, SyntaxNode};

/// The distinct `\label` names defined in `model`, sorted and deduped — the
/// per-file label input to [`ResolvedLabels::build`]. Shared by the CLI
/// (one-shot, non-salsa) and the [`crate::incremental::file_labels`] firewall so
/// both feed identical data into the resolver.
pub fn document_label_names(model: &SemanticModel) -> Vec<SmolStr> {
    let mut names: Vec<SmolStr> = model
        .labels()
        .iter()
        .map(|label| label.name.clone())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// Whether `root` carries a `\documentclass` or a `\begin{document}` — the
/// document-root signal gating the `undefined-ref` lint (see [`ResolvedLabels`]).
/// Shared by the CLI and the [`crate::incremental::file_is_document_root`]
/// firewall.
pub fn is_document_root(root: &SyntaxNode) -> bool {
    root.descendants().any(|node| match node.kind() {
        SyntaxKind::COMMAND => command_name(&node).as_deref() == Some("documentclass"),
        // The `document` environment's name lives on its `\begin{document}`.
        SyntaxKind::BEGIN => environment_name(&node).as_deref() == Some("document"),
        _ => false,
    })
}

/// One label namespace: an undirected connected component of the include graph.
#[derive(Debug, Default)]
struct Component {
    /// Label name → the files in this component that define it, sorted & deduped.
    defs: HashMap<SmolStr, Vec<PathBuf>>,
    /// Whether every include in the component resolves to an analyzed member: no
    /// dynamic and no external (out-of-set) targets. Only then is "defined
    /// nowhere" trustworthy enough to drive `undefined-ref`.
    closed: bool,
    /// Whether any member is a document root (`\documentclass` /
    /// `\begin{document}`). `undefined-ref` fires only inside a rooted namespace.
    rooted: bool,
}

/// The resolved cross-file label model over a set of analyzed files.
///
/// Holds `HashMap`s/`PathBuf`s, so (like [`IncludeGraph`]) it is neither `Eq` nor
/// `salsa::Update`; the [`crate::project::resolved_labels`] query is therefore
/// `no_eq`. Built by [`ResolvedLabels::build`].
#[derive(Debug, Default)]
pub struct ResolvedLabels {
    /// File path → index into [`components`](Self::components).
    component_of: HashMap<PathBuf, usize>,
    components: Vec<Component>,
}

impl ResolvedLabels {
    /// Resolve labels for `files` — each a `(path, distinct sorted label names,
    /// is_document_root)` triple — partitioned by the inclusion `graph`.
    ///
    /// Pure and deterministic: components are assigned in sorted-path order and
    /// every definer list is sorted, so the output never depends on `HashMap`
    /// iteration order.
    pub fn build(files: &[(PathBuf, Vec<SmolStr>, bool)], graph: &IncludeGraph) -> Self {
        // Sorted, unique member paths give union-find a deterministic index space.
        let mut paths: Vec<&Path> = files.iter().map(|(p, _, _)| p.as_path()).collect();
        paths.sort_unstable();
        paths.dedup();
        let index: HashMap<&Path, usize> = paths.iter().enumerate().map(|(i, p)| (*p, i)).collect();

        // Undirected connectivity: union a file with each include neighbor that is
        // itself a member (edges in either direction merge the same namespace).
        let mut uf = UnionFind::new(paths.len());
        for (&path, &i) in &index {
            for edge in graph.outgoing(path) {
                if let Some(&j) = index.get(edge.to.as_path()) {
                    uf.union(i, j);
                }
            }
            for included in graph.included_by(path) {
                if let Some(&j) = index.get(included.as_path()) {
                    uf.union(i, j);
                }
            }
        }

        // Assign compact component ids in first-seen (sorted-path) order.
        let mut root_to_id: HashMap<usize, usize> = HashMap::new();
        let mut component_of: HashMap<PathBuf, usize> = HashMap::new();
        for (i, &path) in paths.iter().enumerate() {
            let root = uf.find(i);
            let next = root_to_id.len();
            let id = *root_to_id.entry(root).or_insert(next);
            component_of.insert(path.to_path_buf(), id);
        }
        let mut components: Vec<Component> = (0..root_to_id.len())
            .map(|_| Component {
                closed: true,
                ..Component::default()
            })
            .collect();

        // Index definitions and the rooted flag per component.
        for (path, names, is_root) in files {
            let Some(&id) = component_of.get(path) else {
                continue;
            };
            let comp = &mut components[id];
            comp.rooted |= *is_root;
            for name in names {
                comp.defs
                    .entry(name.clone())
                    .or_default()
                    .push(path.clone());
            }
        }

        // An unresolved include (dynamic or out-of-set) opens its component: the
        // real label universe may be larger than what we analyzed.
        for edge in graph.unresolved() {
            if let Some(&id) = component_of.get(&edge.from) {
                components[id].closed = false;
            }
        }

        // Canonicalize definer lists (a file appears at most once per name —
        // `file_labels` is already deduped — but distinct files arrive unordered).
        for comp in &mut components {
            for definers in comp.defs.values_mut() {
                definers.sort_unstable();
                definers.dedup();
            }
        }

        Self {
            component_of,
            components,
        }
    }

    /// Files in `file`'s namespace that define `name`, sorted. Empty when `file`
    /// is unknown or `name` is undefined in its component. Includes `file` itself
    /// when it defines `name`; callers wanting *other* definers filter it out.
    pub fn definers(&self, file: &Path, name: &str) -> &[PathBuf] {
        self.component_of
            .get(file)
            .and_then(|&id| self.components[id].defs.get(name))
            .map_or(&[], Vec::as_slice)
    }

    /// Whether `name` is defined anywhere in `file`'s namespace.
    pub fn is_defined(&self, file: &Path, name: &str) -> bool {
        !self.definers(file, name).is_empty()
    }

    /// All member files sharing `file`'s namespace (its connected component),
    /// sorted; empty when `file` is unknown. Includes `file` itself. Unlike
    /// [`definers`](Self::definers) (which files *define* a name) this is every
    /// file in the namespace — the search set for find-references, which must scan
    /// each member for `\ref` use sites.
    pub fn namespace_members(&self, file: &Path) -> Vec<&Path> {
        let Some(&id) = self.component_of.get(file) else {
            return Vec::new();
        };
        let mut members: Vec<&Path> = self
            .component_of
            .iter()
            .filter(|&(_, &cid)| cid == id)
            .map(|(p, _)| p.as_path())
            .collect();
        members.sort_unstable();
        members
    }

    /// Whether `file`'s namespace is closed — every include resolves to an
    /// analyzed member. Gates `undefined-ref` (an open namespace may define the
    /// key in a file we never saw).
    pub fn is_closed(&self, file: &Path) -> bool {
        self.component_of
            .get(file)
            .is_some_and(|&id| self.components[id].closed)
    }

    /// Whether `file`'s namespace contains a document root. Gates `undefined-ref`
    /// so a bare fragment opened standalone is never flagged.
    pub fn is_root_component(&self, file: &Path) -> bool {
        self.component_of
            .get(file)
            .is_some_and(|&id| self.components[id].rooted)
    }
}

/// The cross-file label resolution for `project`, built from the per-file
/// [`file_labels`] firewall and the [`project_graph`].
///
/// `no_eq` + `unsafe(non_update_types)` for the same reason as [`project_graph`]:
/// [`ResolvedLabels`] holds `HashMap`s (not `Eq`/`salsa::Update`) and is a pure
/// function of the interned [`Project`] plus the backdated per-file facts, so it
/// carries no salsa references. The firewall pays off here: a prose edit leaves
/// `file_labels`, `file_is_document_root`, and `include_edges` all backdated, so
/// neither [`project_graph`] nor this query re-executes.
#[salsa::tracked(returns(ref), no_eq, unsafe(non_update_types))]
pub fn resolved_labels<'db>(db: &'db dyn IncrementalDb, project: Project<'db>) -> ResolvedLabels {
    db.record_query(QueryLogEntry {
        kind: QueryKind::ResolvedLabels,
        file: None,
    });

    let graph = project_graph(db, project);
    // Labels live in LaTeX files (`.tex`/`.sty`/`.cls`); `.bib` members carry none
    // and are not part of the include-graph namespace.
    let files: Vec<(PathBuf, Vec<SmolStr>, bool)> = project
        .members(db)
        .iter()
        .filter(|member| member.kind.is_latex())
        .map(|member| {
            (
                member.path.clone(),
                file_labels(db, member.file).clone(),
                *file_is_document_root(db, member.file),
            )
        })
        .collect();

    ResolvedLabels::build(&files, graph)
}

/// A minimal union-find (disjoint-set) with path halving and union by size.
struct UnionFind {
    parent: Vec<usize>,
    size: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            size: vec![1; n],
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (mut ra, mut rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        if self.size[ra] < self.size[rb] {
            std::mem::swap(&mut ra, &mut rb);
        }
        self.parent[rb] = ra;
        self.size[ra] += self.size[rb];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::graph::FileFacts;
    use crate::project::include::{IncludeEdgeKey, IncludeKind, IncludeTarget};

    /// Build an `IncludeGraph` from `(path, [(kind, target)])` tuples.
    fn graph(files: &[(&str, &[(IncludeKind, &str)])]) -> IncludeGraph {
        let facts: Vec<FileFacts> = files
            .iter()
            .map(|(path, edges)| FileFacts {
                path: PathBuf::from(path),
                include_edges: edges
                    .iter()
                    .map(|(kind, target)| IncludeEdgeKey {
                        kind: *kind,
                        target: IncludeTarget::Path(PathBuf::from(target)),
                    })
                    .collect(),
            })
            .collect();
        IncludeGraph::build(&facts, None)
    }

    fn names(list: &[&str]) -> Vec<SmolStr> {
        list.iter().map(SmolStr::new).collect()
    }

    #[test]
    fn lone_file_is_its_own_component() {
        let g = graph(&[("/p/a.tex", &[])]);
        let r = ResolvedLabels::build(&[(PathBuf::from("/p/a.tex"), names(&["x"]), false)], &g);
        assert!(r.is_defined(Path::new("/p/a.tex"), "x"));
        assert!(!r.is_defined(Path::new("/p/a.tex"), "y"));
        // No other file defines `x`.
        assert_eq!(
            r.definers(Path::new("/p/a.tex"), "x"),
            &[PathBuf::from("/p/a.tex")]
        );
    }

    #[test]
    fn input_chain_shares_one_namespace() {
        let g = graph(&[
            ("/p/main.tex", &[(IncludeKind::Input, "/p/chap.tex")]),
            ("/p/chap.tex", &[]),
        ]);
        let r = ResolvedLabels::build(
            &[
                (PathBuf::from("/p/main.tex"), names(&[]), true),
                (PathBuf::from("/p/chap.tex"), names(&["a"]), false),
            ],
            &g,
        );
        // A label in the chapter is visible from the main file's namespace.
        assert!(r.is_defined(Path::new("/p/main.tex"), "a"));
        assert!(r.is_root_component(Path::new("/p/chap.tex")));
        assert!(r.is_closed(Path::new("/p/main.tex")));
    }

    #[test]
    fn diamond_merges_all_four() {
        let g = graph(&[
            (
                "/p/main.tex",
                &[
                    (IncludeKind::Input, "/p/a.tex"),
                    (IncludeKind::Input, "/p/b.tex"),
                ],
            ),
            ("/p/a.tex", &[(IncludeKind::Input, "/p/shared.tex")]),
            ("/p/b.tex", &[(IncludeKind::Input, "/p/shared.tex")]),
            ("/p/shared.tex", &[]),
        ]);
        let r = ResolvedLabels::build(
            &[
                (PathBuf::from("/p/main.tex"), names(&[]), true),
                (PathBuf::from("/p/a.tex"), names(&["k"]), false),
                (PathBuf::from("/p/b.tex"), names(&["k"]), false),
                (PathBuf::from("/p/shared.tex"), names(&[]), false),
            ],
            &g,
        );
        // `k` defined in both a and b → both are cross-file definers, sorted.
        assert_eq!(
            r.definers(Path::new("/p/a.tex"), "k"),
            &[PathBuf::from("/p/a.tex"), PathBuf::from("/p/b.tex")]
        );
        // The whole diamond is one namespace: every member is a reference-search
        // target, regardless of whether it defines anything.
        assert_eq!(
            r.namespace_members(Path::new("/p/shared.tex")),
            &[
                Path::new("/p/a.tex"),
                Path::new("/p/b.tex"),
                Path::new("/p/main.tex"),
                Path::new("/p/shared.tex"),
            ]
        );
    }

    #[test]
    fn namespace_members_isolates_independent_documents() {
        let g = graph(&[("/p/one.tex", &[]), ("/p/two.tex", &[])]);
        let r = ResolvedLabels::build(
            &[
                (PathBuf::from("/p/one.tex"), names(&["x"]), true),
                (PathBuf::from("/p/two.tex"), names(&["x"]), true),
            ],
            &g,
        );
        assert_eq!(
            r.namespace_members(Path::new("/p/one.tex")),
            &[Path::new("/p/one.tex")]
        );
        assert!(r.namespace_members(Path::new("/p/missing.tex")).is_empty());
    }

    #[test]
    fn independent_documents_do_not_share_labels() {
        let g = graph(&[("/p/one.tex", &[]), ("/p/two.tex", &[])]);
        let r = ResolvedLabels::build(
            &[
                (PathBuf::from("/p/one.tex"), names(&["intro"]), true),
                (PathBuf::from("/p/two.tex"), names(&["intro"]), true),
            ],
            &g,
        );
        // Same key in two unrelated docs is NOT a cross-file duplicate.
        assert_eq!(
            r.definers(Path::new("/p/one.tex"), "intro"),
            &[PathBuf::from("/p/one.tex")]
        );
        assert_eq!(
            r.definers(Path::new("/p/two.tex"), "intro"),
            &[PathBuf::from("/p/two.tex")]
        );
    }

    #[test]
    fn cycle_is_one_component() {
        let g = graph(&[
            ("/p/a.tex", &[(IncludeKind::Input, "/p/b.tex")]),
            ("/p/b.tex", &[(IncludeKind::Input, "/p/a.tex")]),
        ]);
        let r = ResolvedLabels::build(
            &[
                (PathBuf::from("/p/a.tex"), names(&["x"]), false),
                (PathBuf::from("/p/b.tex"), names(&[]), false),
            ],
            &g,
        );
        assert!(r.is_defined(Path::new("/p/b.tex"), "x"));
    }

    #[test]
    fn dynamic_include_opens_the_component() {
        let g = {
            let facts = vec![FileFacts {
                path: PathBuf::from("/p/main.tex"),
                include_edges: vec![IncludeEdgeKey {
                    kind: IncludeKind::Input,
                    target: IncludeTarget::Dynamic,
                }],
            }];
            IncludeGraph::build(&facts, None)
        };
        let r = ResolvedLabels::build(&[(PathBuf::from("/p/main.tex"), names(&[]), true)], &g);
        assert!(!r.is_closed(Path::new("/p/main.tex")));
    }

    #[test]
    fn external_include_opens_the_component() {
        // `/p/missing.tex` is not an analyzed member → unresolved → open.
        let g = graph(&[("/p/main.tex", &[(IncludeKind::Input, "/p/missing.tex")])]);
        let r = ResolvedLabels::build(&[(PathBuf::from("/p/main.tex"), names(&[]), true)], &g);
        assert!(!r.is_closed(Path::new("/p/main.tex")));
    }

    #[test]
    fn rootless_component_reports_no_root() {
        let g = graph(&[("/p/frag.tex", &[])]);
        let r = ResolvedLabels::build(&[(PathBuf::from("/p/frag.tex"), names(&["x"]), false)], &g);
        assert!(!r.is_root_component(Path::new("/p/frag.tex")));
        assert!(r.is_closed(Path::new("/p/frag.tex")));
    }
}
