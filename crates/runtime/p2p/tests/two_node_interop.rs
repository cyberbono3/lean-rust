//! Two-node loopback interop smoke test.
//!
//! Brings up two [`P2pService`] instances over `/ip4/127.0.0.1/udp/0/quic-v1`
//! and exercises the three observable wire surfaces the runtime relies on:
//!
//! - `Status` handshake completes on connection establishment.
//! - Gossipsub delivers a published block from publisher to subscriber.
//! - `BlocksByRoot` recovers a block the subscriber did not receive over
//!   gossip.
//!
//! Each node uses real network IO (no mocked transport), so the tests
//! operate in real wall-clock time and rely on bounded [`tokio::time::timeout`]
//! guards. Node B discovers node A via the YAML bootnodes file
//! [`runtime_p2p`] already loads at `DevnetHost::build_with_provider`;
//! no extra dial-API surface is required.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lean_core::Service;
use lean_wire::{BlocksByRootRequest, Status};
use libp2p::{Multiaddr, PeerId};
use protocol::{Block, BlockBody, Checkpoint, SignedBlock, Slot, ValidatorIndex};
use runtime_p2p::{
    DevnetHost, Host, HostOptions, NoOpRpcProvider, P2pService, PublishError, RpcProvider,
};
use ssz::HashTreeRoot;
use tempfile::{tempdir, TempDir};
use tokio::time::{sleep, timeout, Instant};
use tokio_util::sync::CancellationToken;
use types::{Bytes32, Bytes4000};

/// Overall per-test wall-clock budget.
const TEST_DEADLINE: Duration = Duration::from_secs(15);
/// Bound on `send_blocks_by_root` round-trips.
const RPC_DEADLINE: Duration = Duration::from_secs(5);
/// Bound on gossipsub publish-retry until the mesh forms (heartbeat = 1 s,
/// see [`runtime_p2p`] internals); two heartbeats with margin.
const PUBLISH_DEADLINE: Duration = Duration::from_secs(5);
/// Inter-attempt back-off for the publish-retry loop.
const PUBLISH_BACKOFF: Duration = Duration::from_millis(50);
/// Bound on draining the inbound block channel for a target root.
const GOSSIP_DELIVERY_DEADLINE: Duration = Duration::from_secs(5);

/// In-memory [`RpcProvider`] holding a fixed block set and a static
/// `Status` value. Mirrors the shape of the in-`src/` `MapProvider`
/// kept here so the integration crate stays free-standing.
struct StoreProvider {
    blocks: HashMap<Bytes32, SignedBlock>,
    status: Status,
}

impl RpcProvider for StoreProvider {
    fn get_block_by_root(&self, root: &Bytes32) -> Option<SignedBlock> {
        self.blocks.get(root).cloned()
    }

    fn local_status(&self) -> Status {
        self.status
    }
}

/// Builds a [`SignedBlock`] with a non-default `slot`/`proposer_index`
/// pair so two seeds produce distinct tree roots. Returns the block and
/// its hash-tree-root keyed by the [`StoreProvider`].
fn block_with_seed(slot: u64, proposer: u64) -> (SignedBlock, Bytes32) {
    let message = Block {
        slot: Slot::new(slot),
        proposer_index: ValidatorIndex::new(proposer),
        parent_root: Bytes32::zero(),
        state_root: Bytes32::zero(),
        body: BlockBody::default(),
    };
    let signed = SignedBlock {
        message,
        signature: Bytes4000::default(),
    };
    let root = Bytes32::new(signed.hash_tree_root());
    (signed, root)
}

/// Constructs a `StoreProvider` from a slice of `(block, root)` pairs and
/// a matching `Status`.
fn store_provider(blocks: &[(SignedBlock, Bytes32)], status: Status) -> Arc<StoreProvider> {
    let map = blocks
        .iter()
        .cloned()
        .map(|(b, r)| (r, b))
        .collect::<HashMap<_, _>>();
    Arc::new(StoreProvider {
        blocks: map,
        status,
    })
}

