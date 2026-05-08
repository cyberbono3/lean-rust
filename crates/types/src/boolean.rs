//! SSZ-compatible Boolean.
//!
//! `Boolean` aliases the native [`bool`]. SSZ byte-level decode validation
//! lives in [`decode_boolean`] (mirrors the structure of
//! [`crate::uint::decode_u8_le`] etc.).

use crate::error::TypesError;

/// SSZ-compatible Boolean (alias of native [`bool`]).
///
/// SSZ encodes a Boolean as a single byte: `0x00` → `false`, `0x01` → `true`;
/// any other byte is invalid and rejected by [`decode_boolean`].
///
/// # Example
/// ```
/// use types::Boolean;
/// let flag: Boolean = true;
/// assert!(flag);
/// ```
pub type Boolean = bool;

/// Decode a Boolean from a single SSZ byte.
///
/// `0x00` decodes to `false`, `0x01` decodes to `true`; every other byte
/// triggers [`TypesError::InvalidBooleanByte`].
///
/// # Errors
/// - [`TypesError::InvalidByteLength`] when `data.len() != 1`.
/// - [`TypesError::InvalidBooleanByte`] when the byte is neither `0x00` nor `0x01`.
///
/// # Example
/// ```
/// use types::{decode_boolean, TypesError};
/// # fn main() -> Result<(), TypesError> {
/// assert!(!decode_boolean(&[0])?);
/// assert!(decode_boolean(&[1])?);
/// assert!(decode_boolean(&[2]).is_err());
/// assert!(decode_boolean(&[]).is_err());
/// # Ok(())
/// # }
/// ```
pub fn decode_boolean(data: &[u8]) -> Result<Boolean, TypesError> {
    match data {
        [0] => Ok(false),
        [1] => Ok(true),
        [b] => Err(TypesError::InvalidBooleanByte { value: *b }),
        _ => Err(TypesError::InvalidByteLength {
            type_name: "bool",
            want: 1,
            got: data.len(),
        }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn decode_boolean_zero_is_false() {
        assert!(!decode_boolean(&[0]).unwrap());
    }

    #[test]
    fn decode_boolean_one_is_true() {
        assert!(decode_boolean(&[1]).unwrap());
    }

    #[test]
    fn decode_boolean_rejects_non_canonical_bytes() {
        for b in 2_u8..=u8::MAX {
            assert!(matches!(
                decode_boolean(&[b]),
                Err(TypesError::InvalidBooleanByte { value }) if value == b
            ));
        }
    }

    #[test]
    fn decode_boolean_rejects_empty_input() {
        assert!(matches!(
            decode_boolean(&[]),
            Err(TypesError::InvalidByteLength {
                want: 1,
                got: 0,
                ..
            })
        ));
    }

    #[test]
    fn decode_boolean_rejects_too_many_bytes() {
        assert!(matches!(
            decode_boolean(&[0, 0]),
            Err(TypesError::InvalidByteLength {
                want: 1,
                got: 2,
                ..
            })
        ));
    }
}
