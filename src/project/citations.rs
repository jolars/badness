//! Cross-file citation resolution: union the cite keys reachable from each `.tex`
//! file's namespace (via its `\bibliography`/`\addbibresource` resources) so a
//! `\cite{key}` can be checked against the whole bibliography.
//!
//! The citation analog of [`crate::project::labels`]. Namespaces are the same
//! undirected connected components of the include graph, but the "definitions"
//! are cite keys gathered from the `.bib` files each component's members
//! reference, not `\label`s. [`ResolvedCitations::build`] is the **pure** algorithm
//! the CLI calls directly.
//!
//! A namespace is **closed** for citations only when its include graph is closed
//! *and* every bibliography resource resolves to an analyzed `.bib` file — else a
//! `\cite` key we cannot see might still be defined. `undefined-citation` fires
//! only in a closed, rooted namespace with no `\nocite{*}` wildcard, mirroring
//! `undefined-ref`'s gate.
//!
//! The union-find here duplicates [`crate::project::labels`]'s (an EXTRACTION
//! CANDIDATE for a shared component-finder); kept separate for now so the tested
//! label resolver is untouched.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use smol_str::SmolStr;

use crate::project::graph::IncludeGraph;
use crate::project::include::BibTarget;

/// Per-`.tex`-file facts feeding [`ResolvedCitations::build`]: the file's
/// bibliography resource targets, whether it has a `\nocite{*}` wildcard, and
/// whether it is a document root.
#[derive(Debug, Clone)]
pub struct CiteFileFacts {
    pub path: PathBuf,
    pub bib_targets: Vec<BibTarget>,
    pub nocite_all: bool,
    pub is_document_root: bool,
}

/// One citation namespace: an undirected connected component of the include graph.
#[derive(Debug, Default)]
struct Component {
    /// The cite keys available in this namespace (lowercased for case-insensitive
    /// matching, as BibTeX folds key case).
    keys: HashSet<SmolStr>,
    /// Whether the include graph is closed *and* every bibliography resource
    /// resolved to an analyzed `.bib`. Only then is "cited but undefined"
    /// trustworthy.
    closed: bool,
    /// Whether any member is a document root.
    rooted: bool,
    /// Whether any member has a `\nocite{*}` wildcard.
    wildcard: bool,
}

/// The resolved cross-file citation model over a set of analyzed `.tex` files and
/// the `.bib` key sets they reference.
#[derive(Debug, Default)]
pub struct ResolvedCitations {
    component_of: HashMap<PathBuf, usize>,
    components: Vec<Component>,
}

impl ResolvedCitations {
    /// Resolve citations for `files`, partitioned by the inclusion `graph`, with
    /// `bib_keys` mapping each analyzed `.bib` path to its cite keys.
    ///
    /// Pure and deterministic (components assigned in sorted-path order).
    pub fn build(
        files: &[CiteFileFacts],
        graph: &IncludeGraph,
        bib_keys: &HashMap<PathBuf, Vec<SmolStr>>,
    ) -> Self {
        let mut paths: Vec<&Path> = files.iter().map(|f| f.path.as_path()).collect();
        paths.sort_unstable();
        paths.dedup();
        let index: HashMap<&Path, usize> = paths.iter().enumerate().map(|(i, p)| (*p, i)).collect();

        // Undirected connectivity over the include graph (same as label namespaces).
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

        // Gather keys, flags, and bib-resource openness per component.
        for facts in files {
            let Some(&id) = component_of.get(&facts.path) else {
                continue;
            };
            let comp = &mut components[id];
            comp.rooted |= facts.is_document_root;
            comp.wildcard |= facts.nocite_all;
            for target in &facts.bib_targets {
                match target {
                    BibTarget::Dynamic => comp.closed = false,
                    BibTarget::Path(path) => match bib_keys.get(path) {
                        Some(keys) => comp
                            .keys
                            .extend(keys.iter().map(|k| SmolStr::from(k.to_lowercase()))),
                        // A `.bib` we never analyzed: the real key set may be larger.
                        None => comp.closed = false,
                    },
                }
            }
        }

        // An unresolved `.tex` include (dynamic or out-of-set) opens its component,
        // just as it does for labels.
        for edge in graph.unresolved() {
            if let Some(&id) = component_of.get(&edge.from) {
                components[id].closed = false;
            }
        }

        Self {
            component_of,
            components,
        }
    }

    /// Whether cite `key` is defined anywhere in `file`'s namespace
    /// (case-insensitive).
    pub fn is_defined(&self, file: &Path, key: &str) -> bool {
        self.component_of.get(file).is_some_and(|&id| {
            self.components[id]
                .keys
                .contains(&SmolStr::from(key.to_lowercase()))
        })
    }

    /// Whether `file`'s namespace is closed — every `.tex` include and every
    /// bibliography resource resolved to an analyzed file. Gates
    /// `undefined-citation`.
    pub fn is_closed(&self, file: &Path) -> bool {
        self.component_of
            .get(file)
            .is_some_and(|&id| self.components[id].closed)
    }

    /// Whether `file`'s namespace contains a document root. Gates
    /// `undefined-citation` so a bare fragment is never flagged.
    pub fn is_root_component(&self, file: &Path) -> bool {
        self.component_of
            .get(file)
            .is_some_and(|&id| self.components[id].rooted)
    }

