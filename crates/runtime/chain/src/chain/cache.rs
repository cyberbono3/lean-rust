//! `ChainSnapshot` — hot-read snapshot of engine state.
//!
//! Refreshed after each `Accepted` import and each tick. Non-writer
//! services (`runtime/api`, `runtime/p2p`) read through a shared
//! `Arc<RwLock<ChainSnapshot>>` clone instead of contending on the
//! engine mutex.
//!
//! The snapshot is *eventually consistent* with the engine: it reflects
//! the state observed at the most recent refresh, not the live state.
//! Use [`Engine`] accessors directly when strong consistency matters.

use engine::Engine;
use protocol::Checkpoint;
use types::Bytes32;

/// Cached projection of forkchoice state. All fields are `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChainSnapshot {
    /// Canonical head root.
    pub head_root: Bytes32,
    /// Safe attestation target root.
    pub safe_target_root: Bytes32,
    /// Forkchoice clock — slot index.
    pub current_slot: u64,
    /// Latest finalized checkpoint.
    pub latest_finalized: Checkpoint,
}

impl ChainSnapshot {
    /// Captures a fresh snapshot under a single engine-mutex acquisition.
    #[must_use]
    pub(super) fn from_engine(engine: &Engine) -> Self {
        let (head_root, safe_target_root, current_slot, latest_finalized) =
            engine.with_store(|s| {
                (
                    s.head(),
                    s.safe_target(),
                    s.current_slot(),
                    s.latest_finalized(),
                )
            });
        Self {
            head_root,
            safe_target_root,
            current_slot,
            latest_finalized,
        }
    }
}
