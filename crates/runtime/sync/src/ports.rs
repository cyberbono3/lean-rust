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
use engine::BlockImportResult;
use networking::{BlocksByRootRequest, BlocksByRootResponse, Status};
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
#[async_trait]
pub trait PeerEventProvider: Send + Sync + 'static {
    /// Subscribes to outbound peer-connect events.
    ///
    /// # Errors
    /// Subscription failures surface as [`SyncError::Subscription`].
    async fn subscribe_outbound_connected_peers(&self)
        -> Result<mpsc::Receiver<PeerId>, SyncError>;
}
