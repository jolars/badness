//! CLI-level tests for `$BADNESS_CONFIG` (issue #40): the environment variable
//! naming a config file used when no project `badness.toml` is discovered.
//!
//! These run the real binary (`CARGO_BIN_EXE_badness`) so they cover reading
//! the variable itself, not just the injected-path unit tests in `config.rs`.
//! Each test formats stdin from a tempdir containing a `.git` entry, so the
//! ancestor walk stops there and a developer's own `badness.toml` cannot leak
//! in.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

const LONG_LINE: &str = "aa bb cc dd ee ff gg hh ii jj kk ll mm nn oo pp\n";

fn repo_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    dir
}

fn format_stdin(dir: &Path, env_config: Option<&str>, extra_args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_badness"));
    cmd.arg("format")
        .args(extra_args)
        .current_dir(dir)
        .env_remove("BADNESS_CONFIG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(value) = env_config {
        cmd.env("BADNESS_CONFIG", value);
    }
    let mut child = cmd.spawn().expect("run badness");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(LONG_LINE.as_bytes())
        .unwrap();
    child.wait_with_output().expect("wait for badness")
}

fn max_line_width(stdout: &[u8]) -> usize {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(|line| line.len())
        .max()
        .unwrap_or(0)
}

#[test]
fn env_config_applies_when_no_project_config() {
    let dir = repo_dir();
    let config = dir.path().join("elsewhere").join("badness.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "[format]\nline-width = 20\n").unwrap();

    let output = format_stdin(dir.path(), Some(config.to_str().unwrap()), &[]);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(max_line_width(&output.stdout) <= 20);
}

#[test]
fn project_config_beats_env_config() {
    let dir = repo_dir();
    let env_config = dir.path().join("elsewhere").join("badness.toml");
    std::fs::create_dir_all(env_config.parent().unwrap()).unwrap();
    std::fs::write(&env_config, "[format]\nline-width = 60\n").unwrap();
    std::fs::write(
        dir.path().join("badness.toml"),
        "[format]\nline-width = 25\n",
    )
    .unwrap();

    let output = format_stdin(dir.path(), Some(env_config.to_str().unwrap()), &[]);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(max_line_width(&output.stdout) <= 25);
}

#[test]
fn dangling_env_config_is_a_hard_error() {
    let dir = repo_dir();
    let missing = dir.path().join("nowhere").join("badness.toml");

    let output = format_stdin(dir.path(), Some(missing.to_str().unwrap()), &[]);

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("badness:"), "stderr: {stderr}");
}

#[test]
fn no_config_flag_skips_env_config() {
    let dir = repo_dir();
    let missing = dir.path().join("nowhere").join("badness.toml");

    let output = format_stdin(
        dir.path(),
        Some(missing.to_str().unwrap()),
        &["--no-config"],
    );

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn empty_env_config_counts_as_unset() {
    let dir = repo_dir();

    // An empty value must not be treated as a (dangling) path.
    let output = format_stdin(dir.path(), Some(""), &[]);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
