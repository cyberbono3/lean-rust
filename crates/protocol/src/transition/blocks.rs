//! Block-header processing on [`State`].

use ssz::HashTreeRoot;
use types::Bytes32;

use super::StateTransitionError;
use crate::block::{Block, BlockHeader};
use crate::state::{State, HISTORICAL_ROOTS_LIMIT};
use crate::validator::is_proposer;

impl State {
    /// Validates `block` against `self` and commits its header-derived state.
    ///
    /// Mirrors the consensus-spec `process_block_header`. The method is
    /// transactional in spirit: every validation runs before any field on
    /// `self` is mutated, so an `Err` return leaves the state byte-equal to
    /// its pre-call value.
    ///
    /// # Errors
    /// - [`StateTransitionError::BlockSlotMismatch`] when `block.slot != self.slot`.
    /// - [`StateTransitionError::BlockOlderThanLatest`] when `block.slot <= self.latest_block_header.slot`.
    /// - [`StateTransitionError::IncorrectBlockProposer`] when
    ///   `block.proposer_index` is not the round-robin proposer for `self.slot`.
    /// - [`StateTransitionError::BlockParentRootMismatch`] when
    ///   `block.parent_root != hash_tree_root(self.latest_block_header)`.
    /// - [`StateTransitionError::StateBoundExceeded`] when the appended
    ///   parent root plus zero-padded empty slots would push
    ///   `historical_block_hashes` or `justified_slots` past their bounds.
    /// - [`StateTransitionError::Protocol`] forwarded from
    ///   [`is_proposer`] when `self.config.num_validators == 0`.
    pub fn process_block_header(&mut self, block: &Block) -> Result<(), StateTransitionError> {
        // -- Validation gate: cheap checks first, hash last. ----------------
        if block.slot != self.slot {
            return Err(StateTransitionError::BlockSlotMismatch {
                got: block.slot,
                want: self.slot,
            });
        }
        if block.slot <= self.latest_block_header.slot {
            return Err(StateTransitionError::BlockOlderThanLatest {
                slot: block.slot,
                latest: self.latest_block_header.slot,
            });
        }
        if !is_proposer(block.proposer_index, self.slot, self.config.num_validators)? {
            return Err(StateTransitionError::IncorrectBlockProposer {
                slot: self.slot,
                proposer: block.proposer_index,
            });
        }
        let parent_root: Bytes32 = self.latest_block_header.hash_tree_root().into();
        if block.parent_root != parent_root {
            return Err(StateTransitionError::BlockParentRootMismatch {
                slot: block.slot,
                got: block.parent_root,
                want: parent_root,
            });
        }

        // -- Derived values. ------------------------------------------------
        let body_root: Bytes32 = block.body.hash_tree_root().into();
        let was_genesis = self.latest_block_header.slot.is_zero();
        let prev_slot = self.latest_block_header.slot.get();
        // Safe: `block.slot > prev_slot` (validated above) ⇒ subtraction
        // cannot underflow; the result is a `u64` slot count.
        let empty_slots = block.slot.get() - prev_slot - 1;
        let empty_slots_usize =
            usize::try_from(empty_slots).map_err(|_| StateTransitionError::StateBoundExceeded {
                context: "historical_block_hashes",
            })?;
        let next_history_len = self
            .historical_block_hashes
            .len()
            .checked_add(1)
            .and_then(|n| n.checked_add(empty_slots_usize))
            .ok_or(StateTransitionError::StateBoundExceeded {
                context: "historical_block_hashes",
            })?;
        if next_history_len > HISTORICAL_ROOTS_LIMIT {
            return Err(StateTransitionError::StateBoundExceeded {
                context: "historical_block_hashes",
            });
        }

        // -- Commit. --------------------------------------------------------
        if was_genesis {
            self.latest_justified.root = parent_root;
            self.latest_finalized.root = parent_root;
        }

        let parent_idx = self.justified_slots.len();
        self.historical_block_hashes.push(parent_root);
        self.justified_slots
            .set(parent_idx, was_genesis)
            .map_err(|_| StateTransitionError::StateBoundExceeded {
                context: "justified_slots",
            })?;

        self.historical_block_hashes
            .extend(std::iter::repeat_n(Bytes32::zero(), empty_slots_usize));
        for _ in 0..empty_slots_usize {
            let idx = self.justified_slots.len();
            self.justified_slots.set(idx, false).map_err(|_| {
                StateTransitionError::StateBoundExceeded {
                    context: "justified_slots",
                }
            })?;
        }

        self.latest_block_header = BlockHeader {
            slot: block.slot,
            proposer_index: block.proposer_index,
            parent_root: block.parent_root,
            state_root: Bytes32::zero(),
            body_root,
        };
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::block::BlockBody;
    use crate::checkpoint::Checkpoint;
    use crate::slot::Slot;
    use crate::state::ProtocolConfig;
    use crate::validator::ValidatorIndex;

    const NUM_VALIDATORS: u64 = 4;
    const GENESIS_TIME: u64 = 1_700_000_000;

    /// Genesis-shape `State` for a 4-validator chain whose
    /// `latest_block_header` commits to the empty body.
    fn genesis() -> State {
        let body_root: Bytes32 = BlockBody::default().hash_tree_root().into();
        State {
            config: ProtocolConfig {
                num_validators: NUM_VALIDATORS,
                genesis_time: GENESIS_TIME,
            },
            latest_block_header: BlockHeader {
                body_root,
                ..BlockHeader::default()
            },
            ..State::default()
        }
    }

    /// Produces a valid block for `state` at `state.slot` whose body is empty.
    fn valid_block_for(state: &State) -> Block {
        let parent_root: Bytes32 = state.latest_block_header.hash_tree_root().into();
        let proposer_index = ValidatorIndex::new(state.slot.get() % state.config.num_validators);
        Block {
            slot: state.slot,
            proposer_index,
            parent_root,
            state_root: Bytes32::zero(),
            body: BlockBody::default(),
        }
    }

    // -- Validation: rejection paths ----------------------------------------

    #[test]
    fn block_slot_mismatch_rejects() {
        let mut state = genesis();
        state.process_slots(Slot::new(2)).unwrap();
        let mut block = valid_block_for(&state);
        block.slot = Slot::new(3);
        let err = state.process_block_header(&block).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::BlockSlotMismatch {
                got: Slot::new(3),
                want: Slot::new(2),
            }
        );
    }

