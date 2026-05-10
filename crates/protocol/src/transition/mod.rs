//! State-transition methods on [`crate::state::State`].
//!
//! Each submodule mirrors a consensus-spec entry point and attaches the
//! method as `&mut self` on `State` so call sites read as
//! `state.process_block_header(&block)?`.
//!
//! - [`slots`] — per-slot housekeeping ([`State::process_slot`],
//!   [`State::process_slots`]).
//! - [`blocks`] — header validation + commitment ([`State::process_block_header`]).
//! - [`attestations`] — vote tally + justification/finalization
//!   ([`State::process_attestations`]).
//!
//! [`StateTransitionError`] is the single error type returned by all
//! methods in this module tree.

use thiserror::Error;
use types::Bytes32;

use crate::error::ProtocolError;
use crate::slot::Slot;
use crate::validator::ValidatorIndex;

mod attestations;
mod blocks;
mod justifications;
mod slots;

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

    /// Forwarded from [`crate`] domain helpers (e.g. zero-validator
    /// `is_proposer` lookups).
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
}
