//! Consensus execution boundary: wraps the forkchoice store and sequences
//! the import / produce flows without reaching into runtime, networking, or
//! storage layers.
//!
//! Tier 4: depends on [`types`], [`protocol`], [`forkchoice`], [`ssz`],
//! plus `parking_lot` and `thiserror`. No `tokio`, `tracing`, `libp2p`,
//! `runtime`, `networking`, or `storage` imports.
//!
//! # Scope (this revision)
//! - [`Engine`] — `Send + Sync + Clone` handle around an `Arc<Mutex<Store>>`.
//! - [`Engine::import_block`] / [`Engine::import_attestation`] — structured
//!   import flows with sum-type outcomes ([`BlockImportResult`],
//!   [`AttestationImportResult`]).
//! - [`Engine::produce_block`] / [`Engine::produce_attestation_vote`] —
//!   thin pass-throughs to the underlying store.
//! - [`EngineError`] — `thiserror` enum that funnels forkchoice and
//!   state-transition failures.

#![forbid(unsafe_code)]

mod engine;
mod error;
mod importer;
mod results;

#[cfg(any(test, feature = "test-fixtures"))]
pub mod test_fixtures;

pub use engine::Engine;
pub use error::EngineError;
pub use results::{AttestationImportResult, BlockImportResult};
