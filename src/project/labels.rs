//! Cross-file label resolution: union the per-file label definitions across the
//! inclusion graph so a `\ref` can be resolved against the whole document, and
//! a key defined in two files of one document can be flagged as a duplicate.
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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use smol_str::SmolStr;

use crate::ast::{command_name, environment_name};
use crate::incremental::{
    IncrementalDb, QueryKind, QueryLogEntry, file_is_document_root, file_labels, file_refs,
};
use crate::project::graph::{IncludeGraph, project_graph};
use crate::project::include::{IncludeKind, IncludeTarget, collect_include_edges};
use crate::semantic::{RefCommand, SemanticModel};
use crate::syntax::{SyntaxKind, SyntaxNode};

/// The distinct label names defined in `model`, sorted and deduped—the per-file
/// label input to [`ResolvedLabels::build`]. Shared by the CLI
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

/// The distinct `\ref`-family key names *used* in `model`, sorted and deduped —
/// the per-file reference input to [`ResolvedLabels::build`], the mirror image of
/// [`document_label_names`]. A `\cref{a,b}` contributes both `a` and `b` (the
/// model already splits key lists). Feeds the cross-file `unreferenced-label`
/// lint, which asks whether a label definition is targeted *anywhere* in the
/// namespace.
pub fn document_ref_names(model: &SemanticModel) -> Vec<SmolStr> {
    let mut names: Vec<SmolStr> = model.refs().iter().map(|r| r.name.clone()).collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// The distinct glossary/acronym keys defined in `model`
/// (`\newglossaryentry`/`\newacronym`/…), sorted and deduped — the per-file input
/// to the [`crate::incremental::file_glossary_keys`] firewall, the glossary
/// analog of [`document_label_names`].
pub fn document_glossary_keys(model: &SemanticModel) -> Vec<SmolStr> {
    let mut keys: Vec<SmolStr> = model
        .glossary_defs()
        .iter()
        .map(|def| def.key.clone())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
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
    /// Every `\ref`-family key *used* by any file in the component. Membership
    /// only (order-free), so a `HashSet`: `unreferenced-label` asks "is this
    /// label referenced somewhere in the namespace?", the mirror of `defs`.
    refs: HashSet<SmolStr>,
    /// Whether every include in the component resolves to an analyzed member: no
    /// dynamic and no external (out-of-set) targets. Only then is "defined
    /// nowhere" trustworthy enough to drive `undefined-ref`.
    closed: bool,
    /// Whether any member is a document root (`\documentclass` /
    /// `\begin{document}`). `undefined-ref` fires only inside a rooted namespace.
    rooted: bool,
}

#[derive(Debug, Clone)]
struct NumberedUnit {
    name: SmolStr,
    offset: usize,
}

#[derive(Debug, Clone)]
enum RangeEventKind {
    Unit(Option<NumberedUnit>),
    Include(IncludeKind, IncludeTarget),
}

#[derive(Debug, Clone)]
struct RangeEvent {
    start: usize,
    end: usize,
    clean_before: bool,
    kind: RangeEventKind,
}

/// Source-ordered equation and inclusion facts for one parsed file. This is
/// separate from the range-free label-name firewall used by the editor query.
#[derive(Debug, Clone)]
pub struct EquationRangeFacts {
    path: PathBuf,
    labels: Vec<SmolStr>,
    pairs: Vec<(SmolStr, SmolStr)>,
    events: Vec<RangeEvent>,
    trailing_clean: bool,
    ambiguous: bool,
}

impl EquationRangeFacts {
    pub fn collect(path: &Path, root: &SyntaxNode, model: &SemanticModel) -> Self {
        let source = root.text().to_string();
        let mut refs: Vec<_> = model
            .refs()
            .iter()
            .filter(|reference| reference.command == RefCommand::EqRef)
            .collect();
        refs.sort_by_key(|reference| reference.range.start());
        let pairs = refs
            .windows(2)
            .filter_map(|pair| {
                let gap = usize::from(pair[0].range.end())..usize::from(pair[1].range.start());
                (source.get(gap)?.trim() == "--")
                    .then(|| (pair[0].name.clone(), pair[1].name.clone()))
            })
            .collect();

        let mut events = Vec::new();
        for environment in root
            .descendants()
            .filter(|node| node.kind() == SyntaxKind::ENVIRONMENT)
        {
            if environment.ancestors().skip(1).any(|ancestor| {
                ancestor.kind() == SyntaxKind::ENVIRONMENT
                    && ancestor
                        .children()
                        .find(|child| child.kind() == SyntaxKind::BEGIN)
                        .and_then(|begin| environment_name(&begin))
                        .is_some_and(|name| {
                            matches!(name.as_str(), "equation" | "align" | "gather")
                        })
            }) {
                continue;
            }
            let Some(begin) = environment
                .children()
                .find(|node| node.kind() == SyntaxKind::BEGIN)
            else {
                continue;
            };
            let Some(name) = environment_name(&begin) else {
                continue;
            };
            if !matches!(name.as_str(), "equation" | "align" | "gather") {
                continue;
            }
            let Some(end) = environment.children().find(|node| {
                node.kind() == SyntaxKind::END
                    && environment_name(node).as_deref() == Some(name.as_str())
            }) else {
                continue;
            };
            let Some(math) = environment
                .children()
                .find(|node| node.kind() == SyntaxKind::MATH)
            else {
                continue;
            };
            let mut span_start = usize::from(environment.text_range().start());
            let mut content_start = usize::from(math.text_range().start());
            if name != "equation" {
                for line_break in math
                    .children()
                    .filter(|node| node.kind() == SyntaxKind::LINE_BREAK)
                {
                    let content_end = usize::from(line_break.text_range().start());
                    events.push(RangeEvent {
                        start: span_start,
                        end: usize::from(line_break.text_range().end()),
                        clean_before: false,
                        kind: RangeEventKind::Unit(numbered_unit(
                            model,
                            &math,
                            content_start..content_end,
                        )),
                    });
                    span_start = usize::from(line_break.text_range().end());
                    content_start = span_start;
                }
            }
            events.push(RangeEvent {
                start: span_start,
                end: usize::from(end.text_range().end()),
                clean_before: false,
                kind: RangeEventKind::Unit(numbered_unit(
                    model,
                    &math,
                    content_start..usize::from(math.text_range().end()),
                )),
            });
        }

        let executable_includes: HashSet<_> = root
            .descendants()
            .filter(|node| node.kind() == SyntaxKind::COMMAND)
            .filter(|node| {
                node.ancestors()
                    .skip(1)
                    .all(|ancestor| match ancestor.kind() {
                        SyntaxKind::COMMAND
                        | SyntaxKind::GROUP
                        | SyntaxKind::OPTIONAL
                        | SyntaxKind::MATH
                        | SyntaxKind::CONDITIONAL => false,
                        SyntaxKind::ENVIRONMENT => {
                            ancestor
                                .children()
                                .find(|child| child.kind() == SyntaxKind::BEGIN)
                                .and_then(|begin| environment_name(&begin))
                                .as_deref()
                                == Some("document")
                        }
                        _ => true,
                    })
            })
            .map(|node| node.text_range())
            .collect();
        for edge in collect_include_edges(root, path.parent()) {
            if matches!(
                edge.kind,
                IncludeKind::SubFilesParent | IncludeKind::GlsEntries
            ) {
                continue;
            }
            let start = usize::from(edge.range.start());
            let end = usize::from(edge.range.end());
            // Inclusion inside a numbered unit cannot be treated as a separate
            // source event. The unit itself is declined by `numbered_unit`.
            if events
                .iter()
                .any(|event| event.start <= start && end <= event.end)
            {
                continue;
            }
            if executable_includes.contains(&edge.range) {
                events.push(RangeEvent {
                    start,
                    end,
                    clean_before: false,
                    kind: RangeEventKind::Include(edge.kind, edge.target),
                });
            }
        }
        events.sort_by_key(|event| (event.start, event.end));
        let mut cursor = 0;
        for event in &mut events {
            event.clean_before = source
                .get(cursor..event.start)
                .is_some_and(|gap| gap.trim().is_empty());
            cursor = event.end;
        }
        let trailing_clean = source
            .get(cursor..)
            .is_some_and(|gap| gap.trim().is_empty());

        Self {
            path: path.to_path_buf(),
            labels: model
                .labels()
                .iter()
                .map(|label| label.name.clone())
                .collect(),
            pairs,
            events,
            trailing_clean,
            ambiguous: root.descendants().any(|node| {
                node.kind() == SyntaxKind::COMMAND
                    && command_name(&node).as_deref() == Some("includeonly")
            }),
        }
    }

    pub(crate) fn local_inferred(&self, resolution: &ResolvedLabels) -> HashSet<usize> {
        let mut stream = Vec::new();
        for event in &self.events {
            if !event.clean_before {
                stream.push(None);
            }
            match &event.kind {
                RangeEventKind::Unit(Some(unit)) => stream.push(Some(RangeSite {
                    path: self.path.clone(),
                    unit: unit.clone(),
                })),
                _ => stream.push(None),
            }
        }
        let counts = label_counts(std::iter::once(self));
        infer_pairs(resolution, &self.path, &self.pairs, &stream, &counts)
            .into_iter()
            .map(|site| site.unit.offset)
            .collect()
    }
}

fn numbered_unit(
    model: &SemanticModel,
    math: &SyntaxNode,
    content: std::ops::Range<usize>,
) -> Option<NumberedUnit> {
    let labels: Vec<_> = model
        .labels()
        .iter()
        .filter(|label| {
            content.start <= usize::from(label.range.start())
                && usize::from(label.range.end()) <= content.end
        })
        .collect();
    let [label] = labels.as_slice() else {
        return None;
    };
    if model
        .labels()
        .iter()
        .filter(|other| other.name == label.name)
        .count()
        != 1
    {
        return None;
    }
    if math.descendants().any(|node| {
        node.kind() == SyntaxKind::COMMAND
            && content.start <= usize::from(node.text_range().start())
            && usize::from(node.text_range().end()) <= content.end
            && command_name(&node)
                .as_deref()
                .is_some_and(breaks_equation_range)
    }) {
        return None;
    }
    Some(NumberedUnit {
        name: label.name.clone(),
        offset: usize::from(label.range.start()),
    })
}

fn breaks_equation_range(name: &str) -> bool {
    matches!(
        name,
        "tag"
            | "notag"
            | "nonumber"
            | "setcounter"
            | "addtocounter"
            | "stepcounter"
            | "refstepcounter"
            | "counterwithin"
            | "numberwithin"
            | "input"
            | "include"
            | "import"
            | "subimport"
            | "subfile"
            | "subfileinclude"
            | "intertext"
            | "shortintertext"
    )
}

#[derive(Debug, Clone)]
struct RangeSite {
    path: PathBuf,
    unit: NumberedUnit,
}

fn label_counts<'a>(
    facts: impl Iterator<Item = &'a EquationRangeFacts>,
) -> HashMap<SmolStr, usize> {
    let mut counts = HashMap::new();
    for fact in facts {
        for label in &fact.labels {
            *counts.entry(label.clone()).or_insert(0) += 1;
        }
    }
    counts
}

