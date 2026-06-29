//! Differential parse oracle — the **hard acceptance gate** (runs in `cargo test`).
//!
//! A one-way soundness check. For every
//! curated corpus file that *badness itself parses cleanly* (no syntax errors), the
//! external reference parser (texlab) must also parse it without an `ERROR` node. We
//! never compare tree shapes here — that is the soft `parse_compat.rs` gauge's job
//! (badness's generic CST and texlab's semantic CST are not meant to match).
//!
//! Scope is the *curated* `tests/corpus/*.tex` (inputs expected to be clean on both
//! parsers), so this stays a true green regression guard. A clean corpus file that
//! legitimately trips texlab should be recorded, not papered over by weakening the
//! gate.

#[path = "support/parse_skeleton.rs"]
mod parse_skeleton;

use std::fs;
use std::path::Path;

use badness::parser::parse;
use parse_skeleton::texlab_has_error;

#[test]
fn texlab_accepts_badness_clean_corpus() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("corpus");

    let mut checked = 0usize;
    for entry in fs::read_dir(&dir).expect("read corpus dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("tex") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read corpus file");

        // Only assert on inputs badness accepts cleanly (mirrors air_parser_harness).
        if !parse(&text).errors.is_empty() {
            continue;
        }

        let name = path.file_name().unwrap().to_string_lossy();
        assert!(
            !texlab_has_error(&text),
            "texlab reported a parse error for badness-clean corpus file `{name}`"
        );
        checked += 1;
    }

    assert!(checked > 0, "no clean corpus files were checked");
}
