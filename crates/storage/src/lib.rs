//! Narrow persistence layer for the consensus runtime.
//!
//! Depends on [`protocol`] and [`types`] only. No `forkchoice` or
//! runtime imports.
//!
//! # Public surface
//! - [`Store`] — object-safe persistence contract.
//! - [`MemoryStore`] — in-memory adapter for tests and devnet0.
//! - [`HeadInfo`] — `(head, finalized)` checkpoint pair.
//! - [`StorageError`] — concrete error enum.

#![forbid(unsafe_code)]

pub mod error;
pub mod memory;
pub mod store;

pub use error::StorageError;
pub use memory::MemoryStore;
pub use store::{HeadInfo, Store};
