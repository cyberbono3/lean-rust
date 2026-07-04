//! Integration tests for sync `Loop` walk-back semantics:
//! peer-not-ahead skip, multi-chunk backfill, stop-at-known-block,
//! `MaxSyncDepth` cap, and graceful network-failure handling.
//!
//! The chain surface is now the concrete [`runtime::chain::Service`] over a
//! genesis-fixture engine + in-memory store (the `Chain` port was
//! collapsed). Local status is therefore fixed at genesis (head slot 0);
//! "known" blocks are seeded directly into the store. The synthetic
//! walk-back blocks are not engine-valid, so the real engine drops them
//! at import — assertions cover the observable **network** behaviour
//! (status/`BlocksByRoot` request counts) rather than imported blocks.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use lean_wire::{BlocksByRootRequest, BlocksByRootResponse, Status};
use parking_lot::Mutex;
use protocol::{Block, BlockBody, Checkpoint, SignedBlock, Slot, ValidatorIndex};
use runtime::chain::Service as ChainService;
use runtime::core::Service as _;
use runtime::sync::{Config, Loop, Network, PeerEventProvider, PeerId, SyncError};
use ssz::HashTreeRoot;
use storage::{MemoryStore, Store};
use tokio_util::sync::CancellationToken;
use types::{Bytes32, Bytes4000};

mod sync_common;
use sync_common::{poll_until, ChannelPeers};

// ---- Helpers --------------------------------------------------------------

/// Returns the real `hash_tree_root` of the slot-N block in the canonical
/// linear test chain (slot 1's parent is `Bytes32::zero()`; each subsequent
/// slot's parent is the previous block's hash). Computed recursively so
/// the synthetic chain roots match what `loop_::walk_back` validates the
/// peer's `BlocksByRoot` responses against.
fn root_of(n: u8) -> Bytes32 {
    if n == 0 {
        return Bytes32::zero();
    }
    let parent = root_of(n - 1);
    make_block(u64::from(n), parent)
        .message
        .hash_tree_root()
        .into()
}

/// Builds a `SignedBlock` for the canonical linear test chain. The
/// resulting block's `hash_tree_root` is the value returned by
/// `root_of(slot)`, so the fake-network response satisfies the
/// walk-back hash-validation.
fn make_block(slot: u64, parent: Bytes32) -> SignedBlock {
    SignedBlock {
        message: Block {
            slot: Slot::new(slot),
            proposer_index: ValidatorIndex::new(0),
            parent_root: parent,
            state_root: Bytes32::zero(),
            body: BlockBody::default(),
        },
        signature: Bytes4000::new([0; 4000]),
    }
}

fn cp(root: Bytes32, slot: u64) -> Checkpoint {
    Checkpoint::new(root, Slot::new(slot))
}

/// Builds a genesis-fixture chain service whose store is pre-seeded so
/// `has_block(root_of(n))` returns `true` for each `n` in `known` — this
/// is how the walk-back "stop at known block" boundary is set now that
/// the chain is concrete.
fn chain_with_known(known: &[u8]) -> Arc<ChainService> {
    let (state, block) = runtime::chain::engine::test_fixtures::anchor_pair(4);
    let engine = runtime::chain::engine::Engine::from_anchor(state, block).unwrap();
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    for &n in known {
        let parent = if n <= 1 {
            Bytes32::zero()
        } else {
            root_of(n - 1)
        };
        store
            .save_block(root_of(n), make_block(u64::from(n), parent))
            .unwrap();
    }
    Arc::new(ChainService::new(engine, store))
}

// ---- FakeNetwork ----------------------------------------------------------

struct FakeNetwork {
    peer_status: Status,
    /// Maps requested root → block returned. Tests prime this with the
    /// chain to be backfilled.
    chain_by_root: Mutex<HashMap<Bytes32, SignedBlock>>,
    status_calls: AtomicUsize,
    request_calls: AtomicUsize,
    /// When `Some(n)`, the n-th `request_blocks_by_root` call (1-indexed)
    /// returns a transport error.
    fail_request_at: Option<usize>,
}

impl FakeNetwork {
    fn new(peer_status: Status) -> Self {
        Self {
            peer_status,
            chain_by_root: Mutex::new(HashMap::new()),
            status_calls: AtomicUsize::new(0),
            request_calls: AtomicUsize::new(0),
            fail_request_at: None,
        }
    }

    fn with_fail_at(mut self, n: usize) -> Self {
        self.fail_request_at = Some(n);
        self
    }

    fn install(&self, root: Bytes32, block: SignedBlock) {
        self.chain_by_root.lock().insert(root, block);
    }

