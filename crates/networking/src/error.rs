//! Crate-level error type for the networking layer.

use thiserror::Error;

/// Errors raised by the networking codec + validation surface.
#[derive(Debug, Error, PartialEq)]
#[non_exhaustive]
pub enum NetworkingError {
    /// `BlocksByRootRequest::new` was called with more roots than allowed.
    #[error("blocks_by_root request length {len} exceeds max {max}")]
    RequestTooLarge {
        /// Length of the rejected input.
        len: usize,
        /// Inclusive upper bound.
        max: usize,
    },

    /// `BlocksByRootResponse::new` was called with more blocks than allowed.
    #[error("blocks_by_root response length {len} exceeds max {max}")]
    ResponseTooLarge {
        /// Length of the rejected input.
        len: usize,
        /// Inclusive upper bound.
        max: usize,
    },

    /// SSZ codec failure forwarded from the `ssz` crate.
    #[error(transparent)]
    Ssz(#[from] ssz::SszError),
}
