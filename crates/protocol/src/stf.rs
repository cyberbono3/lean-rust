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
    validator::ValidatorIndex, BlockHeader,
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
        justifications_roots: Vec::new(),
        justifications_validators: Bitlist::new(),
    }
}
