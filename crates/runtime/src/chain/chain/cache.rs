//! `ChainSnapshot` — by-value projection of engine state.
//!
//! Captured on demand under one engine-lock acquisition and returned by
//! value (all fields are `Copy`). Non-writer callers (`register_chain_gauges`,
//! `Service::local_status`) read through `Service::snapshot` instead of
//! contending on a derived cache.
//!
//! Each capture is a consistent single-lock read of live engine state.

use crate::chain::engine::Engine;
use protocol::Checkpoint;
use types::Bytes32;

/// By-value projection of forkchoice state. All fields are `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChainSnapshot {
    /// Canonical head root.
    pub head_root: Bytes32,
    /// Forkchoice clock — slot index.
    pub current_slot: u64,
    /// Latest justified checkpoint. Consumed by
    /// `node::devnet::register_chain_gauges` (the `lean_chain_justified_slot`
    /// gauge); no in-crate reader, hence the cross-crate note.
    pub latest_justified: Checkpoint,
    /// Latest finalized checkpoint.
    pub latest_finalized: Checkpoint,
}

impl ChainSnapshot {
    /// Captures a fresh snapshot under a single engine-mutex acquisition.
    #[must_use]
    pub(super) fn from_engine(engine: &Engine) -> Self {
        let (head_root, current_slot, latest_justified, latest_finalized) =
            engine.with_store(|s| {
                (
                    s.head(),
                    s.current_slot(),
                    s.latest_justified(),
                    s.latest_finalized(),
                )
            });
        Self {
            head_root,
            current_slot,
            latest_justified,
            latest_finalized,
        }
    }
}
