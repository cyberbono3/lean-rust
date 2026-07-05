//! Duties service — the narrow devnet0 validator-duty scheduler.
//!
//! Loads local validator assignments
//! from YAML, schedules proposers at slot boundaries and attesters at
//! the `vote_due_bps` deadline. Production calls the concrete
//! [`crate::chain::Service`] directly; publish goes through the concrete
//! [`Publisher`] over the running [`crate::p2p::P2pService`] — the
//! former `Chain`/`Publisher` port traits collapsed to concrete types.
//!
//! Out of scope (deliberate): aggregator duties,
//! direct forkchoice mutation, optional metrics hooks.

mod config;
mod error;
mod proposer;
mod publisher;
mod service;
mod validators;
mod wiring;

pub use config::{
    Config, GenesisTimeUnix, ValidatorGroup, ValidatorsPath, DEFAULT_VALIDATORS_PATH,
    DEFAULT_VALIDATOR_GROUP,
};
pub use error::{DutiesError, DutiesResult};
pub use proposer::LocalProposers;
pub use publisher::{PublishError, Publisher};
pub use service::Service;
pub use validators::ValidatorAssignments;
