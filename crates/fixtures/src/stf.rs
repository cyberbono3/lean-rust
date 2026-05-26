//! Shared sample-value helpers for the canonical 4-validator parity fixture.

use protocol::State;
use statetransition::genesis_state;

/// Validator-set size of the canonical 4-validator parity fixture.
pub const NUM_VALIDATORS: u64 = 4;
/// Genesis Unix timestamp of the canonical 4-validator parity fixture.
pub const GENESIS_TIME: u64 = 1_700_000_000;

/// Genesis state matching the canonical 4-validator wire-parity fixture.
#[must_use]
pub fn genesis_4val() -> State {
    genesis_state(NUM_VALIDATORS, GENESIS_TIME)
}
