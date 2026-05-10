//! SSZ decode entry point.
//!
//! [`decode`] is a thin wrapper over [`eth_ssz::Decode::from_ssz_bytes`]
//! that maps the upstream [`eth_ssz::DecodeError`] into [`SszError`] so
//! downstream crates depend only on the [`ssz`](crate) facade.

use eth_ssz::Decode;

use crate::error::SszError;

/// Decodes `data` into a value of type `T`.
///
/// # Errors
/// Returns [`SszError::Decode`] when the upstream
/// [`eth_ssz::Decode::from_ssz_bytes`] rejects the input. The wrapped
/// [`eth_ssz::DecodeError`] is preserved verbatim and accessible via the
/// [`std::error::Error::source`] chain (or
/// [`SszError::Decode::source`](crate::error::DecodeErrorAdapter)).
///
/// # Example
/// ```
/// use ssz::{decode, encode, SszError};
/// # fn main() -> Result<(), SszError> {
/// let bytes = encode(&42_u64);
/// let value: u64 = decode(&bytes)?;
/// assert_eq!(value, 42);
/// # Ok(())
/// # }
/// ```
pub fn decode<T: Decode>(data: &[u8]) -> Result<T, SszError> {
    T::from_ssz_bytes(data).map_err(SszError::from)
}
