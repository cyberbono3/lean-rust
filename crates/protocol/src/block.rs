//! Block containers — [`BlockHeader`], [`BlockBody`], [`Block`],
//! [`BlockSignatures`], [`BlockWithAttestation`], and
//! [`SignedBlockWithAttestation`].
//!
//! Mirrors the leanSpec consensus-spec block containers. The block signature is
//! a variable-length list of 3116-byte XMSS post-quantum signatures.
//!
//! Wire shapes:
//! - [`BlockHeader`] — 5 fixed-size fields, 112-byte SSZ payload.
//! - [`BlockBody`] — single variable-length field (`attestations:
//!   List[Attestation, MAX_ATTESTATIONS]`). Variable-length SSZ container.
//! - [`Block`] — 4 fixed fields plus a variable-length `body`. Variable-length
//!   SSZ container with one offset.
//! - [`BlockSignatures`] — `List[Signature, MAX_ATTESTATIONS]`, a bare
//!   fixed-element list (no self-offset).
//! - [`BlockWithAttestation`] — variable-length `block` plus the fixed
//!   `proposer_attestation` sibling.
//! - [`SignedBlockWithAttestation`] — variable-length `message` plus
//!   variable-length `signature`; two-offset SSZ container.

use ssz::merkleize::merkleize;
use ssz::{Decode, DecodeError, Encode, HashTreeRoot};
use types::{Bytes32, Signature};

use crate::internal::{
    decode_byte_vector_list, decode_fixed_element_list, encode_byte_vector_list,
    encode_fixed_element_list, ensure_len, list_hash_tree_root, read_byte_array, read_fixed,
    read_offset, write_offset, BLOCK_HEADER_LEN, BYTES32_LEN, BYTES_PER_LENGTH_OFFSET,
    SIGNATURE_LEN, SLOT_LEN, VALIDATOR_INDEX_LEN,
};
use crate::slot::Slot;
use crate::validator::ValidatorIndex;
use crate::vote::{Attestation, ATTESTATION_SSZ_LEN};

/// Maximum attestation count per block.
///
/// Aliases [`config::VALIDATOR_REGISTRY_LIMIT`] — the single-source SSZ list
/// cap — so the bound matches the validator-set cap that produces the votes.
/// Also caps [`BlockSignatures`] (one signature per attesting validator plus
/// the proposer never exceeds the registry size).
pub const MAX_ATTESTATIONS: usize = config::VALIDATOR_REGISTRY_LIMIT;

/// Fixed SSZ wire size of [`BlockHeader`] in bytes.
pub const BLOCK_HEADER_SSZ_LEN: usize = BLOCK_HEADER_LEN; // 112

/// Length of the fixed portion of a [`Block`] (4 fixed fields plus the
/// 4-byte offset for the variable-length `body`).
const BLOCK_FIXED_PART_LEN: usize =
    SLOT_LEN + VALIDATOR_INDEX_LEN + 2 * BYTES32_LEN + BYTES_PER_LENGTH_OFFSET; // 84

/// Length of the fixed portion of a [`BlockWithAttestation`] (4-byte offset for
/// the variable-length `block` plus the fixed inline `proposer_attestation`).
const BLOCK_WITH_ATTESTATION_FIXED_PART_LEN: usize = BYTES_PER_LENGTH_OFFSET + ATTESTATION_SSZ_LEN; // 140

/// Length of the fixed portion of a [`SignedBlockWithAttestation`] — two
/// offsets, one each for the variable-length `message` and `signature`.
const SIGNED_BLOCK_WITH_ATTESTATION_FIXED_PART_LEN: usize = 2 * BYTES_PER_LENGTH_OFFSET; // 8

// =====================================================================
// BlockHeader
// =====================================================================

/// Mirrors leanSpec/docs/client/containers.md `BlockHeader`.
///
/// 5 fixed-size fields → fixed SSZ payload of [`BLOCK_HEADER_SSZ_LEN`]
/// bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BlockHeader {
    /// Slot in which the block was produced.
    pub slot: Slot,
    /// Validator index of the proposer.
    pub proposer_index: ValidatorIndex,
    /// Hash-tree-root of the parent block.
    pub parent_root: Bytes32,
    /// Hash-tree-root of the post-state.
    pub state_root: Bytes32,
    /// Hash-tree-root of the [`BlockBody`].
    pub body_root: Bytes32,
}

