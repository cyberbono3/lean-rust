//! Integration tests for sync `Loop` walk-back semantics:
//! peer-not-ahead skip, multi-chunk backfill, stop-at-known-block,
//! `MaxSyncDepth` cap, and graceful network-failure handling.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use engine::BlockImportResult;
use lean_chain::ChainError;
use lean_core::Service as _;
use lean_sync::{Chain, Config, Loop, Network, PeerEventProvider, PeerId, SyncError};
use networking::{BlocksByRootRequest, BlocksByRootResponse, Status};
use parking_lot::Mutex;
use protocol::{Block, BlockBody, Checkpoint, SignedBlock, Slot, ValidatorIndex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use types::{Bytes32, Bytes4000};

// ---- Helpers --------------------------------------------------------------

fn root_of(n: u8) -> Bytes32 {
    let mut bytes = [0u8; 32];
    bytes[0] = n;
    Bytes32::new(bytes)
}

/// Builds a `SignedBlock` whose hash-tree-root identity is opaque — the
/// loop never inspects it, only `parent_root` and `slot`. The synthetic
/// `block_root` is injected through the fake network's `chain_by_root`
/// map; the engine is never invoked.
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

// ---- FakeChain ------------------------------------------------------------

struct FakeChain {
    local_status: Mutex<Status>,
    known: Mutex<HashSet<Bytes32>>,
    imported: Mutex<Vec<SignedBlock>>,
    /// Returns `MissingParent` if the block's parent is not in `known`
    /// at import time. Mirrors engine semantics so cap-tests can assert
    /// the deepest block is dropped.
    strict_import: bool,
}

impl FakeChain {
    fn new(local_status: Status, strict_import: bool) -> Self {
        Self {
            local_status: Mutex::new(local_status),
            known: Mutex::new(HashSet::new()),
            imported: Mutex::new(Vec::new()),
            strict_import,
        }
    }

    fn imported_slots(&self) -> Vec<u64> {
        self.imported
            .lock()
            .iter()
            .map(|b| b.message.slot.get())
            .collect()
    }

    fn add_known(&self, root: Bytes32) {
        self.known.lock().insert(root);
    }
}

#[async_trait]
impl Chain for FakeChain {
    async fn local_status(&self) -> Result<Status, ChainError> {
        Ok(*self.local_status.lock())
    }
    async fn has_block(&self, root: Bytes32) -> Result<bool, ChainError> {
        Ok(self.known.lock().contains(&root))
    }
    async fn import_block(&self, signed: SignedBlock) -> Result<BlockImportResult, ChainError> {
        let parent = signed.message.parent_root;
        let block_root = root_of(u8::try_from(signed.message.slot.get()).expect("slot < 256"));
        let parent_known = parent == Bytes32::zero() || self.known.lock().contains(&parent);
        if self.strict_import && !parent_known {
            return Ok(BlockImportResult::MissingParent {
                block_root,
                parent_root: parent,
            });
        }
        self.imported.lock().push(signed);
        self.known.lock().insert(block_root);
        Ok(BlockImportResult::Accepted {
            block_root,
            parent_root: parent,
            post_state_root: Bytes32::zero(),
            head_root: block_root,
        })
    }
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

// ---- ChannelPeers ---------------------------------------------------------

struct ChannelPeers {
    tx: Mutex<Option<mpsc::Sender<PeerId>>>,
    /// `Sender` clone surfaced to the test so it can push events after
    /// `start` and close the channel by dropping it.
    handle: Mutex<Option<mpsc::Sender<PeerId>>>,
}

impl ChannelPeers {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            tx: Mutex::new(None),
            handle: Mutex::new(None),
        })
    }

    fn sender(&self) -> mpsc::Sender<PeerId> {
        self.handle
            .lock()
            .as_ref()
            .expect("subscribe before sender")
            .clone()
    }
}

