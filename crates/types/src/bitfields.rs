//! SSZ-compatible [`Bitlist<const LIMIT: usize>`] and
//! [`Bitvector<const N: usize>`] bitfield primitives.
//!
//! Mirrors lean-go's `types/bitlist.go` + `types/bitvector.go` on the wire.
//! Bits are packed LSB-first within each byte: bit `i` lives in
//! `bytes[i / 8] & (1 << (i % 8))`.
//!
//! - [`Bitvector<N>`] is a fixed-length vector of `N` bits, encoded into
//!   `ceil(N / 8)` bytes. Bits beyond `N - 1` in the final byte are required
//!   to be zero on decode (canonical SSZ).
//! - [`Bitlist<LIMIT>`] is a variable-length list of bits with a compile-time
//!   maximum `LIMIT`, encoded into `floor(length / 8) + 1` bytes. The final
//!   byte carries an SSZ "delimiter" bit set immediately above the live
//!   data; the highest set bit of the last byte determines the live length
//!   on decode.

use crate::error::TypesError;

/// Fixed-length bit vector of `N` bits.
///
/// Backed by `Vec<u8>` of length `ceil(N / 8)` (zero on construction). Bit
/// access is O(1); encoding is zero-copy via [`Bitvector::as_bytes`].
///
/// # Example
/// ```
/// use types::{Bitvector, TypesError};
/// # fn main() -> Result<(), TypesError> {
/// let mut bv: Bitvector<5> = Bitvector::new();
/// bv.set(0, true)?;
/// bv.set(2, true)?;
/// assert_eq!(bv.count_ones(), 2);
/// assert_eq!(bv.as_bytes(), &[0b0000_0101]);
/// let round_trip: Bitvector<5> = Bitvector::from_bytes(bv.as_bytes())?;
/// assert_eq!(round_trip, bv);
/// # Ok(())
/// # }
/// ```
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Bitvector<const N: usize> {
    bytes: Vec<u8>,
}

impl<const N: usize> Bitvector<N> {
    /// Constructs an all-zero [`Bitvector<N>`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            bytes: vec![0_u8; N.div_ceil(8)],
        }
    }

    /// Returns the bit at position `index`, or `None` when `index >= N`.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<bool> {
        if index >= N {
            return None;
        }
        Some(self.bytes[index / 8] & (1_u8 << (index % 8)) != 0)
    }

    /// Sets the bit at position `index` to `value`.
    ///
    /// # Errors
    /// Returns [`TypesError::BitvectorIndexOutOfBounds`] when `index >= N`.
    pub fn set(&mut self, index: usize, value: bool) -> Result<(), TypesError> {
        if index >= N {
            return Err(TypesError::BitvectorIndexOutOfBounds {
                length: N,
                got: index,
            });
        }
        let byte = &mut self.bytes[index / 8];
        let mask = 1_u8 << (index % 8);
        if value {
            *byte |= mask;
        } else {
            *byte &= !mask;
        }
        Ok(())
    }

    /// Returns the bit length `N`.
    #[must_use]
    pub const fn len(&self) -> usize {
        N
    }

    /// Returns `true` when `N == 0`.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        N == 0
    }

    /// Returns the number of bits set to `true`.
    #[must_use]
    pub fn count_ones(&self) -> usize {
        self.bytes
            .iter()
            .map(|b| b.count_ones() as usize)
            .sum::<usize>()
    }

    /// Iterates over the indices of the bits set to `true`, in ascending order.
    pub fn iter_set_indices(&self) -> impl Iterator<Item = usize> + '_ {
        let bytes = self.bytes.as_slice();
        (0..N).filter(move |&i| bytes[i / 8] & (1_u8 << (i % 8)) != 0)
    }

    /// Returns the SSZ-encoded bytes of length `ceil(N / 8)` (zero-copy).
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Decodes a [`Bitvector<N>`] from `data`.
    ///
    /// # Errors
    /// - [`TypesError::InvalidByteLength`] when `data.len() != ceil(N / 8)`.
    /// - [`TypesError::InvalidBitvectorEncoding`] when bits beyond position
    ///   `N - 1` in the final byte are non-zero.
    pub fn from_bytes(data: &[u8]) -> Result<Self, TypesError> {
        let want = N.div_ceil(8);
        if data.len() != want {
            return Err(TypesError::InvalidByteLength {
                type_name: "Bitvector",
                want,
                got: data.len(),
            });
        }
        if N % 8 != 0 {
            // Mask of bits ABOVE the live range in the final byte (must be 0).
            let trailing_mask: u8 = !((1_u8 << (N % 8)) - 1);
            if data[want - 1] & trailing_mask != 0 {
                return Err(TypesError::InvalidBitvectorEncoding { length: N });
            }
        }
        Ok(Self {
            bytes: data.to_vec(),
        })
    }
}

