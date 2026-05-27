//! Concurrency / cancellation tests for the sync `Loop`:
//! the `max_concurrent_peer_syncs` semaphore cap, per-`PeerId` dedup of
//! in-flight walks, and per-peer RPC timeout / cancellation.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use lean_chain::engine::BlockImportResult;
use lean_chain::ChainError;
use lean_core::Service as _;
use lean_sync::{Chain, Config, Loop, Network, PeerEventProvider, PeerId, SyncError};
use lean_wire::{BlocksByRootRequest, BlocksByRootResponse, Status};
use parking_lot::Mutex;
use protocol::{Checkpoint, SignedBlock, Slot};
use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;
use types::Bytes32;

fn cp(slot: u64) -> Checkpoint {
    Checkpoint::new(Bytes32::zero(), Slot::new(slot))
}

/// Local status pinned behind the peer so every walk proceeds past the
/// `should_sync` check.
fn behind() -> Status {
    Status {
        finalized: cp(0),
        head: cp(0),
    }
}

fn ahead() -> Status {
    Status {
        finalized: cp(0),
        head: cp(100),
    }
}

/// Minimal chain: always behind, knows nothing, accepts imports.
struct StubChain;

#[async_trait]
impl Chain for StubChain {
    async fn local_status(&self) -> Result<Status, ChainError> {
        Ok(behind())
    }
    async fn has_block(&self, _root: Bytes32) -> Result<bool, ChainError> {
        Ok(false)
    }
    async fn import_block(&self, _block: SignedBlock) -> Result<BlockImportResult, ChainError> {
        Ok(BlockImportResult::Accepted {
            block_root: Bytes32::zero(),
            parent_root: Bytes32::zero(),
            post_state_root: Bytes32::zero(),
            head_root: Bytes32::zero(),
        })
    }
}

struct ChannelPeers {
    handle: Mutex<Option<mpsc::Sender<PeerId>>>,
}

impl ChannelPeers {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            handle: Mutex::new(None),
        })
    }
    fn sender(&self) -> mpsc::Sender<PeerId> {
        self.handle
            .lock()
            .as_ref()
            .expect("subscribe first")
            .clone()
    }
}

#[async_trait]
impl PeerEventProvider for ChannelPeers {
    async fn subscribe_outbound_connected_peers(
        &self,
    ) -> Result<mpsc::Receiver<PeerId>, SyncError> {
        let (tx, rx) = mpsc::channel(256);
        *self.handle.lock() = Some(tx);
        Ok(rx)
    }
}

/// Network whose `send_status` blocks on a gate (a 0-permit semaphore)
/// while tracking the peak number of concurrently in-flight status
/// exchanges. Used to observe the concurrency cap.
struct GatedNetwork {
    in_flight: AtomicUsize,
    peak: AtomicUsize,
    status_calls: AtomicUsize,
    gate: Semaphore,
}

impl GatedNetwork {
    fn new() -> Self {
        Self {
            in_flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            status_calls: AtomicUsize::new(0),
            gate: Semaphore::new(0),
        }
    }
    fn release(&self, n: usize) {
        self.gate.add_permits(n);
    }
}

#[async_trait]
impl Network for GatedNetwork {
    async fn send_status(&self, _peer: &PeerId, _local: Status) -> Result<Status, SyncError> {
        let cur = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(cur, Ordering::SeqCst);
        self.status_calls.fetch_add(1, Ordering::SeqCst);
        // Block until the test opens the gate; consume the permit so the
        // gate is a one-way release, not a rotating one.
        self.gate.acquire().await.expect("gate open").forget();
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(ahead())
    }
    async fn request_blocks_by_root(
        &self,
        _peer: &PeerId,
        _req: BlocksByRootRequest,
    ) -> Result<BlocksByRootResponse, SyncError> {
        Ok(BlocksByRootResponse::new(Vec::new()).expect("empty within cap"))
    }
}

async fn poll_until(deadline_ms: u64, cond: impl Fn() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(deadline_ms);
    while tokio::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    cond()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_walks_capped_at_max_concurrent_peer_syncs() {
    const CAP: usize = 2;
    const PEERS: usize = 12;

    let chain = Arc::new(StubChain);
    let network = Arc::new(GatedNetwork::new());
    let peers = ChannelPeers::new();
    let config = Config::default().with_max_concurrent_peer_syncs(NonZeroUsize::new(CAP).unwrap());

    let lp = Loop::new(
        config,
        chain as Arc<dyn Chain>,
        Arc::clone(&network) as Arc<dyn Network>,
        peers.clone() as Arc<dyn PeerEventProvider>,
    );
    lp.start().await.unwrap();

    let sender = peers.sender();
    for i in 0..PEERS {
        sender
            .send(PeerId::new(format!("peer-{i}")).unwrap())
            .await
            .unwrap();
    }

    // CAP walks reach send_status and block on the gate; the rest wait
    // for a permit, so the in-flight count stabilizes at exactly CAP.
    assert!(
        poll_until(500, || network.status_calls.load(Ordering::SeqCst) >= CAP).await,
        "expected {CAP} walks to start",
    );
    // Give any (incorrectly) un-capped walks a chance to also start.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let peak = network.peak.load(Ordering::SeqCst);
    assert_eq!(
        peak, CAP,
        "peak concurrent walks must equal the cap, got {peak}"
    );

    // Release the gate so the walks drain, then stop.
    network.release(PEERS);
    lp.stop(CancellationToken::new()).await.unwrap();
}
