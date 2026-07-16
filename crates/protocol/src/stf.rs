//! State-transition function — pure (no async, no I/O).
//!
//! # Scope
//! - [`genesis_state`] — slot-0 [`State`] for a given validator-set size and
//!   chain genesis time.
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
use types::{Bitlist, Bytes32};

use crate::{
    block::BlockBody, checkpoint::Checkpoint, slot::Slot, state::ProtocolConfig, state::State,
    validator::ValidatorIndex, validator::Validators, BlockHeader,
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
    let body_root: Bytes32 = BlockBody::default().hash_tree_root().into();

    State {
        config: ProtocolConfig {
            num_validators,
            genesis_time,
        },
        slot: Slot::ZERO,
        latest_block_header: BlockHeader {
            slot: Slot::ZERO,
            proposer_index: ValidatorIndex::new(0),
            parent_root: Bytes32::zero(),
            state_root: Bytes32::zero(),
            body_root,
        },
        latest_justified: Checkpoint::default(),
        latest_finalized: Checkpoint::default(),
        historical_block_hashes: Vec::new(),
        justified_slots: Bitlist::new(),
        validators: Vec::new(),
        justifications_roots: Vec::new(),
        justifications_validators: Bitlist::new(),
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::validator::Validator;
    use types::PublicKey;

    fn validator(seed: u8) -> Validator {
        Validator {
            pubkey: PublicKey::new([seed; PublicKey::LEN]),
            index: ValidatorIndex::new(u64::from(seed)),
        }
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
