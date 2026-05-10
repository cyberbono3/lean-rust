//! Attestation processing on [`State`].
//!
//! Implements `process_attestations` per the 3sf-mini consensus rules:
//!
//! - Each vote is recorded against its target root in the per-target-root
//!   validator bitmap (`state.justifications_roots` /
//!   `state.justifications_validators`).
//! - Once a 2/3 supermajority votes for the same target, the target slot is
//!   justified (`state.justified_slots` and `state.latest_justified` update).
//! - If the target is the next valid justifiable slot after the source (no
//!   other justifiable slot strictly between), the source is finalized
//!   (`state.latest_finalized` updates).

use super::justifications::Justifications;
use super::{AttSlotKind, StateTransitionError};
use crate::slot::Slot;
use crate::state::State;
use crate::vote::SignedVote;

/// Converts `slot` to a `usize` and validates `slot.get() < len`.
///
/// Returns the validated index. Both the `try_from` overflow path and the
/// out-of-bounds path produce
/// [`StateTransitionError::AttestationSlotOutOfRange`] tagged with `kind`.
fn bounded_slot_index(
    slot: Slot,
    kind: AttSlotKind,
    len: usize,
) -> Result<usize, StateTransitionError> {
    usize::try_from(slot.get())
        .ok()
        .filter(|&i| i < len)
        .ok_or(StateTransitionError::AttestationSlotOutOfRange { kind, slot, len })
}

