//! Port traits consumed by the sync [`Loop`](super::Loop).
//!
//! Following the dependency-inversion pattern: this module declares
//! the traits, and adapters in the `node` crate (libp2p-backed) and
//! the [`crate::Service`] adapter implement them. The sync module
//! compiles with zero `libp2p` exposure on its dependency graph.
//!
//! # Trait bounds
//!
//! Each trait carries `Send + Sync + 'static` because [`Loop`] holds
//! its ports as `Arc<dyn Trait>` and shares them across spawned tasks.
//!
//! # Cancellation
//!
//! All async trait methods are expected to be cancellation-safe by
//! drop: if the caller drops the returned future before completion,
//! the impl must abort any in-flight transport request or storage
//! read without corrupting shared state. Impls that cannot honour
//! this contract MUST document the deviation per method.

use async_trait::async_trait;
use lean_chain::engine::BlockImportResult;
use lean_wire::{BlocksByRootRequest, BlocksByRootResponse, Status};
use protocol::SignedBlock;
use tokio::sync::mpsc;
use types::Bytes32;

use lean_chain::ChainError;

use crate::error::SyncError;
use crate::peer_id::PeerId;

/// Narrow chain-facing surface required by the sync loop.
///
/// Implemented by [`crate::Service`] in this crate; consumers that mock
/// the chain in tests supply their own implementation.
#[async_trait]
pub trait Chain: Send + Sync + 'static {
    /// Returns the local node's current [`Status`] for the handshake.
    ///
    /// Backed by the eventually-consistent
    /// [`ChainSnapshot`](crate::ChainSnapshot); peer handshake tolerates
    /// a snapshot lag of one tick / one accepted import.
    async fn local_status(&self) -> Result<Status, ChainError>;

    /// Reports whether `root` is already known to local storage.
    async fn has_block(&self, root: Bytes32) -> Result<bool, ChainError>;

    /// Imports `block` through the engine.
    async fn import_block(&self, block: SignedBlock) -> Result<BlockImportResult, ChainError>;
}

/// Outbound peer RPC surface required by the sync loop.
///
/// Implemented by the `node`-level libp2p adapter.
#[async_trait]
pub trait Network: Send + Sync + 'static {
    /// Sends an outbound `Status` to `peer` and returns the peer's reply.
    ///
    /// # Errors
    /// Transport / decode failures surface as [`SyncError::Network`].
    async fn send_status(&self, peer: &PeerId, local_status: Status) -> Result<Status, SyncError>;

    /// Sends an outbound `BlocksByRoot` request to `peer`.
    ///
    /// # Errors
    /// Transport / decode failures surface as [`SyncError::Network`].
    async fn request_blocks_by_root(
        &self,
        peer: &PeerId,
        request: BlocksByRootRequest,
    ) -> Result<BlocksByRootResponse, SyncError>;
}

/// Peer-connect notification surface required by the sync loop.
///
/// [`Loop::start`](super::Loop::start) subscribes once. Closing the
/// returned receiver shuts the watch task down cleanly; subsequent
/// `Loop::status` calls report whether the task is still alive.
///
/// # Backpressure contract
///
/// The returned receiver is the watch loop's only intake. The loop
/// processes one event at a time and, per event, may block on a
/// `Semaphore` permit while up to `max_concurrent_peer_syncs` peer walks
/// are already in flight (a walk can run for as long as the configured
/// `request_timeout`). While the loop is saturated it does **not** call
/// `recv`, so a bounded sender's `send().await` will park — this is the
/// intended backpressure path and the reason the cap is safe against a
/// flap storm.
///
/// Implementations therefore MUST:
/// - use a **bounded** channel, so a misbehaving or flapping peer source
///   cannot grow an unbounded backlog of pending `PeerId`s in memory;
/// - apply backpressure to the event source (await `send`, or drop on a
///   full channel) rather than spawning unbounded producers — the loop
///   already dedups duplicate in-flight peers, so a dropped duplicate is
///   harmless;
/// - never block the runtime inside the producer while holding a slot
///   that the loop needs to make progress.
///
/// The bounded channel + the loop's permit cap + per-`PeerId` dedup
/// together bound both the queued-event memory and the concurrent-walk
/// memory regardless of peer-connect event rate.
#[async_trait]
pub trait PeerEventProvider: Send + Sync + 'static {
    /// Subscribes to outbound peer-connect events.
    ///
    /// Returns a [`mpsc::Receiver`] that MUST be backed by a bounded
    /// channel — see the trait-level "Backpressure contract".
    ///
    /// # Errors
    /// Subscription failures surface as [`SyncError::Subscription`].
    async fn subscribe_outbound_connected_peers(&self)
        -> Result<mpsc::Receiver<PeerId>, SyncError>;
}
