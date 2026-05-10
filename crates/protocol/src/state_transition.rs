//! Per-slot state-transition methods on [`State`].
//!
//! Exposes [`State::process_slot`] (per-slot housekeeping) and
//! [`State::process_slots`] (multi-slot advancement). Mirrors the consensus
//! spec functions of the same names and lives on `State` as inherent methods
//! so call sites read as `state.process_slot()`.

use ssz::HashTreeRoot;
use thiserror::Error;
use types::Bytes32;

use crate::slot::Slot;
use crate::state::State;

/// Errors raised by [`State::process_slots`] / [`State::process_slot`].
#[derive(Debug, Error, PartialEq, Eq)]
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
}

/// Returns `slot + 1` or [`StateTransitionError::SlotOverflow`].
fn advance_slot(slot: Slot) -> Result<Slot, StateTransitionError> {
    slot.get()
        .checked_add(Slot::ONE.get())
        .map(Slot::new)
        .ok_or(StateTransitionError::SlotOverflow { slot })
}

impl State {
    /// Caches the pre-block state root into `latest_block_header` when block
    /// processing left the header's `state_root` as the all-zero sentinel.
    /// On any other input — including when no block has been applied since
    /// the previous slot — the state is left unchanged.
    pub fn process_slot(&mut self) {
        if self.latest_block_header.state_root != Bytes32::zero() {
            return;
        }
        self.latest_block_header.state_root = self.hash_tree_root().into();
    }

    /// Advances `self` slot-by-slot up to (but not past) `target_slot`.
    ///
    /// Each iteration runs [`State::process_slot`] then increments
    /// `self.slot` by one.
    ///
    /// # Errors
    /// - [`StateTransitionError::TargetSlotNotInFuture`] when
    ///   `target_slot <= self.slot`.
    /// - [`StateTransitionError::SlotOverflow`] when slot arithmetic would
    ///   exceed `u64::MAX`. Cannot fire once the future-target check
    ///   passes, but surfaced explicitly to keep the loop `unwrap`-free.
    pub fn process_slots(&mut self, target_slot: Slot) -> Result<(), StateTransitionError> {
        if target_slot <= self.slot {
            return Err(StateTransitionError::TargetSlotNotInFuture {
                current: self.slot,
                target: target_slot,
            });
        }
        let steps = target_slot.get() - self.slot.get();
        for _ in 0..steps {
            self.process_slot();
            self.slot = advance_slot(self.slot)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    use crate::block::{BlockBody, BlockHeader};

    /// Minimal fixture: a non-default `State` whose `latest_block_header`
    /// commits to the empty `BlockBody`. Mirrors the slot-0 shape used by
    /// `statetransition::genesis_state` without crossing the crate boundary.
    fn fresh_state() -> State {
        State {
            latest_block_header: BlockHeader {
                body_root: BlockBody::default().hash_tree_root().into(),
                ..BlockHeader::default()
            },
            ..State::default()
        }
    }

    // -- advance_slot --------------------------------------------------------

    #[test]
    fn advance_slot_increments() {
        assert_eq!(advance_slot(Slot::ZERO).unwrap(), Slot::ONE);
        assert_eq!(advance_slot(Slot::new(41)).unwrap(), Slot::new(42));
    }

    #[test]
    fn advance_slot_rejects_overflow() {
        let err = advance_slot(Slot::new(u64::MAX)).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::SlotOverflow {
                slot: Slot::new(u64::MAX),
            }
        );
    }

    // -- process_slot --------------------------------------------------------

    #[test]
    fn process_slot_caches_previous_state_root_when_zero() {
        let mut state = fresh_state();
        let pre_root: Bytes32 = state.hash_tree_root().into();
        state.process_slot();
        assert_eq!(state.latest_block_header.state_root, pre_root);
    }

    #[test]
    fn process_slot_no_op_when_state_root_already_set() {
        let mut state = fresh_state();
        state.latest_block_header.state_root = Bytes32::new([0xab; 32]);
        let snapshot = state.clone();
        state.process_slot();
        assert_eq!(state, snapshot);
    }

    // -- process_slots: error paths -----------------------------------------

    #[test]
    fn process_slots_rejects_equal_target() {
        let mut state = fresh_state();
        let target = state.slot;
        let err = state.process_slots(target).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::TargetSlotNotInFuture {
                current: Slot::ZERO,
                target: Slot::ZERO,
            }
        );
    }

    #[test]
    fn process_slots_rejects_past_target() {
        let mut state = fresh_state();
        state.slot = Slot::new(5);
        let err = state.process_slots(Slot::new(3)).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::TargetSlotNotInFuture {
                current: Slot::new(5),
                target: Slot::new(3),
            }
        );
    }

    // -- process_slots: advancement -----------------------------------------

    #[test]
    fn process_slots_advances_to_target() {
        let mut state = fresh_state();
        state.process_slots(Slot::new(5)).unwrap();
        assert_eq!(state.slot, Slot::new(5));
    }

    #[test]
    fn process_slots_single_step_advance() {
        let mut state = fresh_state();
        state.process_slots(Slot::ONE).unwrap();
        assert_eq!(state.slot, Slot::ONE);
    }

    // Genesis-shape state has the zero-root sentinel → first iteration
    // caches it; on subsequent iterations the no-op branch fires, so the
    // cached root survives through the remaining steps.
    #[test]
    fn process_slots_caches_state_root_on_first_step_only() {
        let mut state = fresh_state();
        let pre_root: Bytes32 = state.hash_tree_root().into();
        state.process_slots(Slot::new(3)).unwrap();
        assert_eq!(state.latest_block_header.state_root, pre_root);
    }

    // -- property tests -----------------------------------------------------

    proptest! {
        #[test]
        fn process_slots_path_equivalence(t1 in 1_u64..32, t2_offset in 1_u64..32) {
            let t2 = t1 + t2_offset;

            let mut direct = fresh_state();
            direct.process_slots(Slot::new(t2)).unwrap();

            let mut via_intermediate = fresh_state();
            via_intermediate.process_slots(Slot::new(t1)).unwrap();
            via_intermediate.process_slots(Slot::new(t2)).unwrap();

            prop_assert_eq!(direct.hash_tree_root(), via_intermediate.hash_tree_root());
        }

        #[test]
        fn process_slots_final_slot_equals_target(target in 1_u64..64) {
            let mut state = fresh_state();
            state.process_slots(Slot::new(target)).unwrap();
            prop_assert_eq!(state.slot, Slot::new(target));
        }
    }
}
