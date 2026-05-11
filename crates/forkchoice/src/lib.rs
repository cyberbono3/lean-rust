//! LMD-GHOST fork choice + 4-phase interval ticking.
//!
//! Tier 3: depends on [`protocol`], [`statetransition`], [`config`], [`ssz`],
//! and [`types`]. No `tokio`, `tracing`, `libp2p`, `runtime`, `networking`,
//! or `storage` imports.
//!
//! # Scope (this revision)
//! - [`Store`] — data container for blocks, post-states, and validator
//!   votes; carries the head/safe-target/justified/finalized checkpoints
//!   and the forkchoice clock.
//! - [`Store::from_anchor`] — constructor that seeds the store from a
//!   trusted `(state, anchor_block)` pair.
//! - [`ForkchoiceError`] — crate-level error type.
//!
//! Block insertion, attestation processing, the 4-phase clock, and head
//! resolution land in subsequent issues in this part.

#![forbid(unsafe_code)]

pub use error::ForkchoiceError;
pub use store::{Store, Time};

pub mod error;
pub mod store;

#[cfg(test)]
pub(crate) mod test_fixtures;