    /// Whether `file`'s namespace has a `\nocite{*}` wildcard, which makes every
    /// entry "cited" and so suppresses `undefined-citation`.
    pub fn has_wildcard_nocite(&self, file: &Path) -> bool {
        self.component_of
            .get(file)
            .is_some_and(|&id| self.components[id].wildcard)
    }
}

/// A minimal union-find (disjoint-set) with path halving and union by size. A copy
/// of [`crate::project::labels`]'s (extraction candidate).
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

    fn keys(list: &[&str]) -> Vec<SmolStr> {
        list.iter().map(SmolStr::new).collect()
    }

    fn facts(path: &str, bib: &[&str], root: bool) -> CiteFileFacts {
        CiteFileFacts {
            path: PathBuf::from(path),
            bib_targets: bib
                .iter()
                .map(|b| BibTarget::Path(PathBuf::from(b)))
                .collect(),
            nocite_all: false,
            is_document_root: root,
        }
    }

    #[test]
    fn key_in_referenced_bib_is_defined() {
        let g = graph(&[("/p/main.tex", &[])]);
        let mut bib = HashMap::new();
        bib.insert(PathBuf::from("/p/refs.bib"), keys(&["knuth1984"]));
        let r = ResolvedCitations::build(&[facts("/p/main.tex", &["/p/refs.bib"], true)], &g, &bib);

        assert!(r.is_defined(Path::new("/p/main.tex"), "knuth1984"));
        // Case-insensitive.
        assert!(r.is_defined(Path::new("/p/main.tex"), "Knuth1984"));
        assert!(!r.is_defined(Path::new("/p/main.tex"), "missing"));
        assert!(r.is_closed(Path::new("/p/main.tex")));
        assert!(r.is_root_component(Path::new("/p/main.tex")));
    }

    #[test]
    fn keys_union_across_included_files() {
        let g = graph(&[
            ("/p/main.tex", &[(IncludeKind::Input, "/p/chap.tex")]),
            ("/p/chap.tex", &[]),
        ]);
        let mut bib = HashMap::new();
        bib.insert(PathBuf::from("/p/a.bib"), keys(&["alpha"]));
        bib.insert(PathBuf::from("/p/b.bib"), keys(&["beta"]));
        let r = ResolvedCitations::build(
            &[
                facts("/p/main.tex", &["/p/a.bib"], true),
                facts("/p/chap.tex", &["/p/b.bib"], false),
            ],
            &g,
            &bib,
        );
        // Both files share one namespace, so both bibs' keys are visible from each.
        assert!(r.is_defined(Path::new("/p/chap.tex"), "alpha"));
        assert!(r.is_defined(Path::new("/p/main.tex"), "beta"));
    }

    #[test]
    fn unanalyzed_bib_opens_the_component() {
        let g = graph(&[("/p/main.tex", &[])]);
        let bib = HashMap::new(); // refs.bib not analyzed
        let r = ResolvedCitations::build(&[facts("/p/main.tex", &["/p/refs.bib"], true)], &g, &bib);
        assert!(!r.is_closed(Path::new("/p/main.tex")));
    }

    #[test]
    fn dynamic_bib_target_opens_the_component() {
        let g = graph(&[("/p/main.tex", &[])]);
        let r = ResolvedCitations::build(
            &[CiteFileFacts {
                path: PathBuf::from("/p/main.tex"),
                bib_targets: vec![BibTarget::Dynamic],
                nocite_all: false,
                is_document_root: true,
            }],
            &g,
            &HashMap::new(),
        );
        assert!(!r.is_closed(Path::new("/p/main.tex")));
    }

    #[test]
    fn dynamic_tex_include_opens_the_component() {
        let facts_list = vec![FileFacts {
            path: PathBuf::from("/p/main.tex"),
            include_edges: vec![IncludeEdgeKey {
                kind: IncludeKind::Input,
                target: IncludeTarget::Dynamic,
            }],
        }];
        let g = IncludeGraph::build(&facts_list, None);
        let mut bib = HashMap::new();
        bib.insert(PathBuf::from("/p/refs.bib"), keys(&["k"]));
        let r = ResolvedCitations::build(&[facts("/p/main.tex", &["/p/refs.bib"], true)], &g, &bib);
        assert!(!r.is_closed(Path::new("/p/main.tex")));
    }

    #[test]
    fn rootless_and_wildcard_flags() {
        let g = graph(&[("/p/frag.tex", &[])]);
        let mut bib = HashMap::new();
        bib.insert(PathBuf::from("/p/refs.bib"), keys(&["k"]));
        let r =
            ResolvedCitations::build(&[facts("/p/frag.tex", &["/p/refs.bib"], false)], &g, &bib);
        assert!(!r.is_root_component(Path::new("/p/frag.tex")));

        let with_wildcard = ResolvedCitations::build(
            &[CiteFileFacts {
                path: PathBuf::from("/p/main.tex"),
                bib_targets: vec![BibTarget::Path(PathBuf::from("/p/refs.bib"))],
                nocite_all: true,
                is_document_root: true,
            }],
            &graph(&[("/p/main.tex", &[])]),
            &bib,
        );
        assert!(with_wildcard.has_wildcard_nocite(Path::new("/p/main.tex")));
    }
}
