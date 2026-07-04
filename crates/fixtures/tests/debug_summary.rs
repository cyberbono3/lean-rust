//! Tests for the local-pq debug summary script.

#![allow(clippy::expect_used, clippy::panic)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn script_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/core/debug-summary.sh")
}

fn write_fixture(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).expect("write fixture");
    path
}

#[test]
fn debug_summary_counts_status_timeout_from_combined_container_logs() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let ream = write_fixture(
        dir.path(),
        "ream.log",
        "WARN ream_p2p::network::lean: Publish vote failed slot=1 error=Duplicate\n",
    );
    let lean = write_fixture(
        dir.path(),
        "lean.log",
        "DEBUG runtime::p2p::service: status rpc outbound timeout; peer did not answer optional status request\n",
    );

    let output = Command::new("bash")
        .arg(script_path())
        .env("REAM_CONTAINER_LOG_FILE", &ream)
        .env("LEAN_RUST_CONTAINER_LOG_FILE", &lean)
        .output()
        .expect("run debug summary script");

    assert!(
        output.status.success(),
        "script failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("classification=pass-with-known-reference-noise"));
    assert!(stdout.contains("ream_duplicate_publish=1"));
    assert!(stdout.contains("status_timeouts=1"));
}
