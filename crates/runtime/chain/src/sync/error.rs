//! Error type and opaque [`PeerId`] newtype for the sync module.

use thiserror::Error;

use crate::chain::ChainError;

/// Opaque peer identifier.
///
/// Held as a `String` so this crate never compiles against the libp2p
/// crate. Adapters in `node` construct values via base-58 encodings of
/// the underlying transport's peer id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerId(String);

impl PeerId {
    /// Wraps a raw identifier (typically a base-58-encoded libp2p peer id).
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// Returns the underlying string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for PeerId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Failures raised by the sync module.
///
/// Per-block import errors during walk-back are *not* part of this enum —
/// they are warn-logged and dropped (parity with `lean-go/runtime/sync`),
/// since an unknown parent at the deepest layer is the expected outcome
/// when `MaxSyncDepth` is hit before the walk meets a known block.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SyncError {
    /// [`Config::max_sync_depth`](super::Config::max_sync_depth) was zero.
    #[error("sync max depth must be positive")]
    InvalidMaxSyncDepth,

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
}
