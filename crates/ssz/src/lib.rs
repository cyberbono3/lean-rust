//! SSZ encode/decode + SHA-256 merkleization facade over [`eth_ssz`]
//! (`ethereum_ssz` upstream).
//!
//! This crate is the single project-internal SSZ entry point: downstream
//! crates depend on `ssz`, never on `ethereum_ssz` directly. The upstream
//! choice is therefore swappable in one place.
//!
//! # Scope
//! - [`Encode`] / [`Decode`] re-exported from [`eth_ssz`].
//! - [`encode`] — convenience free function returning `Vec<u8>`.
//! - [`decode`] — convenience free function returning `Result<T, SszError>`.
//! - [`SszError`] — facade error type wrapping [`DecodeError`] via
//!   [`DecodeErrorAdapter`] for the [`std::error::Error::source`] chain;
//!   plus merkleization-specific variants ([`SszError::InvalidNumLeaves`],
//!   [`SszError::InputExceedsLimit`]).
//! - [`merkleize`] module — SHA-256-based Merkle root helpers
//!   (`pack`, `merkleize`, `merkleize_with_limit`, `merkleize_progressive`,
//!   `mix_in_length`, `mix_in_selector`, `zero_tree_root`, `hash_pair`).
//! - [`HashTreeRoot`] trait — extension point for typed merkleization in
//!   downstream consensus crates.
//!
//! # Example
//! ```
//! use ssz::{decode, encode, SszError};
//!
//! # fn main() -> Result<(), SszError> {
//! let bytes = encode(&0xdead_beef_u64);
//! let round_trip: u64 = decode(&bytes)?;
//! assert_eq!(round_trip, 0xdead_beef);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

pub mod decode;
pub mod encode;
pub mod error;
pub mod lists;
pub mod merkleize;

/// Shared SSZ test helpers (SA2). Gated behind the `test-support` feature so
/// downstream crates opt in via a dev-dependency; never in a production build.
#[cfg(feature = "test-support")]
pub mod test_support;

pub use crate::decode::decode;
pub use crate::encode::encode;
pub use crate::error::{DecodeErrorAdapter, SszError};
pub use crate::lists::{
    decode_bytes32_list, decode_fixed_element_list, decode_variable_element_list,
    encode_bytes32_list, encode_fixed_element_list, encode_variable_element_list,
    BYTES_PER_LENGTH_OFFSET,
};
pub use eth_ssz::{Decode, DecodeError, Encode};

/// Computes a 32-byte SSZ hash-tree-root for a typed value.
///
/// Implementations land in the consensus crates (`protocol`, `engine`, …)
/// where each container declares its merkleization shape. The trait is the
/// single project-internal entry point so downstream callers depend on
/// `ssz::HashTreeRoot` and never on a third-party tree-hashing crate.
pub trait HashTreeRoot {
    /// Returns the SSZ hash-tree-root of `self`.
    fn hash_tree_root(&self) -> [u8; 32];
}

