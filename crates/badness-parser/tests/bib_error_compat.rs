//! On-demand comparison of BibTeX syntax acceptance with Biber.

use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
#[ignore = "requires local Biber; run via task bib-error-compat"]
fn bib_error_compat_report() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/bib_corpus");
    let mut paths: Vec<_> = fs::read_dir(corpus)
        .expect("read BibTeX corpus directory")
        .map(|entry| entry.expect("read BibTeX corpus directory entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "bib"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "empty BibTeX corpus");

    let work = tempfile::tempdir().expect("create temporary Biber directory");
    let mut counts = [[0; 2]; 2];
    let mut over_strict = Vec::new();
    let mut under_strict = Vec::new();

    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy();
        let source = fs::read_to_string(path).unwrap_or_else(|err| panic!("{name}: {err}"));
        let badness_dirty = !badness_parser::bib::parse(&source).errors.is_empty();

        fs::write(work.path().join("input.bib"), &source)
            .unwrap_or_else(|err| panic!("{name}: cannot stage Biber input: {err}"));
        let output = Command::new("biber")
            .current_dir(work.path())
            .args([
                "--tool",
                "--noconf",
                "--nolog",
                "--output-file=discard.bib",
                "input.bib",
            ])
            .output()
            .unwrap_or_else(|err| {
                panic!("{name}: cannot run biber (requires biber on PATH): {err}")
            });
        let log = String::from_utf8_lossy(&output.stdout);
        let biber_dirty = biber_has_errors(output.status.code(), &log).unwrap_or_else(|| {
            panic!(
                "{name}: unclassified Biber result ({})\n{log}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )
        });

        counts[usize::from(badness_dirty)][usize::from(biber_dirty)] += 1;
        match (badness_dirty, biber_dirty) {
            (true, false) => over_strict.push(name),
            (false, true) => under_strict.push(name),
            _ => {}
        }
    }

    let [[both_clean, biber_only], [badness_only, both_dirty]] = counts;
    println!(
        "|             | Biber clean | Biber dirty |\n\
         |-------------|------------:|------------:|\n\
         | Badness clean | {both_clean} | {biber_only} |\n\
         | Badness dirty | {badness_only} | {both_dirty} |"
    );
    for (label, files) in [
        ("Potentially over-strict", over_strict),
        ("Potentially under-strict", under_strict),
    ] {
        println!("\n{label} ({})", files.len());
        for name in files {
            println!("- {name}");
        }
    }
}

fn biber_has_errors(exit_code: Option<i32>, log: &str) -> Option<bool> {
    let mut has_errors = false;
    let mut has_error_line = false;
    let mut has_bibtex_error = false;
    for line in log.lines() {
        if let Some(tally) = line.strip_prefix("INFO - ERRORS: ") {
            has_errors |= tally.trim().parse::<u64>().ok()? > 0;
        }
        has_error_line |= line.starts_with("ERROR - ");
        has_bibtex_error |= line.starts_with("ERROR - BibTeX subsystem:");
    }

    match exit_code? {
        0 if !has_errors && !has_error_line => Some(false),
        code if code != 0 && has_errors && has_bibtex_error => Some(true),
        _ => None,
    }
}

#[test]
fn classifies_biber_results() {
    let syntax_error = "ERROR - BibTeX subsystem: input.bib, line 2, syntax error: expected comma\nINFO - ERRORS: 1";
    for (exit_code, log, expected) in [
        (Some(0), "INFO - Writing output", Some(false)),
        (
            Some(0),
            "WARN - Invalid month\nINFO - WARNINGS: 1",
            Some(false),
        ),
        (Some(2), syntax_error, Some(true)),
        (
            Some(2),
            "ERROR - Cannot find input.bib\nINFO - ERRORS: 1",
            None,
        ),
        (Some(2), "", None),
        (Some(0), syntax_error, None),
        (None, syntax_error, None),
    ] {
        assert_eq!(biber_has_errors(exit_code, log), expected, "{log}");
    }
}
