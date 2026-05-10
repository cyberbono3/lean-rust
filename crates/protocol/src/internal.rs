//! Crate-private helpers shared by the consensus newtype and container
//! modules.
//!
//! - [`u64_chunk`] encodes a `u64` as the canonical 32-byte SSZ basic-type
//!   Merkle chunk (low 8 bytes LE, upper 24 zero).
//! - [`bytes_vector_hash_tree_root`] computes the SSZ hash-tree-root of a
//!   fixed-length byte vector (`Vector[byte, N]`) — pack into 32-byte
//!   chunks then merkleize.
//! - [`impl_u64_ssz_newtype`] generates the boilerplate `Encode` / `Decode` /
//!   [`ssz::HashTreeRoot`] / `From` / `Display` impls for a `u64` newtype.

use ssz::merkleize::{merkleize, pack};

/// Encodes `value` as the canonical 32-byte SSZ basic-type Merkle chunk
/// (low 8 bytes LE, upper 24 zero).
pub(crate) const fn u64_chunk(value: u64) -> [u8; 32] {
    let mut out = [0_u8; 32];
    let bytes = value.to_le_bytes();
    let mut i = 0;
    while i < 8 {
        out[i] = bytes[i];
        i += 1;
    }
    out
}

/// Computes the SSZ hash-tree-root of a fixed-length byte vector
/// (`Vector[byte, N]`): pack the bytes into 32-byte chunks (right-padding
/// the final chunk with zeros if `N` is not a multiple of 32) and
/// merkleize. The merkleizer zero-extends to the next power of two width.
pub(crate) fn bytes_vector_hash_tree_root(bytes: &[u8]) -> [u8; 32] {
    merkleize(&pack(bytes))
}

/// Generates the standard SSZ codec, [`ssz::HashTreeRoot`], `From`, and
/// `Display` impls for a `u64` newtype.
///
/// Invoke at module scope after declaring the `pub struct $name(u64);`. The
/// caller controls the struct definition (and any extra inherent methods)
/// so type-specific behaviour remains visible.
macro_rules! impl_u64_ssz_newtype {
    ($name:ident) => {
        impl ::core::convert::From<u64> for $name {
            fn from(value: u64) -> Self {
                Self(value)
            }
        }

        impl ::core::convert::From<$name> for u64 {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::fmt::Display::fmt(&self.0, f)
            }
        }

        impl ::ssz::Encode for $name {
            fn is_ssz_fixed_len() -> bool {
                true
            }

            fn ssz_fixed_len() -> usize {
                <u64 as ::ssz::Encode>::ssz_fixed_len()
            }

            fn ssz_bytes_len(&self) -> usize {
                <u64 as ::ssz::Encode>::ssz_fixed_len()
            }

            fn ssz_append(&self, buf: &mut Vec<u8>) {
                self.0.ssz_append(buf);
            }
        }

        impl ::ssz::Decode for $name {
            fn is_ssz_fixed_len() -> bool {
                true
            }

            fn ssz_fixed_len() -> usize {
                <u64 as ::ssz::Decode>::ssz_fixed_len()
            }

            fn from_ssz_bytes(bytes: &[u8]) -> ::core::result::Result<Self, ::ssz::DecodeError> {
                <u64 as ::ssz::Decode>::from_ssz_bytes(bytes).map(Self)
            }
        }

        impl ::ssz::HashTreeRoot for $name {
            fn hash_tree_root(&self) -> [u8; 32] {
                $crate::internal::u64_chunk(self.0)
            }
        }
    };
}

pub(crate) use impl_u64_ssz_newtype;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::{bytes_vector_hash_tree_root, u64_chunk};
    use ssz::merkleize::{merkleize, pack};

    #[test]
    fn u64_chunk_zero_is_zero_chunk() {
        assert_eq!(u64_chunk(0), [0_u8; 32]);
    }

    #[test]
    fn u64_chunk_low_eight_bytes_le_rest_zero() {
        let chunk = u64_chunk(0xdead_beef);
        assert_eq!(&chunk[..8], &0xdead_beef_u64.to_le_bytes());
        assert!(chunk[8..].iter().all(|&b| b == 0));
    }

    #[test]
    fn bytes_vector_htr_matches_pack_then_merkleize() {
        let payload = [0x77_u8; 4000];
        assert_eq!(
            bytes_vector_hash_tree_root(&payload),
            merkleize(&pack(&payload))
        );
    }

    #[test]
    fn bytes_vector_htr_changes_with_input() {
        let a = [0x11_u8; 4000];
        let mut b = a;
        b[0] = 0x12;
        assert_ne!(
            bytes_vector_hash_tree_root(&a),
            bytes_vector_hash_tree_root(&b)
        );
    }
}
