//! [`Slot`] — `u64` newtype identifying a consensus slot.
//!
//! SSZ-encoded as a fixed-width little-endian `u64` and merkleized as a
//! single 32-byte chunk (low 8 bytes LE, upper 24 zero).

use crate::internal::impl_u64_ssz_newtype;

/// Consensus slot number (`u64` newtype).
///
/// # Example
/// ```
/// use protocol::Slot;
/// let s = Slot::new(42);
/// assert_eq!(s.get(), 42);
/// assert!(!s.is_zero());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Slot(u64);

impl_u64_ssz_newtype!(Slot);

impl Slot {
    /// Slot zero — the genesis slot.
    pub const ZERO: Slot = Slot(0);

    /// One slot — minimal increment used by slot-advance loops.
    pub const ONE: Slot = Slot(1);

    /// Constructs a [`Slot`] from a raw `u64`.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the underlying `u64`.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Returns `true` when the slot is zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Returns `self + 1`, or `None` on overflow. Mirrors `u64::checked_add`.
    #[must_use]
    pub const fn advance(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }

    /// Returns `true` when `self` is a valid justification candidate after
    /// `finalized` per the 3SF-mini consensus rule.
    ///
    /// A slot at distance `δ = self - finalized` from the finalized slot is
    /// justifiable when any of the following holds:
    /// - `δ ≤ 5` (small-distance window),
    /// - `δ` is a perfect square (`δ = k²` for some `k ≥ 0`),
    /// - `δ` is a pronic number (`δ = k·(k+1)` for some `k ≥ 0`).
    ///
    /// Candidates strictly before `finalized` are never justifiable.
    ///
    /// # Example
    /// ```
    /// use protocol::Slot;
    /// let f = Slot::new(0);
    /// assert!(Slot::new(5).is_justifiable_after(f));   // small window
    /// assert!(Slot::new(9).is_justifiable_after(f));   // 3²
    /// assert!(Slot::new(12).is_justifiable_after(f));  // 3·4
    /// assert!(!Slot::new(7).is_justifiable_after(f));  // neither
    /// ```
    #[must_use]
    pub const fn is_justifiable_after(self, finalized: Slot) -> bool {
        if self.0 < finalized.0 {
            return false;
        }
        let delta = self.0 - finalized.0;
        if delta <= 5 {
            return true;
        }
        // s = floor(√δ) ≤ 4_294_967_295 for any δ ≤ u64::MAX, so neither
        // s·s nor s·(s+1) overflow `u64`.
        let s = isqrt_u64(delta);
        delta == s * s || delta == s * (s + 1)
    }
}

