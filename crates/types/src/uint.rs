//! Wide unsigned integers and little-endian SSZ decode helpers.
//!
//! Native `u8`/`u16`/`u32`/`u64` are used directly (idiomatic Rust uses
//! checked arithmetic via `checked_add` / `checked_mul` on primitives —
//! no wrapper type is needed). [`U128`] and [`U256`] are re-exported from
//! [`ruint::aliases`] (replaces lean-go's `holiman/uint256`).

use ruint::aliases::{U128 as RU128, U256 as RU256};

use crate::error::TypesError;

/// 128-bit unsigned integer (re-export of [`ruint::aliases::U128`]).
///
/// # Example
/// ```
/// use types::U128;
/// let one = U128::from(1_u64);
/// assert_eq!(one + one, U128::from(2_u64));
/// ```
pub type U128 = RU128;

/// 256-bit unsigned integer (re-export of [`ruint::aliases::U256`]).
///
/// # Example
/// ```
/// use types::U256;
/// assert_eq!(U256::ZERO + U256::from(7_u64), U256::from(7_u64));
/// ```
pub type U256 = RU256;

/// Decode a little-endian SSZ `u8` from exactly 1 byte.
///
/// # Errors
/// Returns [`TypesError::InvalidByteLength`] when `data.len() != 1`.
///
/// # Example
/// ```
/// use types::{decode_u8_le, TypesError};
/// # fn main() -> Result<(), TypesError> {
/// assert_eq!(decode_u8_le(&[0x42])?, 0x42);
/// assert!(decode_u8_le(&[]).is_err());
/// assert!(decode_u8_le(&[0, 0]).is_err());
/// # Ok(())
/// # }
/// ```
pub fn decode_u8_le(data: &[u8]) -> Result<u8, TypesError> {
    let arr: [u8; 1] = data.try_into().map_err(|_| TypesError::InvalidByteLength {
        type_name: "u8",
        want: 1,
        got: data.len(),
    })?;
    Ok(u8::from_le_bytes(arr))
}

/// Decode a little-endian SSZ `u16` from exactly 2 bytes.
///
/// # Errors
/// Returns [`TypesError::InvalidByteLength`] when `data.len() != 2`.
///
/// # Example
/// ```
/// use types::{decode_u16_le, TypesError};
/// # fn main() -> Result<(), TypesError> {
/// assert_eq!(decode_u16_le(&0x1234_u16.to_le_bytes())?, 0x1234);
/// # Ok(())
/// # }
/// ```
pub fn decode_u16_le(data: &[u8]) -> Result<u16, TypesError> {
    let arr: [u8; 2] = data.try_into().map_err(|_| TypesError::InvalidByteLength {
        type_name: "u16",
        want: 2,
        got: data.len(),
    })?;
    Ok(u16::from_le_bytes(arr))
}

/// Decode a little-endian SSZ `u32` from exactly 4 bytes.
///
/// # Errors
/// Returns [`TypesError::InvalidByteLength`] when `data.len() != 4`.
///
/// # Example
/// ```
/// use types::{decode_u32_le, TypesError};
/// # fn main() -> Result<(), TypesError> {
/// assert_eq!(decode_u32_le(&0xdead_beef_u32.to_le_bytes())?, 0xdead_beef);
/// # Ok(())
/// # }
/// ```
pub fn decode_u32_le(data: &[u8]) -> Result<u32, TypesError> {
    let arr: [u8; 4] = data.try_into().map_err(|_| TypesError::InvalidByteLength {
        type_name: "u32",
        want: 4,
        got: data.len(),
    })?;
    Ok(u32::from_le_bytes(arr))
}

