//! Losslessness for the BibTeX parser: `reconstruct(text) == text`, byte for
//! byte, over inline cases and every file in `tests/bib_corpus`.

use std::fs;
use std::path::Path;

use badness::bib::reconstruct;

fn assert_lossless(text: &str) {
    assert_eq!(reconstruct(text), text);
}

#[test]
fn roundtrip_units() {
    let cases = [
        "",
        "   \n\t\n",
        "@article{k, title = {Hi}, year = 2020}",
        "@book(k, a = {b})",
        "@string{jan = \"January\"}",
        "@preamble{ \"x\" }",
        "@comment{ anything {balanced} here }",
        r#"@misc{k, t = "a" # foo # {b}}"#,
        "@misc{k, t = {nested {deep {braces}}}}",
        "junk before\n@misc{a}\nbetween\n@misc{b}\nafter\n",
        "@misc{k, title = {unterminated",
        "@misc{k, title = \"unterminated",
        "@ no type",
        "@misc",
        "not bibtex at all, just prose",
    ];
    for case in cases {
        assert_lossless(case);
    }
}

#[test]
fn roundtrip_corpus() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/bib_corpus");
    let mut count = 0;
    for entry in fs::read_dir(&dir).expect("read bib corpus dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some("bib") {
            let text = fs::read_to_string(&path).expect("read bib corpus file");
            assert_eq!(reconstruct(&text), text, "losslessness failed for {path:?}");
            count += 1;
        }
    }
    assert!(count > 0, "no .bib corpus files found in {dir:?}");
}
