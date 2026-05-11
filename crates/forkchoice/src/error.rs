//! Crate-level error type for the forkchoice store.
//!
//! [`ForkchoiceError`] is intentionally non-exhaustive: this revision only
//! carries the two variants emitted by [`crate::store::Store::from_anchor`].
//! Subsequent forkchoice issues add variants for block insertion, attestation
//! validation, and head-resolution failure modes.

use thiserror::Error;
use types::Bytes32;

use protocol::Slot;

use crate::time::Time;

/// Errors raised by [`crate::store::Store`] operations.
#[derive(Debug, Error, PartialEq)]
#[non_exhaustive]
pub enum ForkchoiceError {
    /// `Store::from_anchor` was called with an anchor block whose
    /// `state_root` does not match `hash_tree_root(state)`.
    #[error("forkchoice anchor block state root mismatch: got {got:?}, want {want:?}")]
    AnchorStateRootMismatch {
        /// `anchor_block.state_root` declared by the caller.
        got: Bytes32,
        /// `state.hash_tree_root()` computed at call time.
        want: Bytes32,
    },

    /// `Store::from_anchor` was called with an anchor whose slot multiplied
    /// by `INTERVALS_PER_SLOT` overflows `u64`.
    #[error(
        "forkchoice anchor time overflow at slot {slot} (intervals_per_slot={intervals_per_slot})"
    )]
    AnchorTimeOverflow {
        /// `anchor_block.slot` at call time.
        slot: Slot,
        /// The intervals-per-slot constant (4 on devnet0).
        intervals_per_slot: u64,
    },

    /// `Store::tick_interval` was called when `time + 1` would overflow
    /// the raw `u64` underlying [`Time`].
    #[error("forkchoice time overflow at time {time}")]
    TimeOverflow {
        /// `self.time()` at call time.
        time: Time,
    },
}
