//! Crate-private helpers shared by the consensus newtype and container
//! modules.
//!
//! - [`u64_chunk`] encodes a `u64` as the canonical 32-byte SSZ basic-type
//!   Merkle chunk (low 8 bytes LE, upper 24 zero).
//! - [`list_hash_tree_root`] computes the SSZ hash-tree-root of `List[T, max]`.
//! - [`bitlist_hash_tree_root`] computes the SSZ hash-tree-root of
//!   `Bitlist[LIMIT]`.
//! - [`impl_u64_ssz_newtype`] generates the boilerplate `Encode` / `Decode` /
//!   [`ssz::HashTreeRoot`] / `From` / `Display` impls for a `u64` newtype.

use ssz::merkleize::{merkleize_with_limit, mix_in_length, pack, ZERO_HASH};
use ssz::{Decode, DecodeError, HashTreeRoot};
use types::{Bitlist, Bytes32};

/// Number of bytes used by an SSZ length-offset (variable-length container
/// fixed-portion entry).
pub(crate) const BYTES_PER_LENGTH_OFFSET: usize = 4;

/// Wire size (bytes) of a `u64` SSZ field.
pub(crate) const U64_LEN: usize = 8;

/// Wire size (bytes) of a [`types::Bytes32`] SSZ field.
pub(crate) const BYTES32_LEN: usize = 32;

/// Wire size (bytes) of a [`types::Signature`] SSZ field.
///
/// This is the container *width*, not the signature payload length:
/// [`types::Signature`] is a padded envelope that carries a shorter
/// scheme-produced payload.
pub(crate) const SIGNATURE_LEN: usize = 3116;

/// Wire size (bytes) of a `Slot` SSZ field (alias for [`U64_LEN`]).
pub(crate) const SLOT_LEN: usize = U64_LEN;

/// Wire size (bytes) of a `ValidatorIndex` SSZ field (alias for [`U64_LEN`]).
pub(crate) const VALIDATOR_INDEX_LEN: usize = U64_LEN;

/// Wire size (bytes) of a `Checkpoint` SSZ field (`Bytes32 + Slot`).
pub(crate) const CHECKPOINT_LEN: usize = BYTES32_LEN + SLOT_LEN;

/// Wire size (bytes) of a `BlockHeader` SSZ field
/// (`Slot + ValidatorIndex + 3 × Bytes32`).
pub(crate) const BLOCK_HEADER_LEN: usize = SLOT_LEN + VALIDATOR_INDEX_LEN + 3 * BYTES32_LEN;

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
/// advances `*cursor` by `N`.
///
/// Caller is responsible for verifying that the slice covers the read — same
/// contract as [`read_fixed`].
///
/// # Panics
/// Panics if `bytes.len() < *cursor + N`: the slice index below is the failure
/// mode, not the subsequent `copy_from_slice` (which is total once the index
/// has succeeded, since both sides then have length `N`). Every caller in this
/// crate pre-checks the length at the decode boundary — `ensure_len` or an
/// explicit `bytes.len() < ..` guard — so the panic is unreachable from a
/// network-supplied buffer. A future caller that skips that check turns a
/// truncated peer message into a panic.
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

/// Encodes `List[ByteVector<N>, _]` as a flat concatenation of `N`-byte
/// elements.
///
/// A fixed-width byte-vector list is a bare concatenation in SSZ — no length
/// prefix, no per-element offset; the enclosing container supplies the offset
/// that bounds these bytes. `ByteVector<N>` (e.g. [`Bytes32`], [`types::Signature`])
/// implements only [`HashTreeRoot`], not [`ssz::Encode`], so this cannot route
/// through the fixed-element-list codec.
pub(crate) fn encode_byte_vector_list<const N: usize>(
    items: &[types::ByteVector<N>],
    buf: &mut Vec<u8>,
) {
    for item in items {
        buf.extend_from_slice(item.as_slice());
    }
}

/// Decodes `List[ByteVector<N>, max]` from a flat concatenation. Rejects inputs
/// whose length is not a multiple of `N` or whose element count exceeds `max`.
pub(crate) fn decode_byte_vector_list<const N: usize>(
    bytes: &[u8],
    max: usize,
) -> Result<Vec<types::ByteVector<N>>, DecodeError> {
    if N == 0 {
        return Err(DecodeError::ZeroLengthItem);
    }
    if bytes.len() % N != 0 {
        return Err(DecodeError::BytesInvalid(format!(
            "byte-vector list bytes ({}) not divisible by element size ({N})",
            bytes.len(),
        )));
    }
    let count = bytes.len() / N;
    if count > max {
        return Err(DecodeError::BytesInvalid(format!(
            "byte-vector list length ({count}) exceeds max ({max})",
        )));
    }
    // Divisibility was checked above, so `chunks_exact` yields exactly `count`
    // full `N`-byte chunks with no remainder.
    let items = bytes
        .chunks_exact(N)
        .map(|chunk| {
            let mut arr = [0_u8; N];
            arr.copy_from_slice(chunk);
            types::ByteVector::new(arr)
        })
        .collect();
    Ok(items)
}

/// Encodes a `List[Bytes32, _]`. Thin wrapper over [`encode_byte_vector_list`]
/// (`Bytes32 = ByteVector<32>`) kept for call-site readability.
pub(crate) fn encode_bytes32_list(items: &[Bytes32], buf: &mut Vec<u8>) {
    encode_byte_vector_list::<BYTES32_LEN>(items, buf);
}

