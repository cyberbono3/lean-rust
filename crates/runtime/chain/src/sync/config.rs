//! Sync [`Loop`](super::Loop) tunables.

use core::num::NonZeroUsize;

use super::error::SyncError;

/// Builds a [`NonZeroUsize`] from a literal at compile time; panics if
/// the input is zero. Used to define non-zero associated constants
/// without the unstable `NonZeroUsize::unwrap` (stable in const since
/// 1.83; MSRV here is 1.80).
const fn nz(n: usize) -> NonZeroUsize {
    match NonZeroUsize::new(n) {
        Some(v) => v,
        None => panic!("expected non-zero constant"),
    }
}

/// Type-validated configuration for the sync [`Loop`](super::Loop).
///
/// Marked `#[non_exhaustive]` so additional fields can be introduced
/// without breaking downstream callers; construct via [`Config::new`],
/// [`Config::default`], or [`Config::try_from`], then customize with
/// [`Config::with_max_sync_depth`].
///
/// # Examples
/// ```
/// use core::num::NonZeroUsize;
/// use runtime_chain::SyncConfig;
///
/// let cfg = SyncConfig::try_from(32usize).unwrap();
/// assert_eq!(cfg.max_sync_depth.get(), 32);
///
/// let deeper = SyncConfig::default()
///     .with_max_sync_depth(NonZeroUsize::new(128).unwrap());
/// assert_eq!(deeper.max_sync_depth.get(), 128);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Config {
    /// Caps the per-peer-connect walk-back-and-fetch depth. Hitting the
    /// cap leaves the deepest blocks orphaned with an unknown parent;
    /// they are resolved on a future peer-connect or via gossip.
    pub max_sync_depth: NonZeroUsize,
}

impl Config {
    /// Default per-peer-connect walk-back-and-fetch depth. Mirrors the
    /// upstream reference client's budget so cross-client devnet runs
    /// converge in comparable time.
    pub const DEFAULT_MAX_SYNC_DEPTH: NonZeroUsize = nz(64);

    /// Builds a configuration from a non-zero depth.
    #[must_use]
    pub const fn new(max_sync_depth: NonZeroUsize) -> Self {
        Self { max_sync_depth }
    }

    /// Returns a copy with `max_sync_depth` overridden. Enables per-field
    /// customization on a `#[non_exhaustive]` struct.
    #[must_use]
    pub const fn with_max_sync_depth(mut self, max_sync_depth: NonZeroUsize) -> Self {
        self.max_sync_depth = max_sync_depth;
        self
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MAX_SYNC_DEPTH)
    }
}

impl From<NonZeroUsize> for Config {
    fn from(max_sync_depth: NonZeroUsize) -> Self {
        Self::new(max_sync_depth)
    }
}

impl TryFrom<usize> for Config {
    type Error = SyncError;

    /// Builds a configuration from a raw depth, rejecting zero.
    ///
    /// # Errors
    /// [`SyncError::InvalidMaxSyncDepth`] when `max_sync_depth == 0`.
    fn try_from(max_sync_depth: usize) -> Result<Self, Self::Error> {
        NonZeroUsize::new(max_sync_depth)
            .map(Self::new)
            .ok_or(SyncError::InvalidMaxSyncDepth)
    }
}
