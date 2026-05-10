//! Pure consensus state machine — no async, no I/O, no tracing.
//!
//! Tier 3: depends on [`protocol`] (consensus types) plus [`types`], [`ssz`],
//! and [`config`]. No `tokio`, `tracing`, `libp2p`, `runtime`, `networking`,
//! or `storage` imports.
//!
//! # Scope (this revision)
//! - [`genesis_state`] — slot-0 [`protocol::State`] for a given validator
//!   set + genesis time.
//! - [`process_slot`] — per-slot housekeeping (caches the pre-block state
//!   root into the latest header on the slot following an accepted block).
//! - [`process_slots`] — advances the state slot-by-slot up to a future
//!   target slot.
//! - [`StateTransitionError`] — crate-level error enum.
//!
//! # Example
//! ```
//! use protocol::Slot;
//! use statetransition::{genesis_state, process_slots};
//!
//! let state = genesis_state(4, 1_700_000_000);
//! let advanced = process_slots(&state, Slot::new(3)).unwrap();
//! assert_eq!(advanced.slot, Slot::new(3));
//! ```

#![forbid(unsafe_code)]

pub mod error;
pub mod genesis;
pub mod slots;

mod helpers;

pub use error::StateTransitionError;
pub use genesis::genesis_state;
pub use slots::{process_slot, process_slots};