/// Decodes a `List[Bytes32, max]`. Thin wrapper over [`decode_byte_vector_list`].
pub(crate) fn decode_bytes32_list(bytes: &[u8], max: usize) -> Result<Vec<Bytes32>, DecodeError> {
    decode_byte_vector_list::<BYTES32_LEN>(bytes, max)
}

/// SSZ hash-tree-root of `List[T, max]` where `T: HashTreeRoot`:
/// `mix_in_length(merkleize_with_limit(roots, max), len)`.
///
/// Over-length input collapses the merkle root to [`ZERO_HASH`] rather than
/// panicking — the resulting root will not match any well-formed wire
/// payload, surfacing the bug at the first equality check.
pub(crate) fn list_hash_tree_root<T: HashTreeRoot>(items: &[T], max: usize) -> [u8; 32] {
    let roots: Vec<[u8; 32]> = items.iter().map(T::hash_tree_root).collect();
    let merkle = merkleize_with_limit(&roots, max).unwrap_or(ZERO_HASH);
    mix_in_length(&merkle, items.len() as u64)
}

/// SSZ hash-tree-root of `Bitlist[LIMIT]`: pack the live data bytes into
/// 32-byte chunks, [`merkleize_with_limit`] using `ceil(LIMIT / 256)` as the
/// chunk limit, then mix in the live bit length.
///
/// Over-limit input collapses the merkle root to [`ZERO_HASH`] rather than
/// panicking — the resulting root will not match any well-formed wire
/// payload, surfacing the bug at the first equality check.
pub(crate) fn bitlist_hash_tree_root<const LIMIT: usize>(bl: &Bitlist<LIMIT>) -> [u8; 32] {
    let length = bl.len();
    let mut data = vec![0_u8; length.div_ceil(8)];
    for i in bl.iter_set_indices() {
        data[i / 8] |= 1_u8 << (i % 8);
    }
    let chunks = pack(&data);
    let chunk_limit = LIMIT.div_ceil(256).max(1);
    let merkle = merkleize_with_limit(&chunks, chunk_limit).unwrap_or(ZERO_HASH);
    mix_in_length(&merkle, length as u64)
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
    use super::{read_byte_array, u64_chunk};
    use types::{PublicKey, Signature};

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

    // -- devnet-1 wire byte-vectors ride the manual encode/decode path -------
    //
    // There is no `Encode`/`Decode` derive on `ByteVector`: containers write
    // via `.as_slice()` and read via `read_byte_array::<N>`. These witness that
    // `Signature` / `PublicKey` work on that same path at their wire widths.
    // The `Signature::new(..)` wrapper is load-bearing — `read_byte_array`
    // returns `[u8; N]`, not the newtype.

    #[test]
    fn signature_wire_round_trips_through_read_byte_array() {
        let sig = Signature::new([0x5a; Signature::LEN]);

        let encoded = sig.as_slice().to_vec();
        assert_eq!(encoded.len(), Signature::LEN);

        let mut cursor = 0_usize;
        let decoded = Signature::new(read_byte_array::<{ Signature::LEN }>(&encoded, &mut cursor));

        assert_eq!(decoded, sig);
        assert_eq!(cursor, Signature::LEN);
    }

    #[test]
    fn publickey_wire_round_trips_through_read_byte_array() {
        let pk = PublicKey::new([0xa5; PublicKey::LEN]);

        let encoded = pk.as_slice().to_vec();
        assert_eq!(encoded.len(), PublicKey::LEN);

        let mut cursor = 0_usize;
        let decoded = PublicKey::new(read_byte_array::<{ PublicKey::LEN }>(&encoded, &mut cursor));

        assert_eq!(decoded, pk);
        assert_eq!(cursor, PublicKey::LEN);
    }

    /// Pins the documented panic contract: `read_byte_array` does not bounds-check,
    /// so a caller that forgets to `ensure_len` turns a truncated buffer into a
    /// panic rather than a `DecodeError`. Every production caller pre-checks;
    /// this witnesses what happens if one stops.
    #[test]
    #[should_panic(expected = "range end index")]
    fn read_byte_array_panics_on_truncated_input() {
        let truncated = vec![0_u8; Signature::LEN - 1];
        let mut cursor = 0_usize;
        let _ = read_byte_array::<{ Signature::LEN }>(&truncated, &mut cursor);
    }

    /// Decoding from a non-zero offset must honour the cursor rather than
    /// re-reading from the start.
    #[test]
    fn signature_decode_respects_starting_cursor() {
        let sig = Signature::new([0x27; Signature::LEN]);

        let mut buf = vec![0xff_u8; 8];
        buf.extend_from_slice(sig.as_slice());

        let mut cursor = 8_usize;
        let decoded = Signature::new(read_byte_array::<{ Signature::LEN }>(&buf, &mut cursor));

        assert_eq!(decoded, sig);
        assert_eq!(cursor, 8 + Signature::LEN);
    }

    /// The `N == 0` guard returns before any `% N` / `/ N` divide or
    /// `chunks_exact(0)` (both of which would panic), pinning the contract for
    /// a future `ByteVector<0>` instantiation that no current caller reaches.
    #[test]
    fn decode_byte_vector_list_rejects_zero_element_size() {
        let err = super::decode_byte_vector_list::<0>(&[], 4).unwrap_err();
        assert!(matches!(err, super::DecodeError::ZeroLengthItem));
    }
}
