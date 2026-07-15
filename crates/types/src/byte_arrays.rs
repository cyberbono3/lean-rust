//! Fixed-width byte vectors used by the Lean consensus protocol.
//!
//! [`ByteVector<N>`] is a const-generic newtype over `[u8; N]` providing the
//! SSZ `Vector[byte, N]` shape. Aliases used by the consensus protocol:
//! [`Bytes32`] (e.g. block roots, state roots), [`Signature`] and
//! [`PublicKey`] (the devnet-1 XMSS wire types), and the deprecated
//! [`Bytes4000`] signature placeholder that [`Signature`] replaces.
//!
//! # `Copy` semantics
//! `ByteVector<N>` does NOT derive [`Copy`]. Explicit `impl Copy` is provided
//! for `N` in `0..=64` so small fixed-width types ([`Bytes32`], [`PublicKey`],
//! addresses, hashes) move cheaply. Larger sizes — [`Signature`] and
//! [`Bytes4000`] — are intentionally `Clone`-only to prevent silent multi-KB
//! stack copies.

use core::fmt::{self, Write};

/// Fixed-width byte vector of length `N`.
///
/// The inner array is `pub` so callers may pattern-match or take a
/// `&[u8; N]` directly when needed.
///
/// # Example
/// ```
/// use types::{ByteVector, Bytes32};
/// let zero: Bytes32 = ByteVector::zero();
/// assert_eq!(zero.as_slice(), &[0_u8; 32]);
/// assert_eq!(zero.to_hex(), format!("0x{}", "00".repeat(32)));
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ByteVector<const N: usize>(pub [u8; N]);

/// 32-byte vector — block roots, state roots, validator pubkey hashes.
pub type Bytes32 = ByteVector<32>;

/// 3116-byte vector — XMSS post-quantum signature on the devnet-1 wire.
///
/// Mirrors the consensus spec's `Signature(Bytes3116)`. Links are pinned to the
/// spec revision this width was taken from; the paths have since moved upstream,
/// so a branch link would not resolve:
/// - [`class Signature(Bytes3116)`](https://github.com/leanEthereum/leanSpec/blob/050fa4a18881d54d7dc07601fe59e34eb20b9630/src/lean_spec/subspecs/containers/signature.py#L12)
/// - [`Bytes3116.LENGTH = 3116`](https://github.com/leanEthereum/leanSpec/blob/050fa4a18881d54d7dc07601fe59e34eb20b9630/src/lean_spec/types/byte_arrays.py#L241)
///
/// The width is a devnet-1 interop parameter, not a permanent constant — later
/// devnets may change it. Every participating client must agree on it.
///
/// Intentionally not [`Copy`] — 3116 exceeds the `0..=64` range covered by the
/// `Copy` impls below, so every move/clone is an explicit ~3 KB byte copy. Pass
/// by reference (`&Signature`) wherever ownership is not needed.
pub type Signature = ByteVector<3116>;

/// 52-byte vector — XMSS one-time-signature public key on the devnet-1 wire.
///
/// Mirrors the consensus spec's `Validator.pubkey: Bytes52`. Links are pinned to
/// the spec revision this width was taken from; the paths have since moved
/// upstream, so a branch link would not resolve:
/// - [`Validator.pubkey: Bytes52`](https://github.com/leanEthereum/leanSpec/blob/050fa4a18881d54d7dc07601fe59e34eb20b9630/src/lean_spec/subspecs/containers/validator.py#L15)
/// - [`Bytes52.LENGTH = 52`](https://github.com/leanEthereum/leanSpec/blob/050fa4a18881d54d7dc07601fe59e34eb20b9630/src/lean_spec/types/byte_arrays.py#L229)
///
/// The width is a devnet-1 interop parameter, not a permanent constant — later
/// devnets may change it. Every participating client must agree on it.
///
/// 52 falls inside the `0..=64` range covered by the `Copy` impls below, so
/// this type is [`Copy`].
pub type PublicKey = ByteVector<52>;

/// 4000-byte vector — signature placeholder (Lean devnet0 wire format).
///
/// Intentionally not [`Copy`] — every move/clone is an explicit 4 KB byte
/// copy. Pass by reference (`&Bytes4000`) wherever ownership is not needed.
#[deprecated(
    note = "devnet-1 replaces the Bytes4000 placeholder with Signature (Bytes3116); \
            remaining construction sites migrate with the container refactor"
)]
pub type Bytes4000 = ByteVector<4000>;

