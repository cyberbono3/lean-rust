//! Fixed-width byte vectors used by the Lean consensus protocol.
//!
//! [`ByteVector<N>`] is a const-generic newtype over `[u8; N]` providing the
//! SSZ `Vector[byte, N]` shape. Two aliases used by the consensus protocol:
//! [`Bytes32`] (e.g. block roots, state roots) and [`Bytes4000`] (BLS
//! signature placeholder per `leanSpec/docs/client/containers.md`).
//!
//! # `Copy` semantics
//! `ByteVector<N>` does NOT derive [`Copy`]. Explicit `impl Copy` is provided
//! for `N` in `0..=64` so small fixed-width types (`Bytes32`, addresses,
//! hashes) move cheaply. Larger sizes — notably [`Bytes4000`] — are
//! intentionally `Clone`-only to prevent silent 4 KB stack copies.

use core::fmt::{self, Write};

/// Fixed-width byte vector of length `N`.
///
/// Mirrors lean-go's `types.ByteVector[N]`. The inner array is `pub` so
/// callers may pattern-match or take a `&[u8; N]` directly when needed.
///
/// # Example
/// ```
/// use types::{ByteVector, Bytes32};
/// let zero: Bytes32 = ByteVector::zero();
/// assert_eq!(zero.as_slice(), &[0_u8; 32]);
/// assert_eq!(zero.to_hex(), format!("0x{}", "00".repeat(32)));
/// ```
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ByteVector<const N: usize>(pub [u8; N]);

/// 32-byte vector — block roots, state roots, validator pubkey hashes.
pub type Bytes32 = ByteVector<32>;

/// 4000-byte vector — BLS signature placeholder (Lean devnet0 wire format).
///
/// Intentionally not [`Copy`] — every move/clone is an explicit 4 KB byte
/// copy. Pass by reference (`&Bytes4000`) wherever ownership is not needed.
pub type Bytes4000 = ByteVector<4000>;

impl<const N: usize> ByteVector<N> {
    /// Constructs a [`ByteVector`] from an owned `[u8; N]`.
    ///
    /// # Example
    /// ```
    /// use types::ByteVector;
    /// let v = ByteVector::<4>::new([0xde, 0xad, 0xbe, 0xef]);
    /// assert_eq!(v.as_slice(), &[0xde, 0xad, 0xbe, 0xef]);
    /// ```
    #[must_use]
    pub const fn new(bytes: [u8; N]) -> Self {
        Self(bytes)
    }

    /// Returns the underlying bytes as a slice.
    #[must_use]
    pub const fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// Returns the all-zeros [`ByteVector<N>`] — equivalent to `Default`.
    #[must_use]
    pub const fn zero() -> Self {
        Self([0_u8; N])
    }

    /// Returns the lowercase `0x`-prefixed hex encoding (`2 + 2 * N` bytes).
    ///
    /// # Example
    /// ```
    /// use types::ByteVector;
    /// let v = ByteVector::<2>::new([0x0a, 0xff]);
    /// assert_eq!(v.to_hex(), "0x0aff");
    /// ```
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(2 + 2 * N);
        s.push_str("0x");
        for b in &self.0 {
            // SAFETY-equivalent: write! to String never fails per std contract.
            let _ = write!(&mut s, "{b:02x}");
        }
        s
    }
}

impl<const N: usize> Default for ByteVector<N> {
    fn default() -> Self {
        Self::zero()
    }
}

impl<const N: usize> fmt::Debug for ByteVector<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ByteVector<{N}>({})", self.to_hex())
    }
}

/// Explicit [`Copy`] impls for `ByteVector<N>` with `N` in `0..=64`.
///
/// Stable Rust cannot express `impl<const N: usize> Copy where N <= 64`
/// without nightly `generic_const_exprs`; the macro expands to one impl per
/// permitted `N`. [`Bytes4000`] is intentionally NOT covered — see
/// [`Bytes4000`] doc.
macro_rules! impl_copy_for_byte_vector {
    ($($n:literal),* $(,)?) => {
        $( impl Copy for ByteVector<$n> {} )*
    };
}