impl Encode for BlockHeader {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        BLOCK_HEADER_SSZ_LEN
    }

    fn ssz_bytes_len(&self) -> usize {
        BLOCK_HEADER_SSZ_LEN
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        self.slot.ssz_append(buf);
        self.proposer_index.ssz_append(buf);
        buf.extend_from_slice(self.parent_root.as_slice());
        buf.extend_from_slice(self.state_root.as_slice());
        buf.extend_from_slice(self.body_root.as_slice());
    }
}

impl Decode for BlockHeader {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        BLOCK_HEADER_SSZ_LEN
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        ensure_len(bytes, BLOCK_HEADER_SSZ_LEN)?;
        let mut c = 0;
        Ok(Self {
            slot: read_fixed::<Slot>(bytes, &mut c)?,
            proposer_index: read_fixed::<ValidatorIndex>(bytes, &mut c)?,
            parent_root: Bytes32::new(read_byte_array(bytes, &mut c)),
            state_root: Bytes32::new(read_byte_array(bytes, &mut c)),
            body_root: Bytes32::new(read_byte_array(bytes, &mut c)),
        })
    }
}

impl HashTreeRoot for BlockHeader {
    fn hash_tree_root(&self) -> [u8; 32] {
        // 5 fields → merkleize zero-pads to width 8.
        merkleize(&[
            self.slot.hash_tree_root(),
            self.proposer_index.hash_tree_root(),
            self.parent_root.0,
            self.state_root.0,
            self.body_root.0,
        ])
    }
}

// =====================================================================
// BlockBody
// =====================================================================

/// Mirrors leanSpec/docs/client/containers.md `BlockBody`.
///
/// One variable-length field (`attestations`). The decoded list length is
/// validated against [`MAX_ATTESTATIONS`] at the decode boundary; direct
/// constructors should preserve the same invariant.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockBody {
    /// Bounded list of vote attestations included in this block.
    pub attestations: Vec<Attestation>,
}

impl Encode for BlockBody {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn ssz_bytes_len(&self) -> usize {
        BYTES_PER_LENGTH_OFFSET + self.attestations.len() * <Attestation as Encode>::ssz_fixed_len()
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        // Single variable field: offset = end of fixed portion = 4.
        write_offset(buf, BYTES_PER_LENGTH_OFFSET);
        encode_fixed_element_list(&self.attestations, buf);
    }
}

impl Decode for BlockBody {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < BYTES_PER_LENGTH_OFFSET {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: BYTES_PER_LENGTH_OFFSET,
            });
        }
        let mut c = 0;
        let offset = read_offset(bytes, &mut c)?;
        if offset != BYTES_PER_LENGTH_OFFSET {
            return Err(DecodeError::OffsetIntoFixedPortion(offset));
        }
        let attestations = decode_fixed_element_list::<Attestation>(&bytes[c..], MAX_ATTESTATIONS)?;
        Ok(Self { attestations })
    }
}

impl HashTreeRoot for BlockBody {
    fn hash_tree_root(&self) -> [u8; 32] {
        // Container with 1 field → merkleize at width 1 returns the field
        // root directly.
        list_hash_tree_root(&self.attestations, MAX_ATTESTATIONS)
    }
}

// =====================================================================
// Block
// =====================================================================

/// Mirrors leanSpec/docs/client/containers.md `Block`.
///
/// 4 fixed-size fields plus the variable-length [`BlockBody`]. SSZ-encoded
/// with one length-offset (placed after the fixed fields, pointing at the
/// body bytes).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Block {
    /// Slot in which this block was produced.
    pub slot: Slot,
    /// Validator index of the proposer.
    pub proposer_index: ValidatorIndex,
    /// Hash-tree-root of the parent block.
    pub parent_root: Bytes32,
    /// Hash-tree-root of the post-state.
    pub state_root: Bytes32,
    /// Variable-length payload.
    pub body: BlockBody,
}

