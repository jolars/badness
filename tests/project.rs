//! Tests for the cross-file inclusion graph (`project/graph.rs`) over the salsa
//! firewall (`incremental.rs`): that the per-file `include_edges` query backdates
//! so a body edit doesn't rebuild `project_graph`, that an edge change *does*
//! rebuild it, and that re-interning an unchanged membership reuses the memo.
//!
//! Mirrors arity's `tests/salsa_incremental.rs`; the unit tests of the pure
//! extraction and graph algorithm live in `src/project/`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use badness::incremental::{IncrementalDatabase, QueryKind, QueryLogEntry, SourceFile};
use badness::project::{IncludeKind, Project, ProjectMember, project_graph};

fn count_by_kind(entries: &[QueryLogEntry]) -> HashMap<QueryKind, usize> {
    let mut counts = HashMap::new();
    for entry in entries {
        *counts.entry(entry.kind).or_insert(0) += 1;
    }
    counts
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
            path: PathBuf::from("/proj/main.tex"),
        },
        ProjectMember {
            file: part,
            path: PathBuf::from("/proj/part.tex"),
        },
    ];
    members.sort_by(|a, b| a.path.cmp(&b.path));
    Project::new(db, members)
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

    let out = graph.outgoing(Path::new("/proj/main.tex"));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].to, PathBuf::from("/proj/part.tex"));
    assert_eq!(out[0].kind, IncludeKind::Input);
    assert_eq!(
        graph.included_by(Path::new("/proj/part.tex")),
        &[PathBuf::from("/proj/main.tex")]
    );
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
    assert_eq!(graph.unresolved()[0].from, PathBuf::from("/proj/main.tex"));
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
