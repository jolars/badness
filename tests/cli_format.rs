//! CLI-level tests for `badness format --check`: the report a CI step or a
//! pre-commit hook configured with `args: [--check]` actually reads.
//!
//! Under `--check` nothing is written, so this output is the only account of
//! what would change — hence the diff. These run the real binary
//! (`CARGO_BIN_EXE_badness`) so they cover the stdout-vs-stderr split and exit
//! codes, not just the unit tests in `src/formatter/check.rs`. Each test works
//! in a tempdir containing a `.git` entry so the config ancestor walk stops
//! there and a developer's own `badness.toml` cannot leak in.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

/// Double spaces and an authored break the default `reflow` wrap would undo.
const UNFORMATTED: &str = "\\section{Hi}\nsome    text\nmore\n";

fn repo_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    dir
}

fn format(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_badness"))
        .arg("format")
        .args(args)
        .current_dir(dir)
        .env_remove("BADNESS_CONFIG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run badness")
}

#[test]
fn check_prints_a_diff_on_stdout() {
    let dir = repo_dir();
    std::fs::write(dir.path().join("doc.tex"), UNFORMATTED).unwrap();

    let output = format(dir.path(), &["--check", "doc.tex"]);

    assert!(!output.status.success(), "unformatted file should exit 1");
    let stdout = String::from_utf8(output.stdout).unwrap();
    // The rustfmt-style header names the file and the first changed line, then
    // the hunk shows both sides.
    assert!(
        stdout.contains("Diff in doc.tex:1:"),
        "missing diff header, got:\n{stdout}"
    );
    assert!(
        stdout.contains("-some    text"),
        "missing removed line, got:\n{stdout}"
    );
    assert!(
        stdout.contains("+some text more"),
        "missing added line, got:\n{stdout}"
    );
    assert!(
        stdout.contains("1 of 1 file(s) would be reformatted"),
        "missing summary, got:\n{stdout}"
    );
    // Only the error path uses stderr, so the report stays pipeable.
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
}

#[test]
fn check_is_not_colored_when_redirected() {
    let dir = repo_dir();
    std::fs::write(dir.path().join("doc.tex"), UNFORMATTED).unwrap();

    let output = format(dir.path(), &["--check", "doc.tex"]);

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains('\x1b'),
        "piped output should carry no escapes, got:\n{stdout:?}"
    );
}

#[test]
fn check_colors_on_demand() {
    let dir = repo_dir();
    std::fs::write(dir.path().join("doc.tex"), UNFORMATTED).unwrap();

    let output = format(dir.path(), &["--check", "--color", "always", "doc.tex"]);

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("\x1b[31m-some    text"),
        "removed lines should be red, got:\n{stdout:?}"
    );
    assert!(
        stdout.contains("\x1b[32m+some text more"),
        "added lines should be green, got:\n{stdout:?}"
    );
}

#[test]
fn quiet_check_lists_files_without_the_diff() {
    let dir = repo_dir();
    std::fs::write(dir.path().join("doc.tex"), UNFORMATTED).unwrap();

    let output = format(dir.path(), &["--check", "--quiet", "doc.tex"]);

    assert!(!output.status.success(), "unformatted file should exit 1");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout,
        "would reformat doc.tex\n1 of 1 file(s) would be reformatted\n"
    );
}

#[test]
fn check_is_silent_and_zero_when_formatted() {
    let dir = repo_dir();
    let path = dir.path().join("doc.tex");
    std::fs::write(&path, UNFORMATTED).unwrap();
    // Format in place first, so the check has nothing to report (idempotence).
    assert!(format(dir.path(), &["doc.tex"]).status.success());

    let output = format(dir.path(), &["--check", "doc.tex"]);

    assert!(output.status.success(), "formatted file should exit 0");
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
}

#[test]
fn check_errors_go_to_stderr() {
    let dir = repo_dir();

    let output = format(dir.path(), &["--check", "missing.tex"]);

    assert!(!output.status.success(), "a bad path should exit 1");
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("badness:"),
        "the error belongs on stderr"
    );
}
