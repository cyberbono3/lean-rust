//! Integration tests for the `runtime-p2p` host lifecycle.
//!
//! Covers the construction → start → stop happy path, bind-failure
//! fail-fast, idempotent stop, and the host-handle availability
//! contract. Time control is `tokio`'s real runtime — the tests bind a
//! UDP socket on `127.0.0.1` with port `0` so the OS picks a free port
//! and the test never races a fixed port already in use.
//!
//! All tests time-bound the lifecycle calls under a generous 10s
//! `tokio::time::timeout` so a misbehaving service fails the test
//! rather than hanging CI.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::time::Duration;

use lean_core::Service;
use runtime_p2p::{DevnetHost, HostError, HostOptions, P2pService};
use tempfile::TempDir;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

const TEST_DEADLINE: Duration = Duration::from_secs(10);

fn options_with_listen(dir: &Path, listen: &str) -> HostOptions {
    HostOptions::try_new(listen, "test/0.1.0", &dir.join("p2p_priv_key"), None)
        .expect("test options must validate")
}

fn build_service(dir: &TempDir) -> P2pService {
    DevnetHost::build(options_with_listen(
        dir.path(),
        "/ip4/127.0.0.1/udp/0/quic-v1",
    ))
    .expect("DevnetHost::build should succeed against a tempdir")
}

#[tokio::test]
async fn start_then_stop_round_trip() {
    let dir = TempDir::new().unwrap();
    let service = build_service(&dir);

    timeout(TEST_DEADLINE, service.start())
        .await
        .expect("start must complete within the test deadline")
        .expect("start must succeed");

    // Host handle is available exactly while the service is running.
    let host = service
        .host()
        .expect("host handle should exist while running");
    assert_eq!(host.peer_id(), service.peer_id());

    timeout(TEST_DEADLINE, service.stop(CancellationToken::new()))
        .await
        .expect("stop must complete within the test deadline")
        .expect("stop must succeed");

    assert!(service.host().is_none(), "host handle gone after stop");
}

#[tokio::test]
async fn bind_failure_surfaces_typed_error() {
    // QUIC needs `/udp`. A TCP-suffixed multiaddr cannot bind through
    // the QUIC-v1 transport, so `listen_on` either rejects up front or
    // the transport surfaces a listener error within the bind window.
    let dir = TempDir::new().unwrap();
    let service = DevnetHost::build(options_with_listen(
        dir.path(),
        "/ip4/127.0.0.1/tcp/1/quic-v1",
    ))
    .unwrap();

    let err = timeout(TEST_DEADLINE, service.start())
        .await
        .expect("start must complete within the test deadline")
        .expect_err("start must fail on an incompatible listen multiaddr");
    let downcast = err
        .downcast::<HostError>()
        .expect("HostError must round-trip through anyhow");
    assert!(
        matches!(downcast, HostError::Bind { .. }),
        "expected HostError::Bind, got {downcast:?}",
    );
}

#[tokio::test]
async fn stop_is_idempotent_on_idle_service() {
    let dir = TempDir::new().unwrap();
    let service = build_service(&dir);

    timeout(TEST_DEADLINE, service.stop(CancellationToken::new()))
        .await
        .expect("stop must complete within the test deadline")
        .expect("stop on idle service must be a no-op");
}

#[tokio::test]
async fn double_start_returns_already_started() {
    let dir = TempDir::new().unwrap();
    let service = build_service(&dir);

    timeout(TEST_DEADLINE, service.start())
        .await
        .unwrap()
        .unwrap();

    let err = timeout(TEST_DEADLINE, service.start())
        .await
        .expect("second start must complete within the test deadline")
        .expect_err("second start must surface AlreadyStarted");
    let downcast = err.downcast::<HostError>().unwrap();
    assert!(matches!(downcast, HostError::AlreadyStarted));

    timeout(TEST_DEADLINE, service.stop(CancellationToken::new()))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn status_tracks_lifecycle_state() {
    let dir = TempDir::new().unwrap();
    let service = build_service(&dir);

    // Idle state surfaces as "not started".
    assert!(service.status().await.is_err());

    timeout(TEST_DEADLINE, service.start())
        .await
        .unwrap()
        .unwrap();
    assert!(service.status().await.is_ok());

    timeout(TEST_DEADLINE, service.stop(CancellationToken::new()))
        .await
        .unwrap()
        .unwrap();
    assert!(service.status().await.is_err());
}