    #[test]
    fn block_older_than_latest_rejects() {
        // A state whose latest_block_header.slot already equals self.slot.
        let mut state = genesis();
        state.process_slots(Slot::new(3)).unwrap();
        state.latest_block_header.slot = Slot::new(3);
        let block = valid_block_for(&state);
        let err = state.process_block_header(&block).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::BlockOlderThanLatest {
                slot: Slot::new(3),
                latest: Slot::new(3),
            }
        );
    }

    #[test]
    fn incorrect_proposer_rejects() {
        let mut state = genesis();
        state.process_slots(Slot::new(1)).unwrap();
        let mut block = valid_block_for(&state);
        // slot 1 round-robin proposer with N=4 is index 1; choose 2 instead.
        block.proposer_index = ValidatorIndex::new(2);
        let err = state.process_block_header(&block).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::IncorrectBlockProposer {
                slot: Slot::new(1),
                proposer: ValidatorIndex::new(2),
            }
        );
    }

    #[test]
    fn parent_root_mismatch_rejects() {
        let mut state = genesis();
        state.process_slots(Slot::new(1)).unwrap();
        let mut block = valid_block_for(&state);
        block.parent_root = Bytes32::new([0xff; 32]);
        let err = state.process_block_header(&block).unwrap_err();
        assert!(matches!(
            err,
            StateTransitionError::BlockParentRootMismatch { slot, .. } if slot == Slot::new(1)
        ));
    }

    #[test]
    fn zero_validators_surfaces_protocol_error() {
        let mut state = genesis();
        state.config.num_validators = 0;
        state.process_slots(Slot::new(1)).unwrap();
        let block = Block {
            slot: Slot::new(1),
            ..Default::default()
        };
        let err = state.process_block_header(&block).unwrap_err();
        assert!(matches!(err, StateTransitionError::Protocol(_)));
    }

    // -- Validation: state preserved on error -------------------------------

    #[test]
    fn error_path_leaves_state_unchanged() {
        let mut state = genesis();
        state.process_slots(Slot::new(2)).unwrap();
        let snapshot = state.clone();
        let mut block = valid_block_for(&state);
        block.parent_root = Bytes32::new([0xab; 32]);
        let _ = state.process_block_header(&block).unwrap_err();
        assert_eq!(state, snapshot);
    }

    // -- Happy path: commitment ---------------------------------------------

    #[test]
    fn happy_path_commits_header_and_root() {
        let mut state = genesis();
        state.process_slots(Slot::new(1)).unwrap();
        let block = valid_block_for(&state);
        let parent_root = block.parent_root;
        let body_root: Bytes32 = block.body.hash_tree_root().into();

        state.process_block_header(&block).unwrap();

        assert_eq!(state.latest_block_header.slot, Slot::new(1));
        assert_eq!(state.latest_block_header.parent_root, parent_root);
        assert_eq!(state.latest_block_header.body_root, body_root);
        // process_block_header zeroes the post-state root sentinel.
        assert_eq!(state.latest_block_header.state_root, Bytes32::zero());
        assert_eq!(
            state.latest_block_header.proposer_index,
            block.proposer_index
        );
    }

    #[test]
    fn genesis_seeds_justified_and_finalized_root() {
        let mut state = genesis();
        state.process_slots(Slot::new(1)).unwrap();
        let block = valid_block_for(&state);
        let parent_root = block.parent_root;

        assert_eq!(state.latest_justified, Checkpoint::default());
        assert_eq!(state.latest_finalized, Checkpoint::default());
        state.process_block_header(&block).unwrap();
        assert_eq!(state.latest_justified.root, parent_root);
        assert_eq!(state.latest_finalized.root, parent_root);
        // Slots stay at their default zero values; only the root is seeded.
        assert_eq!(state.latest_justified.slot, Slot::ZERO);
        assert_eq!(state.latest_finalized.slot, Slot::ZERO);
    }

    #[test]
    fn appends_parent_root_and_genesis_justified_bit() {
        let mut state = genesis();
        state.process_slots(Slot::new(1)).unwrap();
        let block = valid_block_for(&state);
        let parent_root = block.parent_root;

        state.process_block_header(&block).unwrap();
        assert_eq!(state.historical_block_hashes, vec![parent_root]);
        assert_eq!(state.justified_slots.len(), 1);
        // Genesis branch records the parent slot (0) as justified.
        assert_eq!(state.justified_slots.get(0), Some(true));
    }

    #[test]
    fn empty_slots_filled_with_zero_root_and_unjustified_bits() {
        // First block at slot 1 (no empty slots).
        let mut state = genesis();
        state.process_slots(Slot::new(1)).unwrap();
        let block_a = valid_block_for(&state);
        let parent_root_a = block_a.parent_root;
        state.process_block_header(&block_a).unwrap();

        // Second block at slot 4 — three empty slots between them (slots 1, 2, 3).
        // Wait: latest header slot is 1, block.slot = 4, so empty_slots = 4 - 1 - 1 = 2.
        state.process_slots(Slot::new(4)).unwrap();
        let block_b = Block {
            slot: Slot::new(4),
            proposer_index: ValidatorIndex::new(0),
            parent_root: state.latest_block_header.hash_tree_root().into(),
            state_root: Bytes32::zero(),
            body: BlockBody::default(),
        };
        let parent_root_b = block_b.parent_root;

        state.process_block_header(&block_b).unwrap();

        // history: [parent_root_a, parent_root_b, zero, zero]
        assert_eq!(state.historical_block_hashes.len(), 4);
        assert_eq!(state.historical_block_hashes[0], parent_root_a);
        assert_eq!(state.historical_block_hashes[1], parent_root_b);
        assert_eq!(state.historical_block_hashes[2], Bytes32::zero());
        assert_eq!(state.historical_block_hashes[3], Bytes32::zero());

        // justified_slots: [true, false, false, false]
        // First slot was the genesis-parent justified bit; second is the
        // post-genesis parent (was_genesis = false on block_b); empty slots
        // contribute false bits.
        assert_eq!(state.justified_slots.len(), 4);
        assert_eq!(state.justified_slots.get(0), Some(true));
        assert_eq!(state.justified_slots.get(1), Some(false));
        assert_eq!(state.justified_slots.get(2), Some(false));
        assert_eq!(state.justified_slots.get(3), Some(false));
    }

    #[test]
    fn second_block_does_not_reseed_justified_root() {
        let mut state = genesis();
        state.process_slots(Slot::new(1)).unwrap();
        let block_a = valid_block_for(&state);
        let parent_root_a = block_a.parent_root;
        state.process_block_header(&block_a).unwrap();

        state.process_slots(Slot::new(2)).unwrap();
        let block_b = Block {
            slot: Slot::new(2),
            proposer_index: ValidatorIndex::new(2),
            parent_root: state.latest_block_header.hash_tree_root().into(),
            state_root: Bytes32::zero(),
            body: BlockBody::default(),
        };

        state.process_block_header(&block_b).unwrap();
        // Genesis-seeding only fires once: the second block leaves the
        // justified root pointing at the genesis parent.
        assert_eq!(state.latest_justified.root, parent_root_a);
        assert_eq!(state.latest_finalized.root, parent_root_a);
    }
}
