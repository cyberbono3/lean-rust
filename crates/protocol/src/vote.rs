//! [`AttestationData`] — the unsigned attestation body — plus [`Attestation`],
//! which binds it to the attesting validator, and [`SignedAttestation`], the
//! wire-shape container that pairs an attestation with its post-quantum
//! signature.
//!
//! Wire layout follows the canonical consensus-spec containers:
//!
//! - `AttestationData { slot, head, target, source }` — four fixed-size fields.
//! - `Attestation { validator_id, data }` — `validator_id` lives on the
//!   attestation itself, **not** on the outer signed envelope.
//! - `SignedAttestation { message, signature }` — the envelope carries only the
//!   attestation and its signature.
//!
//! All fields are SSZ-fixed-length, so every container serializes to a fixed
//! byte count: 128 bytes for [`AttestationData`], 136 for [`Attestation`], and
//! 3252 for [`SignedAttestation`].

// `ZERO_HASH` is deliberately absent: it existed only to zero-pad the retired
// 3-field envelope's width-4 tree. Both containers below have 2 fields, so
// their merkleization is a single `hash_pair` with no padding. `hash_pair`
// itself is imported by the test module only — the impls go through
// `merkleize`, and the tests pin the expected trees explicitly.
use ssz::merkleize::merkleize;
use ssz::{Decode, DecodeError, Encode, HashTreeRoot};
use types::Signature;

use crate::checkpoint::Checkpoint;
use crate::internal::{
    ensure_len, read_byte_array, read_fixed, u64_chunk, CHECKPOINT_LEN, SIGNATURE_LEN, SLOT_LEN,
    VALIDATOR_INDEX_LEN,
};
use crate::slot::Slot;
use crate::validator::ValidatorIndex;

const ATTESTATION_DATA_SSZ_LEN: usize = SLOT_LEN + 3 * CHECKPOINT_LEN; // 128
const ATTESTATION_SSZ_LEN: usize = VALIDATOR_INDEX_LEN + ATTESTATION_DATA_SSZ_LEN; // 136
const SIGNED_ATTESTATION_SSZ_LEN: usize = ATTESTATION_SSZ_LEN + SIGNATURE_LEN; // 3252

/// Unsigned attestation body.
///
/// # Example
/// ```
/// use protocol::{AttestationData, Checkpoint, Slot};
/// use types::Bytes32;
/// let d = AttestationData {
///     slot: Slot::new(1),
///     head: Checkpoint::new(Bytes32::zero(), Slot::new(1)),
///     target: Checkpoint::new(Bytes32::zero(), Slot::new(0)),
///     source: Checkpoint::new(Bytes32::zero(), Slot::new(0)),
/// };
/// assert_eq!(d.slot.get(), 1);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AttestationData {
    /// Slot in which the attestation was cast.
    pub slot: Slot,
    /// Block the attester considers the canonical head at `slot`.
    pub head: Checkpoint,
    /// Justification target the attestation attests to.
    pub target: Checkpoint,
    /// Justification source the attestation builds on.
    pub source: Checkpoint,
}

impl Encode for AttestationData {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        ATTESTATION_DATA_SSZ_LEN
    }

    fn ssz_bytes_len(&self) -> usize {
        ATTESTATION_DATA_SSZ_LEN
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        self.slot.ssz_append(buf);
        self.head.ssz_append(buf);
        self.target.ssz_append(buf);
        self.source.ssz_append(buf);
    }
}

impl Decode for AttestationData {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        ATTESTATION_DATA_SSZ_LEN
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        ensure_len(bytes, ATTESTATION_DATA_SSZ_LEN)?;
        let mut c = 0;
        Ok(Self {
            slot: read_fixed::<Slot>(bytes, &mut c)?,
            head: read_fixed::<Checkpoint>(bytes, &mut c)?,
            target: read_fixed::<Checkpoint>(bytes, &mut c)?,
            source: read_fixed::<Checkpoint>(bytes, &mut c)?,
        })
    }
}

impl HashTreeRoot for AttestationData {
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

/// An [`AttestationData`] bound to the validator that produced it.
///
/// `validator_id` precedes `data` on the wire; the order is
/// hash-tree-root-bearing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Attestation {
    /// Index of the validator that produced [`Self::data`].
    pub validator_id: ValidatorIndex,
    /// The unsigned attestation body.
    pub data: AttestationData,
}

impl Encode for Attestation {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        ATTESTATION_SSZ_LEN
    }

    fn ssz_bytes_len(&self) -> usize {
        ATTESTATION_SSZ_LEN
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        self.validator_id.ssz_append(buf);
        self.data.ssz_append(buf);
    }
}

