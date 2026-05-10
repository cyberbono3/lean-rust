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
pub mod merkleize;

pub use crate::decode::decode;
pub use crate::encode::encode;
pub use crate::error::{DecodeErrorAdapter, SszError};
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
}
