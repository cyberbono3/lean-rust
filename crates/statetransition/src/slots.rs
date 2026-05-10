//! Per-slot housekeeping and multi-slot advancement.

use protocol::{Slot, State};
use ssz::HashTreeRoot;
use types::Bytes32;

use crate::error::StateTransitionError;
use crate::helpers::advance_slot;

/// Performs per-slot housekeeping on `state`.
///
/// On the first slot after a block is accepted (the latest header carries an
/// all-zero `state_root` sentinel), caches the pre-block state root into the
/// header. Otherwise the state is returned unchanged.
#[must_use]
pub fn process_slot(state: &State) -> State {
    if state.latest_block_header.state_root != Bytes32::zero() {
        return state.clone();
    }
    let previous_state_root = Bytes32::new(state.hash_tree_root());
    let mut next = state.clone();
    next.latest_block_header.state_root = previous_state_root;
    next
}

/// Advances `state` slot-by-slot up to (but not past) `target_slot`.
///
/// Each iteration runs [`process_slot`] then increments `state.slot` by one.
///
/// # Errors
/// - [`StateTransitionError::TargetSlotNotInFuture`] when
///   `target_slot <= state.slot`.
/// - [`StateTransitionError::SlotOverflow`] when slot arithmetic would exceed
///   `u64::MAX`. Cannot fire once the future-target check passes, but
///   surfaced explicitly to keep the loop `unwrap`-free.
pub fn process_slots(state: &State, target_slot: Slot) -> Result<State, StateTransitionError> {
    if target_slot.get() <= state.slot.get() {
        return Err(StateTransitionError::TargetSlotNotInFuture {
            current: state.slot.get(),
            target: target_slot.get(),
        });
    }

    let mut current = state.clone();
    while current.slot.get() < target_slot.get() {
        let mut next = process_slot(&current);
        next.slot = advance_slot(next.slot)?;
        current = next;
    }
    Ok(current)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    use crate::genesis::genesis_state;

    fn fresh() -> State {
        genesis_state(4, 1_700_000_000)
    }

    // -- process_slot --------------------------------------------------------

    #[test]
    fn process_slot_caches_previous_state_root_when_zero() {
        let state = fresh();
        let pre_root = Bytes32::new(state.hash_tree_root());
        let next = process_slot(&state);
        assert_eq!(next.latest_block_header.state_root, pre_root);
        // Other fields unchanged.
        assert_eq!(next.slot, state.slot);
        assert_eq!(next.config, state.config);
    }

    #[test]
    fn process_slot_no_op_when_state_root_already_set() {
        let mut state = fresh();
        state.latest_block_header.state_root = Bytes32::new([0xab; 32]);
        let next = process_slot(&state);
        assert_eq!(next, state);
    }

    // -- process_slots: error paths -----------------------------------------

    #[test]
    fn process_slots_rejects_equal_target() {
        let state = fresh();
        let err = process_slots(&state, state.slot).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::TargetSlotNotInFuture {
                current: 0,
                target: 0,
            }
        );
    }

    #[test]
    fn process_slots_rejects_past_target() {
        let mut state = fresh();
        state.slot = Slot::new(5);
        let err = process_slots(&state, Slot::new(3)).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::TargetSlotNotInFuture {
                current: 5,
                target: 3,
            }
        );
    }

    // -- process_slots: advancement -----------------------------------------

    #[test]
    fn process_slots_advances_to_target() {
        let state = fresh();
        let result = process_slots(&state, Slot::new(5)).unwrap();
        assert_eq!(result.slot, Slot::new(5));
    }

    #[test]
    fn process_slots_single_step_advance() {
        let state = fresh();
        let result = process_slots(&state, Slot::new(1)).unwrap();
        assert_eq!(result.slot, Slot::new(1));
    }

    #[test]
    fn process_slots_caches_state_root_on_first_step_only() {
        // Genesis has zero state_root → first process_slot caches it; on
        // subsequent steps the cached root is non-zero so the no-op branch
        // fires, leaving state_root unchanged across the remaining iterations.
        let state = fresh();
        let pre_root = Bytes32::new(state.hash_tree_root());
        let result = process_slots(&state, Slot::new(3)).unwrap();
        assert_eq!(result.latest_block_header.state_root, pre_root);
    }

    // -- property tests -----------------------------------------------------

    proptest! {
        #[test]
        fn process_slots_path_equivalence(t1 in 1_u64..32, t2_offset in 1_u64..32) {
            // Going to t1 then to t1+t2 must equal going to t1+t2 directly.
            let state = fresh();
            let t2 = t1 + t2_offset;

            let direct = process_slots(&state, Slot::new(t2)).unwrap();
            let via_intermediate = {
                let mid = process_slots(&state, Slot::new(t1)).unwrap();
                process_slots(&mid, Slot::new(t2)).unwrap()
            };
            prop_assert_eq!(direct.hash_tree_root(), via_intermediate.hash_tree_root());
        }

        #[test]
        fn process_slots_final_slot_equals_target(target in 1_u64..64) {
            let state = fresh();
            let result = process_slots(&state, Slot::new(target)).unwrap();
            prop_assert_eq!(result.slot, Slot::new(target));
        }
    }
}
