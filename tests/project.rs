//! Tests for the cross-file inclusion graph (`project/graph.rs`) over the salsa
//! firewall (`incremental.rs`): that the per-file `include_edges` query backdates
//! so a body edit doesn't rebuild `project_graph`, that an edge change *does*
//! rebuild it, and that re-interning an unchanged membership reuses the memo.
//!
//! The unit tests of the pure
//! extraction and graph algorithm live in `src/project/`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use badness::file_discovery::FileKind;
use badness::incremental::{IncrementalDatabase, QueryKind, QueryLogEntry, SourceFile};
use badness::project::{
    IncludeKind, Project, ProjectMember, project_graph, resolved_citations, resolved_labels,
};

fn count_by_kind(entries: &[QueryLogEntry]) -> HashMap<QueryKind, usize> {
    let mut counts = HashMap::new();
    for entry in entries {
        *counts.entry(entry.kind).or_insert(0) += 1;
    }
    counts
}

/// The path a `SourceFile` is actually tracked under. `upsert_file` lexically
/// normalizes (absolutizes) the path, which is a no-op for the `/proj/...`
/// literals on Unix but prepends a drive prefix on Windows. Member paths and
/// assertions must use this stored form so the include-graph and resolution
/// lookups (keyed on that same normalized space) match on every platform.
fn fpath(db: &IncrementalDatabase, file: SourceFile) -> PathBuf {
    db.file_path(file).to_path_buf()
}

/// Intern the membership `{main.tex, part.tex}` under `/proj`. Re-interns from a
/// fresh (sorted) snapshot on each call, as a real consumer would — so the
/// interned `Project` borrow never spans a `&mut db` write.
fn project_main_part<'db>(
    db: &'db IncrementalDatabase,
    main: SourceFile,
    part: SourceFile,
) -> Project<'db> {
    let mut members = vec![
        ProjectMember {
            file: main,
            path: fpath(db, main),
            kind: FileKind::Tex,
        },
        ProjectMember {
            file: part,
            path: fpath(db, part),
            kind: FileKind::Tex,
        },
    ];
    members.sort_by(|a, b| a.path.cmp(&b.path));
    Project::new(db, members)
}

/// Intern the membership `{main.tex, refs.bib}` under `/proj`, with `main` the
/// document root. Re-interns from a fresh sorted snapshot on each call, as a real
/// consumer would.
fn project_main_bib<'db>(
    db: &'db IncrementalDatabase,
    main: SourceFile,
    bib: SourceFile,
) -> Project<'db> {
    let mut members = vec![
        ProjectMember {
            file: main,
            path: fpath(db, main),
            kind: FileKind::Tex,
        },
        ProjectMember {
            file: bib,
            path: fpath(db, bib),
            kind: FileKind::Bib,
        },
    ];
    members.sort_by(|a, b| a.path.cmp(&b.path));
    Project::new(db, members)
}

fn main_bib(main_text: &str, bib_text: &str) -> (IncrementalDatabase, SourceFile, SourceFile) {
    let mut db = IncrementalDatabase::default();
    let main = db.upsert_file(Path::new("/proj/main.tex"), main_text.to_string());
    let bib = db.upsert_file(Path::new("/proj/refs.bib"), bib_text.to_string());
    (db, main, bib)
}

fn main_part(main_text: &str, part_text: &str) -> (IncrementalDatabase, SourceFile, SourceFile) {
    let mut db = IncrementalDatabase::default();
    let main = db.upsert_file(Path::new("/proj/main.tex"), main_text.to_string());
    let part = db.upsert_file(Path::new("/proj/part.tex"), part_text.to_string());
    (db, main, part)
}

#[test]
fn graph_resolves_an_input_edge() {
    let (db, main, part) = main_part("\\input{part}\n", "hello\n");
    let graph = project_graph(&db, project_main_part(&db, main, part));

    let out = graph.outgoing(&fpath(&db, main));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].to, fpath(&db, part));
    assert_eq!(out[0].kind, IncludeKind::Input);
    assert_eq!(graph.included_by(&fpath(&db, part)), &[fpath(&db, main)]);
    assert!(graph.unresolved().is_empty());
}

#[test]
fn body_edit_does_not_rebuild_graph() {
    // The firewall: editing part.tex's text changes its parse but not its
    // inclusion edges (it has none), so `include_edges` backdates and the
    // cross-file graph memo is reused.
    let (mut db, main, part) = main_part("\\input{part}\n", "hello\n");
    let _ = project_graph(&db, project_main_part(&db, main, part));

    db.clear_query_log();

    // Edit part's body only — still no include edges.
    db.set_file_text(part, "hello world\n");
    let _ = project_graph(&db, project_main_part(&db, main, part));

    let counts = count_by_kind(&db.query_log());
    // part re-parses and its edges are recomputed (the text changed)...
    assert_eq!(counts.get(&QueryKind::IncludeEdges), Some(&1));
    // ...but the edges are unchanged, so the graph memo is reused.
    assert_eq!(
        counts.get(&QueryKind::ProjectGraph),
        None,
        "project graph must not rebuild on a body edit"
    );
}

