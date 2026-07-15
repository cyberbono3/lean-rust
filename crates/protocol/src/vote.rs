//! [`Vote`] — unsigned validator vote — and [`SignedVote`], the wire-shape
//! container that pairs a vote with its post-quantum-signature placeholder.
//!
//! Wire layout follows the canonical consensus-spec containers:
//!
//! - `Vote { slot, head, target, source }` — four fixed-size fields.
//! - `SignedVote { validator_id, message, signature }` — `validator_id` lives
//!   on the outer signed envelope, **not** on the inner `Vote`. The
//!   signature is sized at 4000 bytes for the eventual XMSS post-quantum
//!   scheme.
//!
//! All fields are SSZ-fixed-length, so both containers serialize to a fixed
//! byte count: 128 bytes for [`Vote`], 4136 bytes for [`SignedVote`].

// Retained construction sites for the deprecated `Bytes4000` placeholder.
// Scoped to this file so unrelated deprecations elsewhere in the crate are
// still surfaced; removed when this file's last site moves to `Signature`.
#![allow(deprecated)]

use ssz::merkleize::{hash_pair, merkleize, ZERO_HASH};
use ssz::{Decode, DecodeError, Encode, HashTreeRoot};
use types::Bytes4000;

use crate::checkpoint::Checkpoint;
use crate::internal::{
    ensure_len, read_byte_array, read_fixed, u64_chunk, BYTES4000_LEN, CHECKPOINT_LEN, SLOT_LEN,
    VALIDATOR_INDEX_LEN,
};
use crate::slot::Slot;
use crate::validator::ValidatorIndex;

const VOTE_SSZ_LEN: usize = SLOT_LEN + 3 * CHECKPOINT_LEN; // 128
const SIGNED_VOTE_SSZ_LEN: usize = VALIDATOR_INDEX_LEN + VOTE_SSZ_LEN + BYTES4000_LEN; // 4136

/// Unsigned validator vote.
///
/// # Example
/// ```
/// use protocol::{Checkpoint, Slot, Vote};
/// use types::Bytes32;
/// let v = Vote {
///     slot: Slot::new(1),
///     head: Checkpoint::new(Bytes32::zero(), Slot::new(1)),
///     target: Checkpoint::new(Bytes32::zero(), Slot::new(0)),
///     source: Checkpoint::new(Bytes32::zero(), Slot::new(0)),
/// };
/// assert_eq!(v.slot.get(), 1);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Vote {
    /// Slot in which the vote was cast.
    pub slot: Slot,
    /// Block the voter considers the canonical head at `slot`.
    pub head: Checkpoint,
    /// Justification target the vote attests to.
    pub target: Checkpoint,
    /// Justification source the vote builds on.
    pub source: Checkpoint,
}

impl Encode for Vote {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        VOTE_SSZ_LEN
    }

    fn ssz_bytes_len(&self) -> usize {
        VOTE_SSZ_LEN
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        self.slot.ssz_append(buf);
        self.head.ssz_append(buf);
        self.target.ssz_append(buf);
        self.source.ssz_append(buf);
    }
}

impl Decode for Vote {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        VOTE_SSZ_LEN
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        ensure_len(bytes, VOTE_SSZ_LEN)?;
        let mut c = 0;
        Ok(Self {
            slot: read_fixed::<Slot>(bytes, &mut c)?,
            head: read_fixed::<Checkpoint>(bytes, &mut c)?,
            target: read_fixed::<Checkpoint>(bytes, &mut c)?,
            source: read_fixed::<Checkpoint>(bytes, &mut c)?,
        })
    }
}

impl HashTreeRoot for Vote {
    fn hash_tree_root(&self) -> [u8; 32] {
        // Container with 4 fields → merkleize at width 4 (already a power
        // of two). Two levels of `hash_pair`.
        let chunks = [
            u64_chunk(self.slot.get()),
            self.head.hash_tree_root(),
            self.target.hash_tree_root(),
            self.source.hash_tree_root(),
        ];
        merkleize(&chunks)
    }
}

/// Signed validator vote — a [`Vote`] plus the validator id that produced it
/// and a post-quantum-signature placeholder.
///
/// Not [`Copy`]: [`Bytes4000`] is intentionally non-`Copy` to prevent silent
/// 4 KB stack copies. Pass by reference where ownership is not needed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignedVote {
    /// Index of the validator that produced [`Self::message`].
    pub validator_id: ValidatorIndex,
    /// The unsigned [`Vote`] being attested to.
    pub message: Vote,
    /// 4000-byte XMSS post-quantum-signature placeholder.
    pub signature: Bytes4000,
}

impl Encode for SignedVote {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        SIGNED_VOTE_SSZ_LEN
    }

    fn ssz_bytes_len(&self) -> usize {
        SIGNED_VOTE_SSZ_LEN
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        self.validator_id.ssz_append(buf);
        self.message.ssz_append(buf);
        buf.extend_from_slice(self.signature.as_slice());
    }
}

