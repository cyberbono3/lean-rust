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

// New variants for #18 carry typed `Slot` / `Bytes32` payloads so callers can
// pattern-match without parsing the `Display` string.

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

    /// An attestation referenced a `source` checkpoint whose `root` is not
    /// tracked by the store.
    #[error("forkchoice unknown source block at {root:?}")]
    UnknownSourceBlock {
        /// `vote.source.root` declared by the attester.
        root: Bytes32,
    },

    /// An attestation referenced a `target` checkpoint whose `root` is not
    /// tracked by the store.
    #[error("forkchoice unknown target block at {root:?}")]
    UnknownTargetBlock {
        /// `vote.target.root` declared by the attester.
        root: Bytes32,
    },

    /// An attestation has `source.slot > target.slot` or the resolved
    /// source block's slot exceeds the target block's slot.
    #[error("forkchoice attestation source slot exceeds target")]
    SourceSlotExceedsTarget,

    /// `vote.source.slot` disagrees with the resolved source block's slot.
    #[error("forkchoice source checkpoint slot mismatches anchor block slot")]
    SourceCheckpointSlotMismatch,

    /// `vote.target.slot` disagrees with the resolved target block's slot.
    #[error("forkchoice target checkpoint slot mismatches anchor block slot")]
    TargetCheckpointSlotMismatch,

    /// `vote.slot` is more than one slot ahead of the store's
    /// `current_vote_slot()`.
    #[error("forkchoice attestation slot {vote_slot} > limit {limit} (current_vote_slot + 1)")]
    AttestationTooFarInFuture {
        /// `vote.slot` declared by the attester.
        vote_slot: Slot,
        /// `current_vote_slot + 1`, the inclusive upper bound.
        limit: Slot,
    },

    /// `current_vote_slot + 1` would overflow `u64`.
    #[error("forkchoice attestation future-limit overflow at current_slot {current_slot}")]
    AttestationFutureLimitOverflow {
        /// `current_vote_slot()` at call time.
        current_slot: Slot,
    },

    /// LMD-GHOST descent was asked to start from a non-zero root that is
    /// not present in the supplied block map.
    #[error("forkchoice GHOST traversal: unknown root block at {root:?}")]
    UnknownRootBlock {
        /// Root requested as the descent origin.
        root: Bytes32,
    },

    /// LMD-GHOST vote-weight accumulation walked past a block whose
    /// `parent_root` is absent from the supplied block map.
    #[error("forkchoice GHOST traversal: parent block not found at {root:?}")]
    ParentBlockNotFound {
        /// Missing parent root encountered during the walk.
        root: Bytes32,
    },

    /// LMD-GHOST descent was invoked with a zero root and an empty block
    /// map — no candidate origin can be chosen.
    #[error("forkchoice GHOST traversal over empty block set")]
    NoBlocksAvailable,
}
