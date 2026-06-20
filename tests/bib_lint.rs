//! End-to-end tests for the bib lint driver (`bib::linter::check_document`): the
//! public entry the CLI calls for `.bib` inputs. Exercises rule collection,
//! cross-rule ordering, parse-error passthrough, and a clean file — complementing
//! the focused per-rule unit tests in `src/bib/linter/`.

use std::path::Path;

use badness::bib::linter::{Severity, check_document};

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
