//! Crate-private helpers shared across `genesis` and `slots` modules.

use protocol::Slot;

use crate::error::StateTransitionError;

/// One-slot increment used by [`crate::slots::process_slots`].
pub(crate) const ONE_SLOT: Slot = Slot::new(1);

/// Returns `slot + 1` or [`StateTransitionError::SlotOverflow`] when the
/// addition would overflow `u64`. Defensive: `process_slots` rejects
/// `target_slot > u64::MAX - 1` indirectly via `target > current`, but the
/// explicit `checked_add` keeps the slot-advance loop `unwrap`-free.
pub(crate) fn advance_slot(slot: Slot) -> Result<Slot, StateTransitionError> {
    slot.get()
        .checked_add(ONE_SLOT.get())
        .map(Slot::new)
        .ok_or(StateTransitionError::SlotOverflow { slot: slot.get() })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn one_slot_is_one() {
        assert_eq!(ONE_SLOT.get(), 1);
    }

    #[test]
    fn advance_slot_increments() {
        assert_eq!(advance_slot(Slot::new(0)).unwrap(), Slot::new(1));
        assert_eq!(advance_slot(Slot::new(41)).unwrap(), Slot::new(42));
    }

    #[test]
    fn advance_slot_rejects_overflow() {
        let err = advance_slot(Slot::new(u64::MAX)).unwrap_err();
        assert_eq!(err, StateTransitionError::SlotOverflow { slot: u64::MAX });
    }
}
