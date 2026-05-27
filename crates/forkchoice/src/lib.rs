//! LMD-GHOST fork choice + 4-phase interval ticking.
//!
//! Tier 3: depends on [`protocol`] (which owns `stf`), [`config`], [`ssz`],
//! and [`types`]. No `tokio`, `tracing`, `libp2p`, `runtime`, `networking`,
//! or `storage` imports.
//!
//! # Scope (this revision)
//! - [`Store`] — data container for blocks, post-states, and validator
//!   votes; carries the head/safe-target/justified/finalized checkpoints
//!   and the forkchoice clock.
//! - [`Store::from_anchor`] — constructor that seeds the store from a
//!   trusted `(state, anchor_block)` pair.
//! - [`Store::tick_interval`] — advances the clock one interval and
//!   dispatches the spec phase hook.
//! - [`Time`] / [`Phase`] — typed clock value + 4-phase classification.
//! - [`ForkchoiceError`] — crate-level error type.
//!
//! Phase-hook bodies (attestation processing, head resolution) land in
//! subsequent issues in this part.

#![forbid(unsafe_code)]

pub use error::ForkchoiceError;
pub use store::Store;
pub use time::{Phase, Time};

pub mod error;
pub mod helpers;
pub mod production;
pub mod store;
pub mod time;

pub use production::{ProducedBlock, ProducedVote};

#[cfg(test)]
pub(crate) mod test_fixtures;
