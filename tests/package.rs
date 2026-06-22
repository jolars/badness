//! Tests for the cross-file package-load graph (`project/graph.rs`) and the merged
//! signature-scope query (`incremental.rs`) over the salsa firewall: that the
//! per-file `package_edges` query backdates so a body edit doesn't rebuild
//! `package_graph`, that a load change *does* rebuild it, and that
//! `scope_signatures` pulls a local package's definitions into a document's scope.
//!
//! Mirrors `tests/project.rs`; the pure extraction and graph-algorithm unit tests
//! live in `src/project/`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use badness::file_discovery::FileKind;
use badness::incremental::{
    IncrementalDatabase, QueryKind, QueryLogEntry, SourceFile, scope_signatures,
};
use badness::project::{PackageKind, Project, ProjectMember, package_graph};

fn count_by_kind(entries: &[QueryLogEntry]) -> HashMap<QueryKind, usize> {
    let mut counts = HashMap::new();
    for entry in entries {
        *counts.entry(entry.kind).or_insert(0) += 1;
    }
    counts
}

fn fpath(db: &IncrementalDatabase, file: SourceFile) -> PathBuf {
    db.file_path(file).to_path_buf()
}

/// Intern the membership `{main.tex, mypkg.sty}` under `/proj`. Re-interns from a
/// fresh sorted snapshot on each call, as a real consumer would.
fn project_main_pkg<'db>(
    db: &'db IncrementalDatabase,
    main: SourceFile,
    pkg: SourceFile,
) -> Project<'db> {
    let mut members = vec![
        ProjectMember {
            file: main,
            path: fpath(db, main),
            kind: FileKind::Tex,
        },
        ProjectMember {
            file: pkg,
            path: fpath(db, pkg),
            kind: FileKind::Sty,
        },
    ];
    members.sort_by(|a, b| a.path.cmp(&b.path));
    Project::new(db, members)
}

fn main_pkg(main_text: &str, pkg_text: &str) -> (IncrementalDatabase, SourceFile, SourceFile) {
    let mut db = IncrementalDatabase::default();
    let main = db.upsert_file(Path::new("/proj/main.tex"), main_text.to_string());
    let pkg = db.upsert_file(Path::new("/proj/mypkg.sty"), pkg_text.to_string());
    (db, main, pkg)
}

#[test]
fn graph_resolves_a_local_load() {
    let (db, main, pkg) = main_pkg("\\usepackage{mypkg}\n", "code\n");
    let graph = package_graph(&db, project_main_pkg(&db, main, pkg));

    let loads = graph.loads(&fpath(&db, main));
    assert_eq!(loads.len(), 1);
    assert_eq!(loads[0].to, fpath(&db, pkg));
    assert_eq!(loads[0].kind, PackageKind::UsePackage);
    assert_eq!(graph.loaded_by(&fpath(&db, pkg)), &[fpath(&db, main)]);
    assert!(graph.unresolved().is_empty());
}

#[test]
fn non_local_package_is_unresolved() {
    // `amsmath` has no sibling member, so it stays unresolved (local-only).
    let (db, main, pkg) = main_pkg("\\usepackage{amsmath}\n", "code\n");
    let graph = package_graph(&db, project_main_pkg(&db, main, pkg));
    assert!(graph.loads(&fpath(&db, main)).is_empty());
    assert_eq!(graph.unresolved().len(), 1);
}

#[test]
fn body_edit_does_not_rebuild_package_graph() {
    // The firewall: editing the package's body changes its parse but not its load
    // edges (it has none), so `package_edges` backdates and the graph memo holds.
    let (mut db, main, pkg) = main_pkg("\\usepackage{mypkg}\n", "code\n");
    let _ = package_graph(&db, project_main_pkg(&db, main, pkg));

    db.clear_query_log();
    db.set_file_text(pkg, "more code\n");
    let _ = package_graph(&db, project_main_pkg(&db, main, pkg));

    let counts = count_by_kind(&db.query_log());
    assert_eq!(counts.get(&QueryKind::PackageEdges), Some(&1));
    assert_eq!(
        counts.get(&QueryKind::PackageGraph),
        None,
        "package graph must not rebuild on a body edit"
    );
}

#[test]
fn load_change_rebuilds_package_graph() {
    let (mut db, main, pkg) = main_pkg("\\usepackage{mypkg}\n", "code\n");
    let _ = package_graph(&db, project_main_pkg(&db, main, pkg));

    db.clear_query_log();
    db.set_file_text(main, "\\usepackage{mypkg}\n\\usepackage{extra}\n");
    let graph = package_graph(&db, project_main_pkg(&db, main, pkg));

    let counts = count_by_kind(&db.query_log());
    assert_eq!(
        counts.get(&QueryKind::PackageGraph),
        Some(&1),
        "package graph must rebuild when a load changes"
    );
    // `extra` is not a member, so it lands in unresolved.
    assert_eq!(graph.unresolved().len(), 1);
}

#[test]
fn scope_signatures_pulls_in_local_package_definition() {
    let (db, main, pkg) = main_pkg(
        "\\usepackage{mypkg}\n\\myfoo{a}{b}\n",
        "\\newcommand{\\myfoo}[2]{#1#2}\n",
    );
    let scope = scope_signatures(&db, project_main_pkg(&db, main, pkg), main);
    let sig = scope.command("myfoo").expect("package command in scope");
    assert_eq!(sig.args.len(), 2);
}

#[test]
fn scope_signatures_document_definition_wins() {
    let (db, main, pkg) = main_pkg(
        "\\usepackage{mypkg}\n\\newcommand{\\dup}[2]{#1#2}\n",
        "\\newcommand{\\dup}[1]{#1}\n",
    );
    let scope = scope_signatures(&db, project_main_pkg(&db, main, pkg), main);
    // The document's 2-arg \dup overrides the package's 1-arg one.
    assert_eq!(scope.command("dup").unwrap().args.len(), 2);
}

#[test]
fn scope_signatures_backdates_on_prose_edit() {
    // Editing main's prose changes neither its loads nor its definitions, so
    // `scope_signatures` backdates: the package-defined macro stays in scope and
    // the merged query is not re-executed.
    let (mut db, main, pkg) = main_pkg(
        "\\usepackage{mypkg}\nhello\n",
        "\\newcommand{\\myfoo}[2]{#1#2}\n",
    );
    let _ = scope_signatures(&db, project_main_pkg(&db, main, pkg), main);

    db.clear_query_log();
    db.set_file_text(main, "\\usepackage{mypkg}\nhello world\n");
    let scope = scope_signatures(&db, project_main_pkg(&db, main, pkg), main);

    assert!(scope.command("myfoo").is_some());
    let counts = count_by_kind(&db.query_log());
    assert_eq!(
        counts.get(&QueryKind::ScopeSignatures),
        None,
        "scope signatures must not rebuild on a prose edit"
    );
}
