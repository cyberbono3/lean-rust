//! Test-only fixtures for the `engine` module.
//!
//! Builds anchor state/block pairs from `protocol`'s public surface so the
//! engine tests do not depend on forkchoice's private `test_fixtures` module.
//! Shape mirrors `forkchoice::test_fixtures::genesis_anchor` but uses only
//! re-exported `protocol` types.

// Retained construction sites for the deprecated `Bytes4000` placeholder.
// Scoped to this file so unrelated deprecations elsewhere in the crate are
// still surfaced; removed when this file's last site moves to `Signature`.
#![allow(deprecated)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc
)]

use protocol::stf::genesis_state;
use protocol::{Block, BlockBody, SignedBlock, Slot, State, ValidatorIndex};
use ssz::HashTreeRoot;
use types::{Bytes32, Bytes4000};

use super::handle::Engine;

/// Validator-count constant used by the import / produce tests. Four matches
/// the forkchoice production-test default and keeps the round-robin proposer
/// schedule deterministic across slots.
pub const ENGINE_VALIDATORS: u64 = 4;

const GENESIS_TIME: u64 = 1_700_000_000;

/// Returns a spec-compliant `(state, anchor_block)` pair such that
/// `anchor_block.state_root == state.hash_tree_root()` and `parent_root` is
/// the zero sentinel. Eligible input to [`Engine::from_anchor`].
#[must_use]
pub fn anchor_pair(num_validators: u64) -> (State, Block) {
    let state = genesis_state(num_validators, GENESIS_TIME);
    let block = Block {
        slot: Slot::ZERO,
        proposer_index: ValidatorIndex::new(0),
        parent_root: Bytes32::zero(),
        state_root: state.hash_tree_root().into(),
        body: BlockBody::default(),
    };
    (state, block)
}

/// Builds an [`Engine`] anchored at genesis.
#[must_use]
pub fn engine_at_genesis(num_validators: u64) -> Engine {
    let (state, block) = anchor_pair(num_validators);
    Engine::from_anchor(state, block).expect("genesis anchor invariants")
}

/// Produces a [`SignedBlock`] via [`Engine::produce_block`] and wraps it with
/// a zero-filled signature. Used to manufacture realistic import inputs for
/// the importer-side tests without re-implementing the production flow.
#[must_use]
pub fn produce_signed_block(engine: &Engine, slot: Slot, validator: ValidatorIndex) -> SignedBlock {
    let produced = engine
        .produce_block(slot, validator)
        .expect("produce_block on genesis engine");
    SignedBlock {
        message: produced.block,
        signature: Bytes4000::new([0; 4000]),
    }
}
