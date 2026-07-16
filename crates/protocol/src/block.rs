//! Block containers — [`BlockHeader`], [`BlockBody`], [`Block`], and
//! [`SignedBlock`].
//!
//! Mirrors leanSpec/docs/client/containers.md. The signature placeholder is
//! a 4000-byte vector reserved for the eventual XMSS post-quantum scheme;
//! the documented wire format is authoritative for cross-client
//! compatibility on devnet0.
//!
//! Wire shapes:
//! - [`BlockHeader`] — 5 fixed-size fields, 112-byte SSZ payload.
//! - [`BlockBody`] — single variable-length field (`attestations:
//!   List[SignedAttestation, MAX_ATTESTATIONS]`). Variable-length SSZ container.
//! - [`Block`] — 4 fixed fields plus a variable-length `body`. Variable-length
//!   SSZ container with one offset.
//! - [`SignedBlock`] — variable-length `message: Block` plus the fixed
//!   4000-byte signature. Variable-length SSZ container with one offset.

use ssz::merkleize::merkleize;
use ssz::{Decode, DecodeError, Encode, HashTreeRoot};
use types::Bytes32;
// Retained construction sites for the deprecated `Bytes4000` placeholder; move
// to `Signature` with the container refactor. Scoped to the items that name it
// — the `SignedBlock` field, the decode leg, and the test module — so unrelated
// deprecations in the rest of this file are still surfaced. `expect` rather than
// `allow`: once the sites move, the unfulfilled expectation fails the build.
#[expect(deprecated)]
use types::Bytes4000;

use crate::internal::{
    decode_fixed_element_list, encode_fixed_element_list, ensure_len, list_hash_tree_root,
    read_byte_array, read_fixed, read_offset, write_offset, BLOCK_HEADER_LEN, BYTES32_LEN,
    BYTES4000_LEN, BYTES_PER_LENGTH_OFFSET, SLOT_LEN, VALIDATOR_INDEX_LEN,
};
use crate::slot::Slot;
use crate::validator::ValidatorIndex;
use crate::vote::SignedAttestation;

/// Maximum attestation count per block.
///
/// Pinned to the devnet0 [`config::DEVNET_CONFIG::validator_registry_limit`]
/// so the bound matches the validator-set cap that produces the votes.
#[allow(clippy::cast_possible_truncation)]
pub const MAX_ATTESTATIONS: usize = config::DEVNET_CONFIG.validator_registry_limit as usize;

/// Fixed SSZ wire size of [`BlockHeader`] in bytes.
pub const BLOCK_HEADER_SSZ_LEN: usize = BLOCK_HEADER_LEN; // 112

/// Length of the fixed portion of a [`Block`] (4 fixed fields plus the
/// 4-byte offset for the variable-length `body`).
const BLOCK_FIXED_PART_LEN: usize =
    SLOT_LEN + VALIDATOR_INDEX_LEN + 2 * BYTES32_LEN + BYTES_PER_LENGTH_OFFSET; // 84

/// Length of the fixed portion of a [`SignedBlock`] (4-byte offset for the
/// variable-length `message` plus the fixed 4000-byte signature).
const SIGNED_BLOCK_FIXED_PART_LEN: usize = BYTES_PER_LENGTH_OFFSET + BYTES4000_LEN; // 4004

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
    pub attestations: Vec<SignedAttestation>,
}

impl Encode for BlockBody {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn ssz_bytes_len(&self) -> usize {
        BYTES_PER_LENGTH_OFFSET
            + self.attestations.len() * <SignedAttestation as Encode>::ssz_fixed_len()
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
        let attestations =
            decode_fixed_element_list::<SignedAttestation>(&bytes[c..], MAX_ATTESTATIONS)?;
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
// SignedBlock
// =====================================================================

/// Mirrors leanSpec/docs/client/containers.md `SignedBlock`.
///
/// Variable-length envelope pairing a [`Block`] with the 4000-byte
/// post-quantum-signature placeholder.
// `allow` rather than `expect`, unlike the other sites in this file: the derives
// below expand to code naming the field's type, and a lint *expectation* does
// not propagate into derive expansion (a field- or struct-level `expect` is
// reported fulfilled while the expanded `Clone`/`Default`/`PartialEq` impls
// still warn). `allow` does propagate. Scoped to this struct, and retires with
// the field when it moves to `Signature`.
#[allow(deprecated)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignedBlock {
    /// The unsigned [`Block`] being attested to.
    pub message: Block,
    /// 4000-byte XMSS post-quantum-signature placeholder.
    pub signature: Bytes4000,
}

impl Encode for SignedBlock {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn ssz_bytes_len(&self) -> usize {
        SIGNED_BLOCK_FIXED_PART_LEN + self.message.ssz_bytes_len()
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        write_offset(buf, SIGNED_BLOCK_FIXED_PART_LEN);
        buf.extend_from_slice(self.signature.as_slice());
        self.message.ssz_append(buf);
    }
}

impl Decode for SignedBlock {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    #[expect(deprecated)]
    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < SIGNED_BLOCK_FIXED_PART_LEN {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: SIGNED_BLOCK_FIXED_PART_LEN,
            });
        }
        let mut c = 0;
        let offset = read_offset(bytes, &mut c)?;
        if offset != SIGNED_BLOCK_FIXED_PART_LEN {
            return Err(DecodeError::OffsetIntoFixedPortion(offset));
        }
        let signature = Bytes4000::new(read_byte_array::<BYTES4000_LEN>(bytes, &mut c));
        let message = Block::from_ssz_bytes(&bytes[offset..])?;
        Ok(Self { message, signature })
    }
}

