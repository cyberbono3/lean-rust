//! Error types for the `types` crate.

use thiserror::Error;

/// Errors raised by primitive `types`-crate operations.
///
/// Every fallible function in this crate returns `Result<T, TypesError>`.
///
/// # Example
/// ```
/// use types::{decode_u64_le, TypesError};
/// let err = decode_u64_le(&[0_u8; 4]).unwrap_err();
/// assert!(matches!(err, TypesError::InvalidByteLength { want: 8, got: 4, .. }));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TypesError {
    /// A fixed-width SSZ decode received the wrong number of bytes.
    #[error("{type_name} expects exactly {want} bytes, got {got}")]
    InvalidByteLength {
        /// Human-readable name of the target type (e.g. `"u64"`).
        type_name: &'static str,
        /// Required byte length.
        want: usize,
        /// Actual byte length received.
        got: usize,
    },

    /// SSZ Boolean decode received a byte that was neither `0x00` nor `0x01`.
    #[error("invalid SSZ boolean byte: 0x{value:02x} (must be 0x00 or 0x01)")]
    InvalidBooleanByte {
        /// The non-canonical byte value received.
        value: u8,
    },

    /// [`BasisPoint::new`](crate::BasisPoint::new) received a value outside `0..=10_000`.
    #[error("BasisPoint value {value} exceeds the maximum of 10_000")]
    BasisPointOutOfRange {
        /// Value that violated the invariant.
        value: u64,
    },

    /// [`ByteList::try_new`](crate::ByteList::try_new) or
    /// [`ByteListLimit::try_new`](crate::ByteListLimit::try_new) received
    /// `bytes` longer than the declared `limit`.
    #[error("byte list length {got} exceeds limit {limit}")]
    ByteListLimitExceeded {
        /// Maximum byte length allowed.
        limit: usize,
        /// Actual byte length received.
        got: usize,
    },

    /// [`Bitlist::set`](crate::Bitlist::set) /
    /// [`Bitlist::with_length`](crate::Bitlist::with_length) received an
    /// index or length at or above the compile-time `LIMIT`.
    #[error("bitlist limit {limit} exceeded by {got} bits")]
    BitlistLimitExceeded {
        /// Maximum number of bits allowed.
        limit: usize,
        /// Index or length that violated the cap.
        got: usize,
    },

    /// [`Bitlist::from_bytes`](crate::Bitlist::from_bytes) received an empty
    /// slice or a slice whose final byte is `0x00` (no SSZ delimiter bit).
    #[error("invalid bitlist encoding (missing delimiter bit)")]
    InvalidBitlistEncoding,

    /// [`Bitvector::from_bytes`](crate::Bitvector::from_bytes) received a
    /// canonically-correct number of bytes but the trailing bits beyond
    /// position `length - 1` in the final byte are non-zero.
    #[error("invalid bitvector encoding: trailing bits beyond length {length} must be zero")]
    InvalidBitvectorEncoding {
        /// Bit length declared by the [`Bitvector`](crate::Bitvector) type.
        length: usize,
    },

    /// [`Bitvector::set`](crate::Bitvector::set) received an index `>= N`.
    #[error("bitvector index {got} out of bounds (length {length})")]
    BitvectorIndexOutOfBounds {
        /// Bit length of the [`Bitvector`](crate::Bitvector) (its `N`).
        length: usize,
        /// Index that violated the bound.
        got: usize,
    },

    /// A hex string decoded by [`ByteVector::try_from`](crate::ByteVector)
    /// contained a non-hex character or had an odd number of digits.
    #[error("{type_name} hex decode failed: {detail}")]
    InvalidHexEncoding {
        /// Human-readable name of the target type (e.g. `"ByteVector"`).
        type_name: &'static str,
        /// Static reason (`"non-hex character"` | `"odd number of hex digits"`).
        detail: &'static str,
    },
}