/// `Vector[byte, N]` hash-tree-root: pack the bytes into 32-byte chunks
/// (right-padding the final chunk with zeros if `N` is not a multiple of 32)
/// and merkleize. The merkleizer zero-extends to the next power-of-two width.
impl<const N: usize> HashTreeRoot for types::ByteVector<N> {
    fn hash_tree_root(&self) -> [u8; 32] {
        crate::merkleize::merkleize(&crate::merkleize::pack(self.as_slice()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::error::Error as _;

    // ---------------------------------------------------------------------
    // Primitive round-trip via the facade
    // ---------------------------------------------------------------------

    #[test]
    fn u64_round_trip_via_facade() {
        let value: u64 = 0x0123_4567_89ab_cdef;
        let bytes = encode(&value);
        assert_eq!(bytes, value.to_le_bytes().to_vec());
        let decoded: u64 = decode(&bytes).unwrap();
        assert_eq!(decoded, value);
    }

    #[test]
    fn vec_u8_round_trip_via_facade() {
        let value: Vec<u8> = vec![0x01, 0x02, 0x03, 0xff];
        let bytes = encode(&value);
        // SSZ Vec<u8> is the raw byte sequence (no length prefix).
        assert_eq!(bytes, value);
        let decoded: Vec<u8> = decode(&bytes).unwrap();
        assert_eq!(decoded, value);
    }

    #[test]
    fn vec_u8_empty_round_trip() {
        let value: Vec<u8> = Vec::new();
        let bytes = encode(&value);
        assert!(bytes.is_empty());
        let decoded: Vec<u8> = decode(&bytes).unwrap();
        assert_eq!(decoded, value);
    }

    #[test]
    fn fixed_array_u8_32_round_trip() {
        let mut value = [0_u8; 32];
        for (i, b) in value.iter_mut().enumerate() {
            *b = u8::try_from(i & 0xff).unwrap();
        }
        let bytes = encode(&value);
        assert_eq!(bytes.len(), 32);
        assert_eq!(bytes, value.to_vec());
        let decoded: [u8; 32] = decode(&bytes).unwrap();
        assert_eq!(decoded, value);
    }

    // ---------------------------------------------------------------------
    // SszError carries the underlying DecodeError via #[source]
    // ---------------------------------------------------------------------

    #[test]
    fn decode_invalid_byte_length_for_u64_returns_ssz_error() {
        // u64 is fixed-length 8 bytes; supplying 4 bytes triggers the upstream
        // InvalidByteLength variant.
        let err: SszError = decode::<u64>(&[0_u8; 4]).unwrap_err();
        match &err {
            SszError::Decode { source } => match &source.0 {
                DecodeError::InvalidByteLength { len, expected } => {
                    assert_eq!(*len, 4);
                    assert_eq!(*expected, 8);
                }
                other => panic!("unexpected upstream variant: {other:?}"),
            },
            other => panic!("unexpected SszError variant: {other:?}"),
        }
    }

    #[test]
    fn ssz_error_source_chain_returns_adapter() {
        let err: SszError = decode::<u64>(&[0_u8; 4]).unwrap_err();
        let source = err.source().expect("Decode variant has #[source]");
        let adapter = source
            .downcast_ref::<DecodeErrorAdapter>()
            .expect("source is a DecodeErrorAdapter");
        assert!(matches!(
            adapter.0,
            DecodeError::InvalidByteLength {
                len: 4,
                expected: 8
            }
        ));
    }

    #[test]
    fn ssz_error_from_decode_error_round_trips_inner_value() {
        let upstream = DecodeError::ZeroLengthItem;
        let err: SszError = upstream.clone().into();
        match err {
            SszError::Decode { source } => assert_eq!(source.0, upstream),
            other => panic!("unexpected SszError variant: {other:?}"),
        }
    }

    #[test]
    fn ssz_error_display_includes_upstream_variant() {
        let err: SszError = DecodeError::ZeroLengthItem.into();
        let rendered = format!("{err}");
        assert!(rendered.starts_with("ssz decode failed: "));
        assert!(rendered.contains("ZeroLengthItem"));
    }

    // ---------------------------------------------------------------------
    // Encode/Decode trait re-exports — direct facade access for downstream
    // ---------------------------------------------------------------------

    #[test]
    fn re_exported_encode_trait_method_matches_free_fn() {
        let value: u32 = 0xdead_beef;
        // Using the trait method via the facade re-export.
        let direct = <u32 as Encode>::as_ssz_bytes(&value);
        let via_facade = encode(&value);
        assert_eq!(direct, via_facade);
    }

    #[test]
    fn re_exported_decode_trait_method_matches_free_fn() {
        let value: u32 = 0xdead_beef;
        let bytes = encode(&value);
        let direct = <u32 as Decode>::from_ssz_bytes(&bytes).unwrap();
        let via_facade: u32 = decode(&bytes).unwrap();
        assert_eq!(direct, via_facade);
    }

    // ---------------------------------------------------------------------
    // Property tests — round-trip for arbitrary inputs
    // ---------------------------------------------------------------------

    proptest! {
        #[test]
        fn u64_round_trips(value in any::<u64>()) {
            let bytes = encode(&value);
            let decoded: u64 = decode(&bytes).unwrap();
            prop_assert_eq!(decoded, value);
        }

        #[test]
        fn u32_round_trips(value in any::<u32>()) {
            let bytes = encode(&value);
            let decoded: u32 = decode(&bytes).unwrap();
            prop_assert_eq!(decoded, value);
        }

        #[test]
        fn vec_u8_round_trips(bytes in proptest::collection::vec(any::<u8>(), 0..=1024)) {
            let encoded = encode(&bytes);
            let decoded: Vec<u8> = decode(&encoded).unwrap();
            prop_assert_eq!(decoded, bytes);
        }

        #[test]
        fn fixed_array_u8_32_round_trips(arr in proptest::array::uniform32(any::<u8>())) {
            let bytes = encode(&arr);
            let decoded: [u8; 32] = decode(&bytes).unwrap();
            prop_assert_eq!(decoded, arr);
        }
    }

    // ---------------------------------------------------------------------
    // Hash-tree-root of the devnet-1 wire byte-vectors
    //
    // `Signature` / `PublicKey` inherit `HashTreeRoot` from the blanket
    // `impl<const N: usize> HashTreeRoot for types::ByteVector<N>` above — no
    // per-type impl exists, so these guard the generic path at the two widths
    // that go on the wire.
    //
    // Two kinds of vector below, because they pin different things:
    //
    // - The ZERO roots pin tree depth only. An all-zero root depends solely on
    //   `power_of_two_ceil(ceil(N / 32))`, so it is CONSTANT across huge width
    //   ranges: every N in 2049..=4096 roots to 87eb0d…, and that range includes
    //   4000 — the deprecated `Bytes4000` width this type replaces. A zero root
    //   therefore CANNOT witness the width, and is useless as an interop vector.
    //   Zero input also hides final-chunk padding: with all-zero bytes, right-pad,
    //   left-pad, and no-pad are indistinguishable.
    //
    // - The NON-ZERO roots are the real width pins and the devnet-1 interop
    //   vectors. A distinct fill byte makes the root depend on the exact width
    //   (3116 and 3117 differ) and on the final chunk's padding.
    //
    // Both widths have a partial final chunk — 3116 = 97*32 + 12, 52 = 32 + 20 —
    // so the pad occupies 20 and 12 bytes respectively. That is the only
    // structurally interesting property of either width, and only the non-zero
    // vectors cover it.
    //
    // All four expectations below were cross-checked against the consensus
    // spec's own `hash_tree_root` at the pinned revision (leanSpec@050fa4a,
    // `lean_spec.subspecs.ssz.hash`), so they agree with the spec and not merely
    // with this crate.
    // ---------------------------------------------------------------------

    /// Reference zero-hash recursion — deliberately independent of
    /// `merkleize`/`pack`, so a bug in either is not mirrored into the
    /// expectation.
    ///
    /// `merkleize::zero_tree_root` computes the same value and is deliberately
    /// NOT used: it lives in the crate under test, and re-deriving here from
    /// `sha2` alone keeps the expectation independent of the code it checks.
    fn zero_hash_at_depth(depth: u32) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut h = [0_u8; 32];
        for _ in 0..depth {
            let mut hasher = Sha256::new();
            hasher.update(h);
            hasher.update(h);
            h = hasher.finalize().into();
        }
        h
    }

    #[test]
    fn signature_zero_htr_pins_tree_depth_only() {
        // 3116 bytes -> 98 chunks -> zero-extended to 128 leaves -> depth 7.
        // 98 is NOT a power of two, so this exercises the zero-extension path.
        // This does NOT pin the width — see `signature_htr_matches_pinned_vector`.
        let root = types::Signature::zero().hash_tree_root();

        let expected: [u8; 32] =
            hex_to_32("87eb0ddba57e35f6d286673802a4af5975e22506c7cf4c64bb6be5ee11527f2c");
        assert_eq!(root, expected, "Signature zero-root moved");
        assert_eq!(
            root,
            zero_hash_at_depth(7),
            "zero-extension to 128 leaves broke"
        );
    }

    #[test]
    fn publickey_zero_htr_pins_tree_depth_only() {
        // 52 bytes -> 2 chunks -> already a power of two -> depth 1.
        // This root is the well-known consensus-spec depth-1 zero-hash.
        // This does NOT pin the width — see `publickey_htr_matches_pinned_vector`.
        let root = types::PublicKey::zero().hash_tree_root();

        let expected: [u8; 32] =
            hex_to_32("f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b");
        assert_eq!(root, expected, "PublicKey zero-root moved");
        assert_eq!(root, zero_hash_at_depth(1), "2-leaf merkleization broke");
    }

    /// Width- and padding-discriminating interop vector for the 3116-byte width.
    ///
    /// Cross-checked against the consensus spec's own `hash_tree_root` at the
    /// pinned revision — `hash_tree_root(Bytes3116(b"\x5a" * 3116))` on
    /// [leanSpec@050fa4a](https://github.com/leanEthereum/leanSpec/tree/050fa4a18881d54d7dc07601fe59e34eb20b9630)
    /// returns this value. It is therefore a genuine interop vector, not merely
    /// self-consistent with this crate.
    ///
    /// Unlike the zero root, it changes if the width changes at all (3116 vs
    /// 3117 vs 4000) or if the final chunk's 20 pad bytes move.
    #[test]
    fn signature_htr_matches_pinned_vector() {
        let root = types::Signature::new([0x5a; types::Signature::LEN]).hash_tree_root();

        let expected: [u8; 32] =
            hex_to_32("074c83eb750d70d0e1168a6b3950ac492cb11ba8653c0160ea4d1740d4f7e7e7");
        assert_eq!(
            root, expected,
            "devnet-1 Signature interop vector moved — width or final-chunk padding changed"
        );
    }

    /// Width- and padding-discriminating interop vector for the 52-byte width.
    ///
    /// Cross-checked against the consensus spec's own `hash_tree_root` at the
    /// pinned revision — `hash_tree_root(Bytes52(b"\xa5" * 52))` on
    /// [leanSpec@050fa4a](https://github.com/leanEthereum/leanSpec/tree/050fa4a18881d54d7dc07601fe59e34eb20b9630)
    /// returns this value. It is therefore a genuine interop vector, not merely
    /// self-consistent with this crate.
    ///
    /// Unlike the zero root, it changes if the width changes at all (52 vs 48 vs
    /// 60) or if the final chunk's 12 pad bytes move.
    #[test]
    fn publickey_htr_matches_pinned_vector() {
        let root = types::PublicKey::new([0xa5; types::PublicKey::LEN]).hash_tree_root();

        let expected: [u8; 32] =
            hex_to_32("ce1646f97d08a4f48e8f811835ab0fc34a2a6f8917b5761308838162e0041927");
        assert_eq!(
            root, expected,
            "devnet-1 PublicKey interop vector moved — width or final-chunk padding changed"
        );
    }

    /// Decodes a 64-char hex string into a 32-byte array (test-only helper).
    ///
    /// `hex::decode` rejects odd lengths and non-hex digits, naming the offending
    /// byte; the `try_into` pins the 32-byte width. These inputs are hand-written
    /// constants, so a typo is the expected failure mode and both messages beat
    /// an opaque slice-index panic.
    fn hex_to_32(s: &str) -> [u8; 32] {
        hex::decode(s)
            .expect("test vector must be valid hex")
            .try_into()
            .expect("test vector must be 32 bytes")
    }
}
