//! Sync [`Loop`](crate::Loop) tunables.

use core::num::NonZeroUsize;
use core::time::Duration;

use crate::error::SyncError;

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

/// Type-validated configuration for the sync [`Loop`](crate::Loop).
///
/// Marked `#[non_exhaustive]` so additional fields can be introduced
/// without breaking downstream callers; construct via [`Config::new`],
/// [`Config::default`], or [`Config::try_from`], then customize with
/// [`Config::with_max_sync_depth`].
///
/// # Examples
/// ```
/// use core::num::NonZeroUsize;
/// use lean_sync::Config;
///
/// let cfg = Config::try_from(32usize).unwrap();
/// assert_eq!(cfg.max_sync_depth.get(), 32);
///
/// let deeper = Config::default()
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
    /// Caps the number of peer-walk tasks running concurrently. Without
    /// this bound a flap-storming or buggy peer-event source spawns one
    /// walk per event, each holding a `Vec::with_capacity(max_sync_depth)`
    /// of `SignedBlock` plus port `Arc` clones, all serializing through
    /// the engine mutex — a memory/contention amplifier. The watch loop
    /// acquires a permit before spawning, so excess events backpressure
    /// instead of fanning out.
    pub max_concurrent_peer_syncs: NonZeroUsize,
    /// Per-request budget for a single `BlocksByRoot` RPC during a walk.
    /// A peer that accepts the substream but never answers would
    /// otherwise keep the walk task — and its port `Arc` clones — alive
    /// indefinitely; the timeout aborts that one walk without affecting
    /// other peers. This is a local scheduling bound only — the
    /// `BlocksByRoot` wire protocol is unchanged.
    pub request_timeout: Duration,
}

impl Config {
    /// Default per-peer-connect walk-back-and-fetch depth. Mirrors the
    /// upstream reference client's budget so cross-client devnet runs
    /// converge in comparable time.
    pub const DEFAULT_MAX_SYNC_DEPTH: NonZeroUsize = nz(64);

    /// Default cap on concurrently running peer-walk tasks. Small: devnet
    /// runs a handful of peers, and each walk is engine-mutex-bound, so a
    /// low cap bounds contention without slowing realistic convergence.
    pub const DEFAULT_MAX_CONCURRENT_PEER_SYNCS: NonZeroUsize = nz(4);

    /// Default per-request `BlocksByRoot` timeout. Generous: a healthy
    /// peer answers a single-root request in well under a second, so 10 s
    /// only fires on a genuinely stuck substream.
    pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

    /// Builds a configuration from a non-zero depth, defaulting the other
    /// fields.
    #[must_use]
    pub const fn new(max_sync_depth: NonZeroUsize) -> Self {
        Self {
            max_sync_depth,
            max_concurrent_peer_syncs: Self::DEFAULT_MAX_CONCURRENT_PEER_SYNCS,
            request_timeout: Self::DEFAULT_REQUEST_TIMEOUT,
        }
    }

    /// Returns a copy with `max_sync_depth` overridden. Enables per-field
    /// customization on a `#[non_exhaustive]` struct.
    #[must_use]
    pub const fn with_max_sync_depth(mut self, max_sync_depth: NonZeroUsize) -> Self {
        self.max_sync_depth = max_sync_depth;
        self
    }

    /// Returns a copy with `max_concurrent_peer_syncs` overridden.
    #[must_use]
    pub const fn with_max_concurrent_peer_syncs(
        mut self,
        max_concurrent_peer_syncs: NonZeroUsize,
    ) -> Self {
        self.max_concurrent_peer_syncs = max_concurrent_peer_syncs;
        self
    }

    /// Returns a copy with `request_timeout` overridden.
    #[must_use]
    pub const fn with_request_timeout(mut self, request_timeout: Duration) -> Self {
        self.request_timeout = request_timeout;
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn default_seeds_new_field_constants() {
        assert_eq!(Config::DEFAULT_MAX_CONCURRENT_PEER_SYNCS.get(), 4);
        assert_eq!(Config::DEFAULT_REQUEST_TIMEOUT, Duration::from_secs(10));

        let cfg = Config::default();
        assert_eq!(
            cfg.max_concurrent_peer_syncs,
            Config::DEFAULT_MAX_CONCURRENT_PEER_SYNCS
        );
        assert_eq!(cfg.request_timeout, Config::DEFAULT_REQUEST_TIMEOUT);
    }

    #[test]
    fn builders_override_only_their_field() {
        let cap = NonZeroUsize::new(16).unwrap();
        let timeout = Duration::from_millis(250);

        let cfg = Config::default()
            .with_max_concurrent_peer_syncs(cap)
            .with_request_timeout(timeout);

        assert_eq!(cfg.max_concurrent_peer_syncs, cap);
        assert_eq!(cfg.request_timeout, timeout);
        // The unrelated field is left at its default.
        assert_eq!(cfg.max_sync_depth, Config::DEFAULT_MAX_SYNC_DEPTH);
    }
}