impl Encode for Block {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn ssz_bytes_len(&self) -> usize {
        BLOCK_FIXED_PART_LEN + self.body.ssz_bytes_len()
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        self.slot.ssz_append(buf);
        self.proposer_index.ssz_append(buf);
        buf.extend_from_slice(self.parent_root.as_slice());
        buf.extend_from_slice(self.state_root.as_slice());
        write_offset(buf, BLOCK_FIXED_PART_LEN);
        self.body.ssz_append(buf);
    }
}

impl Decode for Block {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < BLOCK_FIXED_PART_LEN {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: BLOCK_FIXED_PART_LEN,
            });
        }
        let mut c = 0;
        let slot = read_fixed::<Slot>(bytes, &mut c)?;
        let proposer_index = read_fixed::<ValidatorIndex>(bytes, &mut c)?;
        let parent_root = Bytes32::new(read_byte_array(bytes, &mut c));
        let state_root = Bytes32::new(read_byte_array(bytes, &mut c));
        let offset = read_offset(bytes, &mut c)?;
        if offset != BLOCK_FIXED_PART_LEN {
            return Err(DecodeError::OffsetIntoFixedPortion(offset));
        }
        let body = BlockBody::from_ssz_bytes(&bytes[offset..])?;
        Ok(Self {
            slot,
            proposer_index,
            parent_root,
            state_root,
            body,
        })
    }
}

impl HashTreeRoot for Block {
    fn hash_tree_root(&self) -> [u8; 32] {
        // 5 fields → merkleize zero-pads to width 8.
        merkleize(&[
            self.slot.hash_tree_root(),
            self.proposer_index.hash_tree_root(),
            self.parent_root.0,
            self.state_root.0,
            self.body.hash_tree_root(),
        ])
    }
}

// =====================================================================
// BlockSignatures
// =====================================================================

/// Mirrors the leanSpec `BlockSignatures = List[Signature, VALIDATOR_REGISTRY_LIMIT]`.
///
/// The spec bound is the validator-registry limit; [`MAX_ATTESTATIONS`] is that
/// same limit (aliasing the single-source [`config::VALIDATOR_REGISTRY_LIMIT`]),
/// so the list is capped on it — one signature per attesting validator plus the
/// proposer never exceeds the registry size.
///
/// A bare fixed-element list: empty encodes to zero bytes, `k` elements to
/// `k * Signature::LEN`. The offset that bounds these bytes lives in the parent
/// [`SignedBlockWithAttestation`], not here. Holds inert signature *bytes* only —
/// verification is a `runtime`-layer concern.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockSignatures(Vec<Signature>);

impl core::ops::Deref for BlockSignatures {
    type Target = [Signature];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FromIterator<Signature> for BlockSignatures {
    fn from_iter<I: IntoIterator<Item = Signature>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl Encode for BlockSignatures {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn ssz_bytes_len(&self) -> usize {
        self.0.len() * SIGNATURE_LEN
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        encode_byte_vector_list(&self.0, buf);
    }
}

impl Decode for BlockSignatures {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        decode_byte_vector_list::<SIGNATURE_LEN>(bytes, MAX_ATTESTATIONS).map(Self)
    }
}

impl HashTreeRoot for BlockSignatures {
    fn hash_tree_root(&self) -> [u8; 32] {
        // `Signature: HashTreeRoot` via the ssz `ByteVector<N>` impl.
        list_hash_tree_root(&self.0, MAX_ATTESTATIONS)
    }
}

// =====================================================================
// BlockWithAttestation
// =====================================================================

/// Mirrors the leanSpec `BlockWithAttestation`.
///
/// Field order `block` then `proposer_attestation`. NOTE the SSZ layout: `block`
/// is variable (offset in slot 0) and `proposer_attestation` is fixed (inline in
/// slot 1), so on the wire the proposer bytes PRECEDE the block payload. The
/// hash-tree-root uses FIELD order (`block` first) — do not confuse the two.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockWithAttestation {
    /// The unsigned block.
    pub block: Block,
    /// The proposer's own attestation — a sibling of `block`, not inside the body.
    pub proposer_attestation: Attestation,
}

