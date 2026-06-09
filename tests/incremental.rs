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
