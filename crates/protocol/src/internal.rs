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
use ssz::{Decode, DecodeError};

/// Number of bytes used by an SSZ length-offset (variable-length container
/// fixed-portion entry).
pub(crate) const BYTES_PER_LENGTH_OFFSET: usize = 4;

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

/// Returns `Ok(())` when `bytes.len() == expected`, otherwise
/// [`DecodeError::InvalidByteLength`].
pub(crate) fn ensure_len(bytes: &[u8], expected: usize) -> Result<(), DecodeError> {
    if bytes.len() == expected {
        Ok(())
    } else {
        Err(DecodeError::InvalidByteLength {
            len: bytes.len(),
            expected,
        })
    }
}

/// Reads a fixed-length value `T` from `bytes[*cursor..]` and advances
/// `*cursor` by `T::ssz_fixed_len()`.
///
/// Caller is responsible for verifying that the slice covers the read.
pub(crate) fn read_fixed<T: Decode>(bytes: &[u8], cursor: &mut usize) -> Result<T, DecodeError> {
    let len = <T as Decode>::ssz_fixed_len();
    let value = T::from_ssz_bytes(&bytes[*cursor..*cursor + len])?;
    *cursor += len;
    Ok(value)
}

/// Reads `N` raw bytes from `bytes[*cursor..]` into a stack array and
/// advances `*cursor` by `N`. Panic-free for in-bounds slices because
/// `<[u8]>::copy_from_slice` is total when both sides have equal length.
pub(crate) fn read_byte_array<const N: usize>(bytes: &[u8], cursor: &mut usize) -> [u8; N] {
    let mut arr = [0_u8; N];
    arr.copy_from_slice(&bytes[*cursor..*cursor + N]);
    *cursor += N;
    arr
}

/// Writes a 4-byte little-endian SSZ length-offset to `buf`.
///
/// Truncates the offset to `u32::MAX` when `value` exceeds `u32` range.
/// Variable-length SSZ containers in this crate keep their fixed-portion
/// well below `u32::MAX`, so the saturation never fires in practice but is
/// preferred over `as u32` for static-analysis cleanliness.
pub(crate) fn write_offset(buf: &mut Vec<u8>, value: usize) {
    let offset = u32::try_from(value).unwrap_or(u32::MAX);
    buf.extend_from_slice(&offset.to_le_bytes());
}

/// Reads a 4-byte little-endian SSZ length-offset from `bytes[*cursor..]`
/// and advances `*cursor` by 4.
pub(crate) fn read_offset(bytes: &[u8], cursor: &mut usize) -> Result<usize, DecodeError> {
    let value = u32::from_ssz_bytes(&bytes[*cursor..*cursor + BYTES_PER_LENGTH_OFFSET])?;
    *cursor += BYTES_PER_LENGTH_OFFSET;
    Ok(value as usize)
}

/// Encodes a list of fixed-size SSZ elements as a flat concatenation.
///
/// Suitable for `List[T, MAX]` and `Vector[T, N]` where `T::is_ssz_fixed_len()`.
pub(crate) fn encode_fixed_element_list<T: ssz::Encode>(items: &[T], buf: &mut Vec<u8>) {
    for item in items {
        item.ssz_append(buf);
    }
}

/// Decodes a list of fixed-size SSZ elements from a flat concatenation,
/// rejecting inputs whose length is not divisible by the element size or
/// whose element count exceeds `max`.
pub(crate) fn decode_fixed_element_list<T: Decode>(
    bytes: &[u8],
    max: usize,
) -> Result<Vec<T>, DecodeError> {
    let elem_len = <T as Decode>::ssz_fixed_len();
    if elem_len == 0 {
        return Err(DecodeError::ZeroLengthItem);
    }
    if bytes.len() % elem_len != 0 {
        return Err(DecodeError::BytesInvalid(format!(
            "list bytes ({}) not divisible by element size ({})",
            bytes.len(),
            elem_len,
        )));
    }
    let count = bytes.len() / elem_len;
    if count > max {
        return Err(DecodeError::BytesInvalid(format!(
            "list length ({count}) exceeds max ({max})",
        )));
    }
    let mut items = Vec::with_capacity(count);
    for i in 0..count {
        let start = i * elem_len;
        items.push(T::from_ssz_bytes(&bytes[start..start + elem_len])?);
    }
    Ok(items)
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
