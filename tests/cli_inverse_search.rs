//! CLI-level tests for `badness inverse-search`, the command a PDF viewer runs
//! to hand a source position back to the editor.
//!
//! These run the real binary (`CARGO_BIN_EXE_badness`) so they cover the clap
//! wiring — the two line spellings and their mutual exclusion — as well as the
//! failure messages, which are what a user actually sees when inverse search does
//! not work. The delivery path itself is covered in-process by `src/ipc.rs`'s
//! unit tests and `tests/lsp.rs`.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn inverse_search(ipc_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_badness"))
        .arg("inverse-search")
        .arg("--ipc-dir")
        .arg(ipc_dir)
        .args(args)
        .env_remove("BADNESS_IPC_DIR")
        .output()
        .expect("run badness inverse-search")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn reports_that_no_server_is_listening() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("main.tex");
    std::fs::write(&file, "hi\n").unwrap();

    let out = inverse_search(
        &dir.path().join("ipc"),
        &["--input", &file.display().to_string(), "--line", "3"],
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("no badness language server is listening"),
        "the viewer shows this to the user, so it must name the cause: {err}"
    );
    assert!(
        err.contains("showDocument"),
        "inverse search needs a client that can reveal the position: {err}"
    );
}

#[test]
fn unlinks_a_stale_advertisement() {
    let dir = TempDir::new().unwrap();
    let ipc_dir = dir.path().join("ipc");
    std::fs::create_dir_all(&ipc_dir).unwrap();
    let stale = ipc_dir.join("999999.json");
    let advertisement = badness::ipc::Advertisement {
        pid: 999_999,
        transport: if cfg!(unix) {
            badness::ipc::Transport::Unix
        } else {
            badness::ipc::Transport::Tcp
        },
        address: if cfg!(unix) {
            ipc_dir.join("999999.sock").display().to_string()
        } else {
            "127.0.0.1:9".to_owned()
        },
        token: "0".repeat(32),
        roots: vec![],
    };
    std::fs::write(&stale, serde_json::to_vec(&advertisement).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stale, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let file = dir.path().join("main.tex");
    std::fs::write(&file, "hi\n").unwrap();
    let out = inverse_search(
        &ipc_dir,
        &["--input", &file.display().to_string(), "--line", "1"],
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        !stale.exists(),
        "a server that died without cleaning up must not strand its advertisement"
    );
}

#[test]
fn requires_exactly_one_line_spelling() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("main.tex");
    std::fs::write(&file, "hi\n").unwrap();
    let ipc_dir = dir.path().join("ipc");
    let input = file.display().to_string();

    // Neither: our own message, which names *both* spellings. clap's
    // `required_unless_present` would name only `--line`, sending a `--line0`
    // user the wrong way.
    let neither = inverse_search(&ipc_dir, &["--input", &input]);
    assert_eq!(neither.status.code(), Some(2));
    let err = stderr(&neither);
    assert!(err.contains("--line0") && err.contains("--line "), "{err}");

    // Both: mutually exclusive, so the 1-based value is never ambiguous.
    let both = inverse_search(
        &ipc_dir,
        &["--input", &input, "--line", "3", "--line0", "2"],
    );
    assert_eq!(both.status.code(), Some(2));
    assert!(
        stderr(&both).contains("cannot be used with"),
        "{}",
        stderr(&both)
    );
}

#[test]
fn accepts_the_texlab_line1_spelling() {
    // A viewer configuration migrated from texlab must keep working unchanged.
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("main.tex");
    std::fs::write(&file, "hi\n").unwrap();

    let out = inverse_search(
        &dir.path().join("ipc"),
        &["--input", &file.display().to_string(), "--line1", "3"],
    );
    // No server is listening, but the *parse* must have succeeded — a rejected
    // flag would fail with clap's usage message instead.
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("no badness language server is listening"),
        "{}",
        stderr(&out)
    );
}
