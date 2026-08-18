//! BibTeX/BibLaTeX formatter fixtures and invariant tests. Exact output is pinned by
//! `tests/fixtures/bib_format/<name>/{input,expected}.bib` pairs (mirroring the
//! LaTeX `tests/fixtures/formatter/` layout). Every fixture and clean corpus file
//! checks idempotence and preservation of semantic content.

use std::fs;
use std::path::Path;

use badness_formatter::bib::semantic::Model;
use badness_formatter::bib::syntax::SyntaxKind;
use badness_formatter::bib::{ast, format, format_with_style, parse, reconstruct};
use badness_formatter::formatter::{FormatStyle, LineEnding};

/// The semantic facts formatting must preserve: the *multiset* of each entry's
/// (type, key), the `@string` definition names, and the `@string` use names. Every
/// list is sorted so the comparison is order-insensitive: the formatter is now allowed
/// to reorder entries (and fields), so meaning is the *bag* of facts, not their
/// positions. Byte ranges are dropped — they shift when layout changes.
fn meaning(text: &str) -> (Vec<(String, String)>, Vec<String>, Vec<String>) {
    let model = Model::build(&parse(text).syntax());
    let mut entries: Vec<(String, String)> = model
        .entries()
        .iter()
        .map(|e| (e.entry_type.to_string(), e.key.to_string()))
        .collect();
    entries.sort();
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

/// The *multiset* of every field's `(name_lc, value-signature)` across the document.
/// The signature is the value text with all whitespace and value delimiters (`"`,
/// `{`, `}`) removed — the formatter is allowed to insert/remove whitespace (reflow,
/// ` # ` spacing) and rewrite `"…"` → `{…}`, but it must never add, drop, or mangle the
/// actual content characters. This catches a reflow bug (a dropped or duplicated word,
/// a split inside a braced token) that the entry/`@string`-level `meaning()` oracle
/// cannot see, since that oracle never inspects value content.
///
/// The result is **sorted** so the check is order-insensitive: field sorting reorders
/// fields within an entry and entry sorting reorders entries, so the invariant is the
/// bag of `(name, value)` pairs, not their positions. The relative order of *duplicate*
/// fields (`note =` twice) is therefore not pinned here — that is covered by the
/// dedicated `sort_*` fixtures plus the stable-sort guarantee in the formatter.
///
/// A `%` comment sitting inside a value (`title = {a} # % pick one\n {b}`) is trivia,
/// not content, and the formatter hoists it out to its own line — so `COMMENT` nodes
/// are excluded here and checked by [`comments`] instead.
fn field_values(text: &str) -> Vec<(String, String)> {
    fn signature(value: &badness_formatter::bib::syntax::SyntaxNode) -> String {
        value
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .filter(|token| {
                token
                    .parent()
                    .is_none_or(|parent| parent.kind() != SyntaxKind::COMMENT)
            })
            .flat_map(|token| token.text().chars().collect::<Vec<_>>())
            .filter(|c| !c.is_whitespace() && !matches!(c, '"' | '{' | '}'))
            .collect()
    }
    let mut values: Vec<(String, String)> = parse(text)
        .syntax()
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::FIELD)
        .filter_map(|field| {
            let name = ast::field_name(&field)?.to_lowercase();
            let value = ast::field_value(&field)?;
            Some((name, signature(&value)))
        })
        .collect();
    values.sort();
    values
}

/// The *multiset* of every `%` comment in the document, trailing whitespace trimmed.
/// The formatter relocates comments (they ride their bound field through the canonical
/// sort) but must never drop, duplicate, or rewrite one — the check the entry- and
/// value-level oracles above cannot make, since neither looks at trivia.
///
/// Sorted, so it is order-insensitive for the same reason `field_values` is.
fn comments(text: &str) -> Vec<String> {
    let mut found: Vec<String> = parse(text)
        .syntax()
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::COMMENT)
        .map(|n| n.to_string().trim_end().to_string())
        .collect();
    found.sort();
    found
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

    // Comments preserved: relocation is allowed, loss is not.
    assert_eq!(
        comments(input),
        comments(&formatted),
        "formatting dropped or rewrote a comment for {input:?}"
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
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../badness-parser/tests/bib_corpus");
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

// --- Line endings -----------------------------------------------------------

#[test]
fn crlf_input_keeps_crlf_under_auto() {
    let input = "@misc{k,\r\n  t = {x}\r\n}\r\n";
    let out = format(input).expect("formats");
    assert_eq!(out, "@misc{k,\r\n  t = {x}\r\n}\r\n");
    assert!(!out.replace("\r\n", "").contains('\n'), "no bare LF");
}

#[test]
fn line_ending_overrides_the_source() {
    let lf = format_with_style(
        "@misc{k,\r\n  t = {x}\r\n}\r\n",
        FormatStyle {
            line_ending: LineEnding::Lf,
            ..FormatStyle::default()
        },
    )
    .expect("formats");
    assert_eq!(lf, "@misc{k,\n  t = {x}\n}\n");

    let crlf = format_with_style(
        "@misc{k,\n  t = {x}\n}\n",
        FormatStyle {
            line_ending: LineEnding::Crlf,
            ..FormatStyle::default()
        },
    )
    .expect("formats");
    assert_eq!(crlf, "@misc{k,\r\n  t = {x}\r\n}\r\n");
}
