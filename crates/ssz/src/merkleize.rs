//! SSZ merkleization helpers backed by SHA-256.
//!
//! - [`pack`] right-pads serialized basic-type bytes to 32-byte chunks.
//! - [`merkleize`] / [`merkleize_with_limit`] reduce chunks to a single
//!   32-byte root via balanced binary Merkle tree, with optional zero-pad
//!   extension up to a power-of-two limit.
//! - [`merkleize_progressive`] applies the progressive merkleization scheme
//!   used by some container types: recursive split at `num_leaves`, growing
//!   by ×4 at each tail. Always combines `hash_pair(left, right)` even when
//!   the left subtree is [`ZERO_HASH`].
//! - [`mix_in_length`] / [`mix_in_selector`] append a 32-byte LE-encoded
//!   scalar via [`hash_pair`].
//! - [`zero_tree_root`] returns the root of an all-zero Merkle subtree of
//!   `width_pow2` width.

use sha2::{Digest, Sha256};

use crate::error::SszError;

/// SSZ Merkle chunk width in bytes.
pub const BYTES_PER_CHUNK: usize = 32;

/// Canonical zero chunk (used as Merkle padding leaf).
pub const ZERO_HASH: [u8; 32] = [0_u8; 32];

/// Hashes two 32-byte Merkle nodes together with SHA-256.
#[must_use]
pub fn hash_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

/// Packs raw bytes into 32-byte Merkle chunks, right-padding the final chunk
/// with zeros if necessary.
///
/// Returns an empty `Vec` for empty input.
#[must_use]
pub fn pack(bytes: &[u8]) -> Vec<[u8; 32]> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let n_chunks = bytes.len().div_ceil(BYTES_PER_CHUNK);
    let mut chunks = vec![[0_u8; BYTES_PER_CHUNK]; n_chunks];
    for (i, chunk) in bytes.chunks(BYTES_PER_CHUNK).enumerate() {
        chunks[i][..chunk.len()].copy_from_slice(chunk);
    }
    chunks
}

/// Computes the Merkle root of `chunks` with no explicit limit.
///
/// Empty input yields [`ZERO_HASH`]. Otherwise the chunks are zero-extended
/// to the smallest power of two ≥ `chunks.len()` and reduced via a balanced
/// binary tree.
#[must_use]
pub fn merkleize(chunks: &[[u8; 32]]) -> [u8; 32] {
    if chunks.is_empty() {
        return ZERO_HASH;
    }
    let actual_width = power_of_two_ceil(chunks.len());
    merkleize_partial(chunks, actual_width)
}

/// Computes the Merkle root of `chunks` with an explicit leaf-limit.
///
/// The result is identical to merkleizing a `power_of_two_ceil(limit)`-leaf
/// array with `chunks` placed at the leftmost leaves and the remainder
/// zero-padded — but only `O(n + log(limit / n))` hashes are evaluated thanks
/// to the precomputable zero-subtree-root recursion.
///
/// # Errors
/// - [`SszError::InputExceedsLimit`] when `chunks.len() > limit`.
pub fn merkleize_with_limit(chunks: &[[u8; 32]], limit: usize) -> Result<[u8; 32], SszError> {
    let n = chunks.len();
    if n == 0 {
        return Ok(zero_tree_root(power_of_two_ceil(limit)));
    }
    if n > limit {
        return Err(SszError::InputExceedsLimit { got: n, limit });
    }
    let actual_width = power_of_two_ceil(n);
    let root = merkleize_partial(chunks, actual_width);
    let target_width = power_of_two_ceil(limit);
    if actual_width >= target_width {
        Ok(root)
    } else {
        Ok(extend_with_zero_siblings(root, actual_width, target_width))
    }
}

