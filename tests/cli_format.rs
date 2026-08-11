//! CLI-level tests for `badness format --check`: the report a CI step or a
//! pre-commit hook configured with `args: [--check]` actually reads.
//!
//! Under `--check` nothing is written, so this output is the only account of
//! what would change — hence the diff. These run the real binary
//! (`CARGO_BIN_EXE_badness`) so they cover the stdout-vs-stderr split and exit
//! codes, not just the unit tests in `src/formatter/check.rs`. Each test works
//! in a tempdir containing a `.git` entry so the config ancestor walk stops
//! there and a developer's own `badness.toml` cannot leak in.

use std::io::Write;
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

/// Run `badness format` with `input` on stdin (`None` closes it: `/dev/null` is
/// not a terminal, so the interactive-input gate never fires and the run is the
/// same whether or not `cargo test` was started from a terminal).
fn format_stdin(dir: &Path, args: &[&str], input: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_badness"));
    cmd.arg("format")
        .args(args)
        .current_dir(dir)
        .env_remove("BADNESS_CONFIG")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("run badness");
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    child.wait_with_output().expect("wait for badness")
}

fn format(dir: &Path, args: &[&str]) -> Output {
    format_stdin(dir, args, None)
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

/// The positional-input contract: `-` is the explicit stdin spelling, an
/// implicit (piped) stdin still works, and neither can be mixed with paths.
/// The gated case — no paths at an interactive terminal — is a usage error
/// rather than a silent wait (issue #111); it needs a pty to reproduce, so the
/// decision itself is unit-tested in `main.rs` (`resolve_inputs`).
#[test]
fn dash_formats_stdin_to_stdout() {
    let dir = repo_dir();

    let output = format_stdin(dir.path(), &["-"], Some(UNFORMATTED));

    assert!(output.status.success(), "stdin should format cleanly");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "\\section{Hi}\nsome text more\n"
    );
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
}

#[test]
fn piped_stdin_still_needs_no_dash() {
    // The pre-`-` spelling stays valid: a pipe is not a terminal, so nothing a
    // script or CI step does today changes behavior.
    let dir = repo_dir();

    let output = format_stdin(dir.path(), &[], Some(UNFORMATTED));

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "\\section{Hi}\nsome text more\n"
    );
}

#[test]
fn dash_cannot_be_mixed_with_paths() {
    let dir = repo_dir();
    std::fs::write(dir.path().join("doc.tex"), UNFORMATTED).unwrap();

    let output = format_stdin(dir.path(), &["-", "doc.tex"], None);

    // Clap's own usage-error exit code, so the message reads like any other
    // argument mistake.
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("cannot be combined with other paths"),
        "expected the conflict error, got:\n{stderr}"
    );
    assert!(
        stderr.contains("Usage: badness format"),
        "the usage line should name the subcommand, got:\n{stderr}"
    );
    // The named file must be left alone.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("doc.tex")).unwrap(),
        UNFORMATTED
    );
}

#[test]
fn check_rejects_stdin() {
    let dir = repo_dir();

    let output = format_stdin(dir.path(), &["--check", "-"], Some(UNFORMATTED));

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("cannot read from stdin"),
        "`--check` reports on files it leaves on disk"
    );
}