impl<const N: usize> Default for Bitvector<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> core::fmt::Debug for Bitvector<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Bitvector<{N}>(")?;
        for i in 0..N {
            let bit = self.bytes[i / 8] & (1_u8 << (i % 8)) != 0;
            f.write_str(if bit { "1" } else { "0" })?;
        }
        write!(f, ")")
    }
}

/// Variable-length bit list with compile-time maximum `LIMIT` bits.
///
/// Internally tracks `length` (live bit count, `0..=LIMIT`) and a packed
/// `Vec<u8>` of size `ceil(length / 8)`. Bits at positions `>= length` are
/// always zero by invariant — maintained by [`Bitlist::set`] and
/// [`Bitlist::from_bytes`].
///
/// SSZ encoding appends a single delimiter bit at position `length`, so the
/// wire size is `floor(length / 8) + 1` bytes (one byte minimum, even for
/// the empty list).
///
/// # Example
/// ```
/// use types::{Bitlist, TypesError};
/// # fn main() -> Result<(), TypesError> {
/// let mut bl: Bitlist<8> = Bitlist::new();
/// bl.set(0, true)?;
/// bl.set(2, true)?;
/// assert_eq!(bl.len(), 3);
/// assert_eq!(bl.count_ones(), 2);
/// // Encoded: bits 0=1, 1=0, 2=1 + delimiter at bit 3 → 0b0000_1101 = 0x0d.
/// assert_eq!(bl.as_bytes(), vec![0x0d]);
/// let round_trip: Bitlist<8> = Bitlist::from_bytes(&bl.as_bytes())?;
/// assert_eq!(round_trip, bl);
///
/// // Setting at LIMIT is rejected.
/// assert!(matches!(
///     bl.set(8, true),
///     Err(TypesError::BitlistLimitExceeded { limit: 8, got: 8 })
/// ));
/// # Ok(())
/// # }
/// ```
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Bitlist<const LIMIT: usize> {
    bytes: Vec<u8>,
    length: usize,
}