#[test]
fn edge_change_rebuilds_graph() {
    // The complement: adding an `\input` changes main's edges, so the graph
    // *must* rebuild (the firewall doesn't over-cache).
    let (mut db, main, part) = main_part("\\input{part}\n", "hello\n");
    let _ = project_graph(&db, project_main_part(&db, main, part));

    db.clear_query_log();

    db.set_file_text(main, "\\input{part}\n\\input{extra}\n");
    let graph = project_graph(&db, project_main_part(&db, main, part));

    let counts = count_by_kind(&db.query_log());
    assert_eq!(
        counts.get(&QueryKind::ProjectGraph),
        Some(&1),
        "project graph must rebuild when an edge changes"
    );
    // The new edge targets a non-member, so it lands in `unresolved`.
    assert_eq!(graph.unresolved().len(), 1);
    assert_eq!(graph.unresolved()[0].from, fpath(&db, main));
}

#[test]
fn resolved_labels_unions_across_the_include_graph() {
    // main.tex is the document root and `\input`s part.tex, which defines the
    // label `\ref`-ed from main — so the ref resolves cross-file.
    let (db, main, part) = main_part(
        "\\documentclass{article}\n\\input{part}\n\\ref{a}\n",
        "\\label{a}\n",
    );
    let resolved = resolved_labels(&db, project_main_part(&db, main, part));

    assert!(resolved.is_defined(&fpath(&db, main), "a"));
    assert!(!resolved.is_defined(&fpath(&db, main), "missing"));
    // `\input{part}` resolves to an analyzed member, and main is a document root.
    assert!(resolved.is_closed(&fpath(&db, main)));
    assert!(resolved.is_root_component(&fpath(&db, part)));
}

#[test]
fn prose_edit_does_not_rebuild_resolved_labels() {
    // The label/reference firewalls together: a prose edit to part.tex changes its
    // semantic model (so `file_labels` and `file_refs` both re-execute) but neither
    // its `\label`-name set nor its `\ref`-key set — so both backdate and the
    // cross-file `resolved_labels` memo is reused.
    let (mut db, main, part) = main_part(
        "\\documentclass{article}\n\\input{part}\n\\ref{a}\n",
        "\\label{a}\\ref{a}\n",
    );
    let _ = resolved_labels(&db, project_main_part(&db, main, part));

    db.clear_query_log();

    // The model changes (added prose) but the label set is still `{a}` and the ref
    // set is still `{a}`.
    db.set_file_text(part, "Prose.\n\\label{a}\\ref{a}\n");
    let _ = resolved_labels(&db, project_main_part(&db, main, part));

    let counts = count_by_kind(&db.query_log());
    // part's label and ref sets are recomputed (its model changed)...
    assert_eq!(counts.get(&QueryKind::FileLabels), Some(&1));
    assert_eq!(counts.get(&QueryKind::FileRefs), Some(&1));
    // ...but both sets are unchanged, so the resolution memo is reused.
    assert_eq!(
        counts.get(&QueryKind::ResolvedLabels),
        None,
        "resolved labels must not rebuild when neither the label nor ref set changed"
    );
}

#[test]
fn ref_change_rebuilds_resolved_labels() {
    // The reference firewall's complement: adding a `\ref` changes part's ref set,
    // so the cross-file resolution *must* rebuild — `unreferenced-label` depends on
    // the cross-file reference union, so the memo cannot over-cache across it.
    let (mut db, main, part) = main_part(
        "\\documentclass{article}\n\\input{part}\n\\ref{a}\n",
        "\\label{a}\\label{b}\n",
    );
    let _ = resolved_labels(&db, project_main_part(&db, main, part));

    db.clear_query_log();

    // Adding `\ref{b}` grows part's ref set from `{a}` to `{a, b}`.
    db.set_file_text(part, "\\label{a}\\label{b}\\ref{b}\n");
    let resolved = resolved_labels(&db, project_main_part(&db, main, part));

    let counts = count_by_kind(&db.query_log());
    assert_eq!(counts.get(&QueryKind::FileRefs), Some(&1));
    assert_eq!(
        counts.get(&QueryKind::ResolvedLabels),
        Some(&1),
        "resolved labels must rebuild when a ref set changes"
    );
    assert!(resolved.is_referenced(&fpath(&db, main), "b"));
}

#[test]
fn glossary_key_set_survives_key_preserving_edit() {
    // The glossary firewall (`file_glossary_keys`): a `\gls` use edit re-runs the
    // projection (its semantic model changed) but the key set stays *equal*, so
    // salsa can backdate — the property glossary completion's per-member reads
    // rely on. Keys arrive sorted and deduped.
    let (mut db, main, _part) = main_part(
        "\\newacronym{fps}{FPS}{frames per second}\n\\newglossaryentry{ex}{name={x}}\n",
        "\\gls{fps}\n",
    );
    assert_eq!(db.file_glossary_keys(main), ["ex", "fps"]);

    db.set_file_text(
        main,
        "\\newacronym{fps}{FPS}{frames per second}\n\\newglossaryentry{ex}{name={x}}\n\\gls{ex}\n",
    );
    assert_eq!(db.file_glossary_keys(main), ["ex", "fps"]);
}

