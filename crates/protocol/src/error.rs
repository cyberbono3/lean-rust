//! Crate-level error type for the consensus protocol domain.
//!
//! [`ProtocolError`] forwards SSZ codec failures from the [`ssz`] facade
//! and surfaces invariant breaks (e.g. zero-validator proposer lookups)
//! without panicking.

use ssz::SszError;
use thiserror::Error;

/// Errors raised by [`crate`] domain operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProtocolError {
    /// SSZ encode/decode of a domain type failed.
    #[error(transparent)]
    Ssz(#[from] SszError),

    /// A domain invariant was violated (e.g. proposer lookup with zero
    /// validators, slot arithmetic overflow).
    #[error("invariant violation in {context}: {reason}")]
    Invariant {
        /// Static label identifying the call site.
        context: &'static str,
        /// Human-readable reason.
        reason: &'static str,
    },
}
