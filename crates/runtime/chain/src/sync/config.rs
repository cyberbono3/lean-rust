//! Sync [`Loop`](super::Loop) tunables.

use super::error::SyncError;

/// Default per-peer-connect walk-back-and-fetch depth. Mirrors gean's
/// budget so cross-client devnet0 runs converge in comparable time.
pub const DEFAULT_MAX_SYNC_DEPTH: usize = 64;

/// Validated configuration for the sync [`Loop`](super::Loop).
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Caps the per-peer-connect walk-back-and-fetch depth. Hitting the
    /// cap leaves the deepest blocks orphaned with an unknown parent;
    /// they are resolved on a future peer-connect or via gossip.
    pub max_sync_depth: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_sync_depth: DEFAULT_MAX_SYNC_DEPTH,
        }
    }
}

impl Config {
    /// Reports whether the configuration is internally consistent.
    ///
    /// # Errors
    /// [`SyncError::InvalidMaxSyncDepth`] when `max_sync_depth == 0`.
    pub fn validate(&self) -> Result<(), SyncError> {
        if self.max_sync_depth == 0 {
            return Err(SyncError::InvalidMaxSyncDepth);
        }
        Ok(())
    }
}
