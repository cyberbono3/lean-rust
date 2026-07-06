//! Chain service: the single engine writer.
//!
//! # Scope
//!
//! - [`chain::Service`] — wraps [`crate::chain::engine::Engine`] + [`storage::Store`],
//!   exposes async `import_block` / `import_attestation` /
//!   `produce_block` / `produce_attestation` / `tick_interval`, each
//!   funnelling through the single engine mutex. It owns no background
//!   task: the self-driving consensus loop (`node` crate) drives the
//!   forkchoice clock by calling `tick_interval` once per interval.
//! - [`chain::ChainSnapshot`] — projection of engine state for
//!   hot-read callers (`api`, `p2p`).
//! - [`chain::ChainError`] — infrastructure failures (storage,
//!   engine invariant violations, engine forkchoice / state-
//!   transition errors); logical import outcomes stay in the engine's
//!   sum types.
//!
//! The sync backfill loop lives in the sibling `sync` module. Proposer /
//! attester scheduling and forkchoice tick-driving moved into the
//! self-driving consensus loop in the `node` crate; this module stays a
//! passive engine funnel that all of them drive through its async API.

// The chain service lives in a `chain` submodule of the `chain` module (a
// relic of the former standalone `lean-chain` crate); the re-export below is
// the canonical path (`runtime::chain::Service`).
#[allow(clippy::module_inception)]
pub mod chain;
pub mod engine;

pub use chain::{ChainError, ChainSnapshot, Service};
