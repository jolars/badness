//! The cross-file inclusion graph: which files pull in which, assembled from the
//! per-file [`crate::incremental::include_edges`] firewall and wrapped as a
//! tracked salsa query so a body edit doesn't rebuild the whole graph.
//!
//! This is the LaTeX analog of arity's `project/{scope,graph}.rs`. The layering,
//! from the per-file firewall up:
//!
//! - [`crate::incremental::include_edges`] — a per-file projection (range-free
//!   [`IncludeEdgeKey`]s) that stays *equal* across a body edit (salsa backdates).
//! - [`project_graph`] — assembles those into the cross-file [`IncludeGraph`],
//!   keyed on the interned [`Project`] membership snapshot, so an unchanged
//!   project + backdated per-file facts means its memo is reused.
//!
//! [`IncludeGraph::build`] is the **pure** algorithm (no salsa, no disk); the
//! salsa query is a thin wrapper. The pure layer is where the future consumers
//! (label/ref resolution, cross-file `\newcommand` scope) will plug in — like
//! arity, the graph lands "harness + graph only," directly testable, awaiting a
//! consumer to designate the document root.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::file_discovery::FileKind;
use crate::incremental::{IncrementalDb, QueryKind, QueryLogEntry, SourceFile, include_edges};
use crate::project::include::{IncludeEdgeKey, IncludeKind, IncludeTarget};

/// One file's contribution to the inclusion graph: its path and the range-free
/// inclusion edges it declares.
#[derive(Debug, Clone)]
pub struct FileFacts {
    pub path: PathBuf,
    pub include_edges: Vec<IncludeEdgeKey>,
}

/// A resolved inclusion edge: `from` includes `to` (a path within the analyzed
/// member set) via `kind`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedInclude {
    pub from: PathBuf,
    pub to: PathBuf,
    pub kind: IncludeKind,
}

/// An inclusion edge that could not be resolved to an analyzed member: a dynamic
/// target, or a literal path to a file outside the set we were given. Callers
/// must stay conservative about such files (their contents are opaque).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedInclude {
    pub from: PathBuf,
    pub target: IncludeTarget,
    pub kind: IncludeKind,
}

/// The inclusion graph over a set of files.
///
/// Holds `HashMap`s, so it is neither `Eq` nor `salsa::Update`; [`project_graph`]
/// is therefore `no_eq` (see its doc). Built by [`IncludeGraph::build`].
#[derive(Debug, Default)]
pub struct IncludeGraph {
    /// Per file: resolved outgoing edges, in source order.
    edges: HashMap<PathBuf, Vec<ResolvedInclude>>,
    /// Reverse map: included file → the files that include it.
    included_by: HashMap<PathBuf, Vec<PathBuf>>,
    /// Edges that resolve to nothing in the analyzed set (dynamic or external).
    unresolved: Vec<UnresolvedInclude>,
    /// Files reachable from the document root via resolved edges (root inclusive).
    /// Empty when [`build`](Self::build) was given no root — the salsa wrapper
    /// passes `None`, deferring "which file is the main document" to a consumer.
    reachable: HashSet<PathBuf>,
    /// Detected inclusion cycles (each a list of paths forming the cycle).
    /// `\input`/`\include` recursion is illegal in TeX; we expose cycles for a
    /// later linter rather than diagnosing here.
    cycles: Vec<Vec<PathBuf>>,
}

impl IncludeGraph {
    /// Assemble the inclusion graph for `files`. `root`, when given, seeds the
    /// reachability set (the main document and everything it transitively
    /// includes). Pure: resolves edge targets against the member set by path and
    /// never touches the disk.
    pub fn build(files: &[FileFacts], root: Option<&Path>) -> Self {
        let members: HashSet<&Path> = files.iter().map(|f| f.path.as_path()).collect();

        let mut edges: HashMap<PathBuf, Vec<ResolvedInclude>> = HashMap::new();
        let mut included_by: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        let mut unresolved: Vec<UnresolvedInclude> = Vec::new();

        for file in files {
            let mut outgoing = Vec::new();
            for edge in &file.include_edges {
                match &edge.target {
                    IncludeTarget::Path(to) if members.contains(to.as_path()) => {
                        outgoing.push(ResolvedInclude {
                            from: file.path.clone(),
                            to: to.clone(),
                            kind: edge.kind,
                        });
                        included_by
                            .entry(to.clone())
                            .or_default()
                            .push(file.path.clone());
                    }
                    // A resolved path to a file we didn't analyze is just as
                    // opaque as a dynamic target.
                    _ => unresolved.push(UnresolvedInclude {
                        from: file.path.clone(),
                        target: edge.target.clone(),
                        kind: edge.kind,
                    }),
                }
            }
            edges.insert(file.path.clone(), outgoing);
        }

        let reachable = match root {
            Some(root) => reachable_from(root, &edges),
            None => HashSet::new(),
        };
        let cycles = detect_cycles(files, &edges);

        Self {
            edges,
            included_by,
            unresolved,
            reachable,
            cycles,
        }
    }

