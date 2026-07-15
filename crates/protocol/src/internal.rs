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

/// Wire size (bytes) of a [`types::Bytes4000`] SSZ field — signature
/// placeholder used by `SignedVote` / `SignedBlock`.
///
/// Superseded by [`SIGNATURE_LEN`]; retires with its last decode consumer when
/// the containers move to [`types::Signature`].
pub(crate) const BYTES4000_LEN: usize = 4000;

/// Wire size (bytes) of a [`types::Signature`] SSZ field — the devnet-1 XMSS
/// signature width, per the consensus spec's
/// [`class Signature(Bytes3116)`](https://github.com/leanEthereum/leanSpec/blob/050fa4a18881d54d7dc07601fe59e34eb20b9630/src/lean_spec/subspecs/containers/signature.py#L12)
/// at the pinned revision.
///
/// Not yet consumed by a container decode path: the containers still carry
/// [`BYTES4000_LEN`] until the wire refactor lands. The `dead_code` allow is
/// deliberate and comes off with the first real consumer — a `#[cfg(test)]`
/// consumer does not retire it, because `--all-targets` also compiles the lib
/// target without `cfg(test)`, where the const remains dead.
#[allow(dead_code)]
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

/// Encodes a `List[Bytes32, _]` as a flat concatenation of 32-byte elements.
pub(crate) fn encode_bytes32_list(items: &[Bytes32], buf: &mut Vec<u8>) {
    for item in items {
        buf.extend_from_slice(item.as_slice());
    }
}

/// Decodes a `List[Bytes32, max]` from a flat concatenation. Rejects inputs
/// whose length is not a multiple of 32 or whose element count exceeds `max`.
pub(crate) fn decode_bytes32_list(bytes: &[u8], max: usize) -> Result<Vec<Bytes32>, DecodeError> {
    if bytes.len() % BYTES32_LEN != 0 {
        return Err(DecodeError::BytesInvalid(format!(
            "Bytes32 list bytes ({}) not divisible by 32",
            bytes.len(),
        )));
    }
    let count = bytes.len() / BYTES32_LEN;
    if count > max {
        return Err(DecodeError::BytesInvalid(format!(
            "Bytes32 list length ({count}) exceeds max ({max})",
        )));
    }
    let mut items = Vec::with_capacity(count);
    for i in 0..count {
        let start = i * BYTES32_LEN;
        let mut arr = [0_u8; BYTES32_LEN];
        arr.copy_from_slice(&bytes[start..start + BYTES32_LEN]);
        items.push(Bytes32::new(arr));
    }
    Ok(items)
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
    use super::{read_byte_array, u64_chunk, SIGNATURE_LEN};

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
        let sig = types::Signature::new([0x5a; SIGNATURE_LEN]);

        let encoded = sig.as_slice().to_vec();
        assert_eq!(encoded.len(), SIGNATURE_LEN);

        let mut cursor = 0_usize;
        let decoded =
            types::Signature::new(read_byte_array::<SIGNATURE_LEN>(&encoded, &mut cursor));

        assert_eq!(decoded, sig);
        assert_eq!(cursor, SIGNATURE_LEN);
    }

    #[test]
    fn publickey_wire_round_trips_through_read_byte_array() {
        const PUBLIC_KEY_LEN: usize = 52;
        let pk = types::PublicKey::new([0xa5; PUBLIC_KEY_LEN]);

        let encoded = pk.as_slice().to_vec();
        assert_eq!(encoded.len(), PUBLIC_KEY_LEN);

        let mut cursor = 0_usize;
        let decoded =
            types::PublicKey::new(read_byte_array::<PUBLIC_KEY_LEN>(&encoded, &mut cursor));

        assert_eq!(decoded, pk);
        assert_eq!(cursor, PUBLIC_KEY_LEN);
    }

    /// Decoding from a non-zero offset must honour the cursor rather than
    /// re-reading from the start.
    #[test]
    fn signature_decode_respects_starting_cursor() {
        let sig = types::Signature::new([0x27; SIGNATURE_LEN]);

        let mut buf = vec![0xff_u8; 8];
        buf.extend_from_slice(sig.as_slice());

        let mut cursor = 8_usize;
        let decoded = types::Signature::new(read_byte_array::<SIGNATURE_LEN>(&buf, &mut cursor));

        assert_eq!(decoded, sig);
        assert_eq!(cursor, 8 + SIGNATURE_LEN);
    }
}
