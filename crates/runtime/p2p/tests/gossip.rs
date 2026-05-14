//! Integration tests for gossipsub topic registration + publish path.
//!
//! Coverage:
//! - `take_*_receiver` is one-shot on a started service.
//! - `take_*_receiver` returns `None` before `start`.
//! - `Host::publish_block` reaches the swarm task (returns
//!   `PublishError::Gossipsub(InsufficientPeers)` on a single-node
//!   start, proving the command-channel + dispatch wiring without
//!   needing a two-node mesh).
//! - `Host::publish_*` on a stopped service returns
//!   `PublishError::ChannelClosed`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use protocol::{SignedBlock, SignedVote};
use runtime_core::Service;
use runtime_p2p::{DevnetHost, HostOptions, P2pService, PublishError};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

fn options_in(dir: &Path) -> HostOptions {
    HostOptions::try_new(
        "/ip4/127.0.0.1/udp/0/quic-v1",
        "test/0.1.0",
        &dir.join("id"),
        None,
    )
    .unwrap()
}

fn build_service() -> (tempfile::TempDir, P2pService) {
    let dir = tempdir().unwrap();
    let service = DevnetHost::build(options_in(dir.path())).unwrap();
    (dir, service)
}

#[tokio::test]
async fn receivers_are_none_before_start() {
    let (_dir, service) = build_service();
    assert!(service.take_block_receiver().is_none());
    assert!(service.take_vote_receiver().is_none());
}

#[tokio::test]
async fn receivers_are_one_shot_after_start() {
    let (_dir, service) = build_service();
    service.start().await.unwrap();

    assert!(service.take_block_receiver().is_some());
    assert!(
        service.take_block_receiver().is_none(),
        "block receiver must be consumable only once",
    );

    assert!(service.take_vote_receiver().is_some());
    assert!(
        service.take_vote_receiver().is_none(),
        "vote receiver must be consumable only once",
    );

    service.stop(CancellationToken::new()).await.unwrap();
}

#[tokio::test]
async fn publish_block_without_mesh_peers_returns_insufficient_peers() {
    let (_dir, service) = build_service();
    service.start().await.unwrap();
    let host = service.host().expect("host handle available while running");

    let err = host
        .publish_block(&SignedBlock::default())
        .await
        .expect_err("publish must fail without mesh peers");
    // The specific variant is `InsufficientPeers` under single-node
    // conditions, but matching just the outer wrapper avoids pulling
    // libp2p into the test crate's surface — and protects against
    // libp2p version-bump variant churn.
    assert!(
        matches!(err, PublishError::Gossipsub(_)),
        "expected Gossipsub publish error, got {err:?}",
    );

    service.stop(CancellationToken::new()).await.unwrap();
}

#[tokio::test]
async fn publish_vote_without_mesh_peers_returns_insufficient_peers() {
    let (_dir, service) = build_service();
    service.start().await.unwrap();
    let host = service.host().expect("host handle available while running");

    let err = host
        .publish_vote(&SignedVote::default())
        .await
        .expect_err("publish must fail without mesh peers");
    // The specific variant is `InsufficientPeers` under single-node
    // conditions, but matching just the outer wrapper avoids pulling
    // libp2p into the test crate's surface — and protects against
    // libp2p version-bump variant churn.
    assert!(
        matches!(err, PublishError::Gossipsub(_)),
        "expected Gossipsub publish error, got {err:?}",
    );

    service.stop(CancellationToken::new()).await.unwrap();
}

#[tokio::test]
async fn publish_after_stop_returns_channel_closed() {
    let (_dir, service) = build_service();
    service.start().await.unwrap();
    let host = service.host().expect("host handle available while running");
    service.stop(CancellationToken::new()).await.unwrap();

    let err = host
        .publish_block(&SignedBlock::default())
        .await
        .expect_err("publish after stop must fail");
    assert!(
        matches!(err, PublishError::ChannelClosed),
        "expected ChannelClosed, got {err:?}",
    );
}