fn infer_pairs(
    resolution: &ResolvedLabels,
    origin: &Path,
    pairs: &[(SmolStr, SmolStr)],
    stream: &[Option<RangeSite>],
    counts: &HashMap<SmolStr, usize>,
) -> Vec<RangeSite> {
    if pairs.is_empty() {
        return Vec::new();
    }
    let positions: HashMap<_, _> = stream
        .iter()
        .enumerate()
        .filter_map(|(index, site)| site.as_ref().map(|site| (site.unit.name.clone(), index)))
        .collect();
    let mut inferred = Vec::new();
    for (first, last) in pairs {
        if counts.get(first) != Some(&1)
            || counts.get(last) != Some(&1)
            || resolution.definers(origin, first).len() != 1
            || resolution.definers(origin, last).len() != 1
        {
            continue;
        }
        let (Some(&start), Some(&end)) = (positions.get(first), positions.get(last)) else {
            continue;
        };
        if end <= start + 1 || stream[start + 1..end].iter().any(Option::is_none) {
            continue;
        }
        for site in stream[start + 1..end].iter().flatten() {
            if counts.get(&site.unit.name) == Some(&1)
                && resolution.definers(origin, &site.unit.name).len() == 1
            {
                inferred.push(site.clone());
            }
        }
    }
    inferred
}