    /// Resolved outgoing edges of `file`, in source order.
    pub fn outgoing(&self, file: &Path) -> &[ResolvedInclude] {
        self.edges.get(file).map_or(&[], Vec::as_slice)
    }

    /// Files that include `file`.
    pub fn included_by(&self, file: &Path) -> &[PathBuf] {
        self.included_by.get(file).map_or(&[], Vec::as_slice)
    }

    /// Whether `file` is reachable from the document root (always `false` when
    /// the graph was built without a root).
    pub fn is_reachable(&self, file: &Path) -> bool {
        self.reachable.contains(file)
    }

    /// Edges that resolve to nothing in the analyzed set.
    pub fn unresolved(&self) -> &[UnresolvedInclude] {
        &self.unresolved
    }

    /// Detected inclusion cycles.
    pub fn cycles(&self) -> &[Vec<PathBuf>] {
        &self.cycles
    }
}

/// The set of paths reachable from `root` (inclusive) over resolved edges.
fn reachable_from(root: &Path, edges: &HashMap<PathBuf, Vec<ResolvedInclude>>) -> HashSet<PathBuf> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        for edge in edges.get(&path).map_or(&[][..], Vec::as_slice) {
            stack.push(edge.to.clone());
        }
    }
    seen
}

/// Find inclusion cycles via DFS over the resolved-edge digraph. Each cycle is
/// returned once, rotated to start at its lexicographically smallest path so
/// equivalent rotations dedupe.
fn detect_cycles(
    files: &[FileFacts],
    edges: &HashMap<PathBuf, Vec<ResolvedInclude>>,
) -> Vec<Vec<PathBuf>> {
    let mut on_stack: HashSet<PathBuf> = HashSet::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut path: Vec<PathBuf> = Vec::new();
    let mut found: HashSet<Vec<PathBuf>> = HashSet::new();

    // Deterministic root order: the members as given (callers sort them).
    for file in files {
        dfs_cycles(
            &file.path,
            edges,
            &mut on_stack,
            &mut visited,
            &mut path,
            &mut found,
        );
    }

    found.into_iter().collect()
}

fn dfs_cycles(
    node: &Path,
    edges: &HashMap<PathBuf, Vec<ResolvedInclude>>,
    on_stack: &mut HashSet<PathBuf>,
    visited: &mut HashSet<PathBuf>,
    path: &mut Vec<PathBuf>,
    found: &mut HashSet<Vec<PathBuf>>,
) {
    if on_stack.contains(node) {
        // Back-edge: the cycle is the path slice from this node's first
        // appearance to the current end.
        if let Some(start) = path.iter().position(|p| p == node) {
            found.insert(normalize_cycle(&path[start..]));
        }
        return;
    }
    if !visited.insert(node.to_path_buf()) {
        return;
    }

    on_stack.insert(node.to_path_buf());
    path.push(node.to_path_buf());
    for edge in edges.get(node).map_or(&[][..], Vec::as_slice) {
        dfs_cycles(&edge.to, edges, on_stack, visited, path, found);
    }
    path.pop();
    on_stack.remove(node);
}

/// Rotate a cycle so it starts at its smallest path, giving every rotation of
/// the same cycle one canonical form.
fn normalize_cycle(cycle: &[PathBuf]) -> Vec<PathBuf> {
    let Some(min_at) = (0..cycle.len()).min_by_key(|&i| &cycle[i]) else {
        return Vec::new();
    };
    cycle[min_at..]
        .iter()
        .chain(&cycle[..min_at])
        .cloned()
        .collect()
}

/// One member of a project: its tracked input, on-disk path, and which pipeline
/// it feeds. Plain-derived (no `salsa::Update`) so it can key the interned
/// [`Project`]. No `Debug`: `SourceFile` is a salsa input id without a standalone
/// `Debug` impl.
///
/// `kind` lets the project-level queries split `.tex` from `.bib` members
/// without re-sniffing the extension: the include graph and label resolution see
/// only `.tex` members, while the citation resolver folds `.bib` cite keys in.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProjectMember {
    pub file: SourceFile,
    pub path: PathBuf,
    pub kind: FileKind,
}

/// A project as an interned membership snapshot. Interning dedups by value, so
/// an unchanged membership yields the same id across runs (a body edit doesn't
/// change the set) and the [`project_graph`] memo survives. Callers must sort
/// `members` for a stable, dedup-friendly key.
#[salsa::interned]
pub struct Project<'db> {
    #[returns(ref)]
    pub members: Vec<ProjectMember>,
}

