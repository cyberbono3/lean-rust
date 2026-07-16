//! Crate-level error type for the forkchoice store.
//!
//! [`ForkchoiceError`] is marked `#[non_exhaustive]` so adding variants is
//! a minor-version change. Variants carry typed `Slot` / `Bytes32` payloads
//! so callers can pattern-match without parsing the `Display` string.

use thiserror::Error;
use types::Bytes32;

use protocol::{ProtocolError, Slot, StateTransitionError, ValidatorIndex};

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

    /// An attestation's `validator_id` is `>= config.num_validators`. The
    /// vote pool is keyed by validator id; without this gate a peer could
    /// forge arbitrary `u64` ids and grow the pool without bound (~4 KiB
    /// per entry — 250K forged ids ≈ 1 GiB).
    #[error("forkchoice attestation validator id {validator_id} out of range (num_validators={num_validators})")]
    ValidatorIndexOutOfRange {
        /// `signed_attestation.message.validator_id` declared by the attester.
        validator_id: u64,
        /// `Store::config.num_validators` at call time.
        num_validators: u64,
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

    /// `Store::produce_block` was called with a `validator` that is not the
    /// round-robin proposer for `slot`.
    #[error("forkchoice unauthorized proposer: validator {validator} for slot {slot}")]
    UnauthorizedProposer {
        /// `validator` argument passed to `produce_block`.
        validator: ValidatorIndex,
        /// `slot` argument passed to `produce_block`.
        slot: Slot,
    },

    /// `Store::produce_block` resolved a head root whose post-state is not
    /// tracked by the store.
    #[error("forkchoice head state not found at {root:?}")]
    HeadStateNotFound {
        /// Head block root whose post-state was missing.
        root: Bytes32,
    },

    /// `Store::produce_attestation_vote` or `Store::get_vote_target`
    /// resolved a head root whose block is not tracked.
    #[error("forkchoice unknown head block at {root:?}")]
    UnknownHeadBlock {
        /// Missing head root.
        root: Bytes32,
    },

    /// `Store::get_vote_target` resolved a `safe_target` root whose block
    /// is not tracked.
    #[error("forkchoice unknown safe target at {root:?}")]
    UnknownSafeTarget {
        /// Missing safe-target root.
        root: Bytes32,
    },

    /// `Store::track_block` rejected a block whose declared `state_root`
    /// disagrees with `hash_tree_root(post_state)`.
    #[error("forkchoice block state root mismatch: got {got:?}, want {want:?}")]
    BlockStateRootMismatch {
        /// `block.state_root` declared by the producer.
        got: Bytes32,
        /// `post_state.hash_tree_root()` computed at call time.
        want: Bytes32,
    },

    /// `advance_state_to_slot` was called with `state.slot > target`.
    #[error("forkchoice state slot {state_slot} is ahead of target {target_slot}")]
    StateSlotAheadOfTarget {
        /// `state.slot` at call time.
        state_slot: Slot,
        /// Requested target slot.
        target_slot: Slot,
    },

    /// State-transition machinery returned an error (forwarded verbatim).
    #[error(transparent)]
    StateTransition(#[from] StateTransitionError),

    /// Protocol-domain helper (e.g. `is_proposer`) returned an error.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
}