impl State {
    /// Applies `attestations` to `self`.
    ///
    /// Range checks (out-of-range source/target slot, validator id past
    /// `num_validators`) abort the whole call with an error. Semantic
    /// filters (source not yet justified, target already justified, root
    /// mismatch, target not justifiable) cause the offending vote to be
    /// silently skipped.
    ///
    /// All mutation is staged in working copies and committed atomically
    /// after the loop, so an `Err` return leaves the state byte-equal to
    /// its pre-call value.
    ///
    /// # Errors
    /// - [`StateTransitionError::AttestationSlotOutOfRange`] when a vote
    ///   references a slot beyond `state.justified_slots.len()` or
    ///   `state.historical_block_hashes.len()`.
    /// - [`StateTransitionError::AttestationValidatorOutOfRange`] when
    ///   `validator_id >= state.config.num_validators`.
    /// - [`StateTransitionError::StateBoundExceeded`] forwarded from the
    ///   working bitmap rebuild.
    pub fn process_attestations(
        &mut self,
        attestations: &[SignedVote],
    ) -> Result<(), StateTransitionError> {
        let num_validators = self.config.num_validators;
        let just_len = self.justified_slots.len();
        let hist_len = self.historical_block_hashes.len();

        // Working copies — committed at end if every iteration succeeds.
        let mut justifications = Justifications::from_state(self)?;
        let mut justified_slots = self.justified_slots.clone();
        let mut latest_justified = self.latest_justified;
        let mut latest_finalized = self.latest_finalized;

        let validator_limit = usize::try_from(num_validators).map_err(|_| {
            StateTransitionError::StateBoundExceeded {
                context: "num_validators",
            }
        })?;

        for signed in attestations {
            let vote = &signed.message;
            let validator_id = signed.validator_id;
            let source_slot = vote.source.slot;
            let target_slot = vote.target.slot;

            // -- Range checks: any failure aborts the whole call. ----------
            let source_idx = bounded_slot_index(source_slot, AttSlotKind::Source, just_len)?;
            let _ = bounded_slot_index(source_slot, AttSlotKind::Source, hist_len)?;
            let target_idx = bounded_slot_index(target_slot, AttSlotKind::Target, just_len)?;
            let _ = bounded_slot_index(target_slot, AttSlotKind::Target, hist_len)?;
            let validator_idx = usize::try_from(validator_id.get())
                .ok()
                .filter(|&i| i < validator_limit)
                .ok_or(StateTransitionError::AttestationValidatorOutOfRange {
                    validator: validator_id,
                    num_validators,
                })?;

            // -- Semantic filters: skip on mismatch. -----------------------
            let acceptable = justified_slots.get(source_idx) == Some(true)
                && justified_slots.get(target_idx) == Some(false)
                && vote.source.root == self.historical_block_hashes[source_idx]
                && vote.target.root == self.historical_block_hashes[target_idx]
                && target_slot > source_slot
                && target_slot.is_justifiable_after(latest_finalized.slot);
            if !acceptable {
                continue;
            }

            // -- Tally. ----------------------------------------------------
            let n = justifications.num_validators;
            let votes = justifications
                .table
                .entry(vote.target.root)
                .or_insert_with(|| vec![false; n]);
            votes[validator_idx] = true;
            let count = votes.iter().filter(|&&v| v).count();

            // 2/3 supermajority: `3 * count >= 2 * num_validators` avoids
            // integer-division shortfall for small `num_validators`.
            if 3 * count < 2 * validator_limit {
                continue;
            }

            // -- Justify target. ------------------------------------------
            latest_justified = vote.target;
            justified_slots.set(target_idx, true).map_err(|_| {
                StateTransitionError::StateBoundExceeded {
                    context: "justified_slots",
                }
            })?;
            justifications.table.remove(&vote.target.root);

            // -- Finalize source if no justifiable slot lies strictly
            //    between source and target.
            let no_intermediate = ((source_idx + 1)..target_idx).all(|mid| {
                let candidate = Slot::new(mid as u64);
                !candidate.is_justifiable_after(latest_finalized.slot)
            });
            // `mid as u64`: `mid < target_idx <= just_len <= usize::MAX <= u64::MAX`,
            // so the cast is lossless on every supported target.
            if no_intermediate {
                latest_finalized = vote.source;
            }
        }

        // -- Commit. -------------------------------------------------------
        self.justified_slots = justified_slots;
        self.latest_justified = latest_justified;
        self.latest_finalized = latest_finalized;
        justifications.write_back(self)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    use ssz::HashTreeRoot;
    use types::{Bitlist, Bytes32};

    use crate::block::BlockHeader;
    use crate::checkpoint::Checkpoint;
    use crate::state::{ProtocolConfig, State, HISTORICAL_ROOTS_LIMIT};
    use crate::validator::ValidatorIndex;
    use crate::vote::{SignedVote, Vote};

    /// Builds a state with `num_validators` validators, populated history of
    /// `historical_roots`, and `justified_slots` matching the
    /// `justified_pattern` (bool per slot).
    fn populated_state(
        num_validators: u64,
        historical_roots: Vec<Bytes32>,
        justified_pattern: &[bool],
        latest_finalized_slot: Slot,
    ) -> State {
        let mut justified_slots: Bitlist<HISTORICAL_ROOTS_LIMIT> = Bitlist::new();
        for (i, &v) in justified_pattern.iter().enumerate() {
            justified_slots.set(i, v).unwrap();
        }
        State {
            config: ProtocolConfig {
                num_validators,
                genesis_time: 0,
            },
            slot: Slot::new(historical_roots.len() as u64),
            latest_block_header: BlockHeader::default(),
            latest_justified: Checkpoint::default(),
            latest_finalized: Checkpoint::new(Bytes32::zero(), latest_finalized_slot),
            historical_block_hashes: historical_roots,
            justified_slots,
            justifications_roots: Vec::new(),
            justifications_validators: Bitlist::new(),
        }
    }

    fn signed_vote(
        validator_id: u64,
        source_root: Bytes32,
        source_slot: u64,
        target_root: Bytes32,
        target_slot: u64,
    ) -> SignedVote {
        SignedVote {
            validator_id: ValidatorIndex::new(validator_id),
            message: Vote {
                slot: Slot::new(target_slot),
                head: Checkpoint::new(target_root, Slot::new(target_slot)),
                target: Checkpoint::new(target_root, Slot::new(target_slot)),
                source: Checkpoint::new(source_root, Slot::new(source_slot)),
            },
            signature: types::Bytes4000::default(),
        }
    }

    fn root(byte: u8) -> Bytes32 {
        Bytes32::new([byte; 32])
    }

    // -- Range checks: aborting paths ---------------------------------------

    #[test]
    fn out_of_range_source_slot_aborts() {
        let mut state = populated_state(4, vec![root(0xaa)], &[true], Slot::ZERO);
        let votes = vec![signed_vote(0, root(0xaa), 5, root(0xbb), 6)];
        let err = state.process_attestations(&votes).unwrap_err();
        assert!(matches!(
            err,
            StateTransitionError::AttestationSlotOutOfRange {
                kind: AttSlotKind::Source,
                ..
            }
        ));
    }

    #[test]
    fn out_of_range_target_slot_aborts() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let votes = vec![signed_vote(0, root(0xaa), 0, root(0xcc), 9)];
        let err = state.process_attestations(&votes).unwrap_err();
        assert!(matches!(
            err,
            StateTransitionError::AttestationSlotOutOfRange {
                kind: AttSlotKind::Target,
                ..
            }
        ));
    }

