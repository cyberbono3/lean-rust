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
mod genesis_pubkeys;
mod proposer;
mod validators;

pub use config::{
    Config, GenesisTimeUnix, ValidatorGroup, ValidatorsPath, DEFAULT_VALIDATORS_PATH,
    DEFAULT_VALIDATOR_GROUP,
};
pub use error::{DutiesError, DutiesResult};
pub use genesis_pubkeys::GenesisRegistry;
pub use proposer::LocalProposers;
pub use validators::ValidatorAssignments;