/// Computes the progressive SSZ Merkle root of `chunks`.
///
/// Used by container types whose merkleization grows in geometric subtree
/// sizes (1 × `num_leaves`, then 4 × `num_leaves`, then 16 ×, …).
///
/// The right subtree always covers the first `min(num_leaves, len)` chunks;
/// the left subtree recurses over the tail with `num_leaves * 4`. The result
/// is `hash_pair(left, right)` even when the left side is [`ZERO_HASH`].
///
/// # Errors
/// - [`SszError::InvalidNumLeaves`] when `num_leaves == 0`.
/// - [`SszError::InputExceedsLimit`] forwarded from
///   [`merkleize_with_limit`] when an intermediate level rejects its slice
///   (cannot trigger under correct usage but propagated for completeness).
pub fn merkleize_progressive(chunks: &[[u8; 32]], num_leaves: usize) -> Result<[u8; 32], SszError> {
    if num_leaves == 0 {
        return Err(SszError::InvalidNumLeaves { got: num_leaves });
    }
    if chunks.is_empty() {
        return Ok(ZERO_HASH);
    }
    let count = chunks.len().min(num_leaves);
    let right = merkleize_with_limit(&chunks[..count], num_leaves)?;
    let left = if chunks.len() > num_leaves {
        // Tail recurses with the next geometric step. num_leaves * 4 cannot
        // overflow before chunks.len() exhausts because chunks.len() is
        // bounded by usize::MAX.
        let next = num_leaves
            .checked_mul(4)
            .ok_or(SszError::InvalidNumLeaves { got: num_leaves })?;
        merkleize_progressive(&chunks[num_leaves..], next)?
    } else {
        ZERO_HASH
    };
    Ok(hash_pair(&left, &right))
}

/// Mixes a list `length` into the Merkle `root` via [`hash_pair`].
///
/// The length is little-endian encoded into the low 8 bytes of a 32-byte
/// chunk; the upper 24 bytes are zero.
#[must_use]
pub fn mix_in_length(root: &[u8; 32], length: u64) -> [u8; 32] {
    hash_pair(root, &scalar_chunk(length))
}

/// Mixes a union `selector` into the Merkle `root` via [`hash_pair`].
///
/// Selectors are SSZ `u8`-typed; widened to `u64` for the chunk encoding.
#[must_use]
pub fn mix_in_selector(root: &[u8; 32], selector: u8) -> [u8; 32] {
    hash_pair(root, &scalar_chunk(u64::from(selector)))
}

/// Returns the root of an all-zero Merkle subtree of `width_pow2` width.
///
/// `width_pow2` must be a power of two; values `≤ 1` return [`ZERO_HASH`].
/// A non-power-of-two input would silently produce a wrong root (the
/// `width /= 2` loop is integer-divide, which rounds down); the
/// `debug_assert!` guards the precondition in debug builds.
#[must_use]
pub fn zero_tree_root(width_pow2: usize) -> [u8; 32] {
    debug_assert!(
        width_pow2 <= 1 || width_pow2.is_power_of_two(),
        "zero_tree_root: width_pow2 ({width_pow2}) must be a power of two",
    );
    if width_pow2 <= 1 {
        return ZERO_HASH;
    }
    let mut root = ZERO_HASH;
    let mut width = width_pow2;
    while width > 1 {
        root = hash_pair(&root, &root);
        width /= 2;
    }
    root
}

// -- Internal helpers (pub(crate) for testing) ---------------------------

/// Returns the smallest power of two ≥ `x`. `x ≤ 1` returns `1`.
///
/// Saturates at `1 << (usize::BITS - 1)` (the largest representable power of
/// two) to keep the public `merkleize_with_limit` panic-free when an
/// attacker-influenced `limit` reaches the top half of the `usize` range —
/// `power_of_two_ceil(usize::MAX)` would otherwise compute `bits = usize::BITS`
/// and panic on `1_usize << usize::BITS`.
#[must_use]
pub(crate) fn power_of_two_ceil(x: usize) -> usize {
    if x <= 1 {
        return 1;
    }
    let bits = (usize::BITS - (x - 1).leading_zeros()) as usize;
    if bits >= usize::BITS as usize {
        return 1_usize << (usize::BITS - 1);
    }
    1_usize << bits
}

/// Merkleizes exactly `width` leaves where `width` is a power of two. Chunks
/// beyond `chunks.len()` are implicitly zero.
fn merkleize_partial(chunks: &[[u8; 32]], width: usize) -> [u8; 32] {
    if width <= 1 {
        // Invariant: width==1 ⇒ chunks.len() == 1. The fallback to ZERO_HASH
        // keeps the function panic-free under workspace `panic = "deny"`.
        return chunks.first().copied().unwrap_or(ZERO_HASH);
    }
    let mut level: Vec<[u8; 32]> = Vec::with_capacity(width);
    level.extend_from_slice(chunks);
    level.resize(width, ZERO_HASH);
    while level.len() > 1 {
        let mut next: Vec<[u8; 32]> = Vec::with_capacity(level.len() / 2);
        for pair in level.chunks_exact(2) {
            next.push(hash_pair(&pair[0], &pair[1]));
        }
        level = next;
    }
    // level.len() == 1 by loop invariant.
    level.first().copied().unwrap_or(ZERO_HASH)
}

