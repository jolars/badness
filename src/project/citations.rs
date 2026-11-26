//! Cross-file citation resolution: union the cite keys reachable from each `.tex`
//! file's namespace (via its `\bibliography`/`\addbibresource` resources) so a
//! `\cite{key}` can be checked against the whole bibliography.
//!
//! The citation analog of [`crate::project::labels`]. Namespaces are the same
//! document scopes from the include graph, but the "definitions"
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

use super::scopes::{DocumentScopes, document_scopes};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use smol_str::SmolStr;

use crate::bib::semantic::Model as BibModel;
use crate::file_discovery::FileKind;
use crate::incremental::{
    BibliographyAliasesInput, IncrementalDb, QueryKind, QueryLogEntry, file_cite_facts,
    file_cite_names,
};
use crate::project::graph::IncludeGraph;
use crate::project::include::BibTarget;

/// The distinct cite keys defined in a `.bib` `model`, sorted and deduped — the
/// per-file cite-key input to [`ResolvedCitations::build`]. Shared by the CLI
/// (one-shot, non-salsa) and the [`crate::incremental::file_cite_names`] firewall
/// so both feed identical data into the resolver. Kept raw (not lowercased);
/// [`ResolvedCitations::build`] folds case when it indexes the keys.
pub fn document_cite_names(model: &BibModel) -> Vec<SmolStr> {
    let mut names: Vec<SmolStr> = model.entries().iter().map(|e| e.key.clone()).collect();
    names.sort_unstable();
    names.dedup();
    names
}

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

/// Bibliography facts within one document scope.
#[derive(Debug, Default)]
struct Component {
    /// The cite keys available in this namespace (lowercased for case-insensitive
    /// matching, as BibTeX folds key case).
    keys: HashSet<SmolStr>,
    /// The analyzed `.bib` files this namespace draws keys from, sorted and deduped.
    /// Unlike [`keys`](Self::keys) (existence only), this carries provenance so
    /// go-to-definition can locate the entry behind a cite key. Parallel to
    /// [`labels::ResolvedLabels`](crate::project::labels)'s per-name `defs`.
    bib_paths: Vec<PathBuf>,
    /// Whether the include graph is closed *and* every bibliography resource
    /// resolved to an analyzed `.bib`. Only then is "cited but undefined"
    /// trustworthy.
    closed: bool,
    /// Whether any member has a `\nocite{*}` wildcard.
    wildcard: bool,
}

