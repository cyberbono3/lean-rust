//! Duties service — the narrow devnet0 validator-duty scheduler.
//!
//! Loads local validator assignments
//! from YAML, schedules proposers at slot boundaries and attesters at
//! the `vote_due_bps` deadline. Production goes through the [`Chain`]
//! port (satisfied by [`lean_chain::Service`] via an adapter in
//! [`chain_adapter`]); publish goes through the [`Publisher`] port
//! whose impl lives in `node` per Decision 7 (Dependency Inversion).
//! No `lean-p2p-host` import lives in this crate.
//!
//! Out of scope (deliberate): aggregator duties,
//! direct forkchoice mutation, optional metrics hooks.

#![forbid(unsafe_code)]

mod chain_adapter;
mod config;
mod error;
mod ports;
mod proposer;
mod service;
mod validators;
mod wiring;

pub use config::{
    Config, GenesisTimeUnix, ValidatorGroup, ValidatorsPath, DEFAULT_VALIDATORS_PATH,
    DEFAULT_VALIDATOR_GROUP,
};
pub use error::{DutiesError, DutiesResult};
pub use ports::{Chain, PublishError, Publisher};
pub use proposer::LocalProposers;
pub use service::Service;
pub use validators::ValidatorAssignments;
