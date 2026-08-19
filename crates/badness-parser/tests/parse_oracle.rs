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

use badness_parser::parser::parse;
use parse_skeleton::texlab_has_error;

/// Corpus files badness legitimately parses cleanly but texlab cannot, each with
/// the modeled feature texlab lacks. Recorded here per the module contract above
/// rather than weakening the gate.
const TEXLAB_KNOWN_DIVERGENT: &[&str] = &[
    // doc's `\MakeShortVerb{\|}` short verbs: `|}|` and `|\begin{document}|` are
    // opaque VERB spans for badness (issue #57), unmatched braces for texlab.
    "doc_shortverb.tex",
    // TeX char-constant backtick notation (issue #60): `` \char`$ ``/`` \char`} ``
    // are plain data for badness, an unclosed math opener and an unmatched brace
    // for texlab.
    "char_constant.tex",
    // A `$` in `\def` parameter text delimits an argument; it does not open math.
    // Badness models that TeX syntax, while texlab lets the apparent math span
    // swallow the replacement body and following definition (issue #129).
    "def_dollar_delimiter.tex",
    // ltxdockit's `ltxexample`/`ltxcode` are curated verbatim-body envs (issue
    // #98); their bodies quote whole documents, so the `\begin{document}` and
    // stray `}` inside are data for badness, an unclosed env and an unmatched
    // brace for texlab, whose environment DB does not carry the signature.
    "verbatim_ltxexample.tex",
];

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
        if TEXLAB_KNOWN_DIVERGENT.contains(&name.as_ref()) {
            continue;
        }
        assert!(
            !texlab_has_error(&text),
            "texlab reported a parse error for badness-clean corpus file `{name}`"
        );
        checked += 1;
    }

    assert!(checked > 0, "no clean corpus files were checked");
}
