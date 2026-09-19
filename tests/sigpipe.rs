//! `pebbles <cmd> | head` must not panic on a broken pipe.
//!
//! Rust ignores SIGPIPE by default, so writing to a closed pipe panics with
//! "failed printing to stdout: Broken pipe". Pebbles restores default SIGPIPE
//! handling (die by signal, like standard Unix tools). This suite runs the
//! binary with its stdout reader dropped and asserts no panic message.
//!
//! It uses the .no_jj test mode: the fixture has a hand-written index and a
//! `.no_jj` marker, so issue creation is pure file I/O and fast enough to
//! generate output larger than a pipe buffer.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const PEBBLES_BIN: &str = env!("CARGO_BIN_EXE_pebbles");

fn fixture_with_issues(count: usize) -> (tempfile::TempDir, PathBuf) {
    let tempdir = tempfile::Builder::new()
        .prefix("pebbles-sigpipe-")
        .tempdir()
        .expect("tempdir");
    let root = tempdir.path().to_path_buf();
    let pebbles = root.join(".pebbles");
    fs::create_dir_all(pebbles.join("issues")).expect("issues dir");
    fs::write(pebbles.join(".no_jj"), "").expect("no_jj marker");

    let mut index = String::from("{\"issues\":[");
    for i in 0..count {
        let id = format!("{:08x}-1111-4222-8333-{:012x}", i, i);
        let file = format!("bug-{:08x}-issue-number-{}.json", i, i);
        let issue = format!(
            "{{\"id\":\"{}\",\"title\":\"Issue number {} padding padding padding\",\"description\":null,\"status\":\"todo\",\"issue_type\":\"bug\",\"tags\":[],\"relationships\":[],\"metadata\":{{}},\"created_at\":\"2026-01-01T00:00:00Z\",\"updated_at\":\"2026-01-01T00:00:00Z\"}}\n",
            id, i
        );
        fs::write(pebbles.join("issues").join(&file), issue).expect("issue file");
        if i > 0 {
            index.push(',');
        }
        index.push_str(&format!("{{\"id\":\"{}\",\"file_name\":\"{}\"}}", id, file));
    }
    index.push_str("]}");
    fs::write(pebbles.join("index.json"), index).expect("index");

    (tempdir, root)
}

#[test]
fn broken_pipe_does_not_panic() {
    // Enough issues that `pebbles list` output spans multiple line-buffered
    // writes and the child is still writing after the reader is dropped.
    let (_tempdir, root) = fixture_with_issues(400);

    let mut child = Command::new(PEBBLES_BIN)
        .args(["list"])
        .current_dir(&root)
        .env("PEBBLES_DB_PATH", ".pebbles")
        .env("NO_COLOR", "1")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("FORCE_COLOR")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pebbles list");

    // Drop the read end immediately: every subsequent child write hits EPIPE.
    drop(child.stdout.take());

    let output = child.wait_with_output().expect("wait for child");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "pebbles must not panic on a broken pipe, stderr:\n{}",
        stderr
    );
}

#[test]
fn normal_pipe_output_still_works() {
    // Sanity check that the SIGPIPE change does not affect normal output:
    // read the full output of a small repository.
    let (_tempdir, root) = fixture_with_issues(3);

    let mut child = Command::new(PEBBLES_BIN)
        .args(["list"])
        .current_dir(&root)
        .env("PEBBLES_DB_PATH", ".pebbles")
        .env("NO_COLOR", "1")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("FORCE_COLOR")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pebbles list");

    let mut stdout = child.stdout.take().expect("stdout");
    let mut buf = String::new();
    use std::io::Read;
    stdout.read_to_string(&mut buf).expect("read stdout");
    let status = child.wait().expect("wait");
    assert!(status.success());
    assert_eq!(buf.lines().count(), 3);
    let _ = io_sink(); // keep Write import used
}

fn io_sink() -> impl Write {
    std::io::sink()
}
