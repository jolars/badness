//! CLI-level tests for `badness debug format`, the invariant-check command the
//! smoke-test workflow (`.github/workflows/smoke-test.yml`) drives per file.
//!
//! These pin the output contracts the workflow greps — the parenthesized
//! failure labels, the report header, and the sanitized dump-file names — by
//! running the real binary (`CARGO_BIN_EXE_badness`). Every invocation passes
//! `--no-config` so a developer's own `badness.toml` cannot leak in.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn badness(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_badness"))
        .arg("--no-config")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run badness")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn passing_file_exits_zero() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("ok.tex"), "Hello \\emph{world}.\n").unwrap();

    let output = badness(
        dir.path(),
        &["debug", "format", "--checks", "all", "ok.tex"],
    );

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("All checks passed (checks: all, files: 1)"));
}

#[test]
fn report_on_passing_file_has_no_failure_headings() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("ok.tex"), "Hello world.\n").unwrap();

    let output = badness(dir.path(), &["debug", "format", "--report", "ok.tex"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let report = stdout(&output);
    assert!(report.contains("# Debug-format regression report"));
    assert!(report.contains("All checks passed."));
    assert!(!report.contains("(idempotency)"));
    assert!(!report.contains("(losslessness)"));
}

#[test]
fn dump_passes_writes_sanitized_artifact_names() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("sub dir")).unwrap();
    let source = "Hello \\emph{world}.\n";
    std::fs::write(dir.path().join("sub dir/a b.tex"), source).unwrap();

    let output = badness(
        dir.path(),
        &[
            "debug",
            "format",
            "--dump-dir",
            "dumps",
            "--dump-passes",
            "sub dir/a b.tex",
        ],
    );

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    // The stem is the path as passed, sanitized exactly like the workflow's
    // `sed 's/[^[:alnum:]._-]/_/g'` — this is the artifact-lookup contract.
    let dumps = dir.path().join("dumps");
    let input = std::fs::read_to_string(dumps.join("sub_dir_a_b.tex.idempotency.input.txt"))
        .expect("input dump exists");
    let once = std::fs::read_to_string(dumps.join("sub_dir_a_b.tex.idempotency.once.txt"))
        .expect("once dump exists");
    let twice = std::fs::read_to_string(dumps.join("sub_dir_a_b.tex.idempotency.twice.txt"))
        .expect("twice dump exists");
    assert_eq!(input, source);
    assert_eq!(once, twice);
    assert!(
        dumps
            .join("sub_dir_a_b.tex.losslessness.input.txt")
            .exists()
    );
    assert!(
        dumps
            .join("sub_dir_a_b.tex.losslessness.parsed.txt")
            .exists()
    );
}

#[test]
fn dump_passes_requires_dump_dir() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("ok.tex"), "x\n").unwrap();

    let output = badness(dir.path(), &["debug", "format", "--dump-passes", "ok.tex"]);

    assert_eq!(output.status.code(), Some(2), "clap usage error expected");
}

#[test]
fn format_error_never_reads_as_an_invariant_failure() {
    let dir = TempDir::new().unwrap();
    // An unclosed group parses with diagnostics, so the formatter refuses it:
    // a `format-error`, not an idempotency or losslessness regression.
    std::fs::write(dir.path().join("bad.tex"), "a{\n").unwrap();

    let output = badness(dir.path(), &["debug", "format", "bad.tex"]);

    assert_eq!(output.status.code(), Some(1));
    let log = stderr(&output).to_lowercase();
    assert!(log.contains("(format-error)"), "log: {log}");
    assert!(!log.contains("idempot"), "log: {log}");
    assert!(!log.contains("lossless"), "log: {log}");

    let output = badness(dir.path(), &["debug", "format", "--report", "bad.tex"]);
    assert_eq!(output.status.code(), Some(1));
    let report = stdout(&output);
    assert!(report.contains("### 1. `bad.tex` (format-error)"));
    assert!(report.contains("- Files checked: 1"));
}
