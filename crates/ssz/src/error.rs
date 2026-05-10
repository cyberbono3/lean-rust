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
/// Currently only the decode path can fail; encoding via the upstream
/// [`Encode`](eth_ssz::Encode) trait is statically infallible.
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
}

impl From<DecodeError> for SszError {
    fn from(err: DecodeError) -> Self {
        Self::Decode {
            source: DecodeErrorAdapter(err),
        }
    }
}
