//! Concrete RPC provider for the req/resp handlers.
//!
//! Folded in from the former `p2p-rpc` crate (the `RpcProvider` trait +
//! its `NoOpRpcProvider` default). Collapsing that single-impl port
//! leaves one concrete type with two behaviours:
//!
//! - [`RpcProvider::Chain`] — production wiring: the local `Status`
//!   comes from the chain service, `BlocksByRoot` lookups from storage.
//! - [`RpcProvider::NoOp`] — storage/chain-free default used by
//!   lifecycle tests. Returns `None` for every block lookup and the zero
//!   `Status`; two `NoOp` peers still handshake successfully (both report
//!   `Status::default()`).
//!
//! [`RpcError`] is the failure surface returned by outbound RPC calls.

use std::sync::Arc;

use lean_chain::Service as ChainService;
use lean_wire::Status;
use protocol::SignedBlock;
use storage::Store;
use tracing::warn;
use types::Bytes32;

/// Supplies the inputs the req/resp handlers need: the local node's
/// `Status` (handshake exchange + validation) and block lookups by tree
/// root (`BlocksByRoot` responses).
///
/// Concrete two-variant enum — the former `RpcProvider` port trait
/// collapsed to its single production impl plus the no-op default.
pub enum RpcProvider {
    /// No-op default. Returns `None` for every block lookup and the zero
    /// `Status`. Wired by [`crate::DevnetHost::build`] so lifecycle tests
    /// can construct a service without a real storage/chain backend.
    NoOp,
    /// Production provider: `local_status` from the chain snapshot,
    /// `get_block_by_root` from storage.
    Chain {
        /// Chain service supplying the live local `Status`.
        chain: Arc<ChainService>,
        /// Block store answering `BlocksByRoot` lookups.
        store: Arc<dyn Store>,
    },
}

impl RpcProvider {
    /// Builds the production provider over the chain service and block
    /// store.
    #[must_use]
    pub fn chain(chain: Arc<ChainService>, store: Arc<dyn Store>) -> Self {
        Self::Chain { chain, store }
    }

    /// Returns the block whose tree-root matches `root`, or `None` if the
    /// local store has no such block. The `BlocksByRoot` handler filters
    /// `None` lookups out of the response — matching the "unknown roots →
    /// empty entry" spec semantics.
    #[must_use]
    pub fn get_block_by_root(&self, root: &Bytes32) -> Option<SignedBlock> {
        match self {
            Self::NoOp => None,
            Self::Chain { store, .. } => match store.load_block(root) {
                Ok(block) => block,
                Err(err) => {
                    warn!(?root, %err, "p2p rpc block lookup failed");
                    None
                }
            },
        }
    }

    /// Returns the local node's current `Status` — the value sent in
    /// outbound handshakes and validated against the peer's inbound
    /// `Status`.
    #[must_use]
    pub fn local_status(&self) -> Status {
        match self {
            Self::NoOp => Status::default(),
            Self::Chain { chain, .. } => chain.local_status(),
        }
    }
}

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
        assert_eq!(RpcProvider::NoOp.local_status(), Status::default());
    }

    #[test]
    fn no_op_provider_returns_none_for_every_root() {
        assert!(RpcProvider::NoOp
            .get_block_by_root(&Bytes32::zero())
            .is_none());
        assert!(RpcProvider::NoOp
            .get_block_by_root(&Bytes32::new([0xAB; 32]))
            .is_none());
    }
}
