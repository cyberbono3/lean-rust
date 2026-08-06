//! State-transition function — pure (no async, no I/O).
//!
//! # Scope
//! - [`genesis_state`] — slot-0 [`State`] for a given validator-set size and
//!   chain genesis time.
//! - [`genesis_anchor_block`] — the matching slot-0 anchor [`Block`], the one
//!   home for the anchor shape that forkchoice and runtime fixtures both need.
//! - The slot-processing methods (`process_slot`, `process_slots`) live as
//!   inherent methods on [`State`]; this module re-exports
//!   [`StateTransitionError`] for convenience.
//!
//! # Example
//! ```
//! use protocol::{stf::genesis_state, Slot};
//!
//! let mut state = genesis_state(4, 1_700_000_000);
//! state.process_slots(Slot::new(3)).unwrap();
//! assert_eq!(state.slot, Slot::new(3));
//! ```

use ssz::HashTreeRoot;
use types::Bytes32;

use crate::{
    block::Block, block::BlockBody, state::ProtocolConfig, state::State, validator::Validators,
    BlockHeader,
};

pub use crate::error::StateTransitionError;

/// Builds the slot-0 consensus [`State`] for the given validator-set size and
/// chain genesis time.
///
/// The state's `latest_block_header.body_root` commits to the empty
/// [`BlockBody`] (no attestations); all other fields are zero-valued. Lists
/// and bitlists are empty.
///
/// # Example
/// ```
/// use protocol::stf::genesis_state;
/// let s = genesis_state(4, 1_700_000_000);
/// assert_eq!(s.slot.get(), 0);
/// assert_eq!(s.config.num_validators, 4);
/// assert_eq!(s.config.genesis_time, 1_700_000_000);
/// ```
#[must_use]
pub fn genesis_state(num_validators: u64, genesis_time: u64) -> State {
    // Every field but `config` and the header `body_root` is the zero/default
    // value; struct-update keeps this in sync as `State` grows fields. Note
    // `body_root` must stay explicit — the empty `BlockBody` root is non-zero,
    // and the genesis anchor invariant depends on it.
    let body_root: Bytes32 = BlockBody::default().hash_tree_root().into();

    State {
        config: ProtocolConfig {
            num_validators,
            genesis_time,
        },
        latest_block_header: BlockHeader {
            body_root,
            ..BlockHeader::default()
        },
        ..State::default()
    }
}

/// Builds the slot-0 consensus [`State`] with a pre-populated validator
/// registry.
///
/// Delegates to [`genesis_state`] for the empty-registry shape, then installs
/// `validators`. Keeps [`genesis_state`]'s signature stable for existing
/// callers; genesis keygen (a later part) supplies the real `Bytes52` pubkeys.
///
/// # Preconditions
/// An empty registry is the valid pre-keygen shape — it is what [`genesis_state`]
/// produces and what this delegates to. When `validators` is non-empty its
/// length should equal `num_validators`; this constructor does not enforce that
/// coupling (the registry and `config.num_validators` are wired together by the
/// genesis keygen part). A non-empty registry whose length disagrees with
/// `num_validators` produces a `State` whose `process_attestations` validator
/// bound (`config.num_validators`) disagrees with the registry size.
///
/// # Example
/// ```
/// use protocol::stf::{genesis_state, genesis_state_with_validators};
/// let s = genesis_state_with_validators(4, 1_700_000_000, Vec::new());
/// assert_eq!(s, genesis_state(4, 1_700_000_000));
/// ```
#[must_use]
pub fn genesis_state_with_validators(
    num_validators: u64,
    genesis_time: u64,
    validators: Validators,
) -> State {
    let mut state = genesis_state(num_validators, genesis_time);
    state.validators = validators;
    state
}

/// Builds the genesis anchor [`Block`] for `state`: the slot-0, zero-parented
/// block whose `state_root` commits to `state`.
///
/// Satisfies the anchor invariant every store constructor checks —
/// `block.state_root == state.hash_tree_root()` — so the pair can seed a
/// forkchoice store directly.
///
/// `state_root` is the ONLY non-default field: a genesis anchor is slot 0
/// (no prior slot), proposer 0, `parent_root` zero (no prior block), and an
/// empty body. Struct-update keeps that in sync as [`Block`] grows fields,
/// matching [`genesis_state`]'s construction.
///
/// # Example
/// ```
/// use protocol::stf::{genesis_anchor_block, genesis_state};
/// use ssz::HashTreeRoot;
///
/// let state = genesis_state(4, 1_700_000_000);
/// let block = genesis_anchor_block(&state);
/// assert_eq!(block.slot.get(), 0);
/// assert_eq!(block.state_root.0, state.hash_tree_root());
/// ```
#[must_use]
pub fn genesis_anchor_block(state: &State) -> Block {
    Block {
        state_root: state.hash_tree_root().into(),
        ..Block::default()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::slot::Slot;
    use crate::validator::{Validator, ValidatorIndex};
    use types::PublicKey;

    fn validator(seed: u8) -> Validator {
        Validator::new(
            PublicKey::new([seed; PublicKey::LEN]),
            ValidatorIndex::new(u64::from(seed)),
        )
    }

    #[test]
    fn genesis_state_registry_is_empty() {
        assert!(genesis_state(4, 1_700_000_000).validators.is_empty());
    }

    #[test]
    fn genesis_state_populates_registry_in_order() {
        let validators = vec![validator(0), validator(1)];
        let state = genesis_state_with_validators(4, 1_700_000_000, validators.clone());
        assert_eq!(state.validators, validators);
        // Only the registry differs from the empty-registry genesis state.
        assert_eq!(state.slot, Slot::ZERO);
        assert_eq!(state.config.num_validators, 4);
    }
}
