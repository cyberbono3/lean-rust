//! Integration tests for the concrete duties [`Publisher`] over the p2p host.
//!
//! Covers the mapping logic that `Publisher` adds on top of the p2p
//! `Host::publish_*` methods (whose gossipsub behavior is already proven
//! in `runtime/p2p/tests/gossip.rs`):
//! - host-not-running → `"p2p host is not running"` (the `host()` `None`
//!   branch), for both block and attestation;
//! - host-running-but-no-mesh-peers → the `map_err` wrapper strings
//!   `"p2p publish block failed"` / `"p2p publish attestation failed"`.
//!
//! We assert on the wrapper — not the inner `InsufficientPeers` variant —
//! because `gossip.rs` already pins the underlying error; here we only
//! prove `Publisher` wraps it (and picks the right per-method message).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use lean_core::Service;
use lean_duties::Publisher;
use lean_p2p_host::{DevnetHost, HostOptions, P2pService};
use protocol::{SignedBlock, SignedVote};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

/// Builds an unstarted p2p service on an ephemeral loopback QUIC port.
/// `build` is construction-only — it does not bind the listener, so the
/// service is `Constructed` until `start`.
fn build_p2p() -> (TempDir, Arc<P2pService>) {
    let dir = tempfile::tempdir().unwrap();
    let options = HostOptions::try_new(
        "/ip4/127.0.0.1/udp/0/quic-v1",
        "test/0.1.0",
        &dir.path().join("identity.pb"),
        None,
    )
    .unwrap();
    let service = Arc::new(DevnetHost::build(options).unwrap());
    (dir, service)
}

#[tokio::test]
async fn publish_before_start_reports_missing_host() {
    let (_dir, service) = build_p2p();
    let publisher = Publisher::new(service);

    let block_err = publisher
        .publish_block(SignedBlock::default())
        .await
        .expect_err("publish before start must fail: no host handle");
    assert!(
        block_err.to_string().contains("p2p host is not running"),
        "unexpected block error: {block_err}",
    );

    let vote_err = publisher
        .publish_attestation(SignedVote::default())
        .await
        .expect_err("publish before start must fail: no host handle");
    assert!(
        vote_err.to_string().contains("p2p host is not running"),
        "unexpected attestation error: {vote_err}",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_after_start_wraps_publish_error() {
    let (_dir, service) = build_p2p();
    service.start().await.unwrap();
    let publisher = Publisher::new(Arc::clone(&service));

    // Single-node start has no mesh peers, so the host's publish path
    // deterministically errors (InsufficientPeers — see gossip.rs). We
    // assert `Publisher` folds it into its per-method wrapper, and that
    // we reached the publish path rather than the host()-None branch.
    let block_err = publisher
        .publish_block(SignedBlock::default())
        .await
        .expect_err("publish without mesh peers must surface a publish error");
    let block_msg = block_err.to_string();
    assert!(
        block_msg.contains("p2p publish block failed"),
        "expected block publish wrapper, got: {block_msg}",
    );
    assert!(
        !block_msg.contains("not running"),
        "should have reached the publish path, not the host()-None branch: {block_msg}",
    );

    let vote_err = publisher
        .publish_attestation(SignedVote::default())
        .await
        .expect_err("publish without mesh peers must surface a publish error");
    let vote_msg = vote_err.to_string();
    assert!(
        vote_msg.contains("p2p publish attestation failed"),
        "expected attestation publish wrapper, got: {vote_msg}",
    );

    service.stop(CancellationToken::new()).await.unwrap();
}
