//! Shared `(state, anchor_block)` fixtures for forkchoice tests.
//!
//! Bypass chain construction: the helpers here build a state and a block
//! whose `state_root` matches `state.hash_tree_root()` so
//! [`crate::store::Store::from_anchor`] invariants can be tested in
//! isolation. The state internals are NOT a valid post-transition state —
//! this is intentional, and these fixtures are kept private to forkchoice
//! `cfg(test)` builds.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use protocol::{
    Block, BlockBody, BlockHeader, Checkpoint, ProtocolConfig, SignedVote, Slot, State,
    ValidatorIndex, Vote,
};
use ssz::HashTreeRoot;
use types::{Bytes32, Bytes4000};

use crate::store::Store;
use crate::time::Time;

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

/// Builds a spec-compliant genesis `(state, block)` pair: the anchor
/// block has `parent_root = Bytes32::zero()` (no prior block) and the
/// state's `latest_block_header.state_root` is the zero sentinel —
/// `process_slot` fills it later. Block / header `hash_tree_root` agree
/// once the state-root sentinel is cached, so a chain extension at
/// slot 1 will pass `process_block_header`'s parent-root check.
pub(crate) fn genesis_anchor(num_validators: u64) -> (State, Block) {
    let state = genesis_state(num_validators);
    let block = Block {
        slot: Slot::ZERO,
        proposer_index: ValidatorIndex::new(0),
        parent_root: Bytes32::zero(),
        state_root: state.hash_tree_root().into(),
        body: BlockBody::default(),
    };
    (state, block)
}

/// Convenience: builds a forkchoice store anchored at the spec-compliant
/// genesis pair and returns it alongside the anchor root.
pub(crate) fn genesis_store(num_validators: u64) -> (Store, Bytes32) {
    let (state, block) = genesis_anchor(num_validators);
    let root: Bytes32 = block.hash_tree_root().into();
    let store = Store::from_anchor(state, block).expect("genesis anchor invariants");
    (store, root)
}

/// Builds a linear chain pinned to genesis-justified, with the clock set
/// to `time`. Consolidates the per-test-module fixture pattern (chain
/// + justified pin + optional clock position) into one helper.
pub(crate) fn pinned_chain(
    n_blocks: u64,
    num_validators: u64,
    time: Time,
) -> (Store, Vec<Bytes32>) {
    let (mut store, roots, _) = linear_chain(n_blocks, num_validators);
    store.set_latest_justified_for_test(Checkpoint::new(roots[0], Slot::ZERO));
    let store = store.with_time_for_test(time);
    (store, roots)
}

/// Builds a linear chain `genesis → b_1 → … → b_{n_blocks-1}` and inserts
/// every `(root, block, state)` triple into a freshly-anchored [`Store`].
/// Returns the store, the per-block roots in order (root[0] = genesis),
/// and the per-block states (so tests can read `latest_justified` etc.).
///
/// Each non-genesis block carries an empty body. `state_root` reuses the
/// anchor state's root — adequate for forkchoice tests, which never call
/// the state-transition function against these blocks.
pub(crate) fn linear_chain(
    n_blocks: u64,
    num_validators: u64,
) -> (Store, Vec<Bytes32>, Vec<State>) {
    assert!(n_blocks >= 1, "linear_chain requires at least 1 block");

    let (state, anchor_block) = anchor_pair(num_validators);
    let anchor_root: Bytes32 = anchor_block.hash_tree_root().into();
    let anchor_state_root: Bytes32 = state.hash_tree_root().into();
    let mut store = Store::from_anchor(state.clone(), anchor_block).expect("anchor invariants");

    let cap = usize::try_from(n_blocks).expect("n_blocks fits in usize");
    let mut roots = Vec::with_capacity(cap);
    let mut states = Vec::with_capacity(cap);
    roots.push(anchor_root);
    states.push(state);

    let mut parent_root = anchor_root;
    for slot_index in 1..n_blocks {
        let block = Block {
            slot: Slot::new(slot_index),
            proposer_index: ValidatorIndex::new(slot_index % num_validators.max(1)),
            parent_root,
            state_root: anchor_state_root,
            body: BlockBody::default(),
        };
        let root: Bytes32 = block.hash_tree_root().into();
        // Reuse the anchor state — its contents are immaterial to forkchoice
        // tests that touch only block (slot, parent_root, weight).
        store.insert_block(root, block, states[0].clone());
        roots.push(root);
        states.push(states[0].clone());
        parent_root = root;
    }

    (store, roots, states)
}

/// Builds a [`SignedVote`] from explicit `(validator, head, target, source,
/// slot)` parts. `signature` is zero-filled — forkchoice never inspects it.
pub(crate) fn signed_vote(
    validator: ValidatorIndex,
    head: Checkpoint,
    target: Checkpoint,
    source: Checkpoint,
    slot: Slot,
) -> SignedVote {
    SignedVote {
        validator_id: validator,
        message: Vote {
            slot,
            head,
            target,
            source,
        },
        signature: Bytes4000::new([0; 4000]),
    }
}

/// Convenience: build a `SignedVote` whose `head`, `target`, and `source`
/// all point at the same `(root, slot)`. Used by tests that don't care
/// about FFG distinctions.
pub(crate) fn signed_vote_at(
    validator: ValidatorIndex,
    head_root: Bytes32,
    head_slot: Slot,
    vote_slot: Slot,
    source: Checkpoint,
) -> SignedVote {
    let head = Checkpoint::new(head_root, head_slot);
    signed_vote(validator, head, head, source, vote_slot)
}
