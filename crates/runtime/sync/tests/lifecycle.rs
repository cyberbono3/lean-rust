//! Integration tests for sync `Loop` lifecycle (start / stop / status /
//! double-start / stop-before-start).

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use lean_chain::engine::BlockImportResult;
use lean_chain::ChainError;
use lean_core::Service as _;
use lean_sync::{Chain, Config, Loop, Network, PeerEventProvider, PeerId, SyncError};
use lean_wire::{BlocksByRootRequest, BlocksByRootResponse, Status};
use parking_lot::Mutex;
use protocol::SignedBlock;
use static_assertions::assert_impl_all;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use types::Bytes32;

assert_impl_all!(Loop: Send, Sync);
assert_impl_all!(SyncError: Send, Sync, std::error::Error);

// ---- Minimal no-op fakes --------------------------------------------------

struct NoopChain;

#[async_trait]
impl Chain for NoopChain {
    async fn local_status(&self) -> Result<Status, ChainError> {
        Ok(Status::default())
    }
    async fn has_block(&self, _root: Bytes32) -> Result<bool, ChainError> {
        Ok(true)
    }
    async fn import_block(&self, _signed: SignedBlock) -> Result<BlockImportResult, ChainError> {
        Ok(BlockImportResult::DuplicateBlock {
            block_root: Bytes32::zero(),
        })
    }
}

struct NoopNetwork;

#[async_trait]
impl Network for NoopNetwork {
    async fn send_status(&self, _peer: &PeerId, _local: Status) -> Result<Status, SyncError> {
        Ok(Status::default())
    }
    async fn request_blocks_by_root(
        &self,
        _peer: &PeerId,
        _req: BlocksByRootRequest,
    ) -> Result<BlocksByRootResponse, SyncError> {
        Ok(BlocksByRootResponse::new(Vec::<SignedBlock>::new()).expect("empty response"))
    }
}

struct ScriptedPeers {
    tx: Mutex<Option<mpsc::Sender<PeerId>>>,
}

impl ScriptedPeers {
    fn new() -> (Arc<Self>, mpsc::Sender<PeerId>) {
        let (tx, _rx) = mpsc::channel(8);
        let provider = Arc::new(Self {
            tx: Mutex::new(None),
        });
        (provider, tx)
    }
}

#[async_trait]
impl PeerEventProvider for ScriptedPeers {
    async fn subscribe_outbound_connected_peers(
        &self,
    ) -> Result<mpsc::Receiver<PeerId>, SyncError> {
        let (tx, rx) = mpsc::channel(8);
        *self.tx.lock() = Some(tx);
        Ok(rx)
    }
}

fn build_noop_loop() -> Loop {
    let (peers, _) = ScriptedPeers::new();
    Loop::new(
        Config::default(),
        Arc::new(NoopChain),
        Arc::new(NoopNetwork),
        peers,
    )
}

// ---- Config / construction ------------------------------------------------

#[test]
fn default_config_uses_default_max_sync_depth() {
    assert_eq!(
        Config::default().max_sync_depth,
        Config::DEFAULT_MAX_SYNC_DEPTH
    );
}

#[test]
fn config_try_from_rejects_zero_max_sync_depth() {
    let err = Config::try_from(0usize).unwrap_err();
    assert!(matches!(err, SyncError::InvalidMaxSyncDepth));
}

#[test]
fn config_try_from_accepts_non_zero_max_sync_depth() {
    let cfg = Config::try_from(7usize).expect("7 is non-zero");
    assert_eq!(cfg.max_sync_depth.get(), 7);
}

// ---- Lifecycle ------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn start_status_stop_round_trip() {
    let lp = build_noop_loop();
    assert!(lp.status().await.is_err()); // NotStarted
    lp.start().await.unwrap();
    lp.status().await.unwrap();
    lp.stop(CancellationToken::new()).await.unwrap();
    assert!(lp.status().await.is_err()); // back to NotStarted
}

#[tokio::test(flavor = "current_thread")]
async fn double_start_is_rejected() {
    let lp = build_noop_loop();
    lp.start().await.unwrap();
    let err = lp.start().await.unwrap_err();
    assert!(format!("{err}").contains("already started"));
    lp.stop(CancellationToken::new()).await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn stop_before_start_is_noop() {
    let lp = build_noop_loop();
    lp.stop(CancellationToken::new()).await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn subscription_close_keeps_loop_alive_then_stops_cleanly() {
    let lp = build_noop_loop();
    lp.start().await.unwrap();
    // No events pushed; just give the scheduler a tick then stop.
    tokio::time::sleep(Duration::from_millis(5)).await;
    lp.status().await.unwrap();
    lp.stop(CancellationToken::new()).await.unwrap();
}
