//! Chain service: the single engine writer.
//!
//! # Scope
//!
//! - [`chain::Service`] — wraps [`engine::Engine`] + [`storage::Store`],
//!   exposes async `import_block` / `import_attestation`, and drives the
//!   forkchoice tick loop on a `tokio` background task.
//! - [`chain::ChainSnapshot`] — projection of engine state for hot-read
//!   callers (`runtime/api`, `runtime/p2p`).
//! - [`chain::ChainError`] — infrastructure failures (storage, engine
//!   tick); logical import outcomes stay in the engine's sum types.
//!
//! Sync (#29) and duties (#30) build on top of this crate; they are
//! intentionally absent here.

#![forbid(unsafe_code)]

pub mod chain;

pub use chain::{ChainError, ChainSnapshot, Service};
