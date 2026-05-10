//! SSZ encode entry point.
//!
//! [`encode`] is a thin wrapper over [`eth_ssz::Encode::as_ssz_bytes`]
//! kept here so that downstream crates depend on the [`ssz`](crate)
//! facade and never on `ethereum_ssz` directly.

use eth_ssz::Encode;

/// Encodes `value` into its SSZ-serialized byte representation.
///
/// Equivalent to `value.as_ssz_bytes()`. Encoding is statically infallible —
/// failures arise only on the decode path (see
/// [`decode`](crate::decode::decode)).
///
/// # Example
/// ```
/// use ssz::encode;
/// assert_eq!(encode(&42_u64), 42_u64.to_le_bytes().to_vec());
/// ```
#[must_use]
pub fn encode<T: Encode>(value: &T) -> Vec<u8> {
    value.as_ssz_bytes()
}
