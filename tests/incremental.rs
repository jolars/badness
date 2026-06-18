//! Tests for the salsa incremental harness (`incremental.rs`): memoization,
//! revision-driven re-runs, the unchanged-text short-circuit, and that the
//! cached parse path preserves losslessness.

use badness::incremental::{IncrementalDatabase, QueryKind};

/// How many times `parsed_document` actually ran, per the query log.
fn parse_count(db: &IncrementalDatabase) -> usize {
    db.query_log()
        .iter()
        .filter(|entry| entry.kind == QueryKind::ParsedDocument)
        .count()
}

/// How many times `document_signatures` actually ran, per the query log.
fn signatures_count(db: &IncrementalDatabase) -> usize {
    db.query_log()
        .iter()
        .filter(|entry| entry.kind == QueryKind::DocumentSignatures)
        .count()
}

/// An owned, sorted projection of the scanned command names.
fn scanned_commands(
    db: &IncrementalDatabase,
    file: badness::incremental::SourceFile,
) -> Vec<String> {
    let mut names: Vec<String> = db
        .document_signatures(file)
        .command_names()
        .map(str::to_string)
        .collect();
    names.sort();
    names
}

#[test]
fn parsed_document_is_memoized() {
    let db = IncrementalDatabase::default();
    let file = db.add_file("\\section{Hi}\n");

    // Many reads — including two distinct consumers of the cached parse — but
    // the parse itself runs exactly once.
    let _ = db.parsed_tree(file);
    let _ = db.parsed_tree(file);
    let _ = db.parse_diagnostics(file);

    assert_eq!(parse_count(&db), 1);
}

#[test]
fn editing_text_reparses() {
    let mut db = IncrementalDatabase::default();
    let file = db.add_file("a\n");

    let _ = db.parsed_tree(file);
    assert_eq!(parse_count(&db), 1);

    db.set_file_text(file, "b\n");
    let _ = db.parsed_tree(file);
    assert_eq!(parse_count(&db), 2);
}

#[test]
fn upsert_unchanged_text_does_not_reparse() {
    let mut db = IncrementalDatabase::default();
    let path = std::path::Path::new("/tmp/doc.tex");

    let file = db.upsert_file(path, "x\n".to_string());
    let _ = db.parsed_tree(file);
    assert_eq!(parse_count(&db), 1);

    // Re-upserting identical text must not bump the revision, so the cached
    // parse stands.
    let same = db.upsert_file(path, "x\n".to_string());
    assert!(same == file);
    let _ = db.parsed_tree(same);
    assert_eq!(parse_count(&db), 1);

    // Changing the text does re-parse.
    let changed = db.upsert_file(path, "y\n".to_string());
    assert!(changed == file);
    let _ = db.parsed_tree(changed);
    assert_eq!(parse_count(&db), 2);
}

#[test]
fn cached_tree_is_lossless() {
    let db = IncrementalDatabase::default();
    let input = "\\section{Hi}\n\nbody $x^2$ % c\n";
    let file = db.add_file(input);

    assert_eq!(db.parsed_tree(file).to_string(), input);
}

#[test]
fn remove_file_stops_tracking() {
    let mut db = IncrementalDatabase::default();
    let path = std::path::Path::new("/tmp/doc.tex");

    let file = db.upsert_file(path, "x\n".to_string());
    assert!(db.lookup_file(path) == Some(file));

    // Eviction returns the dropped handle and makes the path untracked.
    assert!(db.remove_file(path) == Some(file));
    assert!(db.lookup_file(path).is_none());
    assert!(db.remove_file(path).is_none());

    // Re-opening the same path mints a *fresh* input, not the evicted one.
    let reopened = db.upsert_file(path, "x\n".to_string());
    assert!(reopened != file);
    assert!(db.lookup_file(path) == Some(reopened));
}

#[test]
fn snapshot_reads_cached_parse() {
    let mut db = IncrementalDatabase::default();
    let path = std::path::Path::new("/tmp/snap.tex");
    let file = db.upsert_file(path, "\\emph{hi}\n".to_string());
    let _ = db.parsed_tree(file);

    // A read-only snapshot sees the same cached parse off the writer.
    let snap = db.snapshot();
    let snap_file = snap.lookup_file(path).expect("tracked file");
    assert!(snap_file == file);
    assert_eq!(snap.file_text(file), "\\emph{hi}\n");
    assert!(snap.parse_diagnostics(file).is_empty());
    assert_eq!(snap.parsed_tree(file).to_string(), "\\emph{hi}\n");
}

#[test]
fn document_signatures_is_memoized() {
    let db = IncrementalDatabase::default();
    let file = db.add_file("\\newcommand{\\foo}{x}\n");

    // Many reads, but the scan runs exactly once.
    let _ = db.document_signatures(file);
    let _ = db.document_signatures(file);
    let _ = db.document_signatures(file);

    assert_eq!(signatures_count(&db), 1);
    assert_eq!(scanned_commands(&db, file), vec!["foo".to_string()]);
}

#[test]
fn editing_definitions_rebuilds_signatures() {
    let mut db = IncrementalDatabase::default();
    let file = db.add_file("\\newcommand{\\foo}{x}\n");

    assert_eq!(scanned_commands(&db, file), vec!["foo".to_string()]);
    assert_eq!(signatures_count(&db), 1);

    // Adding a definition changes the text, so the scan re-runs.
    db.set_file_text(file, "\\newcommand{\\foo}{x}\n\\newcommand{\\bar}{y}\n");
    assert_eq!(
        scanned_commands(&db, file),
        vec!["bar".to_string(), "foo".to_string()]
    );
    assert_eq!(signatures_count(&db), 2);
}

#[test]
fn prose_edit_yields_equal_signatures() {
    // Value-stability stand-in for backdating: an edit touching no definition
    // leaves the scanned DB `==` its prior value, the precondition that makes
    // salsa backdate for completion's consumer.
    let db = IncrementalDatabase::default();
    let file = db.add_file("\\newcommand{\\foo}{x}\n");

    // A fresh db with prose appended must scan to an equal DB.
    let other = IncrementalDatabase::default();
    let other_file = other.add_file("\\newcommand{\\foo}{x}\n\nsome text.\n");

    assert_eq!(
        db.document_signatures(file),
        other.document_signatures(other_file)
    );
}

#[test]
fn clone_shares_storage() {
    let db = IncrementalDatabase::default();
    let file = db.add_file("\\emph{hi}\n");
    let _ = db.parsed_tree(file);
    assert_eq!(parse_count(&db), 1);

    // A clone is a second handle onto the same storage: the file's cached parse
    // is visible without re-running, and both handles share the query log.
    let clone = db.clone();
    let _ = clone.parsed_tree(file);
    assert_eq!(parse_count(&clone), 1);
    assert_eq!(clone.file_text(file), "\\emph{hi}\n");
}
