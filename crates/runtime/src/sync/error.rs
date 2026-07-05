//! Error type for the sync module.

use thiserror::Error;

use crate::chain::ChainError;

/// Failures raised by the sync module.
///
/// Per-block import errors during walk-back are *not* part of this enum —
/// they are warn-logged and dropped (parity with the upstream reference
/// implementation), since an unknown parent at the deepest layer is the
/// expected outcome when `MaxSyncDepth` is hit before the walk meets a
/// known block.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SyncError {
    /// [`Config::max_sync_depth`](super::Config::max_sync_depth) was zero.
    #[error("sync max depth must be positive")]
    InvalidMaxSyncDepth,

    /// [`PeerId::new`](super::PeerId::new) was called with an empty raw identifier.
    #[error("peer id must not be empty")]
    EmptyPeerId,

    /// A chain-port call failed.
    #[error("chain: {0}")]
    Chain(#[from] ChainError),

    /// A network-port call failed. Transport-opaque on purpose — the
    /// concrete cause is logged by the adapter at the source.
    #[error("network: {0}")]
    Network(String),

    /// Subscribing to peer-connect events failed.
    #[error("subscription: {0}")]
    Subscription(String),

    /// [`super::Loop::start`] was called twice without an intervening stop.
    #[error("sync loop already started")]
    AlreadyStarted,

    /// An operation requires `start` first.
    #[error("sync loop not started")]
    NotStarted,

    /// The watch task exited before [`super::Loop::stop`] was called —
    /// indicates a panic or an unhandled internal error.
    #[error("sync watch task exited prematurely")]
    WatchExited,
}