impl HashTreeRoot for SignedBlock {
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
#[expect(deprecated)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use ssz::{decode, encode, SszError};

    use crate::test_fixtures::{
        sample_block, sample_block_header, sample_signed_attestation, sample_signed_block,
    };

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
    fn block_body_round_trip_nonempty() {
        let body = BlockBody {
            attestations: vec![
                sample_signed_attestation(1),
                sample_signed_attestation(2),
                sample_signed_attestation(3),
            ],
        };
        let bytes = encode(&body);
        // Layout: 4-byte offset (=4), then concatenated SignedAttestation bytes.
        assert_eq!(&bytes[..4], &[0x04, 0x00, 0x00, 0x00]);
        let elem_len = <SignedAttestation as Encode>::ssz_fixed_len();
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
        bytes.resize(4 + <SignedAttestation as Encode>::ssz_fixed_len(), 0);
        let err = decode::<BlockBody>(&bytes).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    #[test]
    fn block_body_decode_rejects_size_not_divisible_by_element() {
        // Offset = 4, then 1 byte of element data (not 3252).
        let bytes = vec![0x04_u8, 0x00, 0x00, 0x00, 0xaa];
        let err = decode::<BlockBody>(&bytes).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    #[test]
    fn block_body_hash_tree_root_changes_with_element() {
        let body_a = BlockBody {
            attestations: vec![sample_signed_attestation(1)],
        };
        let body_b = BlockBody {
            attestations: vec![sample_signed_attestation(2)],
        };
        assert_ne!(body_a.hash_tree_root(), body_b.hash_tree_root());
    }

    #[test]
    fn block_body_hash_tree_root_changes_with_length() {
        let body_one = BlockBody {
            attestations: vec![sample_signed_attestation(1)],
        };
        let body_two = BlockBody {
            attestations: vec![sample_signed_attestation(1), sample_signed_attestation(1)],
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
        b.body.attestations.push(sample_signed_attestation(99));
        assert_ne!(b.hash_tree_root(), baseline);
    }

    // -- SignedBlock --------------------------------------------------------

    #[test]
    fn signed_block_round_trip() {
        let sb = sample_signed_block();
        let bytes = encode(&sb);
        // Offset must be 4004 (LE).
        assert_eq!(&bytes[..4], &4004_u32.to_le_bytes());
        // Signature occupies bytes 4..4004.
        assert!(bytes[4..4004].iter().all(|&b| b == 0xcd));
        let back: SignedBlock = decode(&bytes).unwrap();
        assert_eq!(back, sb);
    }

    #[test]
    fn signed_block_signature_is_bytes4000_not_bytes32() {
        // Compile-time + runtime sanity that the field type is 4000 bytes.
        let sb = sample_signed_block();
        assert_eq!(sb.signature.as_slice().len(), 4000);
    }

    #[test]
    fn signed_block_decode_rejects_short_input() {
        let err = decode::<SignedBlock>(&[0_u8; SIGNED_BLOCK_FIXED_PART_LEN - 1]).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    #[test]
    fn signed_block_decode_rejects_invalid_offset() {
        let mut bytes = vec![0_u8; SIGNED_BLOCK_FIXED_PART_LEN + BLOCK_FIXED_PART_LEN + 4];
        // Offset = 4003 instead of 4004.
        bytes[..4].copy_from_slice(&4003_u32.to_le_bytes());
        let err = decode::<SignedBlock>(&bytes).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    #[test]
    fn signed_block_hash_tree_root_responds_to_signature_change() {
        let baseline = sample_signed_block().hash_tree_root();
        let mut sb = sample_signed_block();
        let mut sig = [0xcd_u8; 4000];
        sig[0] = 0x00;
        sb.signature = Bytes4000::new(sig);
        assert_ne!(sb.hash_tree_root(), baseline);
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
            let attestations = (0..n).map(|i| sample_signed_attestation(i as u64)).collect();
            let body = BlockBody { attestations };
            let back: BlockBody = decode(&encode(&body)).unwrap();
            prop_assert_eq!(back, body);
        }

        #[test]
        fn block_ssz_round_trips(slot in any::<u64>(), n in 0_usize..=4) {
            let attestations = (0..n).map(|i| sample_signed_attestation(i as u64)).collect();
            let b = Block {
                slot: Slot::new(slot),
                body: BlockBody { attestations },
                ..Default::default()
            };
            let back: Block = decode(&encode(&b)).unwrap();
            prop_assert_eq!(back, b);
        }

        #[test]
        fn signed_block_ssz_round_trips(slot in any::<u64>(), n in 0_usize..=4) {
            let attestations = (0..n).map(|i| sample_signed_attestation(i as u64)).collect();
            let sb = SignedBlock {
                message: Block {
                    slot: Slot::new(slot),
                    body: BlockBody { attestations },
                    ..Default::default()
                },
                signature: Bytes4000::new([0x77; 4000]),
            };
            let back: SignedBlock = decode(&encode(&sb)).unwrap();
            prop_assert_eq!(back, sb);
        }
    }
}
