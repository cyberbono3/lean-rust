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
mod ots_signer;
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
// `AttestationSigner` is the seam `chain::Service` depends on, so it appears in
// the public `Service::with_signer` signature; `sign_attestation` is public with
// it (a trait method cannot be narrower than its trait). `LocalSigner` / its
// errors are `pub` because the composition root (`node`) builds the production
// implementation and passes it in.
// `validator_secret_path` is `pub` so the offline keygen (`lean-cli`, which
// depends on this crate) writes the same file names this loader reads.
// `OtsSigner` is `pub` because the composition root (`node`) wraps the
// production `LocalSigner` in the durable one-time-signature guard before
// injecting it into the chain service; `PersistableSigner` appears in
// `OtsSigner::new`'s signature (the requirement the guard places on its inner
// signer).
pub use ots_signer::OtsSigner;
pub use signer::{
    validator_secret_path, AttestationSigner, LocalSigner, PersistableSigner, SignError,
    SignerLoadError,
};
pub use validators::ValidatorAssignments;