/// The inclusion graph for `project`, built from the per-file firewall query.
///
/// `no_eq` because its output ([`IncludeGraph`]) holds `HashMap`s that aren't
/// `salsa::Update`/`Eq`-comparable here; `unsafe(non_update_types)` asserts it
/// carries no salsa references (the graph is a pure function of the interned
/// membership plus the backdated per-file edges). This costs nothing for the
/// firewall: a body edit leaves the per-file inputs backdated, so this query
/// simply isn't re-executed.
///
/// The root is passed as `None` — no consumer designates the main document yet,
/// so reachability is left to a future caller of [`IncludeGraph::build`]. Edges,
/// reverse map, unresolved targets, and cycles are all order-independent and
/// populated regardless.
#[salsa::tracked(returns(ref), no_eq, unsafe(non_update_types))]
pub fn project_graph<'db>(db: &'db dyn IncrementalDb, project: Project<'db>) -> IncludeGraph {
    db.record_query(QueryLogEntry {
        kind: QueryKind::ProjectGraph,
        file: None,
    });

    // Only `.tex` members are inclusion-graph nodes; `.bib` members carry no
    // `\input` edges and feed the citation resolver instead.
    let facts: Vec<FileFacts> = project
        .members(db)
        .iter()
        .filter(|member| member.kind == FileKind::Tex)
        .map(|member| FileFacts {
            path: member.path.clone(),
            include_edges: include_edges(db, member.file).clone(),
        })
        .collect();

    IncludeGraph::build(&facts, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(path: &str, edges: &[(IncludeKind, &str)]) -> FileFacts {
        FileFacts {
            path: PathBuf::from(path),
            include_edges: edges
                .iter()
                .map(|(kind, target)| IncludeEdgeKey {
                    kind: *kind,
                    target: IncludeTarget::Path(PathBuf::from(target)),
                })
                .collect(),
        }
    }

    #[test]
    fn resolves_edges_within_the_member_set() {
        let files = vec![
            facts("/p/main.tex", &[(IncludeKind::Input, "/p/a.tex")]),
            facts("/p/a.tex", &[]),
        ];
        let g = IncludeGraph::build(&files, None);
        assert_eq!(
            g.outgoing(Path::new("/p/main.tex")),
            &[ResolvedInclude {
                from: PathBuf::from("/p/main.tex"),
                to: PathBuf::from("/p/a.tex"),
                kind: IncludeKind::Input,
            }]
        );
        assert_eq!(
            g.included_by(Path::new("/p/a.tex")),
            &[PathBuf::from("/p/main.tex")]
        );
        assert!(g.unresolved().is_empty());
    }

    #[test]
    fn target_outside_member_set_is_unresolved() {
        let files = vec![facts(
            "/p/main.tex",
            &[(IncludeKind::Input, "/p/missing.tex")],
        )];
        let g = IncludeGraph::build(&files, None);
        assert!(g.outgoing(Path::new("/p/main.tex")).is_empty());
        assert_eq!(g.unresolved().len(), 1);
        assert_eq!(g.unresolved()[0].from, PathBuf::from("/p/main.tex"));
    }

    #[test]
    fn dynamic_target_is_unresolved() {
        let files = vec![FileFacts {
            path: PathBuf::from("/p/main.tex"),
            include_edges: vec![IncludeEdgeKey {
                kind: IncludeKind::Input,
                target: IncludeTarget::Dynamic,
            }],
        }];
        let g = IncludeGraph::build(&files, None);
        assert_eq!(g.unresolved().len(), 1);
        assert_eq!(g.unresolved()[0].target, IncludeTarget::Dynamic);
    }

    #[test]
    fn reachability_follows_transitive_edges_from_root() {
        let files = vec![
            facts("/p/main.tex", &[(IncludeKind::Input, "/p/a.tex")]),
            facts("/p/a.tex", &[(IncludeKind::Input, "/p/b.tex")]),
            facts("/p/b.tex", &[]),
            facts("/p/orphan.tex", &[]),
        ];
        let g = IncludeGraph::build(&files, Some(Path::new("/p/main.tex")));
        assert!(g.is_reachable(Path::new("/p/main.tex")));
        assert!(g.is_reachable(Path::new("/p/a.tex")));
        assert!(g.is_reachable(Path::new("/p/b.tex")));
        assert!(!g.is_reachable(Path::new("/p/orphan.tex")));
    }

    #[test]
    fn no_root_means_nothing_reachable() {
        let files = vec![facts("/p/main.tex", &[])];
        let g = IncludeGraph::build(&files, None);
        assert!(!g.is_reachable(Path::new("/p/main.tex")));
    }

    #[test]
    fn detects_a_cycle() {
        let files = vec![
            facts("/p/a.tex", &[(IncludeKind::Input, "/p/b.tex")]),
            facts("/p/b.tex", &[(IncludeKind::Input, "/p/a.tex")]),
        ];
        let g = IncludeGraph::build(&files, None);
        assert_eq!(g.cycles().len(), 1);
        assert_eq!(
            g.cycles()[0],
            vec![PathBuf::from("/p/a.tex"), PathBuf::from("/p/b.tex")]
        );
    }

    #[test]
    fn self_inclusion_is_a_cycle() {
        let files = vec![facts("/p/a.tex", &[(IncludeKind::Input, "/p/a.tex")])];
        let g = IncludeGraph::build(&files, None);
        assert_eq!(g.cycles(), &[vec![PathBuf::from("/p/a.tex")]]);
    }

    #[test]
    fn acyclic_graph_has_no_cycles() {
        let files = vec![
            facts("/p/main.tex", &[(IncludeKind::Input, "/p/a.tex")]),
            facts("/p/a.tex", &[]),
        ];
        let g = IncludeGraph::build(&files, None);
        assert!(g.cycles().is_empty());
    }
}
