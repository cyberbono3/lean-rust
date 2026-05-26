//! SSZ list-encoding helpers shared across consensus + networking crates.
//!
//! Four pairs cover the wire shapes seen in the consensus protocol:
//!
//! - [`encode_fixed_element_list`] / [`decode_fixed_element_list`] —
//!   `List[T, MAX]` and `Vector[T, N]` where `T::is_ssz_fixed_len()`.
//! - [`encode_bytes32_list`] / [`decode_bytes32_list`] — fast path for
//!   `List[Bytes32, MAX]` (the most common 32-byte-element list).
//! - [`encode_variable_element_list`] / [`decode_variable_element_list`] —
//!   `List[T, MAX]` where `T` is variable-length (each element prefixed by
//!   a 4-byte offset).
//!
//! All `decode_*` helpers cap the element count at `max` before allocating,
//! so an adversarial input can't exhaust memory via a length-claim.

use types::Bytes32;

use crate::{Decode, DecodeError, Encode};

/// Number of bytes used by an SSZ length-offset (the 4-byte LE prefix in
/// variable-length containers and lists).
pub const BYTES_PER_LENGTH_OFFSET: usize = 4;

const BYTES32_LEN: usize = 32;

// =============================================================================
// Fixed-element lists
// =============================================================================

/// Encodes a list of fixed-size SSZ elements as a flat concatenation.
///
/// Suitable for `List[T, MAX]` and `Vector[T, N]` where
/// `T::is_ssz_fixed_len()`.
pub fn encode_fixed_element_list<T: Encode>(items: &[T], buf: &mut Vec<u8>) {
    for item in items {
        item.ssz_append(buf);
    }
}