/// Decode a little-endian SSZ `u64` from exactly 8 bytes.
///
/// # Errors
/// Returns [`TypesError::InvalidByteLength`] when `data.len() != 8`.
///
/// # Example
/// ```
/// use types::{decode_u64_le, TypesError};
/// # fn main() -> Result<(), TypesError> {
/// let v: u64 = 0x0123_4567_89ab_cdef;
/// assert_eq!(decode_u64_le(&v.to_le_bytes())?, v);
/// # Ok(())
/// # }
/// ```
pub fn decode_u64_le(data: &[u8]) -> Result<u64, TypesError> {
    let arr: [u8; 8] = data.try_into().map_err(|_| TypesError::InvalidByteLength {
        type_name: "u64",
        want: 8,
        got: data.len(),
    })?;
    Ok(u64::from_le_bytes(arr))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // -- length-error coverage ----------------------------------------------

    #[test]
    fn decode_u8_le_rejects_wrong_length() {
        assert!(matches!(
            decode_u8_le(&[]),
            Err(TypesError::InvalidByteLength {
                want: 1,
                got: 0,
                ..
            })
        ));
        assert!(matches!(
            decode_u8_le(&[0, 1]),
            Err(TypesError::InvalidByteLength {
                want: 1,
                got: 2,
                ..
            })
        ));
    }

    #[test]
    fn decode_u16_le_rejects_wrong_length() {
        assert!(matches!(
            decode_u16_le(&[0]),
            Err(TypesError::InvalidByteLength {
                want: 2,
                got: 1,
                ..
            })
        ));
    }

    #[test]
    fn decode_u32_le_rejects_wrong_length() {
        assert!(matches!(
            decode_u32_le(&[0; 3]),
            Err(TypesError::InvalidByteLength {
                want: 4,
                got: 3,
                ..
            })
        ));
    }

    #[test]
    fn decode_u64_le_rejects_wrong_length() {
        assert!(matches!(
            decode_u64_le(&[0; 7]),
            Err(TypesError::InvalidByteLength {
                want: 8,
                got: 7,
                ..
            })
        ));
    }

    // -- U128 / U256 boundary coverage --------------------------------------

    #[test]
    fn u128_overflow_saturates_via_checked_add() {
        assert!(U128::MAX.checked_add(U128::from(1_u64)).is_none());
        assert_eq!(
            U128::ZERO.checked_add(U128::from(1_u64)),
            Some(U128::from(1_u64))
        );
    }

    #[test]
    fn u128_underflow_via_checked_sub() {
        assert!(U128::ZERO.checked_sub(U128::from(1_u64)).is_none());
    }

    #[test]
    fn u256_overflow_saturates_via_checked_add() {
        assert!(U256::MAX.checked_add(U256::from(1_u64)).is_none());
    }

    #[test]
    fn u256_underflow_via_checked_sub() {
        assert!(U256::ZERO.checked_sub(U256::from(1_u64)).is_none());
    }

    #[test]
    fn u128_max_round_trips_to_le_bytes() {
        let bytes = U128::MAX.to_le_bytes::<16>();
        assert_eq!(U128::from_le_bytes::<16>(bytes), U128::MAX);
    }

    #[test]
    fn u256_max_round_trips_to_le_bytes() {
        let bytes = U256::MAX.to_le_bytes::<32>();
        assert_eq!(U256::from_le_bytes::<32>(bytes), U256::MAX);
    }

    // -- property tests (AC #2 — round-trip for arbitrary values) -----------

    proptest! {
        #[test]
        fn decode_u8_le_round_trips(v in any::<u8>()) {
            prop_assert_eq!(decode_u8_le(&v.to_le_bytes()).unwrap(), v);
        }

        #[test]
        fn decode_u16_le_round_trips(v in any::<u16>()) {
            prop_assert_eq!(decode_u16_le(&v.to_le_bytes()).unwrap(), v);
        }

        #[test]
        fn decode_u32_le_round_trips(v in any::<u32>()) {
            prop_assert_eq!(decode_u32_le(&v.to_le_bytes()).unwrap(), v);
        }

        #[test]
        fn decode_u64_le_round_trips(v in any::<u64>()) {
            prop_assert_eq!(decode_u64_le(&v.to_le_bytes()).unwrap(), v);
        }
    }
}
