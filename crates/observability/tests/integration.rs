//! End-to-end tests for the `observability` module: file-sink creation
//! and the once-per-process re-init guard.
//!
//! Tests that touch `init_tracing` install a global subscriber — a
//! process-level side effect. Both serialized via `#[serial]` and chained
//! in one process under a single `OnceLock`-guarded init.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::OnceLock;

use lean_observability::{init_tracing, FileSink, TracingGuard, TracingInitError, Verbosity};
use serial_test::serial;
use tempfile::TempDir;

/// Holds the guard returned by the first `init_tracing` call so the
/// non-blocking worker stays alive for the lifetime of the test process.
static FIRST_GUARD: OnceLock<TracingGuard> = OnceLock::new();

/// Holds the temp dir backing the file-sink test so the path stays valid
/// while we assert on its contents.
static SINK_DIR: OnceLock<TempDir> = OnceLock::new();

#[test]
#[serial]
fn file_sink_creates_timestamped_log_file_and_blocks_reinit() {
    // First init: configure the file sink, capture the guard, and assert
    // a single log file lands in the temp directory.
    let dir = TempDir::new().expect("tempdir");

    let guard = init_tracing(
        Verbosity::Info,
        Some(FileSink {
            dir: dir.path(),
            prefix: "test",
        }),
    )
    .expect("init_tracing");

    tracing::info!("hello from observability test");
    // Drop the worker explicitly via the guard going out of scope below
    // (the OnceLock holds it for the process lifetime, so the worker
    // keeps running — for assertion purposes we just need the file
    // *creation* to have happened by now).

    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .expect("read_dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries.len(), 1, "expected one log file, got {entries:?}");
    let name = &entries[0];
    assert!(name.starts_with("test-"), "got {name}");
    let extension = std::path::Path::new(&name).extension();
    assert_eq!(
        extension.and_then(|e| e.to_str()),
        Some("log"),
        "got {name}"
    );

    // Stash the guard + tempdir so the worker keeps running and the path
    // stays valid for any subsequent assertions in this test process.
    FIRST_GUARD.set(guard).expect("guard set once");
    SINK_DIR.set(dir).expect("dir set once");

    // Second init: must error because we already installed a subscriber
    // in this process.
    let err = init_tracing(Verbosity::Debug, None).expect_err("expected re-init error");
    assert!(
        matches!(err, TracingInitError::AlreadyInitialized(_)),
        "got {err:?}",
    );
}
