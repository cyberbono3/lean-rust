//! Genesis state construction.

use protocol::{BlockBody, BlockHeader, Checkpoint, ProtocolConfig, Slot, State, ValidatorIndex};
use ssz::HashTreeRoot;
use types::Bytes32;

/// Builds the slot-0 consensus [`State`] for the given validator-set size and
/// chain genesis time.
///
/// The state's `latest_block_header.body_root` commits to the empty
/// [`BlockBody`] (no attestations); all other fields are zero-valued. Lists
/// and bitlists are empty.
///
/// # Example
/// ```
/// use statetransition::genesis_state;
/// let s = genesis_state(4, 1_700_000_000);
/// assert_eq!(s.slot.get(), 0);
/// assert_eq!(s.config.num_validators, 4);
/// assert_eq!(s.config.genesis_time, 1_700_000_000);
/// ```
#[must_use]
pub fn genesis_state(num_validators: u64, genesis_time: u64) -> State {
    let body = BlockBody::default();
    let body_root = Bytes32::new(body.hash_tree_root());

    State {
        config: ProtocolConfig {
            num_validators,
            genesis_time,
        },
        slot: Slot::new(0),
        latest_block_header: BlockHeader {
            slot: Slot::new(0),
            proposer_index: ValidatorIndex::new(0),
            parent_root: Bytes32::zero(),
            state_root: Bytes32::zero(),
            body_root,
        },
        latest_justified: Checkpoint::default(),
        latest_finalized: Checkpoint::default(),
        historical_block_hashes: Vec::new(),
        justified_slots: types::Bitlist::new(),
        justifications_roots: Vec::new(),
        justifications_validators: types::Bitlist::new(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const NUM_VALIDATORS: u64 = 4;
    const GENESIS_TIME: u64 = 1_700_000_000;

    fn sample() -> State {
        genesis_state(NUM_VALIDATORS, GENESIS_TIME)
    }

    #[test]
    fn genesis_state_default_yields_zero_slot() {
        assert_eq!(sample().slot, Slot::new(0));
    }

    #[test]
    fn genesis_state_records_config() {
        let s = sample();
        assert_eq!(s.config.num_validators, NUM_VALIDATORS);
        assert_eq!(s.config.genesis_time, GENESIS_TIME);
    }

    #[test]
    fn genesis_state_body_root_matches_empty_body() {
        let expected = Bytes32::new(BlockBody::default().hash_tree_root());
        assert_eq!(sample().latest_block_header.body_root, expected);
    }

    #[test]
    fn genesis_state_lists_and_bitlists_are_empty() {
        let s = sample();
        assert!(s.historical_block_hashes.is_empty());
        assert!(s.justifications_roots.is_empty());
        assert_eq!(s.justified_slots.len(), 0);
        assert_eq!(s.justifications_validators.len(), 0);
    }

    #[test]
    fn genesis_state_checkpoints_are_zero() {
        let s = sample();
        assert_eq!(s.latest_justified, Checkpoint::default());
        assert_eq!(s.latest_finalized, Checkpoint::default());
    }

    #[test]
    fn genesis_state_latest_block_header_state_root_is_zero() {
        // Slot processing relies on the zero state-root sentinel to know it
        // hasn't cached the previous-state root yet.
        assert_eq!(sample().latest_block_header.state_root, Bytes32::zero());
    }

    #[test]
    fn genesis_state_hash_tree_root_is_deterministic() {
        assert_eq!(sample().hash_tree_root(), sample().hash_tree_root());
    }

    #[test]
    fn genesis_state_responds_to_config_inputs() {
        let a = genesis_state(4, 1_700_000_000);
        let b = genesis_state(5, 1_700_000_000);
        assert_ne!(a.hash_tree_root(), b.hash_tree_root());

        let c = genesis_state(4, 1_800_000_000);
        assert_ne!(a.hash_tree_root(), c.hash_tree_root());
    }
}
