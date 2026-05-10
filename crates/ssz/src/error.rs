//! Errors raised by the [`ssz`](crate) facade.
//!
//! [`SszError`] wraps the upstream [`eth_ssz::DecodeError`] for failed
//! decodes. Because the upstream type does not implement
//! [`std::error::Error`], a thin [`DecodeErrorAdapter`] newtype is used so
//! that `#[source]` (and the [`std::error::Error::source`] chain) work as
//! expected for downstream callers.

use eth_ssz::DecodeError;
use thiserror::Error;

/// Adapter that wraps [`eth_ssz::DecodeError`] and implements
/// [`std::error::Error`] (the upstream type only derives `Debug`).
///
/// Callers that need the underlying error can recover it via the public
/// `0` field or by downcasting through [`std::error::Error::source`]:
///
/// ```
/// use ssz::{DecodeError, DecodeErrorAdapter, SszError};
/// use std::error::Error;
///
/// let err: SszError = DecodeError::ZeroLengthItem.into();
/// let source = err.source().unwrap();
/// let adapter = source.downcast_ref::<DecodeErrorAdapter>().unwrap();
/// assert_eq!(adapter.0, DecodeError::ZeroLengthItem);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct DecodeErrorAdapter(pub DecodeError);

impl core::fmt::Display for DecodeErrorAdapter {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

impl std::error::Error for DecodeErrorAdapter {}

/// Errors raised by the [`ssz`](crate) facade.
///
/// Decoding errors carry the upstream [`DecodeError`]; merkleization errors
/// surface invalid input shapes for the helpers in [`crate::merkleize`].
#[derive(Debug, Clone, PartialEq, Error)]
#[non_exhaustive]
pub enum SszError {
    /// SSZ decoding failed. Carries the upstream [`DecodeError`] via
    /// [`DecodeErrorAdapter`] so the [`std::error::Error::source`] chain
    /// surfaces it.
    #[error("ssz decode failed: {source}")]
    Decode {
        /// Upstream decode error wrapped in an [`std::error::Error`] adapter.
        #[source]
        source: DecodeErrorAdapter,
    },

    /// [`merkleize_progressive`](crate::merkleize::merkleize_progressive)
    /// was called with `num_leaves == 0`.
    #[error("invalid progressive merkle leaf width: got {got}")]
    InvalidNumLeaves {
        /// The non-positive width that was supplied.
        got: usize,
    },

    /// [`merkleize_with_limit`](crate::merkleize::merkleize_with_limit) was
    /// called with more chunks than the declared `limit`.
    #[error("merkle input exceeds limit: got {got} chunks, limit {limit}")]
    InputExceedsLimit {
        /// Number of chunks supplied.
        got: usize,
        /// Maximum number of chunks permitted.
        limit: usize,
    },
}

impl From<DecodeError> for SszError {
    fn from(err: DecodeError) -> Self {
        Self::Decode {
            source: DecodeErrorAdapter(err),
        }
    }
}
