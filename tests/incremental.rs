//! Tests for the salsa incremental harness (`incremental.rs`): memoization,
//! revision-driven re-runs, the unchanged-text short-circuit, and that the
//! cached parse path preserves losslessness.

use std::path::Path;

use badness::declarations::{Declarations, ResolvedDeclarations};
use badness::incremental::{IncrementalDatabase, IncrementalDb, QueryKind};
use badness::parser::Edit;
use badness::syntax::SyntaxKind;

/// A byte-range edit, the currency the reparse side channel stages.
fn edit(range: std::ops::Range<usize>, insert: &str) -> Edit {
    Edit {
        range,
        insert: insert.to_string(),
    }
}

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

/// How many times `parsed_bib_document` actually ran, per the query log.
fn bib_parse_count(db: &IncrementalDatabase) -> usize {
    db.query_log()
        .iter()
        .filter(|entry| entry.kind == QueryKind::ParsedBibDocument)
        .count()
}

/// How many times `bib_semantic_model` actually ran, per the query log.
fn bib_model_count(db: &IncrementalDatabase) -> usize {
    db.query_log()
        .iter()
        .filter(|entry| entry.kind == QueryKind::BibSemanticModel)
        .count()
}

/// How many times `doc_associations` actually ran, per the query log.
fn doc_assoc_count(db: &IncrementalDatabase) -> usize {
    db.query_log()
        .iter()
        .filter(|entry| entry.kind == QueryKind::DocAssociations)
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
    db.clear_query_log();

    // Many reads — including two distinct consumers of the cached parse — but
    // the parse itself runs exactly once.
    let _ = db.parsed_tree(file);
    let _ = db.parsed_tree(file);
    let _ = db.parse_diagnostics(file);

    assert_eq!(parse_count(&db), 1);
}

#[test]
fn query_log_records_only_after_an_observation_window_opens() {
    let db = IncrementalDatabase::default();
    let file = db.add_file("\\label{a}\n");

    let _ = db.parsed_tree(file);
    assert!(db.query_log().is_empty());

    db.clear_query_log();
    let _ = db.semantic_model(file);
    assert_eq!(db.query_log().len(), 1);
    assert_eq!(db.query_log()[0].kind, QueryKind::SemanticModel);
}