/// The resolved cross-file citation model over a set of analyzed `.tex` files and
/// the `.bib` key sets they reference.
#[derive(Debug, Default)]
pub struct ResolvedCitations {
    pub scopes: Arc<DocumentScopes>,
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
        Self::build_with_aliases(files, graph, bib_keys, &HashMap::new())
    }

    /// Resolve citations with an explicit mapping from the project-local path a
    /// document names to the actual file found through BibTeX's search path.
    /// Keeping this mapping as data preserves the pure resolver and lets
    /// navigation retain the real bibliography path.
    pub fn build_with_aliases(
        files: &[CiteFileFacts],
        graph: &IncludeGraph,
        bib_keys: &HashMap<PathBuf, Vec<SmolStr>>,
        bib_aliases: &HashMap<PathBuf, PathBuf>,
    ) -> Self {
        Self::build_in_scopes(
            files,
            bib_keys,
            bib_aliases,
            DocumentScopes::build(
                graph,
                files
                    .iter()
                    .map(|fact| (fact.path.as_path(), fact.is_document_root)),
            ),
        )
    }

    pub fn build_in_scopes(
        files: &[CiteFileFacts],
        bib_keys: &HashMap<PathBuf, Vec<SmolStr>>,
        bib_aliases: &HashMap<PathBuf, PathBuf>,
        scopes: Arc<DocumentScopes>,
    ) -> Self {
        let mut components: Vec<_> = scopes
            .scopes
            .iter()
            .map(|scope| Component {
                closed: scope.closed,
                ..Component::default()
            })
            .collect();
        for facts in files {
            for &id in scopes.containing(&facts.path) {
                let comp = &mut components[id];
                comp.wildcard |= facts.nocite_all;
                for target in &facts.bib_targets {
                    match target {
                        BibTarget::Dynamic => comp.closed = false,
                        BibTarget::Path(path) => {
                            let resolved_path = bib_keys
                                .contains_key(path)
                                .then_some(path)
                                .or_else(|| bib_aliases.get(path));
                            match resolved_path.and_then(|path| bib_keys.get_key_value(path)) {
                                Some(keys) => {
                                    let (path, keys) = keys;
                                    comp.keys.extend(
                                        keys.iter().map(|k| SmolStr::from(k.to_lowercase())),
                                    );
                                    // Record the analyzed `.bib` so go-to-def can search it.
                                    comp.bib_paths.push(path.clone());
                                }
                                // A `.bib` we never analyzed: the real key set may be larger.
                                None => comp.closed = false,
                            }
                        }
                    }
                }
            }
        }

        // A `.bib` reached from several members lands in `bib_paths` once per
        // reference; collapse to a stable, deduped set (mirrors the label resolver's
        // per-name `definers` sort/dedup).
        for comp in &mut components {
            comp.bib_paths.sort_unstable();
            comp.bib_paths.dedup();
        }

        Self { scopes, components }
    }

    pub fn bib_paths_in(&self, id: usize) -> &[PathBuf] {
        &self.components[id].bib_paths
    }

    pub fn bib_definers(&self, file: &Path) -> Vec<PathBuf> {
        self.scopes
            .containing(file)
            .iter()
            .flat_map(|&id| self.bib_paths_in(id))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn namespace_members(&self, file: &Path) -> Vec<&Path> {
        self.scopes
            .members(self.scopes.containing(file).iter().copied())
    }

    pub fn bib_scopes<'a>(&'a self, bib_path: &'a Path) -> impl Iterator<Item = usize> + 'a {
        self.components
            .iter()
            .enumerate()
            .filter_map(move |(id, component)| {
                component
                    .bib_paths
                    .iter()
                    .any(|path| path == bib_path)
                    .then_some(id)
            })
    }

    pub fn bib_citers(&self, bib_path: &Path) -> Vec<&Path> {
        self.scopes.members(self.bib_scopes(bib_path))
    }

    pub fn is_defined(&self, file: &Path, key: &str) -> bool {
        let key = key.to_lowercase();
        self.scopes
            .containing(file)
            .iter()
            .any(|&id| self.components[id].keys.contains(key.as_str()))
    }

    pub fn is_scope_closed(&self, id: usize) -> bool {
        self.components[id].closed
    }

    pub fn is_closed(&self, file: &Path) -> bool {
        let ids = self.scopes.containing(file);
        !ids.is_empty() && ids.iter().all(|&id| self.is_scope_closed(id))
    }

    pub fn is_root_component(&self, file: &Path) -> bool {
        self.scopes
            .containing(file)
            .iter()
            .any(|&id| self.scopes.scopes[id].root.is_some())
    }

    pub fn has_wildcard_nocite(&self, file: &Path) -> bool {
        self.scopes
            .containing(file)
            .iter()
            .any(|&id| self.components[id].wildcard)
    }
}

