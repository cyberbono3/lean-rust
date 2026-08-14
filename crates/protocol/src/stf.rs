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
//! let mut state = genesis_state(1_700_000_000, Vec::new());
//! state.process_slots(Slot::new(3)).unwrap();
//! assert_eq!(state.slot, Slot::new(3));
//! ```

use ssz::HashTreeRoot;

use crate::{
    block::Block, state::ProtocolConfig, state::State, validator::Validators, BlockHeader,
};

pub use crate::error::StateTransitionError;

/// Builds the slot-0 consensus [`State`] for the given validator registry and
/// chain genesis time.
///
/// The registry is the sole source of the validator-set size — there is no
/// scalar count to keep in step with it. Parameter order mirrors the spec's
/// genesis constructor: `genesis_time` before `validators`.
///
/// The state's `latest_block_header.body_root` commits to the empty
/// [`BlockBody`](crate::BlockBody) (no attestations); all other fields are
/// zero-valued. Lists and bitlists are empty.
///
/// # Example
/// ```
/// use protocol::stf::genesis_state;
/// let s = genesis_state(1_700_000_000, Vec::new());
/// assert_eq!(s.slot.get(), 0);
/// assert_eq!(s.num_validators(), 0);
/// assert_eq!(s.config.genesis_time, 1_700_000_000);
/// ```
#[must_use]
pub fn genesis_state(genesis_time: u64, validators: Validators) -> State {
    // Every field but `config` and `latest_block_header` is the zero/default
    // value; struct-update keeps this in sync as `State` grows fields. The
    // header is not the default one — `BlockHeader::genesis` owns the non-zero
    // empty-body root the genesis anchor invariant depends on.
    State {
        config: ProtocolConfig::new(genesis_time),
        validators,
        latest_block_header: BlockHeader::genesis(),
        ..State::default()
    }
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
/// let state = genesis_state(1_700_000_000, Vec::new());
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
    fn genesis_state_empty_registry_has_no_validators() {
        let state = genesis_state(1_700_000_000, Vec::new());
        assert!(state.validators.is_empty());
        assert_eq!(state.num_validators(), 0);
    }

    #[test]
    fn genesis_state_populates_registry_in_order() {
        let validators = vec![validator(0), validator(1)];
        let state = genesis_state(1_700_000_000, validators.clone());
        assert_eq!(state.validators, validators);
        // The registry IS the validator count.
        assert_eq!(state.num_validators(), 2);
        assert_eq!(state.slot, Slot::ZERO);
    }
}
