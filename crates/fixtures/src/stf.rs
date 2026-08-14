//! Shared sample-value helpers for the canonical 4-validator parity fixture.

use protocol::stf::genesis_state;
use protocol::{State, Validator, ValidatorIndex, Validators};
use types::PublicKey;

/// Validator-set size of the canonical 4-validator parity fixture.
pub const NUM_VALIDATORS: u64 = 4;
/// Genesis Unix timestamp of the canonical 4-validator parity fixture.
pub const GENESIS_TIME: u64 = 1_700_000_000;

/// Registry of `n` entries with sequential indices — the registry length is
/// the validator-set size.
#[must_use]
pub fn validator_registry(n: u64) -> Validators {
    (0..n)
        .map(|i| Validator::new(PublicKey::default(), ValidatorIndex::new(i)))
        .collect()
}

/// Genesis state matching the canonical 4-validator wire-parity fixture.
#[must_use]
pub fn genesis_4val() -> State {
    genesis_state(GENESIS_TIME, validator_registry(NUM_VALIDATORS))
}