impl Decode for Attestation {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        ATTESTATION_SSZ_LEN
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        ensure_len(bytes, ATTESTATION_SSZ_LEN)?;
        let mut c = 0;
        Ok(Self {
            validator_id: read_fixed::<ValidatorIndex>(bytes, &mut c)?,
            data: read_fixed::<AttestationData>(bytes, &mut c)?,
        })
    }
}

impl HashTreeRoot for Attestation {
    fn hash_tree_root(&self) -> [u8; 32] {
        // Container with 2 fields → width 2, already a power of two, so
        // `merkleize` reduces to a single `hash_pair` with no zero padding.
        let chunks = [
            u64_chunk(self.validator_id.get()),
            self.data.hash_tree_root(),
        ];
        merkleize(&chunks)
    }
}

/// An [`Attestation`] plus the post-quantum signature over it.
///
/// Not [`Copy`]: [`Signature`] is intentionally non-`Copy` to prevent silent
/// multi-KB stack copies. Pass by reference where ownership is not needed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignedAttestation {
    /// The [`Attestation`] being signed — carries its own `validator_id`.
    pub message: Attestation,
    /// Post-quantum signature container over [`Self::message`].
    pub signature: Signature,
}

impl Encode for SignedAttestation {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        SIGNED_ATTESTATION_SSZ_LEN
    }

    fn ssz_bytes_len(&self) -> usize {
        SIGNED_ATTESTATION_SSZ_LEN
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        self.message.ssz_append(buf);
        // Borrow rather than clone — `Signature` is a multi-KB container.
        buf.extend_from_slice(self.signature.as_slice());
    }
}

impl Decode for SignedAttestation {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        SIGNED_ATTESTATION_SSZ_LEN
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        // Length guard first: `read_byte_array` does not bounds-check and
        // panics on a short slice, so this is what keeps a truncated peer
        // message a `DecodeError` rather than a panic.
        ensure_len(bytes, SIGNED_ATTESTATION_SSZ_LEN)?;
        let mut c = 0;
        Ok(Self {
            message: read_fixed::<Attestation>(bytes, &mut c)?,
            signature: Signature::new(read_byte_array::<SIGNATURE_LEN>(bytes, &mut c)),
        })
    }
}

