//! Crate-level error type for the storage layer.
//!
//! [`StorageError`] is marked `#[non_exhaustive]` so adapters with real
//! I/O failure modes (disk, `RocksDB`, remote KV) can plug in without
//! bumping the trait signature.

use thiserror::Error;

/// Errors raised by [`crate::Store`] adapters.
#[derive(Debug, Error, PartialEq)]
#[non_exhaustive]
pub enum StorageError {
    /// Backend-specific error wrapping an opaque message. Reserved for
    /// adapters with fallible reads/writes (disk I/O, RPC, remote KV).
    /// [`crate::MemoryStore`] never returns this variant.
    #[error("storage backend error: {message}")]
    Backend {
        /// Free-form message from the underlying backend.
        message: String,
    },
}
