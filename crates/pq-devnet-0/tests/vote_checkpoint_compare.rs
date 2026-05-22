//! Tests for the local-pq vote checkpoint comparison smoke script.

#![allow(clippy::expect_used, clippy::panic)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn temp_case_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "lean-rust-vote-checkpoints-{name}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn script_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/core/compare-vote-checkpoints.sh")
}

fn write_fixture(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).expect("write fixture");
    path
}

fn run_compare(ream_log: &Path, lean_log: &Path) -> std::process::Output {
    Command::new("bash")
        .arg(script_path())
        .env("REAM_LOG_FILE", ream_log)
        .env("LEAN_RUST_LOG_FILE", lean_log)
        .output()
        .expect("run compare script")
}

#[test]
fn compare_vote_checkpoints_succeeds_when_schedules_match() {
    let dir = temp_case_dir("match");
    let ream = write_fixture(
        &dir,
        "ream.log",
        "INFO ream_chain_lean::service: Processing vote by Validator 1 slot=4 source_slot=0 target_slot=1\n",
    );
    let lean = write_fixture(
        &dir,
        "lean.log",
        "DEBUG engine attestation vote produced slot=4 target_slot=1 source_slot=0\n",
    );

    let output = run_compare(&ream, &lean);

    assert!(
        output.status.success(),
        "script failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("| 4 | `0->1` | `0->1` | yes |"));
    assert!(stdout.contains("mismatches=0"));

    fs::remove_dir_all(dir).expect("cleanup temp dir");
}

#[test]
fn compare_vote_checkpoints_fails_on_first_mismatch() {
    let dir = temp_case_dir("mismatch");
    let ream = write_fixture(
        &dir,
        "ream.log",
        "\
INFO ream_chain_lean::service: Processing vote by Validator 1 slot=4 source_slot=0 target_slot=1
INFO ream_chain_lean::service: Processing vote by Validator 1 slot=5 source_slot=1 target_slot=2
",
    );
    let lean = write_fixture(
        &dir,
        "lean.log",
        "\
DEBUG engine attestation vote produced slot=4 target_slot=1 source_slot=0
DEBUG engine attestation vote produced slot=5 target_slot=2 source_slot=0
",
    );

    let output = run_compare(&ream, &lean);

    assert!(
        !output.status.success(),
        "script unexpectedly passed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("| 5 | `1->2` | `0->2` | no |"));
    assert!(stdout.contains("first_mismatch_slot=5"));

    fs::remove_dir_all(dir).expect("cleanup temp dir");
}