    #[test]
    fn out_of_range_validator_aborts() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let votes = vec![signed_vote(99, root(0xaa), 0, root(0xbb), 1)];
        let err = state.process_attestations(&votes).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::AttestationValidatorOutOfRange {
                validator: ValidatorIndex::new(99),
                num_validators: 4,
            }
        );
    }

    // -- Range-check error path leaves state unchanged ----------------------

    #[test]
    fn range_check_error_leaves_state_unchanged() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let snapshot = state.clone();
        let votes = vec![signed_vote(99, root(0xaa), 0, root(0xbb), 1)];
        let _ = state.process_attestations(&votes).unwrap_err();
        assert_eq!(state, snapshot);
    }

    // -- Semantic filters: skip paths --------------------------------------

    #[test]
    fn skips_when_source_not_justified() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[false, false], Slot::ZERO);
        let snapshot = state.clone();
        let votes = vec![signed_vote(0, root(0xaa), 0, root(0xbb), 1)];
        state.process_attestations(&votes).unwrap();
        // No tally recorded.
        assert_eq!(state, snapshot);
    }

    #[test]
    fn skips_when_target_already_justified() {
        let mut state = populated_state(4, vec![root(0xaa), root(0xbb)], &[true, true], Slot::ZERO);
        let snapshot = state.clone();
        let votes = vec![signed_vote(0, root(0xaa), 0, root(0xbb), 1)];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state, snapshot);
    }

    #[test]
    fn skips_when_source_root_mismatch() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let snapshot = state.clone();
        // Wrong source root — should skip.
        let votes = vec![signed_vote(0, root(0xff), 0, root(0xbb), 1)];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state, snapshot);
    }

    #[test]
    fn skips_when_target_le_source() {
        // history len 3, slots 0..2 all justified. Vote with target < source.
        let mut state = populated_state(
            4,
            vec![root(0xaa), root(0xbb), root(0xcc)],
            &[true, true, false],
            Slot::ZERO,
        );
        let snapshot = state.clone();
        // target slot 1 <= source slot 1 — skipped (also: target already justified).
        let votes = vec![signed_vote(0, root(0xaa), 0, root(0xaa), 0)];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state, snapshot);
    }

    #[test]
    fn skips_when_target_not_justifiable() {
        // History through slot 7. Source slot 0 justified, target slot 7
        // unjustified. delta = 7 - 0 = 7 — neither perfect square nor pronic
        // and > 5, so target is NOT justifiable.
        let history: Vec<Bytes32> = (0_u8..8).map(root).collect();
        let mut just_pattern = vec![false; 8];
        just_pattern[0] = true;
        let mut state = populated_state(4, history, &just_pattern, Slot::ZERO);
        let snapshot = state.clone();
        let votes = vec![signed_vote(0, root(0), 0, root(7), 7)];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state, snapshot);
    }

    // -- Tally and supermajority --------------------------------------------

    #[test]
    fn single_subthreshold_vote_does_not_justify() {
        // 4 validators ⇒ supermajority = 3.
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let votes = vec![signed_vote(0, root(0xaa), 0, root(0xbb), 1)];
        state.process_attestations(&votes).unwrap();
        // Target slot 1 NOT justified yet.
        assert_eq!(state.justified_slots.get(1), Some(false));
        // But the vote IS recorded against the target root.
        assert_eq!(state.justifications_roots, vec![root(0xbb)]);
        assert_eq!(state.justifications_validators.len(), 4);
        assert_eq!(state.justifications_validators.get(0), Some(true));
        assert_eq!(state.justifications_validators.get(1), Some(false));
    }

    #[test]
    fn supermajority_justifies_target() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let votes = vec![
            signed_vote(0, root(0xaa), 0, root(0xbb), 1),
            signed_vote(1, root(0xaa), 0, root(0xbb), 1),
            signed_vote(2, root(0xaa), 0, root(0xbb), 1),
        ];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state.justified_slots.get(1), Some(true));
        assert_eq!(state.latest_justified.root, root(0xbb));
        assert_eq!(state.latest_justified.slot, Slot::new(1));
        // Per-target tally is dropped once the target justifies.
        assert!(state.justifications_roots.is_empty());
        assert_eq!(state.justifications_validators.len(), 0);
    }

    #[test]
    fn finalizes_source_when_target_is_next_justifiable_slot() {
        // delta(target, finalized=0) = 1: justifiable. delta(source, finalized) = 0,
        // which means finalized = source = slot 0. No intermediate justifiable
        // slots between source.slot=0 and target.slot=1 (range is empty).
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let votes = vec![
            signed_vote(0, root(0xaa), 0, root(0xbb), 1),
            signed_vote(1, root(0xaa), 0, root(0xbb), 1),
            signed_vote(2, root(0xaa), 0, root(0xbb), 1),
        ];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state.latest_finalized.root, root(0xaa));
        assert_eq!(state.latest_finalized.slot, Slot::ZERO);
    }

    #[test]
    fn does_not_finalize_when_intermediate_justifiable_slot_exists() {
        // History 0..=9 with slot 0 justified. Vote source slot 0, target
        // slot 9 (3² = 9 → justifiable). Slot 1 is justifiable too (delta=1
        // ≤ 5), so the source MUST NOT finalize.
        let history: Vec<Bytes32> = (0_u8..10).map(root).collect();
        let mut just_pattern = vec![false; 10];
        just_pattern[0] = true;
        let mut state = populated_state(4, history, &just_pattern, Slot::ZERO);
        let original_finalized = state.latest_finalized;
        let votes = vec![
            signed_vote(0, root(0), 0, root(9), 9),
            signed_vote(1, root(0), 0, root(9), 9),
            signed_vote(2, root(0), 0, root(9), 9),
        ];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state.justified_slots.get(9), Some(true));
        assert_eq!(state.latest_finalized, original_finalized);
    }

    // -- Idempotence --------------------------------------------------------

    #[test]
    fn duplicate_vote_for_same_validator_is_idempotent() {
        let mut once = populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let mut twice = once.clone();
        let votes = vec![signed_vote(0, root(0xaa), 0, root(0xbb), 1)];
        once.process_attestations(&votes).unwrap();
        let votes_twice = vec![votes[0].clone(), votes[0].clone()];
        twice.process_attestations(&votes_twice).unwrap();
        assert_eq!(once.hash_tree_root(), twice.hash_tree_root());
    }
}
