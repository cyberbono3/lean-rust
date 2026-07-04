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
use lean_wire::{BlocksByRootRequest, BlocksByRootResponse, Status};
use protocol::{Checkpoint, Slot};
use runtime::chain::Service as ChainService;
use runtime::core::Service as _;
use runtime::sync::{Config, Loop, Network, PeerEventProvider, PeerId, SyncError};
use storage::{MemoryStore, Store};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use types::Bytes32;

mod sync_common;
use sync_common::{poll_until, ChannelPeers};

fn cp(slot: u64) -> Checkpoint {
    Checkpoint::new(Bytes32::zero(), Slot::new(slot))
}

fn ahead() -> Status {
    // Non-zero head root so walk_back does not short-circuit on the
    // zero-root guard before issuing a BlocksByRoot request.
    Status {
        finalized: cp(0),
        head: Checkpoint::new(Bytes32::new([7u8; 32]), Slot::new(100)),
    }
}

/// Genesis-fixture chain service: local status is pinned at genesis (head
/// slot 0), behind the `ahead()` peer, and its empty store reports
/// `has_block == false` so every walk proceeds past `should_sync` and
/// issues a `BlocksByRoot` request.
fn chain_service() -> Arc<ChainService> {
    let (state, block) = runtime::chain::engine::test_fixtures::anchor_pair(4);
    let engine = runtime::chain::engine::Engine::from_anchor(state, block).unwrap();
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    Arc::new(ChainService::new(engine, store))
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

/// Network that answers `send_status` immediately (peer ahead) but whose
/// `request_blocks_by_root` blocks forever — modelling a peer that opens
/// the substream but never replies.
struct BlockingRpcNetwork {
    request_calls: AtomicUsize,
}

impl BlockingRpcNetwork {
    fn new() -> Self {
        Self {
            request_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Network for BlockingRpcNetwork {
    async fn send_status(&self, _peer: &PeerId, _local: Status) -> Result<Status, SyncError> {
        Ok(ahead())
    }
    async fn request_blocks_by_root(
        &self,
        _peer: &PeerId,
        _req: BlocksByRootRequest,
    ) -> Result<BlocksByRootResponse, SyncError> {
        self.request_calls.fetch_add(1, Ordering::SeqCst);
        // Never resolves: the walk must abort via timeout or cancel.
        std::future::pending().await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_walks_capped_at_max_concurrent_peer_syncs() {
    const CAP: usize = 2;
    const PEERS: usize = 12;

    let chain = chain_service();
    let network = Arc::new(GatedNetwork::new());
    let peers = ChannelPeers::new();
    let config = Config::default().with_max_concurrent_peer_syncs(NonZeroUsize::new(CAP).unwrap());

    let lp = Loop::new(
        config,
        chain,
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
    // Best-effort negative-window probe (NOT a sync point): give any
    // incorrectly un-capped walks a chance to also reach send_status before
    // we read the peak. The positive path is already gated deterministically
    // by the 0-permit `gate`, so this sleep only widens the window in which a
    // regression (peak > CAP) would surface; it cannot cause a false pass.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_peer_flap_yields_single_walk() {
    const FLAPS: usize = 100;

    let chain = chain_service();
    let network = Arc::new(GatedNetwork::new());
    let peers = ChannelPeers::new();

    let lp = Loop::new(
        Config::default(),
        chain,
        Arc::clone(&network) as Arc<dyn Network>,
        peers.clone() as Arc<dyn PeerEventProvider>,
    );
    lp.start().await.unwrap();

    // The first event spawns a walk that blocks in send_status; the
    // remaining 99 same-peer events are deduped while it is in flight.
    let sender = peers.sender();
    for _ in 0..FLAPS {
        sender.send(PeerId::new("peer-a").unwrap()).await.unwrap();
    }

    assert!(
        poll_until(500, || network.status_calls.load(Ordering::SeqCst) >= 1).await,
        "expected the single walk to start",
    );
    // Best-effort negative-window probe (NOT a sync point): let any
    // incorrectly un-deduped walks also reach send_status before we assert the
    // count. The single walk is held in flight by the 0-permit `gate`, so this
    // sleep only widens the regression-detection window; it cannot false-pass.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let calls = network.status_calls.load(Ordering::SeqCst);
    assert_eq!(
        calls, 1,
        "a flapping peer must yield exactly one walk, got {calls}"
    );

    network.release(FLAPS);
    lp.stop(CancellationToken::new()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_timeout_aborts_walk_and_frees_permit() {
    // cap = 1, request blocks forever, short request timeout. peer-a's
    // walk times out and frees the single permit, so peer-b's walk then
    // reaches its own request — request_calls climbs to 2 only because
    // the first walk aborted on timeout.
    let chain = chain_service();
    let network = Arc::new(BlockingRpcNetwork::new());
    let peers = ChannelPeers::new();
    let config = Config::default()
        .with_max_concurrent_peer_syncs(NonZeroUsize::new(1).unwrap())
        .with_request_timeout(Duration::from_millis(80));

    let lp = Loop::new(
        config,
        chain,
        Arc::clone(&network) as Arc<dyn Network>,
        peers.clone() as Arc<dyn PeerEventProvider>,
    );
    lp.start().await.unwrap();

    let sender = peers.sender();
    sender.send(PeerId::new("peer-a").unwrap()).await.unwrap();
    sender.send(PeerId::new("peer-b").unwrap()).await.unwrap();

    assert!(
        poll_until(1000, || network.request_calls.load(Ordering::SeqCst) >= 2).await,
        "second peer's walk must run only after the first times out; got {}",
        network.request_calls.load(Ordering::SeqCst),
    );

    lp.stop(CancellationToken::new()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_drains_with_peer_mid_rpc() {
    // A walk is parked in a never-resolving request (default 10 s
    // timeout, so the timeout does not fire). Loop::stop must still
    // return Ok by cancelling the per-peer child token — not hang until
    // the shutdown budget elapses or report "per-peer tasks did not
    // drain". (If cancellation regressed this test would hang.)
    let chain = chain_service();
    let network = Arc::new(BlockingRpcNetwork::new());
    let peers = ChannelPeers::new();

    let lp = Loop::new(
        Config::default(),
        chain,
        Arc::clone(&network) as Arc<dyn Network>,
        peers.clone() as Arc<dyn PeerEventProvider>,
    );
    lp.start().await.unwrap();

    peers
        .sender()
        .send(PeerId::new("peer-stuck").unwrap())
        .await
        .unwrap();
    assert!(
        poll_until(500, || network.request_calls.load(Ordering::SeqCst) >= 1).await,
        "walk must reach the blocking request",
    );

    // Pass an un-cancelled budget token: stop must drain via the
    // internal per-peer cancel, not the budget.
    lp.stop(CancellationToken::new()).await.unwrap();
}
