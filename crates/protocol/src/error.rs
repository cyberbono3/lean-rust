//! Crate-level error types for the consensus protocol domain.
//!
//! - [`ProtocolError`] forwards SSZ codec failures from the [`ssz`] facade
//!   and surfaces invariant breaks (e.g. zero-validator proposer lookups)
//!   without panicking.
//! - [`StateTransitionError`] is the single error type returned by every
//!   state-transition method on [`crate::state::State`]
//!   (`process_slot`/`process_slots`, `process_block_header`,
//!   `process_attestations`). [`AttSlotKind`] tags which side of an
//!   attestation produced an out-of-range slot.

use ssz::SszError;
use thiserror::Error;
use types::Bytes32;

use crate::slot::Slot;
use crate::validator::ValidatorIndex;

/// Errors raised by [`crate`] domain operations.
#[derive(Debug, Error, Clone, PartialEq)]
#[non_exhaustive]
pub enum ProtocolError {
    /// SSZ encode/decode of a domain type failed.
    #[error(transparent)]
    Ssz(#[from] SszError),

    /// A domain invariant was violated (e.g. proposer lookup with zero
    /// validators, slot arithmetic overflow).
    #[error("invariant violation in {context}: {reason}")]
    Invariant {
        /// Static label identifying the call site.
        context: &'static str,
        /// Human-readable reason.
        reason: &'static str,
    },
}

/// Identifies which side of an attestation produced an out-of-range slot
/// in [`StateTransitionError::AttestationSlotOutOfRange`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttSlotKind {
    /// `vote.source.slot` was beyond the bound named by `len`.
    Source,
    /// `vote.target.slot` was beyond the bound named by `len`.
    Target,
}

/// Errors raised by the [`crate::state::State`] state-transition methods.
#[derive(Debug, Error, PartialEq)]
#[non_exhaustive]
pub enum StateTransitionError {
    /// `process_slots` was called with `target_slot <= state.slot`.
    #[error("target slot {target} must be greater than current slot {current}")]
    TargetSlotNotInFuture {
        /// `state.slot` at call time.
        current: Slot,
        /// Requested `target_slot`.
        target: Slot,
    },

    /// Slot arithmetic overflowed `u64`.
    #[error("slot arithmetic overflow at slot {slot}")]
    SlotOverflow {
        /// Slot value that caused the overflow.
        slot: Slot,
    },

    /// `block.slot` did not match `state.slot` at `process_block_header`.
    #[error("block slot {got} does not match state slot {want}")]
    BlockSlotMismatch {
        /// `block.slot`.
        got: Slot,
        /// `state.slot`.
        want: Slot,
    },

    /// `block.slot` was not strictly greater than `state.latest_block_header.slot`.
    #[error("block slot {slot} is not after latest header slot {latest}")]
    BlockOlderThanLatest {
        /// `block.slot`.
        slot: Slot,
        /// `state.latest_block_header.slot`.
        latest: Slot,
    },

    /// `block.proposer_index` is not the round-robin proposer for `state.slot`.
    #[error("validator {proposer} is not the proposer for slot {slot}")]
    IncorrectBlockProposer {
        /// `state.slot` at call time.
        slot: Slot,
        /// `block.proposer_index`.
        proposer: ValidatorIndex,
    },

    /// `block.parent_root` did not match `hash_tree_root(state.latest_block_header)`.
    #[error("block parent root mismatch at slot {slot}")]
    BlockParentRootMismatch {
        /// `block.slot`.
        slot: Slot,
        /// `block.parent_root`.
        got: Bytes32,
        /// `hash_tree_root(state.latest_block_header)`.
        want: Bytes32,
    },

    /// An attestation referenced a slot beyond the live state arrays bound.
    #[error("attestation {kind:?} slot {slot} out of range (len {len})")]
    AttestationSlotOutOfRange {
        /// Source vs target side of the attestation.
        kind: AttSlotKind,
        /// The offending slot.
        slot: Slot,
        /// The bound the slot was checked against.
        len: usize,
    },

    /// An attestation's `validator_id` is `>= state.config.num_validators`.
    #[error("attestation validator {validator} >= num_validators {num_validators}")]
    AttestationValidatorOutOfRange {
        /// The offending validator index.
        validator: ValidatorIndex,
        /// `state.config.num_validators`.
        num_validators: u64,
    },

    /// A bounded list or bitlist on `State` would exceed its compile-time cap.
    #[error("state bound exceeded: {context}")]
    StateBoundExceeded {
        /// Static label identifying which field tripped the bound.
        context: &'static str,
    },

    /// `State::state_transition` was called with `validate_state_root = true`
    /// and the post-state hash-tree-root did not match
    /// `signed_block.message.state_root`.
    #[error("post-state root mismatch at slot {slot}: got {got:?}, want {want:?}")]
    StateRootMismatch {
        /// `signed_block.message.slot` at call time.
        slot: Slot,
        /// `state.hash_tree_root()` after the transition committed.
        got: Bytes32,
        /// `signed_block.message.state_root` declared by the proposer.
        want: Bytes32,
    },

    /// Forwarded from [`crate`] domain helpers (e.g. zero-validator
    /// `is_proposer` lookups).
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
}
