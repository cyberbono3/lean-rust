//! Pure consensus state machine — no async, no I/O, no tracing.
//!
//! Tier 3: depends on [`protocol`] (consensus types) plus [`types`], [`ssz`],
//! and [`config`]. No `tokio`, `tracing`, `libp2p`, `runtime`, `networking`,
//! or `storage` imports.
//!
//! # Scope (this revision)
//! - [`genesis_state`] — slot-0 [`protocol::State`] for a given validator
//!   set + genesis time.
//! - The slot-processing methods (`process_slot`, `process_slots`) live as
//!   inherent methods on [`protocol::State`]; this crate re-exports
//!   [`StateTransitionError`] for convenience.
//!
//! # Example
//! ```
//! use protocol::Slot;
//! use statetransition::genesis_state;
//!
//! let mut state = genesis_state(4, 1_700_000_000);
//! state.process_slots(Slot::new(3)).unwrap();
//! assert_eq!(state.slot, Slot::new(3));
//! ```

#![forbid(unsafe_code)]

pub mod genesis;

pub use genesis::genesis_state;
pub use protocol::StateTransitionError;