fn options_in(dir: &Path, bootnodes: Option<&Path>) -> HostOptions {
    HostOptions::try_new(
        "/ip4/127.0.0.1/udp/0/quic-v1",
        "test/0.1.0",
        &dir.join("id"),
        bootnodes,
    )
    .unwrap()
}

/// Writes a one-line bootnodes YAML pointing at `peer_id` reachable at
/// `addr`, matching the format documented in `host/bootnodes.rs`.
fn write_bootnodes(dir: &Path, peer_id: PeerId, addr: &Multiaddr) -> PathBuf {
    let entry = format!("- {addr}/p2p/{peer_id}\n");
    let path = dir.join("bootnodes.yaml");
    std::fs::write(&path, entry).unwrap();
    path
}

/// Convenience: hash-tree-root of a [`SignedBlock`] as the [`Bytes32`]
/// the [`StoreProvider`] keys by. Replaces several inline
/// `Bytes32::new(block.hash_tree_root())` calls.
fn root_of(block: &SignedBlock) -> Bytes32 {
    Bytes32::new(block.hash_tree_root())
}

/// A started `P2pService` plus the on-disk identity directory and the
/// observable handles the tests need (host, peer id, bound multiaddr).
/// `_identity_dir` is held to keep the on-disk identity file alive for
/// the lifetime of the service.
struct NodeHandle {
    _identity_dir: TempDir,
    service: P2pService,
    host: Host,
    peer_id: PeerId,
    bound: Multiaddr,
}

/// Builds a service backed by `provider` and starts it.
async fn start_node(provider: Arc<dyn RpcProvider>, bootnodes: Option<&Path>) -> NodeHandle {
    let dir = tempdir().unwrap();
    let service =
        DevnetHost::build_with_provider(options_in(dir.path(), bootnodes), provider).unwrap();
    service.start().await.unwrap();
    let host = service.host().expect("host handle available while running");
    let peer_id = service.peer_id();
    let bound = service
        .bound_addr()
        .expect("bound addr available while running");
    NodeHandle {
        _identity_dir: dir,
        service,
        host,
        peer_id,
        bound,
    }
}

/// Brings up two nodes wired together: A binds an OS-assigned port,
/// then B starts with a bootnodes file pointing at A. The returned
/// [`TempDir`] holds the bootnodes file and must outlive both nodes.
async fn start_pair(
    provider_a: Arc<dyn RpcProvider>,
    provider_b: Arc<dyn RpcProvider>,
) -> (NodeHandle, NodeHandle, TempDir) {
    let a = start_node(provider_a, None).await;
    let bootnodes_dir = tempdir().unwrap();
    let bootnodes_path = write_bootnodes(bootnodes_dir.path(), a.peer_id, &a.bound);
    let b = start_node(provider_b, Some(&bootnodes_path)).await;
    (a, b, bootnodes_dir)
}

