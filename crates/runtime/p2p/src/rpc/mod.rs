//! Req/resp protocol surface: `Status` handshake + `BlocksByRoot`
//! handler.
//!
//! Public surface:
//! - [`RpcProvider`] — trait the composing binary (`node`) implements
//!   to supply the local `Status` and look up blocks by tree root. The
//!   `runtime-p2p` crate avoids depending on `storage` directly by
//!   accepting an `Arc<dyn RpcProvider>` at construction.
//! - [`NoOpRpcProvider`] — default no-op provider wired by
//!   [`crate::DevnetHost::build`]. Lifecycle tests in this crate use
//!   it; operational deployments must wire a real implementation via
//!   [`crate::DevnetHost::build_with_provider`].
//! - [`RpcError`] — failure surface for outbound RPC calls
//!   ([`crate::Host::send_blocks_by_root`]).
//! - [`RpcRequest`] / [`RpcResponse`] — re-exports of the typed wire
//!   enums driven by the [`crate::host::behaviour::codec::SszSnappyCodec`].

pub(crate) mod blocks_by_root;
mod client;
pub(crate) mod outbound;
pub(crate) mod status;

use std::sync::Arc;

use networking::Status;
use protocol::SignedBlock;
use types::Bytes32;

pub use crate::host::behaviour::codec::{RpcRequest, RpcResponse};

/// Supplies the inputs required to answer inbound RPC requests:
/// - The local node's `Status` (for handshake exchange + validation).
/// - Block lookups by tree root (for `BlocksByRoot` responses).
///
/// Implemented in the composing binary (typically against a
/// `storage::Store` backed by LMDB or similar). The `runtime-p2p` crate
/// remains free of any direct dependency on `storage` by accepting
/// `Arc<dyn RpcProvider>` at construction.
pub trait RpcProvider: Send + Sync {
    /// Returns the block whose tree-root matches `root`, or `None` if
    /// the local store has no such block. The `BlocksByRoot` handler
    /// filters `None` lookups out of the response — matches the
    /// "unknown roots → empty entry" spec semantics.
    fn get_block_by_root(&self, root: &Bytes32) -> Option<SignedBlock>;

    /// Returns the local node's current `Status` — the value sent in
    /// outbound handshakes and validated against the peer's inbound
    /// `Status`.
    fn local_status(&self) -> Status;
}

/// No-op default provider. Returns `None` for every block lookup and
/// the zero `Status` (`finalized` and `head` checkpoints both default).
///
/// Wired by [`crate::DevnetHost::build`] so lifecycle tests can
/// construct a service without a real storage backend. Two peers backed
/// by this provider will handshake successfully (both `Status::default()`
/// values match), but every `BlocksByRoot` response will be empty.
///
/// Operational deployments must pass a real provider via
/// [`crate::DevnetHost::build_with_provider`].
#[derive(Debug, Default, Clone, Copy)]
pub struct NoOpRpcProvider;

impl RpcProvider for NoOpRpcProvider {
    fn get_block_by_root(&self, _root: &Bytes32) -> Option<SignedBlock> {
        None
    }

    fn local_status(&self) -> Status {
        Status::default()
    }
}

/// Convenience alias for the shared-provider shape stored on
/// [`crate::P2pService`]. Internal to this crate — public consumers
/// (notably the `node` adapter at the composition root) construct an
/// `Arc<dyn RpcProvider>` directly and pass it to
/// [`crate::DevnetHost::build_with_provider`].
pub(crate) type SharedRpcProvider = Arc<dyn RpcProvider>;

/// Failure surface for outbound RPC calls.
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    /// The host command channel is closed — the swarm-poll task has
    /// exited (typically `Service::stop` ran).
    #[error("host command channel closed")]
    ChannelClosed,
    /// libp2p surfaced an outbound failure (timeout, connection closed,
    /// or peer ungracefully terminated the substream).
    #[error("rpc outbound failure: {0}")]
    Outbound(String),
    /// The peer answered the request with a response variant that does
    /// not match the request kind (programming error in the peer's
    /// codec).
    #[error("rpc response variant did not match request kind (expected {expected})")]
    UnexpectedResponseKind {
        /// Static label of the expected response kind (`"status"`,
        /// `"blocks_by_root"`).
        expected: &'static str,
    },
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn no_op_provider_returns_default_status() {
        let provider = NoOpRpcProvider;
        assert_eq!(provider.local_status(), Status::default());
    }

    #[test]
    fn no_op_provider_returns_none_for_every_root() {
        let provider = NoOpRpcProvider;
        assert!(provider.get_block_by_root(&Bytes32::zero()).is_none());
        assert!(provider
            .get_block_by_root(&Bytes32::new([0xAB; 32]))
            .is_none());
    }
}