/// Integer square root for `u64`, using Newton's iteration seeded from the
/// bit length of `n`. Returns `floor(√n)`.
const fn isqrt_u64(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    // Bit length of `n` (here 2..=64). `(bits + 1) / 2` is in 2..=32, so the
    // shift produces a tight upper bound on √n without overflow.
    let bits = u64::BITS - n.leading_zeros();
    let mut x = 1_u64 << ((bits + 1) / 2);
    loop {
        let next = (x + n / x) / 2;
        if next >= x {
            return x;
        }
        x = next;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use ssz::{decode, encode, HashTreeRoot, SszError};

    // -- Construction + accessors -------------------------------------------

    #[test]
    fn new_round_trips_to_get() {
        assert_eq!(Slot::new(0xdead_beef).get(), 0xdead_beef);
    }

    #[test]
    fn default_is_zero() {
        assert!(Slot::default().is_zero());
        assert_eq!(Slot::default().get(), 0);
    }

    #[test]
    fn from_u64_and_into_u64() {
        let s: Slot = 7_u64.into();
        let v: u64 = s.into();
        assert_eq!(v, 7);
    }

    #[test]
    fn ord_compares_underlying_values() {
        assert!(Slot::new(1) < Slot::new(2));
        assert_eq!(Slot::new(3).cmp(&Slot::new(3)), core::cmp::Ordering::Equal);
    }

    #[test]
    fn display_is_decimal() {
        assert_eq!(format!("{}", Slot::new(123)), "123");
    }

    // -- SSZ encode/decode round-trip ---------------------------------------

    #[test]
    fn ssz_encode_emits_eight_le_bytes() {
        let bytes = encode(&Slot::new(0x0123_4567_89ab_cdef));
        assert_eq!(bytes, 0x0123_4567_89ab_cdef_u64.to_le_bytes().to_vec());
    }

    #[test]
    fn ssz_round_trip_boundary_values() {
        for original in [Slot::new(0), Slot::new(1), Slot::new(u64::MAX)] {
            let bytes = encode(&original);
            let back: Slot = decode(&bytes).unwrap();
            assert_eq!(back, original);
        }
    }

    #[test]
    fn ssz_decode_rejects_wrong_length() {
        let err = decode::<Slot>(&[0_u8; 4]).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    // -- HashTreeRoot --------------------------------------------------------

    #[test]
    fn hash_tree_root_is_le_chunk_with_zero_upper() {
        let root = Slot::new(0xdead_beef).hash_tree_root();
        assert_eq!(&root[..8], &0xdead_beef_u64.to_le_bytes());
        assert!(root[8..].iter().all(|&b| b == 0));
    }

    #[test]
    fn hash_tree_root_zero_is_zero_chunk() {
        assert_eq!(Slot::new(0).hash_tree_root(), [0_u8; 32]);
    }

    // -- advance -----------------------------------------------------------

    #[test]
    fn advance_increments_by_one() {
        assert_eq!(Slot::ZERO.advance(), Some(Slot::ONE));
        assert_eq!(Slot::new(41).advance(), Some(Slot::new(42)));
    }

    #[test]
    fn advance_at_max_is_none() {
        assert_eq!(Slot::new(u64::MAX).advance(), None);
    }

    // -- isqrt_u64 -----------------------------------------------------------

    #[test]
    fn isqrt_known_values() {
        let cases = [
            (0_u64, 0_u64),
            (1, 1),
            (2, 1),
            (3, 1),
            (4, 2),
            (8, 2),
            (9, 3),
            (15, 3),
            (16, 4),
            (99, 9),
            (100, 10),
            (101, 10),
            (1024, 32),
            (1_048_576, 1_024),
        ];
        for (n, want) in cases {
            assert_eq!(isqrt_u64(n), want, "isqrt({n})");
        }
    }

    #[test]
    fn isqrt_max_u64_is_floor_sqrt() {
        // 4_294_967_295² = 18_446_744_065_119_617_025 ≤ u64::MAX.
        // 4_294_967_296² overflows u64.
        assert_eq!(isqrt_u64(u64::MAX), 4_294_967_295);
    }

    // -- is_justifiable_after — table-driven oracle (delta ∈ 0..=1024) -----

    #[test]
    fn is_justifiable_after_matches_integer_oracle() {
        const CEILING: u64 = 1024;

        // Independent integer-only oracle: enumerate perfect squares + pronics
        // up to CEILING without using sqrt.
        let mut oracle = std::collections::BTreeSet::new();
        let mut k = 0_u64;
        while k * k <= CEILING {
            oracle.insert(k * k);
            k += 1;
        }
        let mut k = 0_u64;
        while k * (k + 1) <= CEILING {
            oracle.insert(k * (k + 1));
            k += 1;
        }

        let finalized = Slot::new(0);
        for delta in 0..=CEILING {
            let got = Slot::new(delta).is_justifiable_after(finalized);
            let want = delta <= 5 || oracle.contains(&delta);
            assert_eq!(got, want, "δ = {delta}");
        }
    }

    #[test]
    fn is_justifiable_after_rejects_candidate_before_finalized() {
        let finalized = Slot::new(10);
        for c in 0..10_u64 {
            assert!(!Slot::new(c).is_justifiable_after(finalized));
        }
    }

    #[test]
    fn is_justifiable_after_small_window_inclusive_of_5() {
        let f = Slot::new(100);
        for d in 0..=5_u64 {
            assert!(Slot::new(100 + d).is_justifiable_after(f));
        }
    }

    #[test]
    fn is_justifiable_after_seven_is_not_justifiable() {
        // δ = 7 is neither perfect-square nor pronic and exceeds the small window.
        assert!(!Slot::new(7).is_justifiable_after(Slot::new(0)));
    }

    // -- property tests ------------------------------------------------------

    proptest! {
        #[test]
        fn ssz_round_trips(value in any::<u64>()) {
            let s = Slot::new(value);
            let back: Slot = decode(&encode(&s)).unwrap();
            prop_assert_eq!(back, s);
        }

        #[test]
        fn isqrt_floor_property(n in 0_u64..=1_000_000) {
            let r = isqrt_u64(n);
            prop_assert!(r * r <= n);
            prop_assert!((r + 1).saturating_mul(r + 1) > n);
        }
    }
}