impl<const LIMIT: usize> Bitlist<LIMIT> {
    /// Constructs an empty [`Bitlist<LIMIT>`] (length 0).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bytes: Vec::new(),
            length: 0,
        }
    }

    /// Constructs an all-zero [`Bitlist<LIMIT>`] of the given `length`.
    ///
    /// # Errors
    /// Returns [`TypesError::BitlistLimitExceeded`] when `length > LIMIT`.
    pub fn with_length(length: usize) -> Result<Self, TypesError> {
        if length > LIMIT {
            return Err(TypesError::BitlistLimitExceeded {
                limit: LIMIT,
                got: length,
            });
        }
        Ok(Self {
            bytes: vec![0_u8; length.div_ceil(8)],
            length,
        })
    }

    /// Returns the bit at position `index`, or `None` when `index >= len()`.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<bool> {
        if index >= self.length {
            return None;
        }
        Some(self.bytes[index / 8] & (1_u8 << (index % 8)) != 0)
    }

    /// Sets the bit at position `index` to `value`. Auto-grows the live
    /// length to `index + 1` when `index >= len()`, padding with `false`.
    ///
    /// # Errors
    /// Returns [`TypesError::BitlistLimitExceeded`] when `index >= LIMIT`.
    pub fn set(&mut self, index: usize, value: bool) -> Result<(), TypesError> {
        if index >= LIMIT {
            return Err(TypesError::BitlistLimitExceeded {
                limit: LIMIT,
                got: index,
            });
        }
        if index >= self.length {
            let new_length = index + 1;
            self.bytes.resize(new_length.div_ceil(8), 0_u8);
            self.length = new_length;
        }
        let byte = &mut self.bytes[index / 8];
        let mask = 1_u8 << (index % 8);
        if value {
            *byte |= mask;
        } else {
            *byte &= !mask;
        }
        Ok(())
    }

    /// Returns the live bit count.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.length
    }

    /// Returns `true` when the live bit count is zero.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Returns the compile-time maximum bit count.
    #[must_use]
    pub const fn limit(&self) -> usize {
        LIMIT
    }

    /// Returns the number of bits set to `true`.
    ///
    /// Sound because bits at positions `>= length` are always zero by invariant.
    #[must_use]
    pub fn count_ones(&self) -> usize {
        self.bytes
            .iter()
            .map(|b| b.count_ones() as usize)
            .sum::<usize>()
    }

    /// Iterates over the indices of the bits set to `true`, in ascending order.
    pub fn iter_set_indices(&self) -> impl Iterator<Item = usize> + '_ {
        let bytes = self.bytes.as_slice();
        let length = self.length;
        (0..length).filter(move |&i| bytes[i / 8] & (1_u8 << (i % 8)) != 0)
    }

    /// Returns the SSZ-encoded bytes including the delimiter bit.
    ///
    /// Output length is `floor(length / 8) + 1` (≥ 1 byte).
    #[must_use]
    pub fn as_bytes(&self) -> Vec<u8> {
        let mut out = self.bytes.clone();
        let delimiter_byte = self.length / 8;
        let delimiter_bit = self.length % 8;
        if delimiter_byte >= out.len() {
            out.push(0_u8);
        }
        out[delimiter_byte] |= 1_u8 << delimiter_bit;
        out
    }

    /// Decodes a [`Bitlist<LIMIT>`] from SSZ-encoded `data`.
    ///
    /// The decoder strips the delimiter bit (the highest set bit of the
    /// final byte) to recover the live length and bit pattern.
    ///
    /// # Errors
    /// - [`TypesError::InvalidBitlistEncoding`] when `data` is empty or the
    ///   final byte is `0x00` (delimiter bit missing).
    /// - [`TypesError::BitlistLimitExceeded`] when the recovered length
    ///   exceeds `LIMIT`.
    pub fn from_bytes(data: &[u8]) -> Result<Self, TypesError> {
        let Some((&last, _)) = data.split_last() else {
            return Err(TypesError::InvalidBitlistEncoding);
        };
        if last == 0 {
            return Err(TypesError::InvalidBitlistEncoding);
        }
        // u8::ilog2 is total over non-zero u8 (panics on 0; already rejected).
        let highest_bit = last.ilog2() as usize;
        let length = (data.len() - 1) * 8 + highest_bit;
        if length > LIMIT {
            return Err(TypesError::BitlistLimitExceeded {
                limit: LIMIT,
                got: length,
            });
        }
        let expected_bytes_len = length.div_ceil(8);
        let mut bytes = data.to_vec();
        let last_idx = bytes.len() - 1;
        bytes[last_idx] &= !(1_u8 << highest_bit);
        bytes.truncate(expected_bytes_len);
        Ok(Self { bytes, length })
    }
}

