//! `ChainSnapshot` — hot-read snapshot of engine state.
//!
//! Refreshed after each `Accepted` import and each tick. Non-writer
//! services (`runtime/api`, `runtime/p2p`) read through a shared
//! `Arc<RwLock<ChainSnapshot>>` clone instead of contending on the
//! engine mutex.

use engine::Engine;
use protocol::Checkpoint;
use types::Bytes32;

/// Cached projection of forkchoice state. All fields are `Copy`.
#[derive(Debug, Clone, Copy, Default)]
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
    /// Reads the four fields under a single engine-mutex acquisition.
    pub(super) fn refresh(&mut self, engine: &Engine) {
        engine.with_store(|s| {
            self.head_root = s.head();
            self.safe_target_root = s.safe_target();
            self.current_slot = s.current_slot();
            self.latest_finalized = s.latest_finalized();
        });
    }
}