impl<const N: usize> ByteVector<N> {
    /// Width in bytes — the single source of truth for `N`.
    ///
    /// Resolves through the aliases (`Signature::LEN` is `3116`), so consumers
    /// read the width off the type instead of restating the literal.
    ///
    /// Equals the SSZ wire size, because [`Self::as_slice`] yields exactly `N`
    /// bytes. That is a property of this type, not an encoder contract:
    /// `ByteVector` has no `Encode` impl, and the containers still carry their
    /// own wire-size constants. This is the width the container refactor is
    /// expected to read once the wire moves to `Signature`, not a value any
    /// encoder consults today.
    ///
    /// # Example
    /// ```
    /// use types::{ByteVector, Signature};
    /// assert_eq!(ByteVector::<4>::LEN, 4);
    /// assert_eq!(Signature::LEN, 3116);
    /// ```
    pub const LEN: usize = N;

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

impl<const N: usize> From<[u8; N]> for ByteVector<N> {
    fn from(bytes: [u8; N]) -> Self {
        Self(bytes)
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

// The `deprecated` allow covers the Bytes4000 witnesses below, which
// deliberately exercise the deprecated alias. Scoped to this test module —
// retires with the alias itself, and is NOT part of the file-level allow set
// carried by the construction sites. Kept as an outer attribute alongside the
// others: mixing inner and outer attributes on one item trips
// `clippy::mixed_attributes_style`.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, deprecated)]
mod tests {
    use super::*;
    use crate::error::TypesError;
    use proptest::prelude::*;
    use static_assertions::{assert_impl_all, assert_not_impl_all, assert_type_eq_all};

    // -- Compile-time witness: Bytes32 is Copy, Bytes4000 is NOT -----------

    assert_impl_all!(Bytes32: Copy, Clone, Default);
    assert_not_impl_all!(Bytes4000: Copy);
    assert_impl_all!(Bytes4000: Clone, Default);

    // -- devnet-1 wire newtypes: shape, width, Copy split ------------------

    // The aliases are structural, not nominal: `Signature` IS `ByteVector<3116>`
    // and is interchangeable with any other `ByteVector<3116>`. Asserted rather
    // than hidden.
    //
    // This says nothing about serde, and is not a no-serde witness: it would
    // pass unchanged if serde derives were added. Domain purity rests on
    // `types` declaring no serde dependency at all (see Cargo.toml) — a derive
    // could not compile without one.
    assert_type_eq_all!(Signature, ByteVector<3116>);
    assert_type_eq_all!(PublicKey, ByteVector<52>);

    // 52 <= 64, so `PublicKey` is covered by the Copy macro below; 3116 is not,
    // keeping `Signature` Clone-only to prevent silent 3 KB stack copies.
    assert_impl_all!(PublicKey: Copy, Clone, Default);
    assert_not_impl_all!(Signature: Copy);
    assert_impl_all!(Signature: Clone, Default);

    #[test]
    fn sizes_match_spec() {
        // The spec widths, pinned against the literals they were taken from.
        assert_eq!(Signature::LEN, 3116);
        assert_eq!(PublicKey::LEN, 52);

        // `LEN` equals the number of bytes that reach the wire. This is the
        // property that makes `LEN` usable as a wire size; `size_of` below is a
        // separate, in-memory claim and says nothing about serialization.
        assert_eq!(Signature::zero().as_slice().len(), Signature::LEN);
        assert_eq!(PublicKey::zero().as_slice().len(), PublicKey::LEN);

        // No padding, so the value costs exactly its payload to move — which is
        // what the `Copy` split above is reasoning about (3 KB vs 52 bytes).
        assert_eq!(core::mem::size_of::<Signature>(), Signature::LEN);
        assert_eq!(core::mem::size_of::<PublicKey>(), PublicKey::LEN);
    }

    // Widths come off `LEN` rather than a restated literal — `sizes_match_spec`
    // is the one place that pins `LEN` to the spec numbers.
    #[test]
    fn signature_new_round_trips_to_as_slice() {
        let sig = Signature::new([0x5a; Signature::LEN]);
        assert_eq!(sig.as_slice(), &[0x5a; Signature::LEN][..]);
        assert_eq!(sig.as_slice().len(), Signature::LEN);
    }

    #[test]
    fn publickey_new_round_trips_to_as_slice() {
        let pk = PublicKey::new([0xa5; PublicKey::LEN]);
        assert_eq!(pk.as_slice(), &[0xa5; PublicKey::LEN][..]);
        assert_eq!(pk.as_slice().len(), PublicKey::LEN);
    }

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

    // -- to_hex emits exactly 0x + 2*N lowercase hex chars -----------------

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

    // -- round-trip property test for Bytes32 ------------------------------

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