fn walk_range_source(
    path: &Path,
    facts: &HashMap<&Path, &EquationRangeFacts>,
    graph: &IncludeGraph,
    visited: &mut HashSet<PathBuf>,
    stream: &mut Vec<Option<RangeSite>>,
) -> bool {
    if visited.len() >= 256 || !visited.insert(path.to_path_buf()) {
        return false;
    }
    let Some(fact) = facts.get(path) else {
        return false;
    };
    for event in &fact.events {
        if !event.clean_before {
            stream.push(None);
        }
        match &event.kind {
            RangeEventKind::Unit(Some(unit)) => stream.push(Some(RangeSite {
                path: path.to_path_buf(),
                unit: unit.clone(),
            })),
            RangeEventKind::Unit(None) => stream.push(None),
            RangeEventKind::Include(kind, IncludeTarget::Path(target)) => {
                if !graph
                    .outgoing(path)
                    .iter()
                    .any(|edge| edge.to == *target && edge.kind == *kind)
                    || !walk_range_source(target, facts, graph, visited, stream)
                {
                    return false;
                }
            }
            RangeEventKind::Include(_, IncludeTarget::Dynamic) => return false,
        }
    }
    if !fact.trailing_clean {
        stream.push(None);
    }
    true
}

/// The resolved cross-file label model over a set of analyzed files.
///
/// Holds `HashMap`s/`PathBuf`s, so (like [`IncludeGraph`]) it is neither `Eq` nor
/// `salsa::SalsaValue`; the [`crate::project::resolved_labels`] query is therefore
/// `no_eq`. Built by [`ResolvedLabels::build`].
#[derive(Debug, Default)]
pub struct ResolvedLabels {
    /// File path → index into [`components`](Self::components).
    component_of: HashMap<PathBuf, usize>,
    components: Vec<Component>,
    inferred_ranges: HashMap<PathBuf, HashSet<usize>>,
    range_fact_paths: HashSet<PathBuf>,
}

