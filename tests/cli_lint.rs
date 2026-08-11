//! CLI-level tests for `badness lint --output json`: the machine-readable
//! findings contract consumed by external tools (e.g. panache's external
//! linter integration).
//!
//! These run the real binary (`CARGO_BIN_EXE_badness`) so they cover the
//! stdout-vs-stderr split and exit codes, not just the serialization unit
//! tests in `src/linter/render.rs`. Each test works in a tempdir containing a
//! `.git` entry so the config ancestor walk stops there and a developer's own
//! `badness.toml` cannot leak in.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

/// Triggers the `ellipsis` rule, which carries a safe fix (`...` → `\dots`).
const FIXABLE: &str = "Wait ... what\n";
const CLEAN: &str = "Nothing to see here.\n";

fn repo_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    dir
}

fn lint(dir: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_badness"));
    cmd.arg("lint")
        .args(args)
        .current_dir(dir)
        .env_remove("BADNESS_CONFIG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd.spawn().expect("run badness");
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    child.wait_with_output().expect("wait for badness")
}

#[test]
fn json_reports_findings_with_fix_on_stdout() {
    let dir = repo_dir();
    std::fs::write(dir.path().join("doc.tex"), FIXABLE).unwrap();

    let output = lint(dir.path(), &["--output=json", "doc.tex"], None);

    assert!(!output.status.success(), "findings should exit non-zero");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    let findings = value.as_array().expect("top-level array");
    let ellipsis = findings
        .iter()
        .find(|d| d["rule"] == "ellipsis")
        .expect("ellipsis finding");
    assert_eq!(ellipsis["severity"], "warning");
    assert_eq!(ellipsis["path"], "doc.tex");
    // `...` sits at bytes 5..8 of `Wait ... what\n`.
    assert_eq!(ellipsis["start"], 5);
    assert_eq!(ellipsis["end"], 8);
    assert_eq!(ellipsis["fix"]["applicability"], "safe");
    assert_eq!(ellipsis["fix"]["edits"][0]["start"], 5);
    // Findings must not leak into stderr in JSON mode.
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains("ellipsis"),
        "stderr should carry no findings, got: {stderr}"
    );
}

#[test]
fn json_reads_stdin_with_stdin_filepath() {
    let dir = repo_dir();

    let output = lint(
        dir.path(),
        &["--output=json", "--stdin-filepath", "doc.tex"],
        Some(FIXABLE),
    );

    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    // The stdin buffer is always reported as `<stdin>`, never the named path.
    assert_eq!(value[0]["path"], "<stdin>");
}

#[test]
fn json_clean_file_emits_empty_array_and_exits_zero() {
    let dir = repo_dir();
    std::fs::write(dir.path().join("doc.tex"), CLEAN).unwrap();

    let output = lint(dir.path(), &["--output=json", "doc.tex"], None);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.trim(), "[]", "clean run still emits valid JSON");
}

#[test]
fn default_output_keeps_findings_on_stderr() {
    let dir = repo_dir();
    std::fs::write(dir.path().join("doc.tex"), FIXABLE).unwrap();

    let output = lint(dir.path(), &["doc.tex"], None);

    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "pretty mode writes nothing to stdout"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("ellipsis"), "got: {stderr}");
}

/// `-` is the explicit stdin spelling here too, and `--fix` has nowhere to write
/// it back (issue #111 added the spelling; the no-paths-at-a-terminal gate it
/// pairs with is unit-tested in `main.rs`).
#[test]
fn dash_lints_stdin() {
    let dir = repo_dir();

    let output = lint(dir.path(), &["--output=json", "-"], Some(FIXABLE));

    assert!(!output.status.success(), "findings should exit non-zero");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    assert_eq!(value[0]["path"], "<stdin>");
}

#[test]
fn fix_leaves_stdin_alone() {
    let dir = repo_dir();

    let output = lint(dir.path(), &["--fix", "--output=json", "-"], Some(FIXABLE));

    // Nothing to write back to, so the finding is still reported, not silently
    // fixed into stdout.
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    assert_eq!(value.as_array().expect("top-level array").len(), 1);
    assert_eq!(value[0]["path"], "<stdin>");
}
