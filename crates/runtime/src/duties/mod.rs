//! Duties helpers — validator-assignment loading + local proposer lookup.
//!
//! The devnet0 duty scheduler moved into the self-driving consensus loop
//! (`node` crate), which owns the genesis-anchored interval loop and drives
//! propose/attest inline. This module now provides the pure helpers that
//! loop consumes: [`ValidatorAssignments`] (YAML loader), [`LocalProposers`]
//! (O(1) proposer lookup over the local set), and [`Config`] (validated
//! paths + genesis time). Production + publish happen in the consensus loop
//! directly against [`crate::chain::Service`] and [`crate::p2p::P2pService`].
//!
//! Out of scope (deliberate): aggregator duties, direct forkchoice mutation.

mod config;
mod error;
// Part 15: until Part 3 wires `sign_own_duty` into `produce_*`, the guard cluster
// is reachable only from `#[cfg(test)]`, so the non-test `--lib` build would fail
// `-D warnings` with `dead_code`. `allow` (NOT `expect` — the cluster IS used in
// the cfg(test) build, so `expect(dead_code)` would misfire) keeps standalone Part
// 2 green. REMOVE this attribute in Part 3 once the production sign sites call it.
#[allow(dead_code)]
pub(crate) mod ots_signer;
mod proposer;
mod validators;

pub use config::{
    Config, GenesisTimeUnix, ValidatorGroup, ValidatorsPath, DEFAULT_VALIDATORS_PATH,
    DEFAULT_VALIDATOR_GROUP,
};
pub use error::{DutiesError, DutiesResult};
pub use proposer::LocalProposers;
pub use validators::ValidatorAssignments;