    fn status_call_count(&self) -> usize {
        self.status_calls.load(Ordering::SeqCst)
    }
    fn request_call_count(&self) -> usize {
        self.request_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Network for FakeNetwork {
    async fn send_status(&self, _peer: &PeerId, _local: Status) -> Result<Status, SyncError> {
        self.status_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.peer_status)
    }
    async fn request_blocks_by_root(
        &self,
        _peer: &PeerId,
        req: BlocksByRootRequest,
    ) -> Result<BlocksByRootResponse, SyncError> {
        let n = self.request_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if Some(n) == self.fail_request_at {
            return Err(SyncError::Network("scripted failure".into()));
        }
        let requested = req.roots().first().copied().unwrap_or_else(Bytes32::zero);
        let resp = match self.chain_by_root.lock().get(&requested).cloned() {
            Some(block) => vec![block],
            None => Vec::new(),
        };
        Ok(BlocksByRootResponse::new(resp).expect("response within cap"))
    }
}

/// Installs the canonical linear chain (slots `1..=hi`) into `network`.
fn install_linear_chain(network: &FakeNetwork, hi: u8) {
    for n in 1..=hi {
        let parent = if n == 1 {
            Bytes32::zero()
        } else {
            root_of(n - 1)
        };
        network.install(root_of(n), make_block(u64::from(n), parent));
    }
}

// ---- Drive helper ---------------------------------------------------------

/// Spawns the loop, pushes one peer event, polls until the network
/// observed the expected effect, then stops the loop. Bounded retry so
/// flakes surface as test failures (max 200ms).
async fn drive_once(
    config: Config,
    chain: Arc<ChainService>,
    network: Arc<FakeNetwork>,
    peers: Arc<ChannelPeers>,
    until: impl Fn(&FakeNetwork) -> bool + Copy,
) {
    let lp = Loop::new(
        config,
        chain,
        network.clone() as Arc<dyn Network>,
        peers.clone() as Arc<dyn PeerEventProvider>,
    );
    lp.start().await.unwrap();

    let sender = peers.sender();
    sender
        .send(PeerId::new("peer-a").expect("non-empty test peer id"))
        .await
        .unwrap();

    let observed = poll_until(200, || until(&network)).await;
    assert!(
        observed,
        "drive_once timed out: status_calls={}, request_calls={}",
        network.status_call_count(),
        network.request_call_count(),
    );

    lp.stop(CancellationToken::new()).await.unwrap();
}

// ---- Tests ----------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn status_rpc_sent_on_peer_connect() {
    // Peer not ahead (genesis == genesis) → loop sends Status, skips walk.
    let chain = chain_with_known(&[]);
    let network = Arc::new(FakeNetwork::new(Status::default()));
    let peers = ChannelPeers::new();

    drive_once(Config::default(), chain, network.clone(), peers, |n| {
        n.status_call_count() == 1
    })
    .await;

    assert_eq!(network.request_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn sync_skips_when_peer_not_ahead() {
    // Peer head equals local genesis head → no backfill.
    let chain = chain_with_known(&[]);
    let peer = Status {
        finalized: cp(Bytes32::zero(), 0),
        head: cp(Bytes32::zero(), 0),
    };
    let network = Arc::new(FakeNetwork::new(peer));
    let peers = ChannelPeers::new();

    drive_once(Config::default(), chain, network.clone(), peers, |n| {
        n.status_call_count() == 1
    })
    .await;

    assert_eq!(network.request_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn sync_walks_back_all_chunks() {
    // Linear chain of 5 blocks; peer head = slot 5. Local (genesis) knows
    // none of them, so walk-back fetches all five before hitting the
    // zero-root genesis boundary.
    let chain = chain_with_known(&[]);
    let peer = Status {
        finalized: cp(Bytes32::zero(), 0),
        head: cp(root_of(5), 5),
    };
    let network = Arc::new(FakeNetwork::new(peer));
    install_linear_chain(&network, 5);

    let peers = ChannelPeers::new();
    drive_once(Config::default(), chain, network.clone(), peers, |n| {
        n.request_call_count() == 5
    })
    .await;

    assert_eq!(network.request_call_count(), 5);
}

#[tokio::test(flavor = "current_thread")]
async fn sync_stops_walk_at_known_block() {
    // 5-block chain; local store already has slot 2 → walk stops after
    // fetching slots 5, 4, 3 (3 requests).
    let chain = chain_with_known(&[2]);
    let peer = Status {
        finalized: cp(Bytes32::zero(), 0),
        head: cp(root_of(5), 5),
    };
    let network = Arc::new(FakeNetwork::new(peer));
    for n in 3..=5u8 {
        network.install(root_of(n), make_block(u64::from(n), root_of(n - 1)));
    }

    let peers = ChannelPeers::new();
    drive_once(Config::default(), chain, network.clone(), peers, |n| {
        n.request_call_count() == 3
    })
    .await;

    assert_eq!(network.request_call_count(), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn sync_caps_walk_at_max_sync_depth() {
    // 5-block chain, cap = 3 → exactly 3 requests before the depth cap
    // halts the walk.
    let chain = chain_with_known(&[]);
    let peer = Status {
        finalized: cp(Bytes32::zero(), 0),
        head: cp(root_of(5), 5),
    };
    let network = Arc::new(FakeNetwork::new(peer));
    install_linear_chain(&network, 5);

    let peers = ChannelPeers::new();
    drive_once(
        Config::try_from(3usize).expect("3 is non-zero"),
        chain,
        network.clone(),
        peers,
        |n| n.request_call_count() == 3,
    )
    .await;

    assert_eq!(network.request_call_count(), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn peer_network_failure_aborts_peer_sync_not_loop() {
    let chain = chain_with_known(&[]);
    let peer = Status {
        finalized: cp(Bytes32::zero(), 0),
        head: cp(root_of(5), 5),
    };
    let network = Arc::new(FakeNetwork::new(peer).with_fail_at(2));
    install_linear_chain(&network, 5);

    let peers = ChannelPeers::new();
    let lp = Loop::new(
        Config::default(),
        chain,
        network.clone() as Arc<dyn Network>,
        peers.clone() as Arc<dyn PeerEventProvider>,
    );
    lp.start().await.unwrap();

    peers
        .sender()
        .send(PeerId::new("peer-x").expect("non-empty test peer id"))
        .await
        .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
    while tokio::time::Instant::now() < deadline {
        if network.request_call_count() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(network.request_call_count() >= 2);

    // Loop still alive after the per-peer walk aborts on the request error.
    lp.status().await.unwrap();

    lp.stop(CancellationToken::new()).await.unwrap();
}