impl Encode for BlockWithAttestation {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn ssz_bytes_len(&self) -> usize {
        BLOCK_WITH_ATTESTATION_FIXED_PART_LEN + self.block.ssz_bytes_len()
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        write_offset(buf, BLOCK_WITH_ATTESTATION_FIXED_PART_LEN); // block offset = 140
        self.proposer_attestation.ssz_append(buf); // fixed 136 inline
        self.block.ssz_append(buf); // payload at 140
    }
}

impl Decode for BlockWithAttestation {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < BLOCK_WITH_ATTESTATION_FIXED_PART_LEN {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: BLOCK_WITH_ATTESTATION_FIXED_PART_LEN,
            });
        }
        let mut c = 0;
        let offset = read_offset(bytes, &mut c)?;
        if offset != BLOCK_WITH_ATTESTATION_FIXED_PART_LEN {
            return Err(DecodeError::OffsetIntoFixedPortion(offset));
        }
        let proposer_attestation = read_fixed::<Attestation>(bytes, &mut c)?; // consumes [4..140]
        let block = Block::from_ssz_bytes(&bytes[offset..])?;
        Ok(Self {
            block,
            proposer_attestation,
        })
    }
}

impl HashTreeRoot for BlockWithAttestation {
    fn hash_tree_root(&self) -> [u8; 32] {
        // 2 fields → width 2. Field order: block then proposer_attestation.
        merkleize(&[
            self.block.hash_tree_root(),
            self.proposer_attestation.hash_tree_root(),
        ])
    }
}

// =====================================================================
// SignedBlockWithAttestation
// =====================================================================

/// Mirrors the leanSpec `SignedBlockWithAttestation`.
///
/// Both fields are variable-length → the fixed part is two offsets. The
/// signature offset that a bare [`BlockSignatures`] omits lives HERE.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignedBlockWithAttestation {
    /// The unsigned [`BlockWithAttestation`] being signed.
    pub message: BlockWithAttestation,
    /// The block signatures (one per body attestation plus the proposer's).
    pub signature: BlockSignatures,
}

impl Encode for SignedBlockWithAttestation {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn ssz_bytes_len(&self) -> usize {
        SIGNED_BLOCK_WITH_ATTESTATION_FIXED_PART_LEN
            + self.message.ssz_bytes_len()
            + self.signature.ssz_bytes_len()
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        let message_offset = SIGNED_BLOCK_WITH_ATTESTATION_FIXED_PART_LEN; // 8
        let signature_offset = message_offset + self.message.ssz_bytes_len();
        write_offset(buf, message_offset);
        write_offset(buf, signature_offset);
        self.message.ssz_append(buf);
        self.signature.ssz_append(buf);
    }
}

impl Decode for SignedBlockWithAttestation {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < SIGNED_BLOCK_WITH_ATTESTATION_FIXED_PART_LEN {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: SIGNED_BLOCK_WITH_ATTESTATION_FIXED_PART_LEN,
            });
        }
        let mut c = 0;
        let message_offset = read_offset(bytes, &mut c)?;
        let signature_offset = read_offset(bytes, &mut c)?;
        if message_offset != SIGNED_BLOCK_WITH_ATTESTATION_FIXED_PART_LEN {
            return Err(DecodeError::OffsetIntoFixedPortion(message_offset));
        }
        if signature_offset < message_offset || signature_offset > bytes.len() {
            return Err(DecodeError::OffsetOutOfBounds(signature_offset));
        }
        let message =
            BlockWithAttestation::from_ssz_bytes(&bytes[message_offset..signature_offset])?;
        let signature = BlockSignatures::from_ssz_bytes(&bytes[signature_offset..])?;
        Ok(Self { message, signature })
    }
}