#[async_trait]
impl PeerEventProvider for ChannelPeers {
    async fn subscribe_outbound_connected_peers(
        &self,
    ) -> Result<mpsc::Receiver<PeerId>, SyncError> {
        let (tx, rx) = mpsc::channel(8);
        *self.tx.lock() = Some(tx.clone());
        *self.handle.lock() = Some(tx);
        Ok(rx)
    }
}

// ---- Drive helper ---------------------------------------------------------

/// Spawns the loop, pushes one peer event, polls until the chain
/// observed the expected effect, then stops the loop. Bounded retry so
/// flakes surface as test failures (max 200ms).
async fn drive_once(
    config: Config,
    chain: Arc<FakeChain>,
    network: Arc<FakeNetwork>,
    peers: Arc<ChannelPeers>,
    until: impl Fn(&FakeChain, &FakeNetwork) -> bool + Copy,
) {
    let lp = Loop::new(
        config,
        chain.clone() as Arc<dyn Chain>,
        network.clone() as Arc<dyn Network>,
        peers.clone() as Arc<dyn PeerEventProvider>,
    );
    lp.start().await.unwrap();

    let sender = peers.sender();
    sender
        .send(PeerId::new("peer-a").expect("non-empty test peer id"))
        .await
        .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
    while tokio::time::Instant::now() < deadline {
        if until(&chain, &network) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(
        until(&chain, &network),
        "drive_once timed out: status_calls={}, request_calls={}, imported={:?}",
        network.status_call_count(),
        network.request_call_count(),
        chain.imported_slots(),
    );

    lp.stop(CancellationToken::new()).await.unwrap();
}

// ---- Tests ----------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn status_rpc_sent_on_peer_connect() {
    // Peer not ahead → loop sends Status but skips walk.
    let local = Status {
        finalized: cp(Bytes32::zero(), 0),
        head: cp(root_of(1), 1),
    };
    let chain = Arc::new(FakeChain::new(local, false));
    let network = Arc::new(FakeNetwork::new(local));
    let peers = ChannelPeers::new();

    drive_once(
        Config::default(),
        chain.clone(),
        network.clone(),
        peers,
        |_c, n| n.status_call_count() == 1,
    )
    .await;

    assert_eq!(network.request_call_count(), 0);
    assert!(chain.imported_slots().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn sync_skips_when_peer_not_ahead() {
    let local = Status {
        finalized: cp(Bytes32::zero(), 1),
        head: cp(root_of(5), 5),
    };
    let peer = Status {
        finalized: cp(Bytes32::zero(), 1),
        head: cp(root_of(5), 5),
    };
    let chain = Arc::new(FakeChain::new(local, false));
    let network = Arc::new(FakeNetwork::new(peer));
    let peers = ChannelPeers::new();

    drive_once(
        Config::default(),
        chain.clone(),
        network.clone(),
        peers,
        |_c, n| n.status_call_count() == 1,
    )
    .await;

    assert_eq!(network.request_call_count(), 0);
    assert!(chain.imported_slots().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn sync_imports_all_chunks_in_forward_order() {
    // Linear chain of 5 blocks rooted at slot 1..=5. Peer head = slot 5.
    // Local knows nothing; walk-back fetches all five, imports oldest first.
    let local = Status {
        finalized: cp(Bytes32::zero(), 0),
        head: cp(Bytes32::zero(), 0),
    };
    let peer = Status {
        finalized: cp(Bytes32::zero(), 0),
        head: cp(root_of(5), 5),
    };
    let chain = Arc::new(FakeChain::new(local, false));
    let network = Arc::new(FakeNetwork::new(peer));

    // Build chain: slot N's parent is root_of(N-1); slot 1's parent is zero.
    for n in 1..=5u8 {
        let parent = if n == 1 {
            Bytes32::zero()
        } else {
            root_of(n - 1)
        };
        network.install(root_of(n), make_block(u64::from(n), parent));
    }

    let peers = ChannelPeers::new();
    drive_once(
        Config::default(),
        chain.clone(),
        network.clone(),
        peers,
        |c, _n| c.imported.lock().len() == 5,
    )
    .await;

    assert_eq!(network.request_call_count(), 5);
    assert_eq!(chain.imported_slots(), vec![1, 2, 3, 4, 5]);
}

#[tokio::test(flavor = "current_thread")]
async fn sync_stops_walk_at_known_block() {
    // 5-block chain; local already has slot 2 → walk should stop after
    // fetching slots 5, 4, 3 (3 requests; 3 imports in forward order).
    let local = Status {
        finalized: cp(Bytes32::zero(), 0),
        head: cp(root_of(2), 2),
    };
    let peer = Status {
        finalized: cp(Bytes32::zero(), 0),
        head: cp(root_of(5), 5),
    };
    let chain = Arc::new(FakeChain::new(local, false));
    chain.add_known(root_of(1));
    chain.add_known(root_of(2));

    let network = Arc::new(FakeNetwork::new(peer));
    for n in 3..=5u8 {
        network.install(root_of(n), make_block(u64::from(n), root_of(n - 1)));
    }

    let peers = ChannelPeers::new();
    drive_once(
        Config::default(),
        chain.clone(),
        network.clone(),
        peers,
        |c, _n| c.imported.lock().len() == 3,
    )
    .await;

    assert_eq!(network.request_call_count(), 3);
    assert_eq!(chain.imported_slots(), vec![3, 4, 5]);
}

#[tokio::test(flavor = "current_thread")]
async fn sync_caps_walk_at_max_sync_depth() {
    // 5-block chain, cap = 3 → exactly 3 requests; deepest block has
    // an unknown parent and is dropped at import (strict_import = true).
    let local = Status {
        finalized: cp(Bytes32::zero(), 0),
        head: cp(Bytes32::zero(), 0),
    };
    let peer = Status {
        finalized: cp(Bytes32::zero(), 0),
        head: cp(root_of(5), 5),
    };
    let chain = Arc::new(FakeChain::new(local, true));
    let network = Arc::new(FakeNetwork::new(peer));
    for n in 1..=5u8 {
        let parent = if n == 1 {
            Bytes32::zero()
        } else {
            root_of(n - 1)
        };
        network.install(root_of(n), make_block(u64::from(n), parent));
    }

    let peers = ChannelPeers::new();
    drive_once(
        Config::try_from(3usize).expect("3 is non-zero"),
        chain.clone(),
        network.clone(),
        peers,
        |_c, n| n.request_call_count() == 3,
    )
    .await;

    // Cap = 3: fetched slots 5, 4, 3. Slot 3's parent is slot 2 (unknown);
    // strict import drops slot 3 with MissingParent → zero imports.
    assert_eq!(network.request_call_count(), 3);
    assert!(
        chain.imported_slots().is_empty(),
        "deepest block must be dropped on missing parent; got {:?}",
        chain.imported_slots(),
    );
}

#[tokio::test(flavor = "current_thread")]
async fn peer_network_failure_aborts_peer_sync_not_loop() {
    let local = Status {
        finalized: cp(Bytes32::zero(), 0),
        head: cp(Bytes32::zero(), 0),
    };
    let peer = Status {
        finalized: cp(Bytes32::zero(), 0),
        head: cp(root_of(5), 5),
    };
    let chain = Arc::new(FakeChain::new(local, false));
    let network = Arc::new(FakeNetwork::new(peer).with_fail_at(2));
    for n in 1..=5u8 {
        let parent = if n == 1 {
            Bytes32::zero()
        } else {
            root_of(n - 1)
        };
        network.install(root_of(n), make_block(u64::from(n), parent));
    }

    let peers = ChannelPeers::new();
    let lp = Loop::new(
        Config::default(),
        chain.clone() as Arc<dyn Chain>,
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

    // Loop still alive after per-peer abort.
    lp.status().await.unwrap();
    // Walk aborted on the 2nd request error → zero forward-imports.
    assert!(chain.imported_slots().is_empty());

    lp.stop(CancellationToken::new()).await.unwrap();
}
