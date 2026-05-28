//! Consensus execution boundary: wraps the forkchoice store and sequences
//! the import / produce flows without reaching into runtime, networking, or
//! storage layers.
//!
//! Pure (no async, no I/O) — depends on [`types`], [`protocol`],
//! [`forkchoice`], [`ssz`], plus `parking_lot` and `thiserror`.
//!
//! # Scope
//! - [`Engine`] — `Send + Sync + Clone` handle around an `Arc<Mutex<Store>>`.
//! - [`Engine::import_block`] / [`Engine::import_attestation`] — structured
//!   import flows with sum-type outcomes ([`BlockImportResult`],
//!   [`AttestationImportResult`]).
//! - [`Engine::produce_block`] / [`Engine::produce_attestation_vote`] —
//!   thin pass-throughs to the underlying store.
//! - [`EngineError`] — `thiserror` enum that funnels forkchoice and
//!   state-transition failures.

mod error;
mod handle;
mod importer;
mod results;

#[cfg(any(test, feature = "test-fixtures"))]
pub mod test_fixtures;

pub use error::EngineError;
pub use handle::Engine;
pub(crate) use handle::PersistPlan;
pub use results::{AttestationImportResult, BlockImportResult};