/// Decodes a list of fixed-size SSZ elements from a flat concatenation.
///
/// # Errors
/// - [`DecodeError::ZeroLengthItem`] when `T::ssz_fixed_len() == 0`.
/// - [`DecodeError::BytesInvalid`] when the input length isn't a multiple
///   of the element size, or when the element count exceeds `max`.
pub fn decode_fixed_element_list<T: Decode>(
    bytes: &[u8],
    max: usize,
) -> Result<Vec<T>, DecodeError> {
    let elem_len = <T as Decode>::ssz_fixed_len();
    if elem_len == 0 {
        return Err(DecodeError::ZeroLengthItem);
    }
    if bytes.len() % elem_len != 0 {
        return Err(DecodeError::BytesInvalid(format!(
            "list bytes ({}) not divisible by element size ({elem_len})",
            bytes.len(),
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

// =============================================================================
// Bytes32 lists (specialized fast path)
// =============================================================================

/// Encodes a `List[Bytes32, _]` as a flat concatenation of 32-byte elements.
pub fn encode_bytes32_list(items: &[Bytes32], buf: &mut Vec<u8>) {
    for item in items {
        buf.extend_from_slice(item.as_slice());
    }
}

/// Decodes a `List[Bytes32, max]` from a flat concatenation.
///
/// # Errors
/// [`DecodeError::BytesInvalid`] when the input length isn't a multiple of
/// 32, or when the element count exceeds `max`.
pub fn decode_bytes32_list(bytes: &[u8], max: usize) -> Result<Vec<Bytes32>, DecodeError> {
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
    for chunk in bytes.chunks_exact(BYTES32_LEN) {
        let mut arr = [0_u8; BYTES32_LEN];
        arr.copy_from_slice(chunk);
        items.push(Bytes32::new(arr));
    }
    Ok(items)
}

// =============================================================================
// Variable-element lists
// =============================================================================

/// Encodes `List[T, _]` of variable-size elements.
///
/// Layout: `[off_0 (4B LE)] … [off_{n-1}] [payload_0] [payload_1] …` where
/// `off_i` is the absolute byte position of `payload_i` from the start of
/// the encoded list. Offsets occupy the leading `4 * n` bytes; `off_0`
/// therefore equals `4 * n`.
///
/// # Panics
/// Panics if the cumulative offset would exceed `u32::MAX` (4 GiB). All
/// domain SSZ list limits are far below this; the panic is a static
/// precondition on the encoder, not a runtime concern. The prior silent
/// `unwrap_or(u32::MAX)` saturation produced corrupt unparseable bytes;
/// failing loudly is preferable.
#[allow(clippy::panic)]
pub fn encode_variable_element_list<T: Encode>(items: &[T], buf: &mut Vec<u8>) {
    let n = items.len();
    if n == 0 {
        return;
    }
    // Reserve the offset prefix area. We fill the offsets after we know
    // each element's encoded length.
    let prefix_start = buf.len();
    let prefix_len = n * BYTES_PER_LENGTH_OFFSET;
    buf.resize(prefix_start + prefix_len, 0);

    let mut current_offset = prefix_len;
    for (i, item) in items.iter().enumerate() {
        let offset_pos = prefix_start + i * BYTES_PER_LENGTH_OFFSET;
        // SSZ wire format caps offsets at u32::MAX. Encoding a list whose
        // total prefix-plus-payload bytes exceed 4 GiB would produce a
        // corrupt wire encoding (silently truncated offset); the prior
        // `unwrap_or(u32::MAX)` masked this. Domain types (blocks, votes,
        // states) all carry SSZ list limits well below 4 GiB so this
        // panic is a static precondition, not a runtime branch.
        let Ok(offset_u32) = u32::try_from(current_offset) else {
            panic!("ssz variable-element list offset {current_offset} exceeds u32::MAX wire limit",);
        };
        buf[offset_pos..offset_pos + BYTES_PER_LENGTH_OFFSET]
            .copy_from_slice(&offset_u32.to_le_bytes());

        let before = buf.len();
        item.ssz_append(buf);
        current_offset += buf.len() - before;
    }
}

/// Decodes `List[T, max]` of variable-size elements.
///
/// # Errors
/// - [`DecodeError::BytesInvalid`] when the leading offset is malformed
///   (zero, not a multiple of 4, or beyond the buffer), when subsequent
///   offsets are non-monotonic (`off[i+1] < off[i]`), or when the implied
///   element count exceeds `max`. Equal offsets are permitted and denote
///   zero-length elements per the SSZ spec.
/// - Any [`DecodeError`] surfaced by `T::from_ssz_bytes`.
pub fn decode_variable_element_list<T: Decode>(
    bytes: &[u8],
    max: usize,
) -> Result<Vec<T>, DecodeError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if bytes.len() < BYTES_PER_LENGTH_OFFSET {
        return Err(DecodeError::BytesInvalid(format!(
            "variable-element list too short for first offset: {}",
            bytes.len(),
        )));
    }
    let first_offset = read_offset(bytes, 0)?;
    if first_offset == 0 || first_offset % BYTES_PER_LENGTH_OFFSET != 0 {
        return Err(DecodeError::BytesInvalid(format!(
            "variable-element list: malformed first offset {first_offset}",
        )));
    }
    if first_offset > bytes.len() {
        return Err(DecodeError::BytesInvalid(format!(
            "variable-element list: first offset {first_offset} exceeds bytes ({})",
            bytes.len(),
        )));
    }
    let count = first_offset / BYTES_PER_LENGTH_OFFSET;
    if count > max {
        return Err(DecodeError::BytesInvalid(format!(
            "variable-element list length ({count}) exceeds max ({max})",
        )));
    }

    // Read all offsets, then carve element payloads with sliding windows.
    let mut offsets = Vec::with_capacity(count + 1);
    offsets.push(first_offset);
    let mut prev = first_offset;
    for i in 1..count {
        let off = read_offset(bytes, i * BYTES_PER_LENGTH_OFFSET)?;
        if off < prev || off > bytes.len() {
            return Err(DecodeError::BytesInvalid(format!(
                "variable-element list: non-monotonic offset {off} at index {i}",
            )));
        }
        offsets.push(off);
        prev = off;
    }
    offsets.push(bytes.len());

    let mut items = Vec::with_capacity(count);
    for window in offsets.windows(2) {
        let (start, end) = (window[0], window[1]);
        items.push(T::from_ssz_bytes(&bytes[start..end])?);
    }
    Ok(items)
}

fn read_offset(bytes: &[u8], pos: usize) -> Result<usize, DecodeError> {
    let end = pos
        .checked_add(BYTES_PER_LENGTH_OFFSET)
        .ok_or_else(|| DecodeError::BytesInvalid(format!("offset position {pos} overflows")))?;
    if end > bytes.len() {
        return Err(DecodeError::BytesInvalid(format!(
            "offset read out of bounds at {pos}",
        )));
    }
    let mut arr = [0_u8; BYTES_PER_LENGTH_OFFSET];
    arr.copy_from_slice(&bytes[pos..end]);
    Ok(u32::from_le_bytes(arr) as usize)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{decode, encode};

    // -- fixed-element lists -------------------------------------------------

    #[test]
    fn fixed_list_round_trip_u32() {
        let items = vec![1_u32, 2, 3, 4];
        let mut buf = Vec::new();
        encode_fixed_element_list(&items, &mut buf);
        assert_eq!(buf.len(), 16);
        let back: Vec<u32> = decode_fixed_element_list(&buf, 100).unwrap();
        assert_eq!(back, items);
    }

    #[test]
    fn fixed_list_rejects_over_max() {
        let items = vec![1_u32, 2, 3, 4];
        let mut buf = Vec::new();
        encode_fixed_element_list(&items, &mut buf);
        let err = decode_fixed_element_list::<u32>(&buf, 3).unwrap_err();
        assert!(matches!(err, DecodeError::BytesInvalid(_)));
    }

    #[test]
    fn fixed_list_rejects_non_aligned_bytes() {
        let err = decode_fixed_element_list::<u32>(&[0_u8; 7], 10).unwrap_err();
        assert!(matches!(err, DecodeError::BytesInvalid(_)));
    }

    // -- Bytes32 lists -------------------------------------------------------

    #[test]
    fn bytes32_list_round_trip() {
        let items = vec![
            Bytes32::new([1; 32]),
            Bytes32::new([2; 32]),
            Bytes32::new([3; 32]),
        ];
        let mut buf = Vec::new();
        encode_bytes32_list(&items, &mut buf);
        assert_eq!(buf.len(), 96);
        let back = decode_bytes32_list(&buf, 10).unwrap();
        assert_eq!(back, items);
    }

    #[test]
    fn bytes32_list_empty_round_trip() {
        let mut buf = Vec::new();
        encode_bytes32_list(&[], &mut buf);
        assert!(buf.is_empty());
        let back = decode_bytes32_list(&buf, 10).unwrap();
        assert!(back.is_empty());
    }

    #[test]
    fn bytes32_list_rejects_over_max() {
        let items = vec![Bytes32::zero(); 5];
        let mut buf = Vec::new();
        encode_bytes32_list(&items, &mut buf);
        let err = decode_bytes32_list(&buf, 3).unwrap_err();
        assert!(matches!(err, DecodeError::BytesInvalid(_)));
    }

    #[test]
    fn bytes32_list_rejects_non_aligned_bytes() {
        let err = decode_bytes32_list(&[0_u8; 33], 10).unwrap_err();
        assert!(matches!(err, DecodeError::BytesInvalid(_)));
    }

    // -- variable-element lists ---------------------------------------------

    #[test]
    fn variable_list_empty_round_trip() {
        let items: Vec<Vec<u8>> = Vec::new();
        let mut buf = Vec::new();
        encode_variable_element_list(&items, &mut buf);
        assert!(buf.is_empty());
        let back: Vec<Vec<u8>> = decode_variable_element_list(&buf, 10).unwrap();
        assert!(back.is_empty());
    }

    #[test]
    fn variable_list_round_trip_byte_vecs() {
        // Vec<u8> is variable-length in SSZ (raw byte list).
        let items: Vec<Vec<u8>> = vec![vec![1, 2, 3], vec![4, 5], vec![6]];
        let mut buf = Vec::new();
        encode_variable_element_list(&items, &mut buf);
        // 3 offsets (12 bytes) + (3 + 2 + 1) payload = 18 bytes.
        assert_eq!(buf.len(), 18);
        let back: Vec<Vec<u8>> = decode_variable_element_list(&buf, 10).unwrap();
        assert_eq!(back, items);
    }

    #[test]
    fn variable_list_rejects_over_max() {
        let items: Vec<Vec<u8>> = vec![vec![1], vec![2], vec![3], vec![4]];
        let mut buf = Vec::new();
        encode_variable_element_list(&items, &mut buf);
        let err = decode_variable_element_list::<Vec<u8>>(&buf, 3).unwrap_err();
        assert!(matches!(err, DecodeError::BytesInvalid(_)));
    }

    #[test]
    fn variable_list_rejects_short_first_offset() {
        let err = decode_variable_element_list::<Vec<u8>>(&[0_u8; 2], 10).unwrap_err();
        assert!(matches!(err, DecodeError::BytesInvalid(_)));
    }

    #[test]
    fn variable_list_rejects_zero_first_offset() {
        let mut bytes = vec![0_u8; 4]; // first offset == 0
        bytes.extend_from_slice(&[1, 2, 3]);
        let err = decode_variable_element_list::<Vec<u8>>(&bytes, 10).unwrap_err();
        assert!(matches!(err, DecodeError::BytesInvalid(_)));
    }

    #[test]
    fn variable_list_offset_consistency() {
        let items: Vec<Vec<u8>> = vec![vec![0xaa; 5], vec![0xbb; 7]];
        let mut buf = Vec::new();
        encode_variable_element_list(&items, &mut buf);
        // Offset 0 should be 4 * 2 = 8; offset 1 should be 8 + 5 = 13.
        assert_eq!(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]), 8);
        assert_eq!(u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]), 13);
        // Final encoded length: 8 (offsets) + 5 + 7 = 20.
        assert_eq!(buf.len(), 20);
    }

    // -- Round-trip via the public encode/decode facade ----------------------

    #[test]
    fn fixed_list_via_facade_for_u8() {
        let items = vec![1_u8, 2, 3, 4, 5];
        let bytes = encode(&items);
        let back: Vec<u8> = decode(&bytes).unwrap();
        assert_eq!(back, items);
    }
}