/// Best-effort parallel teardown for both nodes. Consumes the handles
/// so their identity directories drop after the services have stopped.
async fn stop_both(a: NodeHandle, b: NodeHandle) {
    let cancel = CancellationToken::new();
    let _ = tokio::join!(a.service.stop(cancel.clone()), b.service.stop(cancel));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_handshake_completes_on_dial() {
    timeout(TEST_DEADLINE, async {
        // Both providers expose `Status::default()`. Once B starts with
        // a bootnodes file pointing at A the swarm task fires the
        // Status handshake automatically on `ConnectionEstablished`.
        let (a, b, _bootnodes_dir) =
            start_pair(Arc::new(NoOpRpcProvider), Arc::new(NoOpRpcProvider)).await;

        // Proof: an RPC round-trip succeeds. A mismatched Status would
        // have triggered `disconnect_peer_id` in the inbound handler,
        // surfacing `RpcError::Outbound` here instead of `Ok(empty)`.
        let request = BlocksByRootRequest::new([Bytes32::zero()]).unwrap();
        let response = timeout(RPC_DEADLINE, b.host.send_blocks_by_root(a.peer_id, request))
            .await
            .expect("rpc round-trip did not complete within RPC_DEADLINE")
            .expect("rpc must succeed once the handshake is complete");
        assert!(
            response.blocks().is_empty(),
            "NoOpRpcProvider must yield an empty response, got {} blocks",
            response.blocks().len(),
        );

        stop_both(a, b).await;
    })
    .await
    .expect("status_handshake_completes_on_dial exceeded TEST_DEADLINE");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gossip_and_blocks_by_root_converge() {
    timeout(TEST_DEADLINE, async {
        // Two seed pairs → two distinct tree roots. Sanity-check the
        // distinctness so the assertion logic below does not silently
        // collapse the test if seed mutations stop affecting the root.
        let (b0, r0) = block_with_seed(1, 1);
        let (b1, r1) = block_with_seed(2, 2);
        assert_ne!(r0, r1, "seeds must produce distinct block roots");

        // Both providers report the same `Status`, so the handshake
        // accepts on both sides.
        let status = Status {
            finalized: Checkpoint::new(Bytes32::zero(), Slot::new(0)),
            head: Checkpoint::new(Bytes32::zero(), Slot::new(0)),
        };
        let provider_a = store_provider(&[(b0.clone(), r0), (b1.clone(), r1)], status);

        let (a, b, _bootnodes_dir) = start_pair(provider_a, Arc::new(NoOpRpcProvider)).await;

        let mut block_rx = b
            .service
            .take_block_receiver()
            .expect("block receiver available after start");

        // Publish `b1` from A. Retry past the transient
        // `Gossipsub(InsufficientPeers)` window — the mesh forms after
        // the gossipsub heartbeat fires (1 s; bound is two heartbeats).
        let publish_started = Instant::now();
        loop {
            match a.host.publish_block(&b1).await {
                Ok(_) => break,
                Err(PublishError::Gossipsub(_)) if publish_started.elapsed() < PUBLISH_DEADLINE => {
                    sleep(PUBLISH_BACKOFF).await;
                }
                Err(err) => panic!("publish_block must eventually succeed, last error: {err:?}"),
            }
        }

        // Drain the block receiver until the target root arrives.
        // gossipsub may surface unrelated messages first under load;
        // filter by root rather than accepting the first delivery.
        let delivered = timeout(GOSSIP_DELIVERY_DEADLINE, async {
            loop {
                let block = block_rx
                    .recv()
                    .await
                    .expect("block channel closed before delivery");
                if root_of(&block) == r1 {
                    break block;
                }
            }
        })
        .await
        .expect("gossipsub did not deliver block within GOSSIP_DELIVERY_DEADLINE");
        assert_eq!(root_of(&delivered), r1);

        // Recover `b0` via `BlocksByRoot` — B never saw it over gossip.
        let request = BlocksByRootRequest::new([r0]).unwrap();
        let response = timeout(RPC_DEADLINE, b.host.send_blocks_by_root(a.peer_id, request))
            .await
            .expect("blocks_by_root did not complete within RPC_DEADLINE")
            .expect("blocks_by_root must succeed against a populated provider");
        let recovered_roots: HashSet<Bytes32> = response.blocks().iter().map(root_of).collect();
        assert_eq!(recovered_roots, HashSet::from([r0]));

        // Convergence: B's observed root-set (gossip ∪ RPC) equals A's
        // provider keyset.
        let observed: HashSet<Bytes32> = std::iter::once(root_of(&delivered))
            .chain(recovered_roots)
            .collect();
        assert_eq!(
            observed,
            HashSet::from([r0, r1]),
            "B failed to converge on A's block set",
        );

        stop_both(a, b).await;
    })
    .await
    .expect("gossip_and_blocks_by_root_converge exceeded TEST_DEADLINE");
}