/// The cross-file citation resolution for `project`, built from the per-file
/// [`file_cite_names`] (the `.bib` cite-key firewall), [`file_cite_facts`] (the
/// `.tex` resource/wildcard firewall), and the [`crate::project::project_graph`].
///
/// `no_eq` + `unsafe(non_salsa_values)` for the same reason as
/// [`resolved_labels`](crate::project::resolved_labels): [`ResolvedCitations`]
/// holds `HashMap`s/`HashSet`s (not `Eq`/`salsa::SalsaValue`) and is a pure function
/// of the backdated [`Project`] plus the backdated per-file facts, so it carries no
/// salsa references. The firewall pays off here: a prose or `\cite` edit leaves
/// `file_cite_names`, `file_cite_facts`, `file_is_document_root`, and
/// `include_edges` all backdated, so neither [`crate::project::project_graph`] nor this query
/// re-executes.
#[salsa::tracked(returns(ref), no_eq, unsafe(non_salsa_values))]
pub fn resolved_citations(db: &dyn IncrementalDb) -> ResolvedCitations {
    db.record_query(QueryLogEntry {
        kind: QueryKind::ResolvedCitations,
        file: None,
    });

    let project = crate::project::workspace_project(db);
    let mut cite_facts: Vec<CiteFileFacts> = Vec::new();
    // Cite keys per analyzed `.bib` path, feeding [`ResolvedCitations::build`].
    let mut bib_keys: HashMap<PathBuf, Vec<SmolStr>> = HashMap::new();
    for member in &project.members {
        match member.kind {
            FileKind::Tex
            | FileKind::CodeTex
            | FileKind::Sty
            | FileKind::Cls
            | FileKind::Dtx
            | FileKind::Ins => {
                let facts = file_cite_facts(db, member.file);
                cite_facts.push(CiteFileFacts {
                    path: member.path.clone(),
                    bib_targets: facts.bib_targets.clone(),
                    nocite_all: facts.nocite_all,
                    is_document_root: false,
                });
            }
            FileKind::Bib => {
                bib_keys.insert(
                    member.path.clone(),
                    file_cite_names(db, member.file).clone(),
                );
            }
        }
    }

    let bib_aliases = BibliographyAliasesInput::get(db)
        .aliases(db)
        .iter()
        .cloned()
        .collect();
    ResolvedCitations::build_in_scopes(
        &cite_facts,
        &bib_keys,
        &bib_aliases,
        document_scopes(db).clone(),
    )
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
    fn search_path_alias_uses_the_actual_bibliography() {
        let g = graph(&[("/p/main.tex", &[])]);
        let mut bib = HashMap::new();
        bib.insert(PathBuf::from("/shared/refs.bib"), keys(&["knuth1984"]));
        let aliases = HashMap::from([(
            PathBuf::from("/p/refs.bib"),
            PathBuf::from("/shared/refs.bib"),
        )]);

        let r = ResolvedCitations::build_with_aliases(
            &[facts("/p/main.tex", &["/p/refs.bib"], true)],
            &g,
            &bib,
            &aliases,
        );

        assert!(r.is_defined(Path::new("/p/main.tex"), "knuth1984"));
        assert!(r.is_closed(Path::new("/p/main.tex")));
        assert_eq!(
            r.bib_definers(Path::new("/p/main.tex")),
            &[PathBuf::from("/shared/refs.bib")]
        );
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
    fn bib_definers_are_namespace_scoped() {
        // Two disjoint projects: main1 → a.bib, main2 → b.bib (no include edge
        // between them). Each file sees only its own component's analyzed bib.
        let g = graph(&[("/p/main1.tex", &[]), ("/p/main2.tex", &[])]);
        let mut bib = HashMap::new();
        bib.insert(PathBuf::from("/p/a.bib"), keys(&["alpha"]));
        bib.insert(PathBuf::from("/p/b.bib"), keys(&["beta"]));
        let r = ResolvedCitations::build(
            &[
                facts("/p/main1.tex", &["/p/a.bib"], true),
                facts("/p/main2.tex", &["/p/b.bib"], true),
            ],
            &g,
            &bib,
        );
        assert_eq!(
            r.bib_definers(Path::new("/p/main1.tex")),
            &[PathBuf::from("/p/a.bib")]
        );
        // b.bib lives in the other component and is not returned for main1.
        assert_eq!(
            r.bib_definers(Path::new("/p/main2.tex")),
            &[PathBuf::from("/p/b.bib")]
        );
        // An unknown file has no namespace, so no definers.
        assert!(r.bib_definers(Path::new("/p/none.tex")).is_empty());
    }

    #[test]
    fn namespace_members_and_bib_citers_scope_to_the_component() {
        // Two disjoint projects share `common.bib`; sharing a bibliography does not
        // merge their include components.
        let g = graph(&[
            ("/p/main1.tex", &[(IncludeKind::Input, "/p/chap1.tex")]),
            ("/p/chap1.tex", &[]),
            ("/p/main2.tex", &[]),
        ]);
        let mut bib = HashMap::new();
        bib.insert(PathBuf::from("/p/common.bib"), keys(&["k"]));
        let r = ResolvedCitations::build(
            &[
                facts("/p/main1.tex", &["/p/common.bib"], true),
                facts("/p/chap1.tex", &[], false),
                facts("/p/main2.tex", &["/p/common.bib"], true),
            ],
            &g,
            &bib,
        );
        // The first project's namespace is its two LaTeX members (the `.bib` is not
        // keyed in `component_of`).
        assert_eq!(
            r.namespace_members(Path::new("/p/chap1.tex")),
            &[Path::new("/p/chap1.tex"), Path::new("/p/main1.tex")]
        );
        assert_eq!(
            r.namespace_members(Path::new("/p/main2.tex")),
            &[Path::new("/p/main2.tex")]
        );
        // The shared bib is cited from both independent documents, so its citers
        // union the LaTeX members of every component referencing it.
        assert_eq!(
            r.bib_citers(Path::new("/p/common.bib")),
            &[
                Path::new("/p/chap1.tex"),
                Path::new("/p/main1.tex"),
                Path::new("/p/main2.tex"),
            ]
        );
        assert!(r.bib_citers(Path::new("/p/unknown.bib")).is_empty());
        assert!(r.namespace_members(Path::new("/p/none.tex")).is_empty());
    }

    #[test]
    fn bib_definers_only_lists_analyzed_bibs() {
        // A resolved bib plus a never-analyzed one: only the analyzed path has a
        // location to jump to, so only it is a definer (and the component is open).
        let g = graph(&[("/p/main.tex", &[])]);
        let mut bib = HashMap::new();
        bib.insert(PathBuf::from("/p/a.bib"), keys(&["alpha"]));
        let r = ResolvedCitations::build(
            &[facts("/p/main.tex", &["/p/a.bib", "/p/missing.bib"], true)],
            &g,
            &bib,
        );
        assert_eq!(
            r.bib_definers(Path::new("/p/main.tex")),
            &[PathBuf::from("/p/a.bib")]
        );
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
