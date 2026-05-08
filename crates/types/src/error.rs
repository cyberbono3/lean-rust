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
}