#[test]
fn editing_text_reparses() {
    let mut db = IncrementalDatabase::default();
    let file = db.add_file("a\n");
    db.clear_query_log();

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
    db.clear_query_log();
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

/// The language server re-upserts the *same* `Arc<str>` the live buffer holds
/// on every keystroke that lands on an unedited file, and asks whether a read
/// job's captured buffer is still current before every cached-tree read. Both
/// settle by pointer; both must still settle correctly for a text that arrived
/// by another route (a disk re-read), which shares no allocation.
#[test]
fn a_shared_text_handle_is_recognized_without_a_content_compare() {
    use std::sync::Arc;

    let mut db = IncrementalDatabase::default();
    let path = std::path::Path::new("/tmp/shared.tex");

    let held: Arc<str> = Arc::from("x\n");
    let file = db.upsert_file(path, Arc::clone(&held));
    db.clear_query_log();
    let _ = db.parsed_tree(file);
    assert_eq!(parse_count(&db), 1);

    // The same allocation, and an equal one built independently.
    let _ = db.upsert_file(path, Arc::clone(&held));
    let _ = db.upsert_file(path, "x\n".to_string());
    let _ = db.parsed_tree(file);
    assert_eq!(parse_count(&db), 1);

    assert!(db.text_is_current(file, &held));
    assert!(db.text_is_current(file, "x\n"));
    assert!(!db.text_is_current(file, "y\n"));
    // A prefix shares the pointer but not the length.
    assert!(!db.text_is_current(file, &held[..1]));
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
    assert_eq!(db.snapshot().project_members().len(), 1);

    // Eviction returns the dropped handle and makes the path untracked.
    assert!(db.remove_file(path) == Some(file));
    assert!(db.lookup_file(path).is_none());
    assert!(db.snapshot().project_members().is_empty());
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
    db.clear_query_log();

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
    db.clear_query_log();

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
fn doc_associations_is_memoized() {
    // A `.dtx` path runs the docstrip mode, so the documentation margins parse and
    // the documented `macro` surfaces. Many reads, but the query runs once.
    let mut db = IncrementalDatabase::default();
    let file = db.upsert_file(
        Path::new("doc.dtx"),
        "% \\begin{macro}{\\foo}\n% docs.\n% \\end{macro}\n".to_string(),
    );
    db.clear_query_log();

    let _ = db.doc_associations(file);
    let _ = db.doc_associations(file);
    let _ = db.doc_associations(file);

    assert_eq!(doc_assoc_count(&db), 1);
    let assocs = db.doc_associations(file);
    assert_eq!(assocs.len(), 1);
    assert_eq!(assocs[0].name, "\\foo");
}

#[test]
fn editing_dtx_rebuilds_doc_associations() {
    let mut db = IncrementalDatabase::default();
    let file = db.upsert_file(
        Path::new("doc.dtx"),
        "% \\begin{macro}{\\foo}\n% docs.\n% \\end{macro}\n".to_string(),
    );
    db.clear_query_log();

    assert_eq!(db.doc_associations(file).len(), 1);
    assert_eq!(doc_assoc_count(&db), 1);

    // Documenting a second macro changes the text, so the query re-runs.
    db.upsert_file(
        Path::new("doc.dtx"),
        "% \\begin{macro}{\\foo}\n% docs.\n% \\end{macro}\n% \\begin{macro}{\\bar}\n% docs.\n% \\end{macro}\n".to_string(),
    );
    let names: Vec<_> = db
        .doc_associations(file)
        .iter()
        .map(|a| a.name.clone())
        .collect();
    assert_eq!(names, vec!["\\foo".to_string(), "\\bar".to_string()]);
    assert_eq!(doc_assoc_count(&db), 2);
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
fn parsed_bib_document_is_memoized() {
    let db = IncrementalDatabase::default();
    let file = db.add_file("@article{k, title = {Hi}}\n");
    db.clear_query_log();

    // Several consumers of the cached bib parse, but the parse runs once.
    let _ = db.parsed_bib_tree(file);
    let _ = db.parsed_bib_tree(file);
    let _ = db.bib_parse_diagnostics(file);
    let _ = db.bib_semantic_model(file);

    assert_eq!(bib_parse_count(&db), 1);
}

#[test]
fn editing_bib_text_reparses() {
    let mut db = IncrementalDatabase::default();
    let file = db.add_file("@misc{a}\n");
    db.clear_query_log();

    let _ = db.parsed_bib_tree(file);
    assert_eq!(bib_parse_count(&db), 1);

    db.set_file_text(file, "@misc{b}\n");
    let _ = db.parsed_bib_tree(file);
    assert_eq!(bib_parse_count(&db), 2);
}

#[test]
fn cached_bib_tree_is_lossless() {
    let db = IncrementalDatabase::default();
    let input = "@article{k,\n  title = {Hi},\n  year = 2020,\n}\n";
    let file = db.add_file(input);

    assert_eq!(db.parsed_bib_tree(file).to_string(), input);
}

#[test]
fn bib_semantic_model_is_memoized() {
    let db = IncrementalDatabase::default();
    let file = db.add_file("@book{k, publisher = cup}\n@string{cup = {C}}\n");
    db.clear_query_log();

    let _ = db.bib_semantic_model(file);
    let _ = db.bib_semantic_model(file);

    assert_eq!(bib_model_count(&db), 1);
}

#[test]
fn equal_bib_edit_yields_equal_model() {
    // Value-stability stand-in for backdating: two files whose entries/keys and
    // `@string` set match build `==` models, the precondition that makes salsa
    // backdate `bib_semantic_model` (it is `Eq`, not `no_eq`).
    let db = IncrementalDatabase::default();
    let file = db.add_file("@article{k, title = {A}}\n");

    let other = IncrementalDatabase::default();
    let other_file = other.add_file("@article{k, title = {A}}\n");

    assert_eq!(
        db.bib_semantic_model(file),
        other.bib_semantic_model(other_file)
    );
}

#[test]
fn clone_shares_storage() {
    let db = IncrementalDatabase::default();
    let file = db.add_file("\\emph{hi}\n");
    db.clear_query_log();
    let _ = db.parsed_tree(file);
    assert_eq!(parse_count(&db), 1);

    // A clone is a second handle onto the same storage: the file's cached parse
    // is visible without re-running, and both handles share the query log.
    let clone = db.clone();
    let _ = clone.parsed_tree(file);
    assert_eq!(parse_count(&clone), 1);
    assert_eq!(clone.file_text(file), "\\emph{hi}\n");
}

// --- declarations (`badness.toml`; AGENTS.md decision #12) -------------------

/// A resolved declaration block, written in the TOML the user actually types.
fn declared(toml_src: &str) -> ResolvedDeclarations {
    toml::from_str::<Declarations>(toml_src)
        .expect("declarations deserialize")
        .resolve()
        .expect("declarations resolve")
}

/// Declares `mycode` to behave like `lstlisting`, so its body is verbatim — a
/// fact no scan of the document below could ever discover.
const MYCODE_VERBATIM: &str = "[environments.mycode]\nlike = 'lstlisting'\n";

/// A document whose `mycode` body only reads as protected if the environment is
/// known to be verbatim.
const MYCODE_DOC: &str = "\\begin{mycode}\n\\bad{x}\n\\end{mycode}\n";

/// Whether the cached parse captured a protected body.
fn has_verbatim_body(db: &IncrementalDatabase, file: badness::incremental::SourceFile) -> bool {
    db.parsed_tree(file)
        .descendants_with_tokens()
        .any(|el| el.kind() == SyntaxKind::VERBATIM_BODY)
}

#[test]
fn declaring_an_environment_reparses_the_file() {
    let mut db = IncrementalDatabase::default();
    let file = db.upsert_file(Path::new("main.tex"), MYCODE_DOC.to_owned());
    db.clear_query_log();

    // Declaration-blind, `mycode` is an unknown environment with an ordinary body.
    assert!(!has_verbatim_body(&db, file));
    assert_eq!(parse_count(&db), 1);

    db.clear_query_log();
    assert!(db.set_declarations(declared(MYCODE_VERBATIM)));

    // Editing the declarations reparses: the memo may not survive a change to
    // the one non-text input the parse is allowed to read.
    assert!(has_verbatim_body(&db, file));
    assert_eq!(parse_count(&db), 1);
}

#[test]
fn unchanged_declarations_do_not_reparse() {
    let mut db = IncrementalDatabase::default();
    let file = db.upsert_file(Path::new("main.tex"), MYCODE_DOC.to_owned());
    assert!(db.set_declarations(declared(MYCODE_VERBATIM)));
    let _ = db.parsed_tree(file);

    db.clear_query_log();
    // Re-publishing the same block is what the language server does on every
    // dispatch, so it must not bump the revision — a write here would reparse
    // the whole database per keystroke.
    assert!(!db.set_declarations(declared(MYCODE_VERBATIM)));
    let _ = db.parsed_tree(file);

    assert_eq!(parse_count(&db), 0);
}

/// The other half of the firewall: a command alias cannot change a tree, so
/// editing one must leave every parse memo — and every reparse base — standing.
///
/// The two tiers share one salsa input, so without the
/// `parse_declarations`/`semantic_declarations` split this write would bump the
/// revision for every parse in the project, and `parsed_document` (`no_eq`)
/// could never backdate its way out.
#[test]
fn command_declarations_do_not_reparse() {
    let mut db = IncrementalDatabase::default();
    let file = db.upsert_file(Path::new("main.tex"), MYCODE_DOC.to_owned());
    assert!(db.set_declarations(declared(MYCODE_VERBATIM)));
    assert!(has_verbatim_body(&db, file));

    db.clear_query_log();
    // The environment half is byte-for-byte what it was; only a command is added.
    let both = format!("{MYCODE_VERBATIM}\n[commands.myref]\nlike = 'cref'\n");
    assert!(db.set_declarations(declared(&both)));

    assert!(
        has_verbatim_body(&db, file),
        "the declared environment still stands"
    );
    assert_eq!(
        parse_count(&db),
        0,
        "a `[commands]` edit must not reparse the project"
    );
}

#[test]
fn command_declarations_rebuild_the_semantic_model() {
    let mut db = IncrementalDatabase::default();
    let file = db.upsert_file(
        Path::new("main.tex"),
        "\\label{a}\\label{b}\\eqrefs{a,b}\n".to_owned(),
    );
    assert!(db.semantic_model(file).refs().is_empty());

    db.clear_query_log();
    assert!(db.set_declarations(declared("[commands.eqrefs]\nlike = 'cref'\n")));
    let model = db.semantic_model(file);
    assert_eq!(
        model
            .refs()
            .iter()
            .map(|reference| reference.name.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert!(model.labels().iter().all(|label| label.referenced));
    assert!(
        db.query_log()
            .iter()
            .any(|entry| entry.kind == QueryKind::SemanticModel)
    );
    assert_eq!(
        parse_count(&db),
        0,
        "the model rebuilt on the cached tree, not a fresh parse"
    );
}

#[test]
fn declarations_are_the_top_tier_of_the_signature_scope() {
    let mut db = IncrementalDatabase::default();
    // The file defines `mycode` itself, one-argument and non-verbatim; the
    // declaration corrects that inference and must win.
    let main = db.upsert_file(
        Path::new("main.tex"),
        "\\newenvironment{mycode}[1]{#1}{}\n".to_owned(),
    );
    let scanned = db
        .snapshot()
        .scope_signatures(main)
        .environment("mycode")
        .expect("scanned environment")
        .clone();
    assert_eq!(scanned.args.len(), 1);
    assert!(!scanned.verbatim_body);

    db.set_declarations(declared(MYCODE_VERBATIM));
    let scope = db.snapshot();
    let declared_sig = scope
        .scope_signatures(main)
        .environment("mycode")
        .expect("declared environment");
    assert!(
        declared_sig.verbatim_body,
        "a declaration outranks the file's own definition"
    );
}

// ---------------------------------------------------------------------------
// The incremental-reparse side channel
// ---------------------------------------------------------------------------
//
// Reuse is invisible in a query's value by construction — the governing invariant
// is that a splice and a full parse agree byte for byte — so these assert on the
// cache's own state, and assert the value is right anyway alongside it. Phase 1
// implements no tier, so nothing splices yet; what is pinned here is the channel's
// bookkeeping, which every later phase runs on.

/// A base is installed by the first parse and refreshed by the next one, so a
/// reparse always has something to splice against.
#[test]
fn parsing_populates_and_refreshes_the_reparse_base() {
    let mut db = IncrementalDatabase::default();
    let file = db.upsert_file(Path::new("a.tex"), "\\section{One}\n".to_owned());

    assert!(db.reparse_prev(file).is_none(), "nothing parsed yet");

    db.parsed_tree(file);
    let first = db.reparse_prev(file).expect("a base after the first parse");
    assert_eq!(&*first.text, "\\section{One}\n");

    db.upsert_file(Path::new("a.tex"), "\\section{Two}\n".to_owned());
    db.parsed_tree(file);
    let second = db.reparse_prev(file).expect("a base after the edit");
    assert_eq!(&*second.text, "\\section{Two}\n");
}

/// The base shares the tracked text rather than copying it, so holding one costs a
/// refcount bump. A copy here would be a whole extra document per open buffer.
#[test]
fn the_reparse_base_shares_the_tracked_text() {
    let mut db = IncrementalDatabase::default();
    let text: std::sync::Arc<str> = std::sync::Arc::from("\\section{Hi}\n");
    let file = db.upsert_file(Path::new("a.tex"), text.clone());
    db.parsed_tree(file);

    let base = db.reparse_prev(file).expect("a base");
    assert!(
        std::sync::Arc::ptr_eq(&base.text, &text),
        "the base should hold the same allocation, not a copy"
    );
}

/// The base's tree is the one a full parse produces, so answering from it (which
/// the query does when it re-executes on unchanged text after a memo eviction) is
/// lossless like every other route.
///
/// The fast path's *predicate* is unit-tested in `src/incremental.rs`, where it is
/// visible; salsa gives no deterministic way to force a memo eviction from out here.
#[test]
fn the_reparse_base_holds_a_lossless_tree() {
    let mut db = IncrementalDatabase::default();
    let source = "\\section{Hi}\n\nbody $x^2$ % c\n\\begin{verbatim}\n  raw {\n\\end{verbatim}\n";
    let file = db.upsert_file(Path::new("a.tex"), source.to_owned());
    db.parsed_tree(file);

    let base = db.reparse_prev(file).expect("a base");
    let from_base = badness::syntax::SyntaxNode::new_root(base.green.clone());
    assert_eq!(from_base.to_string(), source);
    assert_eq!(db.parsed_tree(file).to_string(), source);
}

/// The chain is bounded where it is *staged*: under pull diagnostics the worker
/// stages an edit per keystroke and may demand no parse for a long time, so an
/// unbounded chain would grow one edit per keypress.
#[test]
fn an_unread_edit_chain_stays_bounded() {
    let mut db = IncrementalDatabase::default();
    let file = db.upsert_file(Path::new("a.tex"), "x\n".to_owned());

    for _ in 0..100 {
        db.reparse_stage_edits(file, Some(vec![edit(0..0, "a")]));
        assert!(db.reparse_pending_edits(file).len() <= 16);
    }

    // A single oversized paste clears the chain outright rather than carrying 64 KiB
    // of insert text no splice would accept anyway.
    db.reparse_stage_edits(file, Some(vec![edit(0..0, &"z".repeat(65 * 1024))]));
    assert!(db.reparse_pending_edits(file).is_empty());
}

/// The language server's write phase, end to end: splice the `didChange` into the
/// live buffer, hand the text to salsa, stage the transform. This is the phase-2
/// contract — the chain the editor stages is *exactly* the transform out of the
/// base, which is the property `reparse_edits` verifies before it splices anything.
///
/// Driven through the real `apply_content_changes` rather than hand-built edits,
/// because the thing under test is precisely that the LSP-side offset resolution
/// and the parser-side chain agree.
#[test]
fn the_language_server_write_phase_stages_the_transform_out_of_the_base() {
    use badness::lsp::apply_content_changes;
    use badness::parser::apply_edits;
    use badness::text::{PositionEncoding, TextBuffer};
    use lsp_types::{Position, Range, TextDocumentContentChangeEvent};

    let source = "\\section{Hi}\n\nbody $x^2$\n";
    let mut buffer = std::sync::Arc::new(TextBuffer::new(source, PositionEncoding::Utf16));
    let mut db = IncrementalDatabase::default();
    let path = Path::new("a.tex");
    let file = db.upsert_file(path, buffer.text_arc());
    db.parsed_tree(file);

    let base = db.reparse_prev(file).expect("a base after the first parse");
    let base_text = base.text.to_string();

    // Three keystrokes, no parse demanded in between — the pull-diagnostics shape.
    for (line, character, insert) in [(2, 4, "X"), (2, 5, "Y"), (0, 9, "Z")] {
        let at = Position::new(line, character);
        let edits = apply_content_changes(
            &mut buffer,
            vec![TextDocumentContentChangeEvent {
                range: Some(Range::new(at, at)),
                range_length: None,
                text: insert.to_owned(),
            }],
        );
        let file = db.upsert_file(path, buffer.text_arc());
        db.reparse_stage_edits(file, edits);
    }

    let staged = db.reparse_pending_edits(file);
    assert_eq!(staged.len(), 3, "one edit per keystroke, appended in order");
    assert_eq!(
        apply_edits(&base_text, &staged),
        buffer.text(),
        "the staged chain must reconstruct the buffer from the base",
    );

    // And the parse it feeds still answers exactly what a full parse would, then
    // drains what it consumed.
    assert_eq!(db.parsed_tree(file).to_string(), buffer.text());
    assert!(db.reparse_pending_edits(file).is_empty());
    let refreshed = db.reparse_prev(file).expect("a refreshed base");
    assert_eq!(&*refreshed.text, buffer.text());
}

/// Staging `None` means "the text changed by a route carrying no edits" — a disk
/// reload, a sweep. The chain must go, or it would claim to describe a transform it
/// does not.
#[test]
fn staging_an_unknown_transform_clears_the_chain() {
    let mut db = IncrementalDatabase::default();
    let file = db.upsert_file(Path::new("a.tex"), "x\n".to_owned());

    db.reparse_stage_edits(file, Some(vec![edit(0..0, "a")]));
    assert_eq!(db.reparse_pending_edits(file).len(), 1);

    db.reparse_stage_edits(file, None);
    assert!(db.reparse_pending_edits(file).is_empty());
}

/// Clearing a chain a file does not have is a no-op, entry included. The language
/// server pairs *every* `upsert_file` with a stage so the rule needs no exceptions,
/// and most of those writes are project seeding — a directory walk that reads every
/// sibling off disk. Minting an empty entry per sibling would fill the cache with
/// files nothing is editing, and only a later store ever sweeps them.
#[test]
fn staging_an_unknown_transform_does_not_mint_a_cache_entry() {
    let mut db = IncrementalDatabase::default();
    let file = db.upsert_file(Path::new("a.tex"), "x\n".to_owned());

    db.reparse_stage_edits(file, None);
    assert_eq!(
        db.reparse_cache_len(),
        0,
        "nothing to clear, nothing to hold"
    );

    // A real chain still creates one, so the no-op is about the absent entry and
    // not about `None` generally.
    db.reparse_stage_edits(file, Some(vec![edit(0..0, "a")]));
    assert_eq!(db.reparse_cache_len(), 1);
    db.reparse_stage_edits(file, None);
    assert_eq!(db.reparse_cache_len(), 1);
}

/// A parse drains the chain it consumed **even when it did not splice**. A chain
/// kept back because it failed to verify is stale forever after — it describes a
/// transform out of a text the base no longer holds — so it would fail on every
/// later parse and poison them all.
#[test]
fn a_chain_is_drained_even_when_it_does_not_splice() {
    let mut db = IncrementalDatabase::default();
    let file = db.upsert_file(Path::new("a.tex"), "x\n".to_owned());
    db.parsed_tree(file);

    // A chain that does not describe the transform at all.
    db.reparse_stage_edits(file, Some(vec![edit(0..0, "nonsense")]));
    db.upsert_file(Path::new("a.tex"), "y\n".to_owned());
    assert_eq!(db.parsed_tree(file).to_string(), "y\n");

    assert!(
        db.reparse_pending_edits(file).is_empty(),
        "the consumed chain must be dropped whether or not it spliced"
    );
}

/// The cache is bounded, and eviction is a performance concern only: an evicted
/// file still parses correctly and simply repopulates.
#[test]
fn the_reparse_cache_is_bounded() {
    let mut db = IncrementalDatabase::default();
    let mut files = Vec::new();
    for n in 0..200 {
        let file = db.upsert_file(
            Path::new(&format!("f{n}.tex")),
            format!("\\section{{S{n}}}\n"),
        );
        db.parsed_tree(file);
        files.push(file);
    }

    assert!(
        db.reparse_cache_len() <= 64,
        "cache grew to {}",
        db.reparse_cache_len()
    );

    let first = files[0];
    assert_eq!(db.parsed_tree(first).to_string(), "\\section{S0}\n");
}

/// A project-wide sweep parses every member, each storing a base it will never hit.
/// Under a plain LRU that would cost the buffer being edited its base and turn the
/// next keystroke into a full parse, so the sweep's cold entries must go first.
#[test]
fn a_sweep_does_not_evict_an_edited_buffer() {
    let mut db = IncrementalDatabase::default();
    let edited = db.upsert_file(Path::new("edited.tex"), "\\section{Hi}\n".to_owned());
    db.parsed_tree(edited);
    // An editor staging a real chain is what marks the entry hot.
    db.reparse_stage_edits(edited, Some(vec![edit(0..0, "x")]));

    // Now sweep far more files than the cache holds.
    for n in 0..200 {
        let file = db.upsert_file(
            Path::new(&format!("swept{n}.tex")),
            format!("\\section{{S{n}}}\n"),
        );
        db.parsed_tree(file);
    }

    assert!(
        db.reparse_prev(edited).is_some(),
        "the swept files should have evicted each other, not the edited buffer"
    );
}

/// Closing a file drops its base: the buffer it described is gone, and a later
/// `didOpen` mints a fresh input that could never hit the entry anyway.
#[test]
fn closing_a_file_evicts_its_reparse_base() {
    let mut db = IncrementalDatabase::default();
    let file = db.upsert_file(Path::new("a.tex"), "\\section{Hi}\n".to_owned());
    db.parsed_tree(file);
    assert!(db.reparse_prev(file).is_some());

    db.remove_file(Path::new("a.tex"));
    assert!(db.reparse_prev(file).is_none());
}

/// The base carries the inputs its tree was produced under, not just the text, so a
/// later parse can tell whether it is usable. Editing `badness.toml` changes what a
/// parse means for the same bytes.
#[test]
fn the_reparse_base_carries_the_declarations_it_was_parsed_under() {
    let mut db = IncrementalDatabase::default();
    let file = db.upsert_file(
        Path::new("main.tex"),
        "\\newenvironment{mycode}[1]{#1}{}\n".to_owned(),
    );
    db.parsed_tree(file);
    let before = db.reparse_prev(file).expect("a base");
    assert_eq!(before.declared, ResolvedDeclarations::default());

    db.set_declarations(declared(MYCODE_VERBATIM));
    db.parsed_tree(file);
    let after = db.reparse_prev(file).expect("a base");
    assert_ne!(
        after.declared,
        ResolvedDeclarations::default(),
        "the refreshed base must record the declarations in force"
    );
}