impl HashTreeRoot for SignedAttestation {
    fn hash_tree_root(&self) -> [u8; 32] {
        // Container with 2 fields → width 2: a single `hash_pair`, no zero
        // pad. The envelope this replaced had 3 fields and padded to width 4 —
        // the tree shape changed, not just the field values.
        let chunks = [
            self.message.hash_tree_root(),
            self.signature.hash_tree_root(),
        ];
        merkleize(&chunks)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use ssz::merkleize::hash_pair;
    use ssz::{decode, encode, SszError};
    use types::Bytes32;

    use crate::error::ProtocolError;
    use crate::test_fixtures::{assert_ssz_round_trip, sample_signature};

    fn sample_attestation_data() -> AttestationData {
        AttestationData {
            slot: Slot::new(7),
            head: Checkpoint::new(Bytes32::new([0x11; 32]), Slot::new(7)),
            target: Checkpoint::new(Bytes32::new([0x22; 32]), Slot::new(4)),
            source: Checkpoint::new(Bytes32::new([0x33; 32]), Slot::new(0)),
        }
    }

    fn sample_attestation() -> Attestation {
        Attestation {
            validator_id: ValidatorIndex::new(42),
            data: sample_attestation_data(),
        }
    }

    // The single in-module signed sample. The crate-level
    // `sample_signed_attestation(seed)` serves the seeded cases; this one pins
    // the fixed 0xab sample the layout assertions below read byte-for-byte.
    fn sample_signed_attestation_fixed() -> SignedAttestation {
        SignedAttestation {
            message: sample_attestation(),
            signature: sample_signature(0xab),
        }
    }

    // -- AttestationData: fixed-length / encode layout ----------------------

    #[test]
    fn attestation_data_ssz_fixed_len_is_128() {
        assert_eq!(<AttestationData as Encode>::ssz_fixed_len(), 128);
        assert!(<AttestationData as Encode>::is_ssz_fixed_len());
    }

    #[test]
    fn attestation_data_encode_layout_slot_head_target_source() {
        let d = sample_attestation_data();
        let bytes = encode(&d);
        assert_eq!(bytes.len(), ATTESTATION_DATA_SSZ_LEN);
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

    // -- AttestationData: round-trip ---------------------------------------

    #[test]
    fn attestation_data_ssz_round_trip_default_and_populated() {
        assert_ssz_round_trip(&AttestationData::default());
        assert_ssz_round_trip(&sample_attestation_data());
    }

    #[test]
    fn attestation_data_decode_rejects_short_input() {
        let err = decode::<AttestationData>(&[0_u8; ATTESTATION_DATA_SSZ_LEN - 1]).unwrap_err();
        match err {
            SszError::Decode { source } => match source.0 {
                DecodeError::InvalidByteLength { len, expected } => {
                    assert_eq!(len, ATTESTATION_DATA_SSZ_LEN - 1);
                    assert_eq!(expected, ATTESTATION_DATA_SSZ_LEN);
                }
                other => panic!("unexpected upstream variant: {other:?}"),
            },
            other => panic!("unexpected SszError variant: {other:?}"),
        }
    }

    #[test]
    fn attestation_data_decode_rejects_long_input() {
        let err = decode::<AttestationData>(&[0_u8; ATTESTATION_DATA_SSZ_LEN + 1]).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    // -- AttestationData: HashTreeRoot -------------------------------------

    #[test]
    fn attestation_data_hash_tree_root_is_hash_pair_tree_of_field_roots() {
        let d = sample_attestation_data();
        // 4 chunks → width 4 → two levels of `hash_pair`. Written out rather
        // than re-calling `merkleize`, which would merely restate the impl.
        let expected = hash_pair(
            &hash_pair(&u64_chunk(d.slot.get()), &d.head.hash_tree_root()),
            &hash_pair(&d.target.hash_tree_root(), &d.source.hash_tree_root()),
        );
        assert_eq!(d.hash_tree_root(), expected);
    }

    #[test]
    fn attestation_data_hash_tree_root_distinguishes_field_swaps() {
        let mut d = sample_attestation_data();
        // Swap target and source — different field positions must yield
        // different roots even though the byte content is the same set.
        let original = d.hash_tree_root();
        std::mem::swap(&mut d.target, &mut d.source);
        assert_ne!(original, d.hash_tree_root());
    }

    // -- Attestation: fixed-length / encode layout -------------------------

    #[test]
    fn attestation_ssz_fixed_len_is_136() {
        assert_eq!(<Attestation as Encode>::ssz_fixed_len(), 136);
        assert!(<Attestation as Encode>::is_ssz_fixed_len());
    }

    #[test]
    fn attestation_encode_layout_validator_then_data() {
        let a = sample_attestation();
        let bytes = encode(&a);
        assert_eq!(bytes.len(), ATTESTATION_SSZ_LEN);
        // validator_id first — spec order, and hash-tree-root-bearing.
        assert_eq!(&bytes[..8], &42_u64.to_le_bytes());
        assert_eq!(&bytes[8..], encode(&a.data).as_slice());
    }

    // -- Attestation: round-trip -------------------------------------------

    #[test]
    fn attestation_ssz_round_trip_default_and_populated() {
        assert_ssz_round_trip(&Attestation::default());
        assert_ssz_round_trip(&sample_attestation());
    }

    #[test]
    fn attestation_decode_rejects_short_and_long_input() {
        let short = decode::<Attestation>(&[0_u8; ATTESTATION_SSZ_LEN - 1]).unwrap_err();
        assert!(matches!(short, SszError::Decode { .. }));
        let long = decode::<Attestation>(&[0_u8; ATTESTATION_SSZ_LEN + 1]).unwrap_err();
        assert!(matches!(long, SszError::Decode { .. }));
    }

    // -- Attestation: HashTreeRoot -----------------------------------------

    #[test]
    fn attestation_hash_tree_root_is_hash_pair() {
        let a = sample_attestation();
        // 2 fields → width 2 → one `hash_pair`, no zero pad.
        let expected = hash_pair(&u64_chunk(a.validator_id.get()), &a.data.hash_tree_root());
        assert_eq!(a.hash_tree_root(), expected);
    }

    #[test]
    fn attestation_htr_responds_to_validator_id_and_data() {
        let baseline = sample_attestation().hash_tree_root();

        let mut a = sample_attestation();
        a.validator_id = ValidatorIndex::new(43);
        assert_ne!(a.hash_tree_root(), baseline);

        let mut b = sample_attestation();
        b.data.slot = Slot::new(8);
        assert_ne!(b.hash_tree_root(), baseline);
    }

    // -- SignedAttestation: fixed-length / encode layout --------------------

    #[test]
    fn signed_attestation_ssz_fixed_len_is_3252() {
        assert_eq!(<SignedAttestation as Encode>::ssz_fixed_len(), 3252);
        assert!(<SignedAttestation as Encode>::is_ssz_fixed_len());
    }

    #[test]
    fn signed_attestation_encode_layout_message_then_signature() {
        let sa = sample_signed_attestation_fixed();
        let bytes = encode(&sa);
        assert_eq!(bytes.len(), SIGNED_ATTESTATION_SSZ_LEN);
        // message first — the envelope no longer leads with validator_id,
        // which now lives inside message.
        assert_eq!(
            &bytes[..ATTESTATION_SSZ_LEN],
            encode(&sa.message).as_slice()
        );
        assert!(bytes[ATTESTATION_SSZ_LEN..].iter().all(|&b| b == 0xab));
    }

    // -- SignedAttestation: round-trip -------------------------------------

    #[test]
    fn signed_attestation_ssz_round_trip_default_and_populated() {
        assert_ssz_round_trip(&SignedAttestation::default());
        assert_ssz_round_trip(&sample_signed_attestation_fixed());
    }

    #[test]
    fn signed_attestation_decode_rejects_short_input() {
        let err = decode::<SignedAttestation>(&[0_u8; SIGNED_ATTESTATION_SSZ_LEN - 1]).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    #[test]
    fn signed_attestation_decode_rejects_long_input() {
        let err = decode::<SignedAttestation>(&[0_u8; SIGNED_ATTESTATION_SSZ_LEN + 1]).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    #[test]
    fn signed_attestation_decode_propagates_protocol_error_via_question_mark() {
        let result: Result<SignedAttestation, ProtocolError> =
            decode::<SignedAttestation>(&[]).map_err(Into::into);
        assert!(matches!(result, Err(ProtocolError::Ssz(_))));
    }

    // -- SignedAttestation: HashTreeRoot ------------------------------------

    #[test]
    fn signed_attestation_hash_tree_root_is_hash_pair() {
        let sa = sample_signed_attestation_fixed();
        // 2 fields → width 2 → one `hash_pair`, no zero pad. The container
        // this replaced had 3 fields and padded to width 4; copying that shape
        // here would produce a wrong root that every length check still passes.
        let expected = hash_pair(&sa.message.hash_tree_root(), &sa.signature.hash_tree_root());
        assert_eq!(sa.hash_tree_root(), expected);
    }

    #[test]
    fn signed_attestation_htr_responds_to_each_field() {
        let baseline = sample_signed_attestation_fixed().hash_tree_root();

        let mut a = sample_signed_attestation_fixed();
        a.message.validator_id = ValidatorIndex::new(43);
        assert_ne!(a.hash_tree_root(), baseline);

        let mut b = sample_signed_attestation_fixed();
        b.message.data.slot = Slot::new(8);
        assert_ne!(b.hash_tree_root(), baseline);

        let mut c = sample_signed_attestation_fixed();
        let mut sig = [0xab_u8; Signature::LEN];
        sig[0] = 0xac;
        c.signature = Signature::new(sig);
        assert_ne!(c.hash_tree_root(), baseline);
    }

    // -- property tests -----------------------------------------------------

    proptest! {
        #[test]
        fn attestation_data_ssz_round_trips(
            slot in any::<u64>(),
            head_root in proptest::array::uniform32(any::<u8>()),
            head_slot in any::<u64>(),
            target_root in proptest::array::uniform32(any::<u8>()),
            target_slot in any::<u64>(),
            source_root in proptest::array::uniform32(any::<u8>()),
            source_slot in any::<u64>(),
        ) {
            let d = AttestationData {
                slot: Slot::new(slot),
                head: Checkpoint::new(Bytes32::new(head_root), Slot::new(head_slot)),
                target: Checkpoint::new(Bytes32::new(target_root), Slot::new(target_slot)),
                source: Checkpoint::new(Bytes32::new(source_root), Slot::new(source_slot)),
            };
            let back: AttestationData = decode(&encode(&d)).unwrap();
            prop_assert_eq!(back, d);
        }

        #[test]
        fn attestation_ssz_round_trips(validator in any::<u64>(), slot in any::<u64>()) {
            let a = Attestation {
                validator_id: ValidatorIndex::new(validator),
                data: AttestationData { slot: Slot::new(slot), ..Default::default() },
            };
            let back: Attestation = decode(&encode(&a)).unwrap();
            prop_assert_eq!(back, a);
        }

        #[test]
        fn signed_attestation_ssz_round_trips(
            validator in any::<u64>(),
            slot in any::<u64>(),
            sig_byte in any::<u8>(),
        ) {
            let sa = SignedAttestation {
                message: Attestation {
                    validator_id: ValidatorIndex::new(validator),
                    data: AttestationData { slot: Slot::new(slot), ..Default::default() },
                },
                signature: sample_signature(sig_byte),
            };
            let back: SignedAttestation = decode(&encode(&sa)).unwrap();
            prop_assert_eq!(back, sa);
        }

        #[test]
        fn attestation_data_hash_tree_root_is_deterministic(slot in any::<u64>()) {
            let d = AttestationData { slot: Slot::new(slot), ..Default::default() };
            prop_assert_eq!(d.hash_tree_root(), d.hash_tree_root());
        }
    }
}
