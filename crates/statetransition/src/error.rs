//! Crate-level error type for the consensus state machine.

use thiserror::Error;

/// Errors raised by the consensus state-transition functions.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum StateTransitionError {
    /// `process_slots` was called with `target_slot <= state.slot`.
    #[error("target slot {target} must be greater than current slot {current}")]
    TargetSlotNotInFuture {
        /// `state.slot` at call time.
        current: u64,
        /// Requested `target_slot`.
        target: u64,
    },

    /// Slot arithmetic overflowed `u64`.
    #[error("slot arithmetic overflow at slot {slot}")]
    SlotOverflow {
        /// Slot value that caused the overflow.
        slot: u64,
    },
}
