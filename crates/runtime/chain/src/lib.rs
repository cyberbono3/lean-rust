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
//! - [`sync::Loop`] — peer-driven `BlocksByRoot` backfill orchestrator.
//!   Declares the [`sync::Chain`], [`sync::Network`], and
//!   [`sync::PeerEventProvider`] port traits (Decision 7); the chain
//!   port is implemented for [`Service`] in this crate.
//!
//! Duties (#30) builds on top of this crate.

#![forbid(unsafe_code)]

pub mod chain;
pub mod sync;

pub use chain::{ChainError, ChainSnapshot, Service};
pub use sync::{
    Chain as SyncChain, Config as SyncConfig, Loop as SyncLoop, Network as SyncNetwork,
    PeerEventProvider, PeerId, SyncError, DEFAULT_MAX_SYNC_DEPTH,
};
