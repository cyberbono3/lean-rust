//! Shared `(state, anchor_block)` fixtures for forkchoice tests.
//!
//! Bypass chain construction: the helpers here build a state and a block
//! whose `state_root` matches `state.hash_tree_root()` so
//! [`crate::store::Store::from_anchor`] invariants can be tested in
//! isolation. The state internals are NOT a valid post-transition state —
//! this is intentional, and these fixtures are kept private to forkchoice
//! `cfg(test)` builds.

use protocol::{Block, BlockBody, BlockHeader, ProtocolConfig, Slot, State, ValidatorIndex};
use ssz::HashTreeRoot;
use types::Bytes32;

const GENESIS_TIME: u64 = 1_700_000_000;

/// Genesis-shape `State` for an `n`-validator chain whose
/// `latest_block_header` commits to the empty `BlockBody`. Inlined here
/// rather than re-exported from `statetransition` so forkchoice tests stay
/// independent of the genesis builder's evolution.
fn genesis_state(num_validators: u64) -> State {
    let body_root: Bytes32 = BlockBody::default().hash_tree_root().into();
    State {
        config: ProtocolConfig {
            num_validators,
            genesis_time: GENESIS_TIME,
        },
        latest_block_header: BlockHeader {
            body_root,
            ..BlockHeader::default()
        },
        ..State::default()
    }
}

/// Builds a `(state, block)` pair such that `block.state_root ==
/// state.hash_tree_root()`. The state is genesis-shape with `state.slot` and
/// `state.latest_block_header.slot` set to `slot`. Used to test
/// `Store::from_anchor` at non-zero slots without running the full chain.
pub(crate) fn anchor_pair_at_slot(slot: Slot, num_validators: u64) -> (State, Block) {
    let mut state = genesis_state(num_validators);
    state.slot = slot;
    state.latest_block_header.slot = slot;

    let parent_root: Bytes32 = state.latest_block_header.hash_tree_root().into();
    let proposer_index = ValidatorIndex::new(slot.get() % num_validators.max(1));

    let block = Block {
        slot,
        proposer_index,
        parent_root,
        state_root: state.hash_tree_root().into(),
        body: BlockBody::default(),
    };
    (state, block)
}

/// Slot-0 anchor pair. Convenience wrapper over
/// [`anchor_pair_at_slot`] with `slot = Slot::ZERO`.
pub(crate) fn anchor_pair(num_validators: u64) -> (State, Block) {
    anchor_pair_at_slot(Slot::ZERO, num_validators)
}
