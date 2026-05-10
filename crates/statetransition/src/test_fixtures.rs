//! Shared sample-value helpers for `cfg(test)` use across the crate.

#![allow(dead_code)]

use protocol::State;

use crate::genesis::genesis_state;

/// Validator-set size of the canonical 4-validator parity fixture.
pub(crate) const NUM_VALIDATORS: u64 = 4;
/// Genesis Unix timestamp of the canonical 4-validator parity fixture.
pub(crate) const GENESIS_TIME: u64 = 1_700_000_000;

/// Genesis state matching the canonical 4-validator wire-parity fixture.
pub(crate) fn genesis_4val() -> State {
    genesis_state(NUM_VALIDATORS, GENESIS_TIME)
}