#[test]
fn label_change_rebuilds_resolved_labels() {
    // The complement: adding a `\label` changes part's label set, so the
    // cross-file resolution *must* rebuild (the firewall doesn't over-cache).
    let (mut db, main, part) = main_part(
        "\\documentclass{article}\n\\input{part}\n\\ref{a}\n",
        "\\label{a}\n",
    );
    let _ = resolved_labels(&db, project_main_part(&db, main, part));

    db.clear_query_log();

    db.set_file_text(part, "\\label{a}\\label{b}\n");
    let resolved = resolved_labels(&db, project_main_part(&db, main, part));

    let counts = count_by_kind(&db.query_log());
    assert_eq!(
        counts.get(&QueryKind::ResolvedLabels),
        Some(&1),
        "resolved labels must rebuild when a label set changes"
    );
    assert!(resolved.is_defined(&fpath(&db, main), "b"));
}

#[test]
fn resolved_citations_unions_referenced_bib_keys() {
    // main.tex is the document root and `\addbibresource`s refs.bib, which defines
    // the cite key `\cite`d from main — so the citation resolves cross-file.
    let (db, main, bib) = main_bib(
        "\\documentclass{article}\n\\addbibresource{refs.bib}\n\\cite{knuth}\n",
        "@article{knuth, title={x}}\n",
    );
    let resolved = resolved_citations(&db, project_main_bib(&db, main, bib));

    assert!(resolved.is_defined(&fpath(&db, main), "knuth"));
    assert!(!resolved.is_defined(&fpath(&db, main), "missing"));
    // The bib resource resolves to an analyzed member, and main is a document root.
    assert!(resolved.is_closed(&fpath(&db, main)));
    assert!(resolved.is_root_component(&fpath(&db, main)));
}

#[test]
fn cite_set_preserving_edit_does_not_rebuild_resolved_citations() {
    // The cite firewall: adding a `@string` to refs.bib changes its bib model (so
    // `file_cite_names` re-executes) but not its cite-key set — so `file_cite_names`
    // backdates and the cross-file `resolved_citations` memo is reused.
    let (mut db, main, bib) = main_bib(
        "\\documentclass{article}\n\\addbibresource{refs.bib}\n\\cite{knuth}\n",
        "@article{knuth, title={x}}\n",
    );
    let _ = resolved_citations(&db, project_main_bib(&db, main, bib));

    db.clear_query_log();

    // The model changes (a new `@string` def) but the cite-key set is still `{knuth}`.
    db.set_file_text(bib, "@article{knuth, title={x}}\n@string{foo = \"bar\"}\n");
    let _ = resolved_citations(&db, project_main_bib(&db, main, bib));

    let counts = count_by_kind(&db.query_log());
    // refs.bib's cite-key set is recomputed (its model changed)...
    assert_eq!(counts.get(&QueryKind::FileCiteNames), Some(&1));
    // ...but the set is unchanged, so the resolution memo is reused.
    assert_eq!(
        counts.get(&QueryKind::ResolvedCitations),
        None,
        "resolved citations must not rebuild when no cite-key set changed"
    );
}

#[test]
fn cite_key_change_rebuilds_resolved_citations() {
    // The complement: adding an entry changes refs.bib's cite-key set, so the
    // cross-file resolution *must* rebuild (the firewall doesn't over-cache).
    let (mut db, main, bib) = main_bib(
        "\\documentclass{article}\n\\addbibresource{refs.bib}\n\\cite{knuth}\n",
        "@article{knuth, title={x}}\n",
    );
    let _ = resolved_citations(&db, project_main_bib(&db, main, bib));

    db.clear_query_log();

    db.set_file_text(
        bib,
        "@article{knuth, title={x}}\n@article{lamport, title={y}}\n",
    );
    let resolved = resolved_citations(&db, project_main_bib(&db, main, bib));

    let counts = count_by_kind(&db.query_log());
    assert_eq!(
        counts.get(&QueryKind::ResolvedCitations),
        Some(&1),
        "resolved citations must rebuild when a cite-key set changes"
    );
    assert!(resolved.is_defined(&fpath(&db, main), "lamport"));
}

#[test]
fn reinterning_same_membership_reuses_graph_memo() {
    let (db, main, part) = main_part("\\input{part}\n", "hello\n");
    let project = project_main_part(&db, main, part);
    let _ = project_graph(&db, project);

    db.clear_query_log();

    // Re-intern the identical membership: same files, same sorted paths.
    let project2 = project_main_part(&db, main, part);
    assert!(
        project == project2,
        "same membership should re-intern to the same id"
    );

    let _ = project_graph(&db, project2);
    assert_eq!(
        count_by_kind(&db.query_log()).get(&QueryKind::ProjectGraph),
        None,
        "an unchanged membership must not rebuild the graph"
    );
}