/// Extends a populated subtree of width `from` out to width `to` by hashing
/// against precomputed zero-subtree roots at each depth.
///
/// Both `from` and `to` must be powers of two with `from ≤ to`.
fn extend_with_zero_siblings(root: [u8; 32], from: usize, to: usize) -> [u8; 32] {
    let mut width = from;
    let mut zero_sibling = zero_tree_root(width);
    let mut current = root;
    while width < to {
        current = hash_pair(&current, &zero_sibling);
        zero_sibling = hash_pair(&zero_sibling, &zero_sibling);
        // width *= 2; bounded by `to` so cannot overflow before loop exits.
        width = width.saturating_mul(2);
    }
    current
}

/// Encodes `value` as a 32-byte chunk: low 8 bytes little-endian, upper 24
/// bytes zero.
fn scalar_chunk(value: u64) -> [u8; 32] {
    let mut out = [0_u8; 32];
    out[..8].copy_from_slice(&value.to_le_bytes());
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------
    // power_of_two_ceil
    // ---------------------------------------------------------------------

    #[test]
    fn power_of_two_ceil_boundary_cases() {
        assert_eq!(power_of_two_ceil(0), 1);
        assert_eq!(power_of_two_ceil(1), 1);
        assert_eq!(power_of_two_ceil(2), 2);
        assert_eq!(power_of_two_ceil(3), 4);
        assert_eq!(power_of_two_ceil(4), 4);
        assert_eq!(power_of_two_ceil(5), 8);
        assert_eq!(power_of_two_ceil(8), 8);
        assert_eq!(power_of_two_ceil(9), 16);
        assert_eq!(power_of_two_ceil(1023), 1024);
        assert_eq!(power_of_two_ceil(1024), 1024);
        assert_eq!(power_of_two_ceil(1025), 2048);
    }

    // ---------------------------------------------------------------------
    // hash_pair — SHA-256 sanity
    // ---------------------------------------------------------------------

    #[test]
    fn hash_pair_of_two_zero_chunks_equals_sha256_of_64_zeros() {
        let got = hash_pair(&ZERO_HASH, &ZERO_HASH);
        let expected = Sha256::digest([0_u8; 64]);
        assert_eq!(got, <[u8; 32]>::from(expected));
    }

    #[test]
    fn hash_pair_is_deterministic() {
        let left = [1_u8; 32];
        let right = [2_u8; 32];
        let a = hash_pair(&left, &right);
        let b = hash_pair(&left, &right);
        assert_eq!(a, b);
    }

    // ---------------------------------------------------------------------
    // pack — chunking + right-padding
    // ---------------------------------------------------------------------

    #[test]
    fn pack_empty_yields_empty_vec() {
        assert!(pack(&[]).is_empty());
    }

    #[test]
    fn pack_under_one_chunk_right_pads_with_zeros() {
        let chunks = pack(&[1, 2, 3]);
        assert_eq!(chunks.len(), 1);
        let mut expected = [0_u8; 32];
        expected[..3].copy_from_slice(&[1, 2, 3]);
        assert_eq!(chunks[0], expected);
    }

    #[test]
    fn pack_exact_chunk_no_padding() {
        let chunks = pack(&[0xff_u8; 32]);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], [0xff_u8; 32]);
    }

    #[test]
    fn pack_thirty_three_bytes_yields_two_chunks() {
        let chunks = pack(&[0xff_u8; 33]);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], [0xff_u8; 32]);
        let mut expected_chunk1 = [0_u8; 32];
        expected_chunk1[0] = 0xff;
        assert_eq!(chunks[1], expected_chunk1);
    }

    // ---------------------------------------------------------------------
    // zero_tree_root — depths 0..32
    // ---------------------------------------------------------------------

    #[test]
    fn zero_tree_root_width_zero_or_one_is_zero_hash() {
        assert_eq!(zero_tree_root(0), ZERO_HASH);
        assert_eq!(zero_tree_root(1), ZERO_HASH);
    }

    #[test]
    fn zero_tree_root_depth_one_is_hash_of_two_zeros() {
        // width=2 → one hash combine: hash_pair(ZERO_HASH, ZERO_HASH).
        let expected = hash_pair(&ZERO_HASH, &ZERO_HASH);
        assert_eq!(zero_tree_root(2), expected);
    }

    #[test]
    fn zero_tree_root_recursive_doubling() {
        // width=4: hash_pair(zero_tree_root(2), zero_tree_root(2)).
        let r2 = zero_tree_root(2);
        let expected_r4 = hash_pair(&r2, &r2);
        assert_eq!(zero_tree_root(4), expected_r4);
        // width=8: hash_pair(zero_tree_root(4), zero_tree_root(4)).
        let expected_r8 = hash_pair(&expected_r4, &expected_r4);
        assert_eq!(zero_tree_root(8), expected_r8);
    }

    #[test]
    fn zero_tree_root_depths_zero_through_thirty_two_are_distinct() {
        // Every doubling produces a different root.
        let mut seen = std::collections::HashSet::new();
        seen.insert(zero_tree_root(0));
        for k in 0..=32 {
            let width = 1_usize << k;
            let root = zero_tree_root(width);
            // Note: width=0 and width=1 collapse to ZERO_HASH (already inserted),
            // so we expect 33 distinct values overall (ZERO_HASH + 32 derivatives).
            seen.insert(root);
        }
        assert_eq!(seen.len(), 33);
    }

    // ---------------------------------------------------------------------
    // merkleize — zero-extension + hash_pair reduction
    // ---------------------------------------------------------------------

    #[test]
    fn merkleize_empty_chunks_is_zero_hash() {
        assert_eq!(merkleize(&[]), ZERO_HASH);
    }

    #[test]
    fn merkleize_single_chunk_returns_chunk_unchanged() {
        let chunk = [7_u8; 32];
        // power_of_two_ceil(1) = 1 → merkleize_partial returns chunk[0].
        assert_eq!(merkleize(&[chunk]), chunk);
    }

    #[test]
    fn merkleize_two_chunks_is_hash_of_concatenation() {
        let a = [1_u8; 32];
        let b = [2_u8; 32];
        assert_eq!(merkleize(&[a, b]), hash_pair(&a, &b));
    }

    #[test]
    fn merkleize_three_chunks_zero_extends_to_four() {
        let a = [1_u8; 32];
        let b = [2_u8; 32];
        let c = [3_u8; 32];
        // width = power_of_two_ceil(3) = 4. Tree: hash(hash(a, b), hash(c, 0)).
        let left = hash_pair(&a, &b);
        let right = hash_pair(&c, &ZERO_HASH);
        let expected = hash_pair(&left, &right);
        assert_eq!(merkleize(&[a, b, c]), expected);
    }

    // SSZ hash-tree-root for 33 bytes of 0xff.
    // Cross-checked against the published value in upstream SSZ test
    // vectors:
    //   input  = 33 bytes of 0xff
    //   output = "fe9011a8425e3de1282b9a6af280b8adc3676718291cf8a2efb21239f8cb077a"
    #[test]
    fn raw_bytes_33_ff_known_root() {
        let payload = [0xff_u8; 33];
        let chunks = pack(&payload);
        let root = merkleize(&chunks);
        let expected_hex = "fe9011a8425e3de1282b9a6af280b8adc3676718291cf8a2efb21239f8cb077a";
        let mut expected = [0_u8; 32];
        for (i, byte) in expected.iter_mut().enumerate() {
            let lo = i * 2;
            *byte = u8::from_str_radix(&expected_hex[lo..lo + 2], 16).unwrap();
        }
        assert_eq!(root, expected, "33 × 0xff hash-tree-root mismatch");
    }

    // ---------------------------------------------------------------------
    // merkleize_with_limit
    // ---------------------------------------------------------------------

    #[test]
    fn merkleize_with_limit_empty_chunks_uses_zero_tree_root() {
        // Empty chunks under limit=4 → zero_tree_root(power_of_two_ceil(4)) = zero_tree_root(4).
        assert_eq!(merkleize_with_limit(&[], 4).unwrap(), zero_tree_root(4));
        // Limit=0 maps to width=1 → ZERO_HASH.
        assert_eq!(merkleize_with_limit(&[], 0).unwrap(), ZERO_HASH);
    }

    #[test]
    fn merkleize_with_limit_rejects_overflow() {
        let chunks = vec![[1_u8; 32]; 5];
        let err = merkleize_with_limit(&chunks, 4).unwrap_err();
        assert!(matches!(
            err,
            SszError::InputExceedsLimit { got: 5, limit: 4 }
        ));
    }

    #[test]
    fn merkleize_with_limit_extends_to_target_width() {
        // n=2 chunks, limit=4. actual_width=2, target_width=4. Single
        // extension step: hash_pair(actual_root, zero_tree_root(2)).
        let a = [1_u8; 32];
        let b = [2_u8; 32];
        let actual_root = hash_pair(&a, &b);
        let expected = hash_pair(&actual_root, &zero_tree_root(2));
        assert_eq!(merkleize_with_limit(&[a, b], 4).unwrap(), expected);
    }

    #[test]
    fn merkleize_with_limit_no_extension_when_actual_meets_target() {
        // n=2 chunks, limit=2. actual_width=2, target_width=2 → no extension.
        let a = [1_u8; 32];
        let b = [2_u8; 32];
        assert_eq!(merkleize_with_limit(&[a, b], 2).unwrap(), hash_pair(&a, &b));
    }

    #[test]
    fn merkleize_with_limit_at_limit_passes() {
        let chunks = vec![[1_u8; 32]; 4];
        // No InputExceedsLimit fires since 4 == 4.
        merkleize_with_limit(&chunks, 4).unwrap();
    }

    // ---------------------------------------------------------------------
    // merkleize_progressive
    // ---------------------------------------------------------------------

    #[test]
    fn merkleize_progressive_zero_num_leaves_errors() {
        let err = merkleize_progressive(&[], 0).unwrap_err();
        assert!(matches!(err, SszError::InvalidNumLeaves { got: 0 }));
    }

    #[test]
    fn merkleize_progressive_empty_chunks_is_zero_hash() {
        assert_eq!(merkleize_progressive(&[], 4).unwrap(), ZERO_HASH);
    }

    #[test]
    fn merkleize_progressive_chunks_fit_combines_zero_left() {
        // num_leaves=4, chunks=[a, b]. count=2, right=merkleize_with_limit([a,b], 4).
        // chunks.len() == 2 ≤ num_leaves=4 → left=ZERO_HASH.
        // Result = hash_pair(ZERO_HASH, right).
        let a = [1_u8; 32];
        let b = [2_u8; 32];
        let right = merkleize_with_limit(&[a, b], 4).unwrap();
        let expected = hash_pair(&ZERO_HASH, &right);
        assert_eq!(merkleize_progressive(&[a, b], 4).unwrap(), expected);
    }

    #[test]
    fn merkleize_progressive_chunks_overflow_recurses_geometrically() {
        // num_leaves=1, chunks=[a, b]. First call:
        //   count=1, right=merkleize_with_limit([a], 1) = a
        //   chunks.len() > num_leaves → left = merkleize_progressive([b], 4)
        //     count=1, right=merkleize_with_limit([b], 4) =
        //       extend_with_zero_siblings(b, 1, 4)
        //     chunks.len() ≤ num_leaves=4 → left=ZERO_HASH
        //     return hash_pair(ZERO_HASH, that)
        //   return hash_pair(left, a)
        let a = [0xaa_u8; 32];
        let b = [0xbb_u8; 32];
        let inner_right = merkleize_with_limit(&[b], 4).unwrap();
        let inner = hash_pair(&ZERO_HASH, &inner_right);
        let expected = hash_pair(&inner, &a);
        assert_eq!(merkleize_progressive(&[a, b], 1).unwrap(), expected);
    }

    // ---------------------------------------------------------------------
    // mix_in_length / mix_in_selector
    // ---------------------------------------------------------------------

    #[test]
    fn mix_in_length_appends_le_length_chunk() {
        let root = [9_u8; 32];
        let length: u64 = 0x0123_4567_89ab_cdef;
        let mut len_chunk = [0_u8; 32];
        len_chunk[..8].copy_from_slice(&length.to_le_bytes());
        let expected = hash_pair(&root, &len_chunk);
        assert_eq!(mix_in_length(&root, length), expected);
    }

    #[test]
    fn mix_in_length_zero_length_uses_zero_chunk() {
        let root = [3_u8; 32];
        assert_eq!(mix_in_length(&root, 0), hash_pair(&root, &ZERO_HASH));
    }

    #[test]
    fn mix_in_selector_uses_widened_le_chunk() {
        let root = [4_u8; 32];
        let selector: u8 = 0x7f;
        let mut sel_chunk = [0_u8; 32];
        sel_chunk[0] = selector;
        let expected = hash_pair(&root, &sel_chunk);
        assert_eq!(mix_in_selector(&root, selector), expected);
    }

    // ---------------------------------------------------------------------
    // scalar_chunk (private)
    // ---------------------------------------------------------------------

    #[test]
    fn scalar_chunk_low_eight_bytes_le_rest_zero() {
        let chunk = scalar_chunk(0xdead_beef_u64);
        assert_eq!(&chunk[..8], &0xdead_beef_u64.to_le_bytes());
        assert!(chunk[8..].iter().all(|&b| b == 0));
    }

    #[test]
    fn scalar_chunk_zero_is_zero_hash() {
        assert_eq!(scalar_chunk(0), ZERO_HASH);
    }
}
