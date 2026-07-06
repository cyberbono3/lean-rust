//! `Loop::initial_sync` no-ops when no peer is connected — the
//! single-process path that lets a lone node self-drive.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use runtime::chain::Service as ChainService;
use runtime::p2p::{DevnetHost, HostOptions};
use runtime::sync::{Config, Loop};
use storage::MemoryStore;
use tempfile::tempdir;
use tokio::time::timeout;

#[tokio::test]
async fn initial_sync_is_noop_without_peers() {
    let (state, block) = runtime::chain::engine::test_fixtures::anchor_pair(4);
    let engine = runtime::chain::engine::Engine::from_anchor(state, block).unwrap();
    let chain = Arc::new(ChainService::new(engine, Arc::new(MemoryStore::default())));

    // A constructed-but-not-started host: its peer registry is empty, so
    // `connected_peers()` yields nothing.
    let dir = tempdir().unwrap();
    let options = HostOptions::try_new(
        "/ip4/127.0.0.1/udp/0/quic-v1",
        "test/0.1.0",
        &dir.path().join("id"),
        None,
    )
    .unwrap();
    let p2p = Arc::new(DevnetHost::build(options).unwrap());
    assert!(p2p.connected_peers().is_empty(), "no peers before start");

    let sync = Loop::new(Config::default(), chain, p2p);

    // With no connected peers, initial_sync must return promptly (no walk,
    // no hang). The bound guards against a regression that would block on an
    // absent peer.
    timeout(Duration::from_secs(5), sync.initial_sync())
        .await
        .expect("initial_sync must return promptly with no peers");
}
