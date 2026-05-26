//! Chain service: the single engine writer.
//!
//! # Scope
//!
//! - [`chain::Service`] — wraps [`engine::Engine`] + [`storage::Store`],
//!   exposes async `import_block` / `import_attestation` /
//!   `produce_block` / `produce_attestation`, and drives the
//!   forkchoice tick loop on a `tokio` background task.
//! - [`chain::ChainSnapshot`] — projection of engine state for
//!   hot-read callers (`runtime/api`, `runtime/p2p`).
//! - [`chain::ChainError`] — infrastructure failures (storage,
//!   engine invariant violations, engine forkchoice / state-
//!   transition errors); logical import outcomes stay in the engine's
//!   sum types.
//!
//! The sync backfill loop lives in the sibling `lean-sync` crate;
//! the proposer / attester scheduler lives in `runtime-duties`. Each
//! drives this crate's [`Service`] through a narrow async port whose
//! adapter `impl` lives in the consumer crate (orphan rule).

#![forbid(unsafe_code)]

pub mod chain;

pub use chain::{ChainError, ChainSnapshot, Service};