impl ResolvedLabels {
    /// Resolve labels for `files` — each a `(path, distinct sorted label names,
    /// distinct sorted `\ref` key names, is_document_root)` tuple — partitioned by
    /// the inclusion `graph`.
    ///
    /// Pure and deterministic: components are assigned in sorted-path order and
    /// every definer list is sorted, so the output never depends on `HashMap`
    /// iteration order. (The per-component reference set is queried by membership
    /// only, so its iteration order never reaches the output.)
    pub fn build(
        files: &[(PathBuf, Vec<SmolStr>, Vec<SmolStr>, bool)],
        graph: &IncludeGraph,
    ) -> Self {
        // Sorted, unique member paths give union-find a deterministic index space.
        let mut paths: Vec<&Path> = files.iter().map(|(p, _, _, _)| p.as_path()).collect();
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

        // Index definitions, references, and the rooted flag per component.
        for (path, names, refs, is_root) in files {
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
            comp.refs.extend(refs.iter().cloned());
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
            inferred_ranges: HashMap::new(),
            range_fact_paths: HashSet::new(),
        }
    }

    /// Build label resolution with source-ordered equation facts for the lint
    /// path. The range-free `build` remains the editor query's cheap input.
    pub fn build_with_range_facts(
        files: &[(PathBuf, Vec<SmolStr>, Vec<SmolStr>, bool)],
        graph: &IncludeGraph,
        facts: &[EquationRangeFacts],
    ) -> Self {
        let mut resolved = Self::build(files, graph);
        resolved.range_fact_paths = facts.iter().map(|fact| fact.path.clone()).collect();
        let by_path: HashMap<&Path, &EquationRangeFacts> = facts
            .iter()
            .map(|fact| (fact.path.as_path(), fact))
            .collect();

        // Same-file inference remains available even when cross-file execution
        // order cannot be established for the component.
        for fact in facts {
            resolved
                .inferred_ranges
                .insert(fact.path.clone(), fact.local_inferred(&resolved));
        }

        for component_id in 0..resolved.components.len() {
            if !resolved.components[component_id].closed
                || !resolved.components[component_id].rooted
            {
                continue;
            }
            let members: Vec<_> = resolved
                .component_of
                .iter()
                .filter(|&(_, &id)| id == component_id)
                .map(|(path, _)| path.as_path())
                .collect();
            let roots: Vec<_> = files
                .iter()
                .filter(|(path, _, _, rooted)| {
                    *rooted && resolved.component_of.get(path) == Some(&component_id)
                })
                .map(|(path, _, _, _)| path.as_path())
                .collect();
            let [root] = roots.as_slice() else { continue };
            if members
                .iter()
                .any(|path| by_path.get(path).is_none_or(|fact| fact.ambiguous))
            {
                continue;
            }
            if members
                .iter()
                .all(|path| by_path.get(path).is_none_or(|fact| fact.pairs.is_empty()))
            {
                continue;
            }
            let mut visited = HashSet::new();
            let mut stream = Vec::new();
            if !walk_range_source(root, &by_path, graph, &mut visited, &mut stream) {
                continue;
            }
            if visited.len() != members.len() {
                continue;
            }
            let component_facts: Vec<_> = members
                .iter()
                .filter_map(|path| by_path.get(path).copied())
                .collect();
            let counts = label_counts(component_facts.iter().copied());
            for fact in component_facts {
                for site in infer_pairs(&resolved, &fact.path, &fact.pairs, &stream, &counts) {
                    resolved
                        .inferred_ranges
                        .entry(site.path)
                        .or_default()
                        .insert(site.unit.offset);
                }
            }
        }
        resolved
    }