impl Decode for SignedVote {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        SIGNED_VOTE_SSZ_LEN
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        ensure_len(bytes, SIGNED_VOTE_SSZ_LEN)?;
        let mut c = 0;
        Ok(Self {
            validator_id: read_fixed::<ValidatorIndex>(bytes, &mut c)?,
            message: read_fixed::<Vote>(bytes, &mut c)?,
            signature: Bytes4000::new(read_byte_array::<BYTES4000_LEN>(bytes, &mut c)),
        })
    }
}

impl HashTreeRoot for SignedVote {
    fn hash_tree_root(&self) -> [u8; 32] {
        // Container with 3 fields → merkleize at width 4 (next power of
        // two). The fourth slot is `ZERO_HASH`. Equivalent to
        // `merkleize(&chunks)` but written explicitly so the layout is
        // visible.
        let validator = u64_chunk(self.validator_id.get());
        let message = self.message.hash_tree_root();
        let signature = self.signature.hash_tree_root();
        hash_pair(
            &hash_pair(&validator, &message),
            &hash_pair(&signature, &ZERO_HASH),
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use ssz::{decode, encode, SszError};
    use types::Bytes32;

    use crate::error::ProtocolError;

    fn sample_vote() -> Vote {
        Vote {
            slot: Slot::new(7),
            head: Checkpoint::new(Bytes32::new([0x11; 32]), Slot::new(7)),
            target: Checkpoint::new(Bytes32::new([0x22; 32]), Slot::new(4)),
            source: Checkpoint::new(Bytes32::new([0x33; 32]), Slot::new(0)),
        }
    }

    fn sample_signed_vote() -> SignedVote {
        SignedVote {
            validator_id: ValidatorIndex::new(42),
            message: sample_vote(),
            signature: Bytes4000::new([0xab; 4000]),
        }
    }

    // -- Vote: fixed-length / encode layout --------------------------------

    #[test]
    fn vote_ssz_fixed_len_is_one_twenty_eight() {
        assert_eq!(<Vote as Encode>::ssz_fixed_len(), 128);
        assert!(<Vote as Encode>::is_ssz_fixed_len());
    }

    #[test]
    fn vote_encode_layout_concatenates_fields_in_order() {
        let v = sample_vote();
        let bytes = encode(&v);
        assert_eq!(bytes.len(), VOTE_SSZ_LEN);
        let mut cursor = 0;
        assert_eq!(&bytes[cursor..cursor + 8], &7_u64.to_le_bytes());
        cursor += 8;
        assert_eq!(&bytes[cursor..cursor + 32], &[0x11_u8; 32]); // head.root
        assert_eq!(&bytes[cursor + 32..cursor + 40], &7_u64.to_le_bytes());
        cursor += 40;
        assert_eq!(&bytes[cursor..cursor + 32], &[0x22_u8; 32]); // target.root
        assert_eq!(&bytes[cursor + 32..cursor + 40], &4_u64.to_le_bytes());
        cursor += 40;
        assert_eq!(&bytes[cursor..cursor + 32], &[0x33_u8; 32]); // source.root
        assert_eq!(&bytes[cursor + 32..cursor + 40], &0_u64.to_le_bytes());
    }

    // -- Vote: round-trip --------------------------------------------------

    #[test]
    fn vote_ssz_round_trip_default() {
        let v = Vote::default();
        let back: Vote = decode(&encode(&v)).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn vote_ssz_round_trip_populated() {
        let v = sample_vote();
        let back: Vote = decode(&encode(&v)).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn vote_decode_rejects_short_input() {
        let err = decode::<Vote>(&[0_u8; VOTE_SSZ_LEN - 1]).unwrap_err();
        match err {
            SszError::Decode { source } => match source.0 {
                DecodeError::InvalidByteLength { len, expected } => {
                    assert_eq!(len, VOTE_SSZ_LEN - 1);
                    assert_eq!(expected, VOTE_SSZ_LEN);
                }
                other => panic!("unexpected upstream variant: {other:?}"),
            },
            other => panic!("unexpected SszError variant: {other:?}"),
        }
    }

    #[test]
    fn vote_decode_rejects_long_input() {
        let err = decode::<Vote>(&[0_u8; VOTE_SSZ_LEN + 1]).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    // -- Vote: HashTreeRoot ------------------------------------------------

    #[test]
    fn vote_hash_tree_root_is_merkleize_of_field_roots() {
        let v = sample_vote();
        let chunks = [
            u64_chunk(v.slot.get()),
            v.head.hash_tree_root(),
            v.target.hash_tree_root(),
            v.source.hash_tree_root(),
        ];
        assert_eq!(v.hash_tree_root(), merkleize(&chunks));
    }

    #[test]
    fn vote_hash_tree_root_distinguishes_field_swaps() {
        let mut v = sample_vote();
        // Swap target and source — different field positions must yield
        // different roots even though the byte content is the same set.
        let original = v.hash_tree_root();
        std::mem::swap(&mut v.target, &mut v.source);
        assert_ne!(original, v.hash_tree_root());
    }

    // -- SignedVote: fixed-length / encode layout --------------------------

    #[test]
    fn signed_vote_ssz_fixed_len_is_four_one_three_six() {
        assert_eq!(<SignedVote as Encode>::ssz_fixed_len(), 4136);
        assert!(<SignedVote as Encode>::is_ssz_fixed_len());
    }

    #[test]
    fn signed_vote_encode_layout_validator_message_signature() {
        let sv = sample_signed_vote();
        let bytes = encode(&sv);
        assert_eq!(bytes.len(), SIGNED_VOTE_SSZ_LEN);
        assert_eq!(&bytes[..8], &42_u64.to_le_bytes());
        assert_eq!(&bytes[8..8 + VOTE_SSZ_LEN], encode(&sv.message).as_slice());
        assert!(bytes[8 + VOTE_SSZ_LEN..].iter().all(|&b| b == 0xab));
    }

    // -- SignedVote: round-trip --------------------------------------------

    #[test]
    fn signed_vote_ssz_round_trip_default() {
        let sv = SignedVote::default();
        let back: SignedVote = decode(&encode(&sv)).unwrap();
        assert_eq!(back, sv);
    }

    #[test]
    fn signed_vote_ssz_round_trip_populated() {
        let sv = sample_signed_vote();
        let back: SignedVote = decode(&encode(&sv)).unwrap();
        assert_eq!(back, sv);
    }

    #[test]
    fn signed_vote_decode_rejects_short_input() {
        let err = decode::<SignedVote>(&[0_u8; SIGNED_VOTE_SSZ_LEN - 1]).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    #[test]
    fn signed_vote_decode_rejects_long_input() {
        let err = decode::<SignedVote>(&[0_u8; SIGNED_VOTE_SSZ_LEN + 1]).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    #[test]
    fn signed_vote_decode_propagates_protocol_error_via_question_mark() {
        let result: Result<SignedVote, ProtocolError> =
            decode::<SignedVote>(&[]).map_err(Into::into);
        assert!(matches!(result, Err(ProtocolError::Ssz(_))));
    }

    // -- SignedVote: HashTreeRoot -----------------------------------------

    #[test]
    fn signed_vote_hash_tree_root_is_merkleize_with_zero_pad() {
        let sv = sample_signed_vote();
        let chunks = [
            u64_chunk(sv.validator_id.get()),
            sv.message.hash_tree_root(),
            sv.signature.hash_tree_root(),
        ];
        // 3 fields → width 4 → expect zero-padded merkleization.
        assert_eq!(sv.hash_tree_root(), merkleize(&chunks));
    }

    #[test]
    fn signed_vote_hash_tree_root_responds_to_each_field() {
        let baseline = sample_signed_vote().hash_tree_root();

        let mut a = sample_signed_vote();
        a.validator_id = ValidatorIndex::new(43);
        assert_ne!(a.hash_tree_root(), baseline);

        let mut b = sample_signed_vote();
        b.message.slot = Slot::new(8);
        assert_ne!(b.hash_tree_root(), baseline);

        let mut c = sample_signed_vote();
        let mut sig = [0xab_u8; 4000];
        sig[0] = 0xac;
        c.signature = Bytes4000::new(sig);
        assert_ne!(c.hash_tree_root(), baseline);
    }

    // -- property tests ---------------------------------------------------

    proptest! {
        #[test]
        fn vote_ssz_round_trips(
            slot in any::<u64>(),
            head_root in proptest::array::uniform32(any::<u8>()),
            head_slot in any::<u64>(),
            target_root in proptest::array::uniform32(any::<u8>()),
            target_slot in any::<u64>(),
            source_root in proptest::array::uniform32(any::<u8>()),
            source_slot in any::<u64>(),
        ) {
            let v = Vote {
                slot: Slot::new(slot),
                head: Checkpoint::new(Bytes32::new(head_root), Slot::new(head_slot)),
                target: Checkpoint::new(Bytes32::new(target_root), Slot::new(target_slot)),
                source: Checkpoint::new(Bytes32::new(source_root), Slot::new(source_slot)),
            };
            let back: Vote = decode(&encode(&v)).unwrap();
            prop_assert_eq!(back, v);
        }

        #[test]
        fn signed_vote_ssz_round_trips(
            validator in any::<u64>(),
            slot in any::<u64>(),
            sig_byte in any::<u8>(),
        ) {
            let sv = SignedVote {
                validator_id: ValidatorIndex::new(validator),
                message: Vote { slot: Slot::new(slot), ..Default::default() },
                signature: Bytes4000::new([sig_byte; 4000]),
            };
            let back: SignedVote = decode(&encode(&sv)).unwrap();
            prop_assert_eq!(back, sv);
        }

        #[test]
        fn vote_hash_tree_root_is_deterministic(slot in any::<u64>()) {
            let v = Vote { slot: Slot::new(slot), ..Default::default() };
            prop_assert_eq!(v.hash_tree_root(), v.hash_tree_root());
        }
    }
}
