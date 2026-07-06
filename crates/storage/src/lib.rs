//! Narrow persistence layer for the consensus runtime.
//!
//! Depends on [`protocol`], [`types`], and [`ssz`] (for adapter value
//! encoding), plus the embedded `redb` key-value store. No `forkchoice` or
//! runtime imports.
//!
//! # Public surface
//! - [`Store`] — object-safe persistence contract.
//! - [`MemoryStore`] — in-memory adapter for tests and fast local runs.
//! - [`RedbStore`] — durable embedded-KV adapter that survives restarts.
//! - [`HeadInfo`] — `(head, finalized)` checkpoint pair.
//! - [`StorageError`] — concrete error enum.

#![forbid(unsafe_code)]

pub mod error;
pub mod memory;
pub mod redb_store;
pub mod store;

pub use error::StorageError;
pub use memory::MemoryStore;
pub use redb_store::RedbStore;
pub use store::{HeadInfo, Store};
