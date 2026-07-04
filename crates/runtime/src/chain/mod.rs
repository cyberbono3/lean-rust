//! Chain service: the single engine writer.
//!
//! # Scope
//!
//! - [`chain::Service`] — wraps [`crate::chain::engine::Engine`] + [`storage::Store`],
//!   exposes async `import_block` / `import_attestation` /
//!   `produce_block` / `produce_attestation`, and drives the
//!   forkchoice tick loop on a `tokio` background task.
//! - [`chain::ChainSnapshot`] — projection of engine state for
//!   hot-read callers (`lean-api`, `lean-p2p-host`).
//! - [`chain::ChainError`] — infrastructure failures (storage,
//!   engine invariant violations, engine forkchoice / state-
//!   transition errors); logical import outcomes stay in the engine's
//!   sum types.
//!
//! The sync backfill loop lives in the sibling `lean-sync` crate;
//! the proposer / attester scheduler lives in `lean-duties`. Each
//! drives this crate's [`Service`] through a narrow async port whose
//! adapter `impl` lives in the consumer crate (orphan rule).

// The chain service lives in a `chain` submodule of the `chain` module (a
// relic of the former standalone `lean-chain` crate); the re-export below is
// the canonical path (`runtime::chain::Service`).
#[allow(clippy::module_inception)]
pub mod chain;
pub mod engine;

pub use chain::{ChainError, ChainSnapshot, Service};