impl<const LIMIT: usize> Default for Bitlist<LIMIT> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const LIMIT: usize> core::fmt::Debug for Bitlist<LIMIT> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Bitlist<{LIMIT}>(len={})(", self.length)?;
        for i in 0..self.length {
            let bit = self.bytes[i / 8] & (1_u8 << (i % 8)) != 0;
            f.write_str(if bit { "1" } else { "0" })?;
        }
        write!(f, ")")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // ---------------------------------------------------------------------
    // Bitvector — construction / accessors
    // ---------------------------------------------------------------------

    #[test]
    fn bitvector_new_is_all_zeros() {
        let bv: Bitvector<16> = Bitvector::new();
        assert_eq!(bv.len(), 16);
        assert_eq!(bv.count_ones(), 0);
        assert_eq!(bv.as_bytes(), &[0_u8, 0_u8]);
    }

    #[test]
    fn bitvector_default_equals_new() {
        let a: Bitvector<5> = Bitvector::default();
        let b: Bitvector<5> = Bitvector::new();
        assert_eq!(a, b);
    }

    #[test]
    fn bitvector_zero_n_has_no_bytes() {
        let bv: Bitvector<0> = Bitvector::new();
        assert_eq!(bv.len(), 0);
        assert!(bv.is_empty());
        assert_eq!(bv.as_bytes(), &[] as &[u8]);
        assert_eq!(bv.count_ones(), 0);
    }

    #[test]
    fn bitvector_get_returns_none_out_of_range() {
        let bv: Bitvector<4> = Bitvector::new();
        assert_eq!(bv.get(4), None);
        assert_eq!(bv.get(usize::MAX), None);
    }

    #[test]
    fn bitvector_set_and_get_round_trip() {
        let mut bv: Bitvector<10> = Bitvector::new();
        bv.set(0, true).unwrap();
        bv.set(3, true).unwrap();
        bv.set(9, true).unwrap();
        assert_eq!(bv.get(0), Some(true));
        assert_eq!(bv.get(1), Some(false));
        assert_eq!(bv.get(3), Some(true));
        assert_eq!(bv.get(9), Some(true));
        assert_eq!(bv.count_ones(), 3);
    }

    #[test]
    fn bitvector_set_clears_bit() {
        let mut bv: Bitvector<8> = Bitvector::new();
        bv.set(2, true).unwrap();
        assert_eq!(bv.get(2), Some(true));
        bv.set(2, false).unwrap();
        assert_eq!(bv.get(2), Some(false));
        assert_eq!(bv.count_ones(), 0);
    }

    #[test]
    fn bitvector_set_rejects_out_of_bounds() {
        let mut bv: Bitvector<5> = Bitvector::new();
        assert!(matches!(
            bv.set(5, true),
            Err(TypesError::BitvectorIndexOutOfBounds { length: 5, got: 5 })
        ));
        assert!(matches!(
            bv.set(99, true),
            Err(TypesError::BitvectorIndexOutOfBounds { length: 5, got: 99 })
        ));
    }

    #[test]
    fn bitvector_iter_set_indices_matches_count_ones() {
        let mut bv: Bitvector<13> = Bitvector::new();
        for i in [1_usize, 4, 7, 12] {
            bv.set(i, true).unwrap();
        }
        let set: Vec<usize> = bv.iter_set_indices().collect();
        assert_eq!(set, vec![1, 4, 7, 12]);
        assert_eq!(set.len(), bv.count_ones());
    }

    // ---------------------------------------------------------------------
    // Bitvector — encoding / decoding
    // ---------------------------------------------------------------------

    #[test]
    fn bitvector_known_encoding_5_bits() {
        // bits = [1, 0, 1, 0, 0] → byte 0 = 0b00000101 = 0x05
        let mut bv: Bitvector<5> = Bitvector::new();
        bv.set(0, true).unwrap();
        bv.set(2, true).unwrap();
        assert_eq!(bv.as_bytes(), &[0b0000_0101]);
    }

    #[test]
    fn bitvector_from_bytes_rejects_wrong_length() {
        // Bitvector<10> needs 2 bytes; supply 1.
        let err = Bitvector::<10>::from_bytes(&[0]).unwrap_err();
        assert!(matches!(
            err,
            TypesError::InvalidByteLength {
                type_name: "Bitvector",
                want: 2,
                got: 1
            }
        ));
        let err = Bitvector::<10>::from_bytes(&[0, 0, 0]).unwrap_err();
        assert!(matches!(
            err,
            TypesError::InvalidByteLength {
                type_name: "Bitvector",
                want: 2,
                got: 3
            }
        ));
    }

    #[test]
    fn bitvector_from_bytes_rejects_non_canonical_trailing_bits() {
        // Bitvector<5> uses bits 0..4 of byte 0; bit 5 must be zero.
        let err = Bitvector::<5>::from_bytes(&[0b0010_0000]).unwrap_err();
        assert!(matches!(
            err,
            TypesError::InvalidBitvectorEncoding { length: 5 }
        ));
        // Top bit set is also rejected.
        let err = Bitvector::<5>::from_bytes(&[0b1000_0000]).unwrap_err();
        assert!(matches!(
            err,
            TypesError::InvalidBitvectorEncoding { length: 5 }
        ));
    }

    #[test]
    fn bitvector_round_trip_preserves_state() {
        let mut bv: Bitvector<17> = Bitvector::new();
        for i in [0_usize, 5, 8, 16] {
            bv.set(i, true).unwrap();
        }
        let bytes = bv.as_bytes().to_vec();
        let decoded: Bitvector<17> = Bitvector::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, bv);
    }

    #[test]
    fn bitvector_zero_n_round_trip() {
        let bv: Bitvector<0> = Bitvector::new();
        let bytes = bv.as_bytes().to_vec();
        let decoded: Bitvector<0> = Bitvector::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, bv);
    }

    #[test]
    fn bitvector_n_multiple_of_8_no_trailing_bits_check() {
        let mut bv: Bitvector<16> = Bitvector::new();
        bv.set(7, true).unwrap();
        bv.set(15, true).unwrap();
        let bytes = bv.as_bytes().to_vec();
        let decoded: Bitvector<16> = Bitvector::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, bv);
    }

    // ---------------------------------------------------------------------
    // Bitlist — construction / accessors
    // ---------------------------------------------------------------------

    #[test]
    fn bitlist_new_is_empty() {
        let bl: Bitlist<32> = Bitlist::new();
        assert_eq!(bl.len(), 0);
        assert!(bl.is_empty());
        assert_eq!(bl.limit(), 32);
        assert_eq!(bl.count_ones(), 0);
    }

    #[test]
    fn bitlist_default_equals_new() {
        let a: Bitlist<8> = Bitlist::default();
        let b: Bitlist<8> = Bitlist::new();
        assert_eq!(a, b);
    }

    #[test]
    fn bitlist_with_length_zero_filled() {
        let bl: Bitlist<16> = Bitlist::with_length(10).unwrap();
        assert_eq!(bl.len(), 10);
        assert_eq!(bl.count_ones(), 0);
        for i in 0..10 {
            assert_eq!(bl.get(i), Some(false));
        }
    }

    #[test]
    fn bitlist_with_length_rejects_over_limit() {
        let err = Bitlist::<8>::with_length(9).unwrap_err();
        assert!(matches!(
            err,
            TypesError::BitlistLimitExceeded { limit: 8, got: 9 }
        ));
    }

    #[test]
    fn bitlist_with_length_at_limit_succeeds() {
        let bl: Bitlist<8> = Bitlist::with_length(8).unwrap();
        assert_eq!(bl.len(), 8);
    }

    #[test]
    fn bitlist_get_out_of_range_is_none() {
        let bl: Bitlist<32> = Bitlist::with_length(4).unwrap();
        assert_eq!(bl.get(4), None);
        assert_eq!(bl.get(usize::MAX), None);
    }

    #[test]
    fn bitlist_set_grows_to_index_plus_one() {
        let mut bl: Bitlist<32> = Bitlist::new();
        bl.set(5, true).unwrap();
        assert_eq!(bl.len(), 6);
        assert_eq!(bl.get(5), Some(true));
        assert_eq!(bl.get(0), Some(false));
        assert_eq!(bl.count_ones(), 1);
    }

    #[test]
    fn bitlist_set_does_not_shrink() {
        let mut bl: Bitlist<32> = Bitlist::new();
        bl.set(10, true).unwrap();
        bl.set(2, true).unwrap();
        assert_eq!(bl.len(), 11);
        assert_eq!(bl.get(2), Some(true));
        assert_eq!(bl.get(10), Some(true));
    }

    #[test]
    fn bitlist_set_clears_existing_bit() {
        let mut bl: Bitlist<32> = Bitlist::new();
        bl.set(3, true).unwrap();
        bl.set(3, false).unwrap();
        assert_eq!(bl.get(3), Some(false));
        assert_eq!(bl.count_ones(), 0);
    }

    // AC #1: Bitlist::set(LIMIT, true) returns BitlistLimitExceeded.
    #[test]
    fn bitlist_set_at_limit_returns_error() {
        let mut bl: Bitlist<8> = Bitlist::new();
        let err = bl.set(8, true).unwrap_err();
        assert!(matches!(
            err,
            TypesError::BitlistLimitExceeded { limit: 8, got: 8 }
        ));
        // Live length must not have changed.
        assert_eq!(bl.len(), 0);
    }

    #[test]
    fn bitlist_set_above_limit_returns_error() {
        let mut bl: Bitlist<8> = Bitlist::new();
        let err = bl.set(99, false).unwrap_err();
        assert!(matches!(
            err,
            TypesError::BitlistLimitExceeded { limit: 8, got: 99 }
        ));
    }

    #[test]
    fn bitlist_zero_limit_rejects_any_set() {
        let mut bl: Bitlist<0> = Bitlist::new();
        let err = bl.set(0, true).unwrap_err();
        assert!(matches!(
            err,
            TypesError::BitlistLimitExceeded { limit: 0, got: 0 }
        ));
    }

    #[test]
    fn bitlist_iter_set_indices_matches_count_ones() {
        let mut bl: Bitlist<32> = Bitlist::new();
        for i in [0_usize, 3, 7, 12, 25] {
            bl.set(i, true).unwrap();
        }
        let set: Vec<usize> = bl.iter_set_indices().collect();
        assert_eq!(set, vec![0, 3, 7, 12, 25]);
        assert_eq!(set.len(), bl.count_ones());
    }

    // ---------------------------------------------------------------------
    // Bitlist — encoding / decoding (incl. delimiter bit)
    // ---------------------------------------------------------------------

    #[test]
    fn bitlist_empty_encoding_is_single_delimiter_byte() {
        let bl: Bitlist<32> = Bitlist::new();
        assert_eq!(bl.as_bytes(), vec![0x01_u8]);
    }

    #[test]
    fn bitlist_known_encoding_3_bits() {
        // bits 0=1, 1=0, 2=1, delimiter at bit 3 → 0b00001101 = 0x0d
        let mut bl: Bitlist<8> = Bitlist::new();
        bl.set(0, true).unwrap();
        bl.set(2, true).unwrap();
        assert_eq!(bl.as_bytes(), vec![0x0d_u8]);
    }

    #[test]
    fn bitlist_known_encoding_8_bits() {
        // 8 bits (all zero) + delimiter at bit 8 (start of byte 1) → [0x00, 0x01]
        let bl: Bitlist<32> = Bitlist::with_length(8).unwrap();
        assert_eq!(bl.as_bytes(), vec![0x00_u8, 0x01_u8]);
    }

    #[test]
    fn bitlist_known_encoding_7_bits_all_zero() {
        // 7 zero bits + delimiter at bit 7 → byte 0 = 0b1000_0000 = 0x80
        let bl: Bitlist<32> = Bitlist::with_length(7).unwrap();
        assert_eq!(bl.as_bytes(), vec![0x80_u8]);
    }

    #[test]
    fn bitlist_zero_limit_empty_round_trip() {
        let bl: Bitlist<0> = Bitlist::new();
        let encoded = bl.as_bytes();
        assert_eq!(encoded, vec![0x01_u8]);
        let decoded: Bitlist<0> = Bitlist::from_bytes(&encoded).unwrap();
        assert_eq!(decoded, bl);
        assert_eq!(decoded.len(), 0);
    }

    #[test]
    fn bitlist_zero_limit_rejects_any_data_bit() {
        // [0x03] would mean length=1 (delimiter at bit 1, data bit 0 set), > LIMIT=0.
        let err = Bitlist::<0>::from_bytes(&[0x03]).unwrap_err();
        assert!(matches!(
            err,
            TypesError::BitlistLimitExceeded { limit: 0, got: 1 }
        ));
    }

    #[test]
    fn bitlist_from_bytes_rejects_empty() {
        let err = Bitlist::<32>::from_bytes(&[]).unwrap_err();
        assert!(matches!(err, TypesError::InvalidBitlistEncoding));
    }

    #[test]
    fn bitlist_from_bytes_rejects_zero_last_byte() {
        let err = Bitlist::<32>::from_bytes(&[0x00]).unwrap_err();
        assert!(matches!(err, TypesError::InvalidBitlistEncoding));
        // Same with multi-byte input ending in zero (no delimiter).
        let err = Bitlist::<32>::from_bytes(&[0xff, 0x00]).unwrap_err();
        assert!(matches!(err, TypesError::InvalidBitlistEncoding));
    }

    #[test]
    fn bitlist_from_bytes_rejects_length_over_limit() {
        // [0x80] decodes to length=7 (delimiter at bit 7).
        let err = Bitlist::<6>::from_bytes(&[0x80]).unwrap_err();
        assert!(matches!(
            err,
            TypesError::BitlistLimitExceeded { limit: 6, got: 7 }
        ));
    }

    // AC #3: round-trip for Bitlist includes the delimiter bit.
    #[test]
    fn bitlist_round_trip_preserves_length_and_bits() {
        let mut bl: Bitlist<64> = Bitlist::new();
        for i in [0_usize, 1, 5, 17, 23, 47] {
            bl.set(i, true).unwrap();
        }
        let encoded = bl.as_bytes();
        let decoded: Bitlist<64> = Bitlist::from_bytes(&encoded).unwrap();
        assert_eq!(decoded, bl);
        assert_eq!(decoded.len(), 48);
        assert_eq!(decoded.count_ones(), 6);
    }

    #[test]
    fn bitlist_round_trip_at_byte_boundary() {
        // length=16 has the delimiter at bit 0 of a fresh third byte.
        let mut bl: Bitlist<32> = Bitlist::with_length(16).unwrap();
        bl.set(15, true).unwrap();
        let encoded = bl.as_bytes();
        assert_eq!(encoded.len(), 3);
        let decoded: Bitlist<32> = Bitlist::from_bytes(&encoded).unwrap();
        assert_eq!(decoded, bl);
    }

    // ---------------------------------------------------------------------
    // Property tests — AC #4 + AC #2/#3 round-trips
    // ---------------------------------------------------------------------

    proptest! {
        // AC #4: count_ones() == iter_set_indices().count() for arbitrary Bitlist.
        #[test]
        fn bitlist_count_ones_matches_iter_set_indices(
            indices in proptest::collection::vec(0_usize..256, 0..64),
        ) {
            let mut bl: Bitlist<256> = Bitlist::new();
            for &i in &indices {
                bl.set(i, true).unwrap();
            }
            prop_assert_eq!(bl.count_ones(), bl.iter_set_indices().count());
        }

        // AC #4 — same invariant for Bitvector.
        #[test]
        fn bitvector_count_ones_matches_iter_set_indices(
            mask in any::<u64>(),
        ) {
            let mut bv: Bitvector<64> = Bitvector::new();
            for i in 0..64 {
                if mask & (1_u64 << i) != 0 {
                    bv.set(i, true).unwrap();
                }
            }
            prop_assert_eq!(bv.count_ones(), bv.iter_set_indices().count());
        }

        // AC #2: Bitvector::<N>::from_bytes(b.as_bytes()) == b.
        #[test]
        fn bitvector_round_trip_for_arbitrary_byte_input(
            byte in any::<u8>(),
        ) {
            // Bitvector<7>: only the low 7 bits are valid; mask the input.
            let canonical = byte & 0b0111_1111_u8;
            let bv: Bitvector<7> = Bitvector::from_bytes(&[canonical]).unwrap();
            let round_trip: Bitvector<7> = Bitvector::from_bytes(bv.as_bytes()).unwrap();
            prop_assert_eq!(round_trip, bv);
        }

        // AC #2 (cont.) — full-byte Bitvector accepts any byte.
        #[test]
        fn bitvector_full_byte_round_trip(byte in any::<u8>()) {
            let bv: Bitvector<8> = Bitvector::from_bytes(&[byte]).unwrap();
            let round_trip: Bitvector<8> = Bitvector::from_bytes(bv.as_bytes()).unwrap();
            prop_assert_eq!(round_trip, bv);
        }

        // AC #2 — wider Bitvector built bit-by-bit from a random mask.
        #[test]
        fn bitvector_64_round_trip(mask in any::<u64>()) {
            let mut bv: Bitvector<64> = Bitvector::new();
            for i in 0..64 {
                if mask & (1_u64 << i) != 0 {
                    bv.set(i, true).unwrap();
                }
            }
            let bytes = bv.as_bytes().to_vec();
            let decoded: Bitvector<64> = Bitvector::from_bytes(&bytes).unwrap();
            prop_assert_eq!(decoded, bv);
        }

        // AC #3: Bitlist round-trip includes the delimiter bit, for arbitrary lengths.
        #[test]
        fn bitlist_round_trip_for_arbitrary_input(
            length in 0_usize..=128,
            mask in any::<u128>(),
        ) {
            let mut bl: Bitlist<128> = Bitlist::with_length(length).unwrap();
            for i in 0..length {
                if mask & (1_u128 << i) != 0 {
                    bl.set(i, true).unwrap();
                }
            }
            let encoded = bl.as_bytes();
            // Encoded byte length is exactly floor(length / 8) + 1.
            prop_assert_eq!(encoded.len(), length / 8 + 1);
            let decoded: Bitlist<128> = Bitlist::from_bytes(&encoded).unwrap();
            prop_assert_eq!(decoded.clone(), bl);
            prop_assert_eq!(decoded.len(), length);
        }

        // AC #1 (property form) — set(i, _) for i in [LIMIT, LIMIT+8] always errors.
        #[test]
        fn bitlist_set_above_limit_always_errors(
            offset in 0_usize..=8,
        ) {
            let mut bl: Bitlist<32> = Bitlist::new();
            let target = 32 + offset;
            match bl.set(target, true) {
                Err(TypesError::BitlistLimitExceeded { limit, got }) => {
                    prop_assert_eq!(limit, 32);
                    prop_assert_eq!(got, target);
                }
                other => prop_assert!(false, "unexpected result: {other:?}"),
            }
        }
    }
}