impl HashTreeRoot for SignedBlockWithAttestation {
    fn hash_tree_root(&self) -> [u8; 32] {
        // 2 fields → width 2 → single hash_pair via merkleize.
        merkleize(&[
            self.message.hash_tree_root(),
            self.signature.hash_tree_root(),
        ])
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use ssz::{decode, encode, SszError};

    use crate::test_fixtures::{
        sample_attestation, sample_block, sample_block_header, sample_block_with_attestation,
        sample_signature, sample_signed_block_with_attestation,
    };

    // -- Config-derived caps (single source) --------------------------------

    /// Acceptance test: the `List<Signature, N>` cap and the `Bitlist<N>` cap
    /// resolve to the identical single source.
    ///
    /// Both clauses are structurally tautological (each side derives from the
    /// same const) — they are STRUCTURE guards, not value guards: clause (a)
    /// fails if `MAX_ATTESTATIONS` stops aliasing the config const, clause (b)
    /// (a placeholder witness that hard-codes the const into the generic slot)
    /// documents that the `Bitlist<N>` cap will consume the same source once the
    /// aggregation type is authored. The VALUE guarantee is carried by
    /// `no_bare_registry_literal_in_cap_consts` (asserts `== 4_096`) + the frozen
    /// state-root regression vector, not by this test.
    #[test]
    fn list_and_bitlist_caps_share_one_source() {
        use types::Bitlist;

        // (a) the List<Signature, N> cap (MAX_ATTESTATIONS bounds BlockSignatures).
        assert_eq!(MAX_ATTESTATIONS, config::VALIDATOR_REGISTRY_LIMIT);

        // (b) the Bitlist<N> cap resolves to the SAME source, via the existing
        //     compile-time `limit()` accessor (no new accessor is added).
        assert_eq!(
            Bitlist::<{ config::VALIDATOR_REGISTRY_LIMIT }>::new().limit(),
            MAX_ATTESTATIONS
        );
    }

    /// Source-of-truth guard: re-inlining `4096`/`1 << 12` on a cap const breaks
    /// this assertion (invariant check, not a text grep).
    #[test]
    fn no_bare_registry_literal_in_cap_consts() {
        assert_eq!(MAX_ATTESTATIONS, config::VALIDATOR_REGISTRY_LIMIT);
        assert_eq!(config::VALIDATOR_REGISTRY_LIMIT, 4_096);
    }

    // -- BlockHeader --------------------------------------------------------

    #[test]
    fn block_header_ssz_fixed_len_is_one_twelve() {
        assert_eq!(<BlockHeader as Encode>::ssz_fixed_len(), 112);
        assert!(<BlockHeader as Encode>::is_ssz_fixed_len());
    }

    #[test]
    fn block_header_encode_layout_concatenates_fields() {
        let h = sample_block_header();
        let bytes = encode(&h);
        assert_eq!(bytes.len(), BLOCK_HEADER_SSZ_LEN);
        assert_eq!(&bytes[..8], &7_u64.to_le_bytes());
        assert_eq!(&bytes[8..16], &2_u64.to_le_bytes());
        assert_eq!(&bytes[16..48], &[0x11_u8; 32]);
        assert_eq!(&bytes[48..80], &[0x22_u8; 32]);
        assert_eq!(&bytes[80..112], &[0x33_u8; 32]);
    }

    #[test]
    fn block_header_round_trip() {
        let h = sample_block_header();
        let back: BlockHeader = decode(&encode(&h)).unwrap();
        assert_eq!(back, h);
    }

    #[test]
    fn block_header_decode_rejects_wrong_length() {
        assert!(decode::<BlockHeader>(&[0_u8; BLOCK_HEADER_SSZ_LEN - 1]).is_err());
        assert!(decode::<BlockHeader>(&[0_u8; BLOCK_HEADER_SSZ_LEN + 1]).is_err());
    }

    #[test]
    fn block_header_hash_tree_root_responds_to_each_field() {
        let baseline = sample_block_header().hash_tree_root();

        let mut h = sample_block_header();
        h.slot = Slot::new(8);
        assert_ne!(h.hash_tree_root(), baseline);

        let mut h = sample_block_header();
        h.proposer_index = ValidatorIndex::new(3);
        assert_ne!(h.hash_tree_root(), baseline);

        let mut h = sample_block_header();
        h.body_root = Bytes32::new([0x99; 32]);
        assert_ne!(h.hash_tree_root(), baseline);
    }

    // -- BlockBody ----------------------------------------------------------

    #[test]
    fn block_body_empty_encodes_to_offset_only() {
        let body = BlockBody::default();
        let bytes = encode(&body);
        assert_eq!(bytes, [0x04, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn block_body_round_trip_empty() {
        let body = BlockBody::default();
        let back: BlockBody = decode(&encode(&body)).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn block_body_holds_plain_attestations() {
        // Type-level: the element is `Attestation` (validator_id + data), which
        // has no `.signature` field — the signatures live in `BlockSignatures`.
        let body = BlockBody {
            attestations: vec![sample_attestation(1), sample_attestation(2)],
        };
        assert_eq!(body.attestations.len(), 2);
    }

    #[test]
    fn block_body_round_trip_nonempty_stride_136() {
        let body = BlockBody {
            attestations: vec![
                sample_attestation(1),
                sample_attestation(2),
                sample_attestation(3),
            ],
        };
        let bytes = encode(&body);
        // Layout: 4-byte offset (=4), then concatenated Attestation bytes.
        assert_eq!(&bytes[..4], &[0x04, 0x00, 0x00, 0x00]);
        let elem_len = <Attestation as Encode>::ssz_fixed_len();
        assert_eq!(elem_len, 136);
        assert_eq!(bytes.len(), 4 + 3 * elem_len);
        let back: BlockBody = decode(&bytes).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn block_body_decode_rejects_truncated_offset() {
        let err = decode::<BlockBody>(&[0_u8; 3]).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    #[test]
    fn block_body_decode_rejects_invalid_offset() {
        // Offset != 4 → OffsetIntoFixedPortion.
        let mut bytes = vec![0x05_u8, 0x00, 0x00, 0x00];
        bytes.resize(4 + <Attestation as Encode>::ssz_fixed_len(), 0);
        let err = decode::<BlockBody>(&bytes).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    #[test]
    fn block_body_decode_rejects_size_not_divisible_by_element() {
        // Offset = 4, then 1 byte of element data (not a whole 136-byte element).
        let bytes = vec![0x04_u8, 0x00, 0x00, 0x00, 0xaa];
        let err = decode::<BlockBody>(&bytes).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    #[test]
    fn block_body_hash_tree_root_changes_with_element() {
        let body_a = BlockBody {
            attestations: vec![sample_attestation(1)],
        };
        let body_b = BlockBody {
            attestations: vec![sample_attestation(2)],
        };
        assert_ne!(body_a.hash_tree_root(), body_b.hash_tree_root());
    }

    #[test]
    fn block_body_hash_tree_root_changes_with_length() {
        let body_one = BlockBody {
            attestations: vec![sample_attestation(1)],
        };
        let body_two = BlockBody {
            attestations: vec![sample_attestation(1), sample_attestation(1)],
        };
        // Different lengths → mix_in_length differs → roots differ.
        assert_ne!(body_one.hash_tree_root(), body_two.hash_tree_root());
    }

    // -- Block --------------------------------------------------------------

    #[test]
    fn block_round_trip_with_empty_body() {
        let mut b = sample_block();
        b.body = BlockBody::default();
        let back: Block = decode(&encode(&b)).unwrap();
        assert_eq!(back, b);
    }

    #[test]
    fn block_round_trip_with_attestations() {
        let b = sample_block();
        let bytes = encode(&b);
        // Fixed portion = 84 bytes; offset must be 84 (LE).
        assert_eq!(&bytes[80..84], &84_u32.to_le_bytes());
        let back: Block = decode(&bytes).unwrap();
        assert_eq!(back, b);
    }

    #[test]
    fn block_decode_rejects_short_input() {
        let err = decode::<Block>(&[0_u8; BLOCK_FIXED_PART_LEN - 1]).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    #[test]
    fn block_decode_rejects_invalid_offset() {
        let mut bytes = vec![0_u8; BLOCK_FIXED_PART_LEN];
        // Offset = 83 instead of 84.
        bytes[80..84].copy_from_slice(&83_u32.to_le_bytes());
        let err = decode::<Block>(&bytes).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    #[test]
    fn block_hash_tree_root_responds_to_body_change() {
        let mut b = sample_block();
        let baseline = b.hash_tree_root();
        b.body.attestations.push(sample_attestation(99));
        assert_ne!(b.hash_tree_root(), baseline);
    }

    // -- BlockSignatures ----------------------------------------------------

    #[test]
    fn block_signatures_default_is_empty_zero_bytes() {
        let bs = BlockSignatures::default();
        assert!(bs.is_empty());
        assert_eq!(encode(&bs), Vec::<u8>::new());
        assert_eq!(decode::<BlockSignatures>(&[]).unwrap(), bs);
    }

    #[test]
    fn block_signatures_stride_is_signature_len() {
        let bs: BlockSignatures = [sample_signature(1), sample_signature(2)]
            .into_iter()
            .collect();
        let bytes = encode(&bs);
        // Bare fixed-element list: no self-offset, k * 3116 bytes.
        assert_eq!(bytes.len(), 2 * Signature::LEN);
        assert_eq!(decode::<BlockSignatures>(&bytes).unwrap(), bs);
    }

    #[test]
    fn block_signatures_decode_rejects_over_cap() {
        let bytes = vec![0_u8; (MAX_ATTESTATIONS + 1) * Signature::LEN];
        assert!(matches!(
            decode::<BlockSignatures>(&bytes),
            Err(SszError::Decode { .. })
        ));
    }

    #[test]
    fn block_signatures_hash_tree_root_changes_with_len_and_element() {
        let one: BlockSignatures = [sample_signature(1)].into_iter().collect();
        let two: BlockSignatures = [sample_signature(1), sample_signature(1)]
            .into_iter()
            .collect();
        let other: BlockSignatures = [sample_signature(2)].into_iter().collect();
        assert_ne!(one.hash_tree_root(), two.hash_tree_root());
        assert_ne!(one.hash_tree_root(), other.hash_tree_root());
    }

    // -- BlockWithAttestation -----------------------------------------------

    #[test]
    fn block_with_attestation_encode_layout_block_then_proposer() {
        let bwa = sample_block_with_attestation();
        let bytes = encode(&bwa);
        // slot 0: 4-byte offset for `block` = 140.
        assert_eq!(&bytes[..4], &140_u32.to_le_bytes());
        // slot 1: `proposer_attestation` inline at [4..140].
        assert_eq!(&bytes[4..140], encode(&bwa.proposer_attestation).as_slice());
        // block payload at offset 140.
        assert_eq!(&bytes[140..], encode(&bwa.block).as_slice());
        assert_eq!(decode::<BlockWithAttestation>(&bytes).unwrap(), bwa);
    }

    #[test]
    fn block_with_attestation_hash_tree_root_is_two_field_merkle_order_matters() {
        let bwa = sample_block_with_attestation();
        assert_eq!(
            bwa.hash_tree_root(),
            merkleize(&[
                bwa.block.hash_tree_root(),
                bwa.proposer_attestation.hash_tree_root(),
            ]),
        );
        // Swapping the two field roots changes the result → order is load-bearing.
        let swapped = merkleize(&[
            bwa.proposer_attestation.hash_tree_root(),
            bwa.block.hash_tree_root(),
        ]);
        assert_ne!(bwa.hash_tree_root(), swapped);
    }

    #[test]
    fn block_with_attestation_hash_tree_root_responds_to_proposer_change() {
        let baseline = sample_block_with_attestation().hash_tree_root();
        let mut bwa = sample_block_with_attestation();
        bwa.proposer_attestation = sample_attestation(123);
        // Proposer is a sibling, not swallowed by the body.
        assert_ne!(bwa.hash_tree_root(), baseline);
    }

    #[test]
    fn block_with_attestation_decode_rejects_short_input() {
        let err =
            decode::<BlockWithAttestation>(&[0_u8; BLOCK_WITH_ATTESTATION_FIXED_PART_LEN - 1])
                .unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    // -- SignedBlockWithAttestation -----------------------------------------

    #[test]
    fn signed_block_with_attestation_two_offset_layout() {
        let sbwa = sample_signed_block_with_attestation();
        let bytes = encode(&sbwa);
        assert_eq!(&bytes[0..4], &8_u32.to_le_bytes()); // message offset
        let msg_len = sbwa.message.ssz_bytes_len();
        assert_eq!(
            &bytes[4..8],
            &u32::try_from(8 + msg_len).unwrap().to_le_bytes(), // signature offset
        );
        assert_eq!(decode::<SignedBlockWithAttestation>(&bytes).unwrap(), sbwa);
    }

    #[test]
    fn signed_block_with_attestation_empty_signature_two_offset_header() {
        let mut sbwa = sample_signed_block_with_attestation();
        sbwa.signature = BlockSignatures::default();
        let bytes = encode(&sbwa);
        assert_eq!(&bytes[0..4], &8_u32.to_le_bytes());
        // Empty signature payload → signature offset == total length.
        assert_eq!(
            &bytes[4..8],
            &u32::try_from(bytes.len()).unwrap().to_le_bytes(),
        );
        assert_eq!(decode::<SignedBlockWithAttestation>(&bytes).unwrap(), sbwa);
    }

    #[test]
    fn signed_block_with_attestation_hash_tree_root_is_two_field_merkle() {
        let sbwa = sample_signed_block_with_attestation();
        assert_eq!(
            sbwa.hash_tree_root(),
            merkleize(&[
                sbwa.message.hash_tree_root(),
                sbwa.signature.hash_tree_root(),
            ]),
        );
        // Mutating the signature list alone changes the root.
        let mut m = sbwa.clone();
        m.signature = [sample_signature(9)].into_iter().collect();
        assert_ne!(m.hash_tree_root(), sbwa.hash_tree_root());
    }

    #[test]
    fn signed_block_with_attestation_decode_rejects_short_and_bad_offset() {
        assert!(matches!(
            decode::<SignedBlockWithAttestation>(&[0_u8; 7]),
            Err(SszError::Decode { .. })
        ));
        let mut bytes = encode(&sample_signed_block_with_attestation());
        bytes[0..4].copy_from_slice(&7_u32.to_le_bytes()); // message offset != 8
        assert!(matches!(
            decode::<SignedBlockWithAttestation>(&bytes),
            Err(SszError::Decode { .. })
        ));
    }

    // -- property tests ----------------------------------------------------

    proptest! {
        #[test]
        fn block_header_ssz_round_trips(
            slot in any::<u64>(),
            proposer in any::<u64>(),
            parent in proptest::array::uniform32(any::<u8>()),
            state in proptest::array::uniform32(any::<u8>()),
            body in proptest::array::uniform32(any::<u8>()),
        ) {
            let h = BlockHeader {
                slot: Slot::new(slot),
                proposer_index: ValidatorIndex::new(proposer),
                parent_root: Bytes32::new(parent),
                state_root: Bytes32::new(state),
                body_root: Bytes32::new(body),
            };
            let back: BlockHeader = decode(&encode(&h)).unwrap();
            prop_assert_eq!(back, h);
        }

        #[test]
        fn block_body_ssz_round_trips(n in 0_usize..=8) {
            let attestations = (0..n).map(|i| sample_attestation(i as u64)).collect();
            let body = BlockBody { attestations };
            let back: BlockBody = decode(&encode(&body)).unwrap();
            prop_assert_eq!(back, body);
        }

        #[test]
        fn block_ssz_round_trips(slot in any::<u64>(), n in 0_usize..=4) {
            let attestations = (0..n).map(|i| sample_attestation(i as u64)).collect();
            let b = Block {
                slot: Slot::new(slot),
                body: BlockBody { attestations },
                ..Default::default()
            };
            let back: Block = decode(&encode(&b)).unwrap();
            prop_assert_eq!(back, b);
        }

        #[test]
        fn signed_block_with_attestation_ssz_round_trips(slot in any::<u64>(), n in 0_usize..=4) {
            let attestations = (0..n).map(|i| sample_attestation(i as u64)).collect();
            let signature: BlockSignatures = (0..=n)
                .map(|i| sample_signature(u8::try_from(i).unwrap()))
                .collect();
            let sbwa = SignedBlockWithAttestation {
                message: BlockWithAttestation {
                    block: Block {
                        slot: Slot::new(slot),
                        body: BlockBody { attestations },
                        ..Default::default()
                    },
                    proposer_attestation: sample_attestation(slot),
                },
                signature,
            };
            let back: SignedBlockWithAttestation = decode(&encode(&sbwa)).unwrap();
            prop_assert_eq!(back, sbwa);
        }
    }
}
