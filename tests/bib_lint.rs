//! End-to-end tests for the bib lint driver (`bib::linter::check_document`): the
//! public entry the CLI calls for `.bib` inputs. Exercises rule collection,
//! cross-rule ordering, parse-error passthrough, and a clean file — complementing
//! the focused per-rule unit tests in `src/bib/linter/`.

use std::path::Path;

use badness::bib::format;
use badness::bib::linter::{Severity, apply_fixes, check_document};

/// Lint `src` through the public bib driver, returning `(rule, severity)` per
/// finding in driver order (sorted by byte position).
fn lint(src: &str) -> Vec<(&'static str, Severity)> {
    check_document(Path::new("refs.bib"), src)
        .into_iter()
        .map(|d| (d.rule, d.severity))
        .collect()
}

/// The rule ids of every finding, in order.
fn rules(src: &str) -> Vec<&'static str> {
    lint(src).into_iter().map(|(rule, _)| rule).collect()
}

#[test]
fn clean_file_is_silent() {
    let src = "\
@article{knuth1984,
  author = {Knuth, Donald E.},
  title = {Literate Programming},
  journaltitle = {The Computer Journal},
  year = 1984,
}
";
    assert!(lint(src).is_empty(), "got: {:?}", lint(src));
}

#[test]
fn collects_findings_across_rules_sorted_by_position() {
    // An unused @string, then an entry that is a duplicate-key target, missing a
    // required field, carrying an unknown field and an empty field.
    let src = "\
@string{unused = {Cambridge University Press}}
@article{dup,
  title = {First},
  journaltitle = {J},
  author = {A},
  year = 2020,
}
@article{dup,
  title = {Second},
  bogusfield = {x},
  note = {},
}
";
    let found = rules(src);
    // The unused @string comes first (earliest byte), then the duplicate-key entry's
    // findings. All five rule families are exercised here.
    assert_eq!(found.first(), Some(&"unused-string"));
    for expected in [
        "unused-string",
        "duplicate-key",
        "missing-required-field",
        "unknown-field",
        "empty-field",
    ] {
        assert!(found.contains(&expected), "missing {expected}: {found:?}");
    }

    // Findings are sorted by byte position (non-decreasing start offsets).
    let diags = check_document(Path::new("refs.bib"), src);
    assert!(
        diags.windows(2).all(|w| w[0].start <= w[1].start),
        "diagnostics not sorted by position"
    );
}

#[test]
fn parse_errors_pass_through_as_diagnostics() {
    // An unterminated entry: the parser recovers and reports an error, which the
    // driver folds in as a `parse` diagnostic (Severity::Error).
    let diags = check_document(Path::new("refs.bib"), "@article{k, title = {unterminated\n");
    assert!(
        diags
            .iter()
            .any(|d| d.rule == "parse" && d.severity == Severity::Error),
        "expected a parse diagnostic, got: {:?}",
        diags
            .iter()
            .map(|d| (d.rule, d.severity))
            .collect::<Vec<_>>()
    );
}

#[test]
fn paths_are_stamped() {
    let diags = check_document(Path::new("refs.bib"), "@string{x = {y}}\n");
    assert!(!diags.is_empty());
    assert!(diags.iter().all(|d| d.path == Path::new("refs.bib")));
}

#[test]
fn phase_4b_rules_surface() {
    // undefined-string, title-capitalization, and encoding-hints all fire.
    let src = "@article{k,\n  title = {The DNA of Erdős},\n  publisher = nope,\n}\n";
    let found = rules(src);
    assert!(found.contains(&"undefined-string"), "{found:?}");
    assert!(found.contains(&"title-capitalization"), "{found:?}");
    assert!(found.contains(&"encoding-hints"), "{found:?}");
}

#[test]
fn comment_directive_suppresses_following_entry() {
    // A `@comment{badness-ignore …}` carrier suppresses the named rule on the next
    // entry only.
    let src = "\
@comment{badness-ignore unused-string: intentional}
@string{cup = {Cambridge University Press}}
@string{other = {O}}
";
    let found = rules(src);
    // Only the second @string's unused-string survives.
    assert_eq!(
        found.iter().filter(|r| **r == "unused-string").count(),
        1,
        "{found:?}"
    );
}

#[test]
fn file_directive_suppresses_all() {
    let src = "\
@comment{badness-ignore-file: quiet}
@string{a = {A}}
@misc{k, title = {DNA}}
";
    assert!(rules(src).is_empty(), "got: {:?}", rules(src));
}

#[test]
fn empty_field_fix_survives_format_roundtrip() {
    // Tenet 5: format → lint --fix → format --check stays green. Start messy,
    // format, apply the empty-field fix, then assert the result is already
    // formatted (a second format is a no-op).
    let messy = "@article{k, title = {T}, note = {}, year = 2020}\n";
    let formatted = format(messy).unwrap();

    // Apply every available fix (the empty-field deletion) to the formatted text.
    let fixes: Vec<_> = check_document(Path::new("refs.bib"), &formatted)
        .into_iter()
        .filter_map(|d| d.fix)
        .collect();
    assert!(!fixes.is_empty(), "expected an empty-field fix");
    let fixed = apply_fixes(&formatted, &fixes, false).output;

    // The empty field is gone and the fixed text is already format-clean.
    assert!(
        !fixed.contains("note"),
        "empty field not removed: {fixed:?}"
    );
    assert_eq!(
        format(&fixed).unwrap(),
        fixed,
        "fix output is not format-clean"
    );

    // And no empty-field finding remains.
    let remaining = rules(&fixed);
    assert!(
        !remaining.contains(&"empty-field"),
        "empty-field should be cleared: {remaining:?}"
    );
}

#[test]
fn duplicate_field_fix_survives_format_roundtrip() {
    // Tenet 5: format → lint --fix → format --check stays green. A field repeated
    // with an identical value gets a deletion fix; the result must be format-clean.
    let messy = "@article{k, author = {A}, author = {A}, title = {T}, year = 2020}\n";
    let formatted = format(messy).unwrap();

    let fixes: Vec<_> = check_document(Path::new("refs.bib"), &formatted)
        .into_iter()
        .filter_map(|d| d.fix)
        .collect();
    assert!(!fixes.is_empty(), "expected a duplicate-field fix");
    let fixed = apply_fixes(&formatted, &fixes, false).output;

    // Exactly one `author` survives, and the fixed text is already format-clean.
    assert_eq!(fixed.matches("author").count(), 1, "got: {fixed:?}");
    assert_eq!(
        format(&fixed).unwrap(),
        fixed,
        "fix output is not format-clean"
    );

    // And no duplicate-field finding remains.
    let remaining = rules(&fixed);
    assert!(
        !remaining.contains(&"duplicate-field"),
        "duplicate-field should be cleared: {remaining:?}"
    );
}
