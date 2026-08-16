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

// --- declared environments (`[environments.…]`; AGENTS.md decision #12) ------

/// The issue-#109 shape, with the pair defined somewhere this file cannot see —
/// so the inferred alias scan has nothing to find and only the declaration can
/// make `\bea … \eea` an environment.
const ALIAS_CONFIG: &str = "[environments.eqnarray]\nbegin = ['\\bea']\nend = ['\\eea']\n";
const ALIAS_DOC: &str = "\\bea a&=&b \\\\ &=&c \\eea\n";
/// What `\begin{eqnarray} … \end{eqnarray}` formats to: body indented one step,
/// columns aligned on `&`.
const ALIAS_FORMATTED: &str = "\\bea\n  a & = & b \\\\\n    & = & c\n\\eea\n";

#[test]
fn a_declared_alias_formats_like_the_environment_it_names() {
    let dir = repo_dir();
    std::fs::write(dir.path().join("badness.toml"), ALIAS_CONFIG).unwrap();
    std::fs::write(dir.path().join("doc.tex"), ALIAS_DOC).unwrap();

    let output = format(dir.path(), &["doc.tex"]);

    assert!(output.status.success(), "{output:?}");
    let formatted = std::fs::read_to_string(dir.path().join("doc.tex")).unwrap();
    assert_eq!(formatted, ALIAS_FORMATTED);
}

/// The control: without the declaration the same file has no environment in it,
/// so nothing indents. Keeps the test above from passing for some other reason.
#[test]
fn the_same_file_is_left_alone_without_the_declaration() {
    let dir = repo_dir();
    std::fs::write(dir.path().join("doc.tex"), ALIAS_DOC).unwrap();

    let output = format(dir.path(), &["doc.tex"]);

    assert!(output.status.success(), "{output:?}");
    let formatted = std::fs::read_to_string(dir.path().join("doc.tex")).unwrap();
    assert_ne!(formatted, ALIAS_FORMATTED);
    assert!(
        !formatted.contains("\\bea\n"),
        "an undeclared `\\bea` is a plain command, got:\n{formatted}"
    );
}

/// Issue #117, end to end and from both directions. The reporter defines only
/// `\bsplit` and writes `\end{split}` out; the literal delimiter is a spelling
/// of the closing side, so the pair formats with **no configuration at all**.
/// The declared half-entry below is the block they tried to write and could not
/// — `end` used to be mandatory, and `end = ['\end{split}']` is not a control
/// word.
#[test]
fn a_one_sided_alias_formats_like_the_environment_it_names() {
    let dir = repo_dir();
    let doc = "\\def\\bsplit{\\begin{split}}\n\\bsplit\na&=b,\\\\\nc&=d.\n\\end{split}\n";
    let expected =
        "\\def\\bsplit{\\begin{split}}\n\\bsplit\n  a & = b, \\\\\n  c & = d.\n\\end{split}\n";
    std::fs::write(dir.path().join("doc.tex"), doc).unwrap();

    let output = format(dir.path(), &["doc.tex"]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("doc.tex")).unwrap(),
        expected
    );

    // The mirror, and the declared spelling of the same one-sided shape.
    let dir = repo_dir();
    std::fs::write(
        dir.path().join("badness.toml"),
        "[environments.equation]\nend = ['\\eeq']\n",
    )
    .unwrap();
    let doc = "\\begin{equation}\na&=b,\\\\\nc&=d.\n\\eeq\n";
    std::fs::write(dir.path().join("doc.tex"), doc).unwrap();

    let output = format(dir.path(), &["doc.tex"]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("doc.tex")).unwrap(),
        "\\begin{equation}\n  a & = b, \\\\\n  c & = d.\n\\eeq\n"
    );
}

/// Stdin has no path to anchor package resolution against, but it does have a
/// project config — so it must honor declarations too. A formatter that treats
/// `badness format doc.tex` and `badness format < doc.tex` differently is a trap.
#[test]
fn stdin_honors_declarations() {
    let dir = repo_dir();
    std::fs::write(dir.path().join("badness.toml"), ALIAS_CONFIG).unwrap();

    let output = format_stdin(
        dir.path(),
        &["--stdin-filepath", "doc.tex"],
        Some(ALIAS_DOC),
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8(output.stdout).unwrap(), ALIAS_FORMATTED);
}

/// `--check` shares the format entry, so the two cannot disagree: a file already
/// laid out the way the declaration implies must check clean.
#[test]
fn check_agrees_with_format_under_a_declaration() {
    let dir = repo_dir();
    std::fs::write(dir.path().join("badness.toml"), ALIAS_CONFIG).unwrap();
    std::fs::write(dir.path().join("doc.tex"), ALIAS_FORMATTED).unwrap();

    let output = format(dir.path(), &["--check", "doc.tex"]);

    assert!(
        output.status.success(),
        "formatted-under-declaration file should check clean, got:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// A declaration that breaks a rule fails at config load, before any file is
/// touched — the "never a silent no-op" contract, seen from the CLI.
#[test]
fn a_broken_declaration_is_a_config_error() {
    let dir = repo_dir();
    std::fs::write(
        dir.path().join("badness.toml"),
        "[environments.myenv]\nlike = \"algin\"\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("doc.tex"), UNFORMATTED).unwrap();

    let output = format(dir.path(), &["doc.tex"]);

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("environments.myenv.like"), "{stderr}");
    assert!(stderr.contains("algin"), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("doc.tex")).unwrap(),
        UNFORMATTED,
        "no file is touched when the config is rejected"
    );
}