    /// Whether a particular label definition is used by a proven equation
    /// range. Explicit reference membership remains in `is_referenced`.
    pub fn is_range_referenced(&self, file: &Path, label_offset: usize) -> bool {
        self.inferred_ranges
            .get(file)
            .is_some_and(|offsets| offsets.contains(&label_offset))
    }

    pub(crate) fn has_range_facts(&self, file: &Path) -> bool {
        self.range_fact_paths.contains(file)
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

    /// Whether `name` is targeted by a `\ref`-family command anywhere in `file`'s
    /// namespace. The mirror of [`is_defined`](Self::is_defined): `undefined-ref`
    /// asks whether a *reference* has a definition, `unreferenced-label` asks
    /// whether a *definition* has a reference. Both are trustworthy only over a
    /// closed, rooted namespace (see [`is_closed`](Self::is_closed) /
    /// [`is_root_component`](Self::is_root_component)).
    pub fn is_referenced(&self, file: &Path, name: &str) -> bool {
        self.component_of
            .get(file)
            .is_some_and(|&id| self.components[id].refs.contains(name))
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
/// `no_eq` + `unsafe(non_salsa_values)` for the same reason as [`project_graph`]:
/// [`ResolvedLabels`] holds `HashMap`s (not `Eq`/`salsa::SalsaValue`) and is a pure
/// function of the backdated [`Project`] plus the backdated per-file facts, so it
/// carries no salsa references. The firewall pays off here: a prose edit leaves
/// `file_labels`, `file_refs`, `file_is_document_root`, and `include_edges` all
/// backdated, so neither [`project_graph`] nor this query re-executes. A `\ref`
/// edit *does* rebuild this query (it changes `file_refs`), because
/// `unreferenced-label` depends on the cross-file reference union — but a pure
/// prose edit still backdates both firewalls.
#[salsa::tracked(returns(ref), no_eq, unsafe(non_salsa_values))]
pub fn resolved_labels(db: &dyn IncrementalDb) -> ResolvedLabels {
    db.record_query(QueryLogEntry {
        kind: QueryKind::ResolvedLabels,
        file: None,
    });

    let project = crate::project::workspace_project(db);
    let graph = project_graph(db);
    // Labels live in LaTeX files (`.tex`/`.sty`/`.cls`); `.bib` members carry none
    // and are not part of the include-graph namespace.
    let files: Vec<(PathBuf, Vec<SmolStr>, Vec<SmolStr>, bool)> = project
        .members
        .iter()
        .filter(|member| member.kind.is_latex())
        .map(|member| {
            (
                member.path.clone(),
                file_labels(db, member.file).clone(),
                file_refs(db, member.file).clone(),
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
    fn inferred_range_uses_do_not_enter_explicit_reference_set() {
        let path = Path::new("/p/doc.tex");
        let source = "\\documentclass{article}\n\\begin{equation}a=1\\label{A}\\end{equation}\n\\begin{equation}b=2\\label{B}\\end{equation}\n\\begin{equation}c=3\\label{C}\\end{equation}\n\\eqref{A}--\\eqref{C}\n";
        let root = crate::parser::parse(source).syntax();
        let model = SemanticModel::build(&root);
        let graph = graph(&[("/p/doc.tex", &[])]);
        let facts = [EquationRangeFacts::collect(path, &root, &model)];
        let resolved = ResolvedLabels::build_with_range_facts(
            &[(
                path.to_path_buf(),
                document_label_names(&model),
                document_ref_names(&model),
                true,
            )],
            &graph,
            &facts,
        );
        let middle = model
            .labels()
            .iter()
            .find(|label| label.name == "B")
            .unwrap();
        assert!(resolved.is_range_referenced(path, usize::from(middle.range.start())));
        assert!(!resolved.is_referenced(path, "B"));
    }

    #[test]
    fn lone_file_is_its_own_component() {
        let g = graph(&[("/p/a.tex", &[])]);
        let r = ResolvedLabels::build(
            &[(PathBuf::from("/p/a.tex"), names(&["x"]), names(&[]), false)],
            &g,
        );
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
                (
                    PathBuf::from("/p/main.tex"),
                    names(&[]),
                    names(&["a"]),
                    true,
                ),
                (
                    PathBuf::from("/p/chap.tex"),
                    names(&["a"]),
                    names(&[]),
                    false,
                ),
            ],
            &g,
        );
        // A label in the chapter is visible from the main file's namespace.
        assert!(r.is_defined(Path::new("/p/main.tex"), "a"));
        assert!(r.is_root_component(Path::new("/p/chap.tex")));
        assert!(r.is_closed(Path::new("/p/main.tex")));
        // The chapter's `\label{a}` is referenced cross-file (from main), visible
        // when the whole namespace is queried from either member.
        assert!(r.is_referenced(Path::new("/p/chap.tex"), "a"));
        assert!(!r.is_referenced(Path::new("/p/chap.tex"), "b"));
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
                (PathBuf::from("/p/main.tex"), names(&[]), names(&[]), true),
                (PathBuf::from("/p/a.tex"), names(&["k"]), names(&[]), false),
                (PathBuf::from("/p/b.tex"), names(&["k"]), names(&[]), false),
                (
                    PathBuf::from("/p/shared.tex"),
                    names(&[]),
                    names(&[]),
                    false,
                ),
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
                (PathBuf::from("/p/one.tex"), names(&["x"]), names(&[]), true),
                (PathBuf::from("/p/two.tex"), names(&["x"]), names(&[]), true),
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
                (
                    PathBuf::from("/p/one.tex"),
                    names(&["intro"]),
                    names(&[]),
                    true,
                ),
                (
                    PathBuf::from("/p/two.tex"),
                    names(&["intro"]),
                    names(&[]),
                    true,
                ),
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
                (PathBuf::from("/p/a.tex"), names(&["x"]), names(&[]), false),
                (PathBuf::from("/p/b.tex"), names(&[]), names(&[]), false),
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
        let r = ResolvedLabels::build(
            &[(PathBuf::from("/p/main.tex"), names(&[]), names(&[]), true)],
            &g,
        );
        assert!(!r.is_closed(Path::new("/p/main.tex")));
    }

    #[test]
    fn external_include_opens_the_component() {
        // `/p/missing.tex` is not an analyzed member → unresolved → open.
        let g = graph(&[("/p/main.tex", &[(IncludeKind::Input, "/p/missing.tex")])]);
        let r = ResolvedLabels::build(
            &[(PathBuf::from("/p/main.tex"), names(&[]), names(&[]), true)],
            &g,
        );
        assert!(!r.is_closed(Path::new("/p/main.tex")));
    }

    #[test]
    fn rootless_component_reports_no_root() {
        let g = graph(&[("/p/frag.tex", &[])]);
        let r = ResolvedLabels::build(
            &[(
                PathBuf::from("/p/frag.tex"),
                names(&["x"]),
                names(&[]),
                false,
            )],
            &g,
        );
        assert!(!r.is_root_component(Path::new("/p/frag.tex")));
        assert!(r.is_closed(Path::new("/p/frag.tex")));
    }

    #[test]
    fn is_referenced_tracks_the_component_reference_union() {
        // One namespace: `a` is defined and referenced (in-file), `b` is defined
        // but never referenced anywhere, `c` is referenced but undefined.
        let g = graph(&[("/p/a.tex", &[])]);
        let r = ResolvedLabels::build(
            &[(
                PathBuf::from("/p/a.tex"),
                names(&["a", "b"]),
                names(&["a", "c"]),
                true,
            )],
            &g,
        );
        assert!(r.is_referenced(Path::new("/p/a.tex"), "a"));
        assert!(!r.is_referenced(Path::new("/p/a.tex"), "b"));
        // A referenced-but-undefined key still reads as referenced (that is
        // `undefined-ref`'s concern, not this method's).
        assert!(r.is_referenced(Path::new("/p/a.tex"), "c"));
        // An unknown file has an empty reference set.
        assert!(!r.is_referenced(Path::new("/p/missing.tex"), "a"));
    }
}
