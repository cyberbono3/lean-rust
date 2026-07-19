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
mod signer;
#[cfg(any(test, feature = "test-fixtures"))]
pub mod test_fixtures;
mod validators;

pub use config::{
    Config, GenesisTimeUnix, ValidatorGroup, ValidatorsPath, DEFAULT_VALIDATORS_PATH,
    DEFAULT_VALIDATOR_GROUP,
};
pub use error::{DutiesError, DutiesResult};
pub use genesis_pubkeys::GenesisRegistry;
pub use proposer::LocalProposers;
// `LocalSigner` / its errors are `pub`: the composition root (`node`) builds the
// signer and passes it to the public `chain::Service::new`, so the type appears
// in a public signature. `sign_attestation` itself is `pub(crate)` — only the
// chain service calls it.
// `validator_secret_path` is `pub` so the offline keygen (`lean-cli`, which
// depends on this crate) writes the same file names this loader reads.
pub use signer::{validator_secret_path, LocalSigner, SignError, SignerLoadError};
pub use validators::ValidatorAssignments;
