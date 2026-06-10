//! Tests for the `semantic_model` salsa query (`incremental.rs`): memoization,
//! revision-driven re-runs, and the value-stability that the (deferred)
//! cross-file resolver's backdating firewall will rely on.

use badness::incremental::{IncrementalDatabase, QueryKind, SourceFile};
use badness::semantic::{RefCommand, SemanticModel};

/// How many times `semantic_model` actually ran, per the query log.
fn model_count(db: &IncrementalDatabase) -> usize {
    db.query_log()
        .iter()
        .filter(|entry| entry.kind == QueryKind::SemanticModel)
        .count()
}

/// An owned, comparable projection of the model — used to assert value
/// stability across an edit without holding a salsa reference across a write.
fn snapshot(db: &IncrementalDatabase, file: SourceFile) -> Snapshot {
    project(db.semantic_model(file))
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    labels: Vec<(String, bool)>,
    refs: Vec<(String, RefCommand, bool)>,
}

fn project(model: &SemanticModel) -> Snapshot {
    Snapshot {
        labels: model
            .labels()
            .iter()
            .map(|l| (l.name.to_string(), l.referenced))
            .collect(),
        refs: model
            .refs()
            .iter()
            .map(|r| (r.name.to_string(), r.command, r.resolved))
            .collect(),
    }
}

#[test]
fn semantic_model_is_memoized() {
    let db = IncrementalDatabase::default();
    let file = db.add_file("\\label{a}\\ref{a}\n");

    // Many reads, but the model is built exactly once.
    let _ = db.semantic_model(file);
    let _ = db.semantic_model(file);
    let _ = db.semantic_model(file);

    assert_eq!(model_count(&db), 1);
}

#[test]
fn editing_labels_rebuilds_model() {
    let mut db = IncrementalDatabase::default();
    let file = db.add_file("\\label{a}\n");

    let _ = db.semantic_model(file);
    assert_eq!(model_count(&db), 1);

    // Adding a reference changes the text, so the query re-runs.
    db.set_file_text(file, "\\label{a}\\ref{a}\n");
    let _ = db.semantic_model(file);
    assert_eq!(model_count(&db), 2);
}

#[test]
fn whitespace_edit_yields_equal_model() {
    // Value-stability stand-in for backdating: an edit that touches no
    // `\label`/`\ref` leaves the model `==` its prior value. That equality is
    // exactly the precondition that makes salsa backdate once a downstream
    // consumer exists — observing the actual no-rebuild firewall is deferred to
    // the slice adding the cross-file label resolver.
    let mut db = IncrementalDatabase::default();
    let file = db.add_file("\\label{a}\\ref{a}\n");

    let before = snapshot(&db, file);

    // Insert prose and a blank line; no label/reference is altered.
    db.set_file_text(file, "\\label{a}\\ref{a}\n\nsome text.\n");
    let after = snapshot(&db, file);

    assert_eq!(before, after);
}

#[test]
fn label_edit_changes_model_value() {
    // Complement of the stability test: editing a label key must produce a
    // non-equal model, guarding against an over-eager `Eq`.
    let mut db = IncrementalDatabase::default();
    let file = db.add_file("\\label{a}\\ref{a}\n");

    let before = snapshot(&db, file);

    db.set_file_text(file, "\\label{b}\\ref{a}\n");
    let after = snapshot(&db, file);

    assert_ne!(before, after);
}