impl_copy_for_byte_vector!(
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49,
    50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64,
);

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::error::TypesError;
    use proptest::prelude::*;
    use static_assertions::{assert_impl_all, assert_not_impl_all};

    // -- Compile-time AC #5: Bytes32 is Copy, Bytes4000 is NOT --------------

    assert_impl_all!(Bytes32: Copy, Clone, Default);
    assert_not_impl_all!(Bytes4000: Copy);
    assert_impl_all!(Bytes4000: Clone, Default);

    // -- Construction + accessors -------------------------------------------

    #[test]
    fn new_round_trips_to_as_slice() {
        let arr = [1_u8, 2, 3, 4];
        let v = ByteVector::<4>::new(arr);
        assert_eq!(v.as_slice(), &arr);
    }

    #[test]
    fn zero_is_all_zeros() {
        let v: Bytes32 = ByteVector::zero();
        assert_eq!(v.as_slice(), &[0_u8; 32]);
    }

    #[test]
    fn default_equals_zero() {
        let d: Bytes32 = ByteVector::default();
        let z: Bytes32 = ByteVector::zero();
        assert_eq!(d, z);
    }

    #[test]
    fn bytes4000_default_is_all_zeros() {
        let d: Bytes4000 = ByteVector::default();
        assert!(d.as_slice().iter().all(|b| *b == 0));
        assert_eq!(d.as_slice().len(), 4000);
    }

    // -- AC #2: to_hex emits exactly 0x + 2*N lowercase hex chars -----------

    #[test]
    fn to_hex_zero_bytes32() {
        let v: Bytes32 = ByteVector::zero();
        let hex = v.to_hex();
        assert_eq!(hex.len(), 2 + 2 * 32);
        assert!(hex.starts_with("0x"));
        assert_eq!(&hex[2..], &"00".repeat(32));
    }

    #[test]
    fn to_hex_emits_lowercase() {
        let v = ByteVector::<3>::new([0xde, 0xad, 0xbe]);
        assert_eq!(v.to_hex(), "0xdeadbe");
    }

    #[test]
    fn to_hex_pads_each_byte_to_two_chars() {
        let v = ByteVector::<3>::new([0x00, 0x0f, 0xa0]);
        assert_eq!(v.to_hex(), "0x000fa0");
    }

    #[test]
    fn to_hex_rejects_uppercase() {
        let v = ByteVector::<2>::new([0xab, 0xcd]);
        let h = v.to_hex();
        assert!(h.chars().skip(2).all(|c| !c.is_ascii_uppercase()));
    }

    // -- Debug formatter ----------------------------------------------------

    #[test]
    fn debug_includes_size_and_hex() {
        let v = ByteVector::<2>::new([0xab, 0xcd]);
        assert_eq!(format!("{v:?}"), "ByteVector<2>(0xabcd)");
    }

    // -- Equality / Hash ----------------------------------------------------

    #[test]
    fn eq_same_bytes() {
        let a = ByteVector::<4>::new([1, 2, 3, 4]);
        let b = ByteVector::<4>::new([1, 2, 3, 4]);
        assert_eq!(a, b);
    }

    #[test]
    fn ne_different_bytes() {
        let a = ByteVector::<4>::new([1, 2, 3, 4]);
        let b = ByteVector::<4>::new([1, 2, 3, 5]);
        assert_ne!(a, b);
    }

    #[test]
    fn hash_same_for_equal_vectors() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let a = ByteVector::<4>::new([9, 8, 7, 6]);
        let b = ByteVector::<4>::new([9, 8, 7, 6]);
        let mut ha = DefaultHasher::new();
        let mut hb = DefaultHasher::new();
        a.hash(&mut ha);
        b.hash(&mut hb);
        assert_eq!(ha.finish(), hb.finish());
    }

    // -- Copy boundary witness (N=64 is Copy, N=65 is not) ------------------

    assert_impl_all!(ByteVector<64>: Copy);
    assert_not_impl_all!(ByteVector<65>: Copy);

    // -- AC #1: round-trip property test for Bytes32 ------------------------

    proptest! {
        #[test]
        fn bytes32_new_as_slice_round_trip(arr in proptest::array::uniform32(any::<u8>())) {
            let v: Bytes32 = ByteVector::new(arr);
            prop_assert_eq!(v.as_slice(), &arr);
        }

        #[test]
        fn bytes32_to_hex_length_invariant(arr in proptest::array::uniform32(any::<u8>())) {
            let v: Bytes32 = ByteVector::new(arr);
            let h = v.to_hex();
            prop_assert_eq!(h.len(), 2 + 2 * 32);
            prop_assert!(h.starts_with("0x"));
            prop_assert!(h.chars().skip(2).all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        }

        #[test]
        fn bytes32_to_hex_round_trips_via_decode(arr in proptest::array::uniform32(any::<u8>())) {
            let v: Bytes32 = ByteVector::new(arr);
            let h = v.to_hex();
            let decoded: Vec<u8> = (2..h.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
                .collect();
            prop_assert_eq!(decoded.as_slice(), v.as_slice());
        }

        #[test]
        fn bytes4000_clone_is_equal(bytes in proptest::collection::vec(any::<u8>(), 4000..=4000)) {
            let mut arr = [0_u8; 4000];
            arr.copy_from_slice(&bytes);
            let a: Bytes4000 = ByteVector::new(arr);
            let b = a.clone();
            prop_assert_eq!(a, b);
        }
    }

    // ByteListLimitExceeded variant exists (compile witness so this file
    // breaks if the variant is renamed/removed).
    #[test]
    fn byte_list_limit_exceeded_variant_compiles() {
        let e = TypesError::ByteListLimitExceeded { limit: 0, got: 1 };
        assert!(matches!(
            e,
            TypesError::ByteListLimitExceeded { limit: 0, got: 1 }
        ));
    }
}
