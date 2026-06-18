//! Phase 2 BibTeX/BibLaTeX formatter tests. Exact output is pinned by
//! `tests/fixtures/bib_format/<name>/{input,expected}.bib` pairs (mirroring the
//! LaTeX `tests/fixtures/formatter/` layout). The `AGENTS.md` invariants —
//! idempotence and losslessness *of the formatted text* (the formatter normalizes,
//! so its output need not equal its input) — plus a meaning-preservation check are
//! asserted on every fixture and every clean-parsing corpus file.

use std::fs;
use std::path::Path;

use badness::bib::semantic::Model;
use badness::bib::syntax::SyntaxKind;
use badness::bib::{ast, format, format_with_style, parse, reconstruct};
use badness::formatter::FormatStyle;

/// The semantic facts formatting must preserve: each entry's (type, key) in source
/// order, the `@string` definition names, and the `@string` use names. Byte ranges
/// are intentionally dropped — they shift when layout changes — so this compares the
/// *meaning*, not the positions.
fn meaning(text: &str) -> (Vec<(String, String)>, Vec<String>, Vec<String>) {
    let model = Model::build(&parse(text).syntax());
    let entries = model
        .entries()
        .iter()
        .map(|e| (e.entry_type.to_string(), e.key.to_string()))
        .collect();
    let mut defs: Vec<String> = model
        .string_defs()
        .iter()
        .map(|d| d.name.to_string())
        .collect();
    let mut uses: Vec<String> = model
        .string_uses()
        .iter()
        .map(|u| u.name.to_string())
        .collect();
    defs.sort();
    uses.sort();
    (entries, defs, uses)
}

/// Every field's `(name_lc, value-signature)` across the document, in source order.
/// The signature is the value text with all whitespace and value delimiters (`"`,
/// `{`, `}`) removed — the formatter is allowed to insert/remove whitespace (reflow,
/// ` # ` spacing) and rewrite `"…"` → `{…}`, but it must never add, drop, reorder, or
/// mangle the actual content characters. This catches a reflow bug (a dropped or
/// duplicated word, a split inside a braced token) that the entry/`@string`-level
/// `meaning()` oracle cannot see, since that oracle never inspects value content.
fn field_values(text: &str) -> Vec<(String, String)> {
    fn signature(value_text: &str) -> String {
        value_text
            .chars()
            .filter(|c| !c.is_whitespace() && !matches!(c, '"' | '{' | '}'))
            .collect()
    }
    parse(text)
        .syntax()
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::FIELD)
        .filter_map(|field| {
            let name = ast::field_name(&field)?.to_lowercase();
            let value = ast::field_value(&field)?;
            Some((name, signature(&value.to_string())))
        })
        .collect()
}

/// Assert the formatter invariants for one clean-parsing input. Inputs the parser
/// rejects are out of scope (the formatter refuses them), so callers filter those.
fn assert_bib_format_invariants(input: &str) {
    let formatted = format(input).expect("clean input should format");

    // Idempotence: fmt(fmt(x)) == fmt(x).
    let twice = format(&formatted).expect("formatted output should re-format");
    assert_eq!(twice, formatted, "format is not idempotent for {input:?}");

    // The formatted output is itself a clean, lossless document.
    assert!(
        parse(&formatted).errors.is_empty(),
        "formatted output should parse without diagnostics for {input:?}"
    );
    assert_eq!(
        reconstruct(&formatted),
        formatted,
        "formatted output should round-trip losslessly for {input:?}"
    );

    // Meaning preserved: same entries, @string defs, and @string uses.
    assert_eq!(
        meaning(input),
        meaning(&formatted),
        "formatting changed meaning for {input:?}"
    );

    // Value content preserved modulo whitespace and delimiters: reflow only moves
    // whitespace, so no field's content characters may change.
    assert_eq!(
        field_values(input),
        field_values(&formatted),
        "formatting changed a field value's content for {input:?}"
    );
}

#[test]
fn format_fixtures() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bib_format");
    let mut count = 0;
    for entry in fs::read_dir(&dir).expect("read bib_format fixtures dir") {
        let case = entry.expect("dir entry").path();
        if !case.is_dir() {
            continue;
        }
        let input = fs::read_to_string(case.join("input.bib")).expect("read input.bib");
        let expected = fs::read_to_string(case.join("expected.bib")).expect("read expected.bib");

        let formatted = format(&input).expect("fixture input should format");
        assert_eq!(
            formatted,
            expected,
            "fixture {:?} output mismatch",
            case.file_name().unwrap()
        );
        assert_bib_format_invariants(&input);
        count += 1;
    }
    assert!(count > 0, "no fixtures found in {dir:?}");
}

#[test]
fn format_invariants_corpus() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/bib_corpus");
    let mut count = 0;
    for entry in fs::read_dir(&dir).expect("read bib corpus dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("bib") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read bib corpus file");
        // The corpus exercises recovery too; only clean-parsing files are in scope
        // for the formatter (it refuses inputs the parser flags).
        if parse(&text).errors.is_empty() {
            assert_bib_format_invariants(&text);
            count += 1;
        }
    }
    assert!(count > 0, "no clean .bib corpus files found in {dir:?}");
}

#[test]
fn format_refuses_unparseable_input() {
    // An unterminated brace is a parse error; the formatter refuses the document
    // rather than reshaping around the parser's recovery (AGENTS.md tenet 3).
    let input = "@misc{k, title = {unterminated";
    assert!(!parse(input).errors.is_empty(), "test input must be dirty");
    assert!(format(input).is_err());
}

#[test]
fn indent_width_is_honored() {
    let input = "@misc{k, t = {x}}\n";
    let style = FormatStyle {
        indent_width: 4,
        ..FormatStyle::default()
    };
    let out = format_with_style(input, style).expect("formats");
    assert_eq!(out, "@misc{k,\n    t = {x}\n}\n");
}

#[test]
fn empty_input_stays_empty() {
    assert_eq!(format("").expect("formats"), "");
    assert_eq!(format("   \n\n").expect("formats"), "");
}
