//! Real-data check for the bib semantic model: build it over the vendored
//! `biblatex-examples.bib` and assert it collects every entry and produces no
//! false-positive duplicate-key or undefined-`@string` findings on known-good data.
//!
//! This is the end-to-end proof for Phase 1, in the spirit of the bib parse oracle:
//! the model must hold up on real biblatex input, not just hand-written fixtures.

use std::fs;
use std::path::Path;

use badness::bib::parse;
use badness::bib::semantic::Model;

fn model_of_corpus_file(name: &str) -> Model {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/bib_corpus")
        .join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let parsed = parse(&text);
    assert!(
        parsed.errors.is_empty(),
        "{name} should parse cleanly, got {} errors",
        parsed.errors.len()
    );
    Model::build(&parsed.syntax())
}

#[test]
fn biblatex_examples_collects_all_entries() {
    let model = model_of_corpus_file("biblatex-examples.bib");
    // The file carries 92 regular entries and 8 `@string` definitions.
    assert_eq!(model.entries().len(), 92, "regular entries");
    assert_eq!(model.string_defs().len(), 8, "@string definitions");
}

#[test]
fn biblatex_examples_has_no_duplicate_keys() {
    let model = model_of_corpus_file("biblatex-examples.bib");
    let dups: Vec<_> = model.duplicate_keys().map(|e| e.key.as_str()).collect();
    assert!(dups.is_empty(), "unexpected duplicate cite keys: {dups:?}");
}

#[test]
fn biblatex_examples_has_no_undefined_string_uses() {
    let model = model_of_corpus_file("biblatex-examples.bib");
    let undefined: Vec<_> = model
        .undefined_string_uses()
        .map(|u| u.name.as_str())
        .collect();
    assert!(
        undefined.is_empty(),
        "unexpected undefined @string uses (every macro is defined in-file or a month): {undefined:?}"
    );
}
