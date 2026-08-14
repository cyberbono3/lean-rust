//! Test-only fixtures for the `engine` module.
//!
//! Builds anchor state/block pairs from `protocol`'s public surface so the
//! engine tests do not depend on forkchoice's private `test_fixtures` module.
//! Shape mirrors `forkchoice::test_fixtures::genesis_anchor` but uses only
//! re-exported `protocol` types.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc
)]

use protocol::stf::{genesis_anchor_block, genesis_state};
use protocol::{
    Attestation, Block, BlockSignatures, BlockWithAttestation, SignedBlockWithAttestation, Slot,
    State, Validator, ValidatorIndex, Validators,
};
use types::PublicKey;

use super::handle::Engine;

/// Validator-count constant used by the import / produce tests. Four matches
/// the forkchoice production-test default and keeps the round-robin proposer
/// schedule deterministic across slots.
pub const ENGINE_VALIDATORS: u64 = 4;

const GENESIS_TIME: u64 = 1_700_000_000;

/// Anchors `state` into an [`Engine`]. Single call site of
/// [`Engine::from_anchor`] across the fixtures.
fn engine_from_state(state: State) -> Engine {
    let block = genesis_anchor_block(&state);
    Engine::from_anchor(state, block).expect("genesis anchor invariants")
}

/// A registry of `num_validators` entries with default pubkeys and sequential
/// indices. Plain [`genesis_state`] leaves `State::validators` empty, so tests
/// that resolve `validator_id` build the registry from here.
#[must_use]
pub fn validator_registry(num_validators: u64) -> Validators {
    (0..num_validators)
        .map(|i| Validator::new(PublicKey::default(), ValidatorIndex::new(i)))
        .collect()
}

/// Returns a spec-compliant `(state, anchor_block)` pair such that
/// `anchor_block.state_root == state.hash_tree_root()` and `parent_root` is
/// the zero sentinel. Eligible input to [`Engine::from_anchor`].
#[must_use]
pub fn anchor_pair(num_validators: u64) -> (State, Block) {
    let state = genesis_state(GENESIS_TIME, validator_registry(num_validators));
    let block = genesis_anchor_block(&state);
    (state, block)
}

/// Builds an [`Engine`] anchored at genesis with a populated
/// [`validator_registry`], so `validator_id` lookups and proposer selection
/// both resolve against a registry of the declared size.
#[must_use]
pub fn engine_at_genesis(num_validators: u64) -> Engine {
    engine_from_state(genesis_state(
        GENESIS_TIME,
        validator_registry(num_validators),
    ))
}

/// Produces a [`SignedBlockWithAttestation`] via [`Engine::produce_block`] and wraps it with
/// a zero-filled signature. Used to manufacture realistic import inputs for
/// the importer-side tests without re-implementing the production flow.
#[must_use]
pub fn produce_signed_block(
    engine: &Engine,
    slot: Slot,
    validator: ValidatorIndex,
) -> SignedBlockWithAttestation {
    let produced = engine
        .produce_block(slot, validator)
        .expect("produce_block on genesis engine");
    SignedBlockWithAttestation {
        message: BlockWithAttestation {
            block: produced.block,
            proposer_attestation: Attestation::default(),
        },
        signature: BlockSignatures::default(),
    }
}
