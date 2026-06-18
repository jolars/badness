//! Differential parse oracle for BibTeX — the **hard acceptance gate** (runs in
//! `cargo test`).
//!
//! The bib analog of `parse_oracle.rs`, with one twist forced by the reference: texlab's
//! BibTeX parser has **no error channel** (no `ERROR` kind; a missing `}`/`=`/key is
//! silently skipped). So "texlab must not error" — the LaTeX gate's check — is vacuous
//! here. Instead this gate enforces an **entry-recognition floor**: for every curated
//! corpus file that *badness itself parses cleanly* (no syntax errors), texlab must
//! recognize **at least as many** top-level structured entries (`@entry` / `@string` /
//! `@preamble`) as badness does. That catches the dangerous direction — badness
//! hallucinating entry structure texlab does not see — while tolerating value-internal
//! and cite-key differences that are expected (see `bib_skeleton`).
//!
//! Scope is the *curated* `tests/bib_corpus/*.bib` (inputs expected to be clean on both
//! parsers), so this stays a true green regression guard. A clean corpus file that
//! legitimately makes the two disagree should be recorded, not papered over.

#[path = "support/bib_skeleton.rs"]
mod bib_skeleton;

use std::fs;
use std::path::Path;

use bib_skeleton::{count_entries, project_badness, project_texlab};

#[test]
fn texlab_recognizes_badness_clean_entries() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("bib_corpus");

    let mut checked = 0usize;
    for entry in fs::read_dir(&dir).expect("read bib_corpus dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("bib") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read bib corpus file");

        // Only assert on inputs badness accepts cleanly (mirrors `parse_oracle.rs`).
        if !badness::bib::parse(&text).errors.is_empty() {
            continue;
        }

        let name = path.file_name().unwrap().to_string_lossy();
        let badness_entries = count_entries(&project_badness(&text));
        let texlab_entries = count_entries(&project_texlab(&text));
        assert!(
            texlab_entries >= badness_entries,
            "entry-recognition floor violated for badness-clean corpus file `{name}`: \
             badness recognized {badness_entries} structured entries but texlab only {texlab_entries}"
        );
        checked += 1;
    }

    assert!(checked > 0, "no clean bib corpus files were checked");
}
