//! [`Checkpoint`] — `(root, slot)` pair identifying one block at one slot.
//!
//! SSZ-encoded as a fixed-size container: 32-byte root followed by an
//! 8-byte little-endian slot. Merkleized as `hash_pair(root, slot_chunk)`
//! since the container has exactly two fields.

use ssz::merkleize::hash_pair;
use ssz::{Decode, DecodeError, Encode, HashTreeRoot};
use types::Bytes32;

use crate::internal::{u64_chunk, BYTES32_LEN, CHECKPOINT_LEN};
use crate::slot::Slot;

const SSZ_LEN: usize = CHECKPOINT_LEN;

/// Identifies one chain root at one slot.
///
/// # Example
/// ```
/// use protocol::{Checkpoint, Slot};
/// use types::Bytes32;
/// let cp = Checkpoint::new(Bytes32::zero(), Slot::new(7));
/// assert_eq!(cp.slot.get(), 7);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Checkpoint {
    /// 32-byte block root identifying the checkpointed block.
    pub root: Bytes32,
    /// Slot at which the checkpointed block was produced.
    pub slot: Slot,
}

impl Checkpoint {
    /// Constructs a [`Checkpoint`] from an explicit `(root, slot)` pair.
    #[must_use]
    pub const fn new(root: Bytes32, slot: Slot) -> Self {
        Self { root, slot }
    }
}

impl Encode for Checkpoint {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        SSZ_LEN
    }

    fn ssz_bytes_len(&self) -> usize {
        SSZ_LEN
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(self.root.as_slice());
        self.slot.ssz_append(buf);
    }
}

impl Decode for Checkpoint {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        SSZ_LEN
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != SSZ_LEN {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: SSZ_LEN,
            });
        }
        // Length verified above; both slice ranges are in-bounds.
        let mut root_arr = [0_u8; BYTES32_LEN];
        root_arr.copy_from_slice(&bytes[..BYTES32_LEN]);
        let slot = Slot::from_ssz_bytes(&bytes[BYTES32_LEN..])?;
        Ok(Self {
            root: Bytes32::new(root_arr),
            slot,
        })
    }
}

impl HashTreeRoot for Checkpoint {
    fn hash_tree_root(&self) -> [u8; 32] {
        // Container with exactly 2 fields → merkleize at width 2 collapses
        // to a single hash_pair of the two field roots. The root field is
        // already a 32-byte chunk; the slot is encoded as a basic-type chunk.
        hash_pair(&self.root.0, &u64_chunk(self.slot.get()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use ssz::{decode, encode, SszError};

    use crate::error::ProtocolError;

    // -- Construction + accessors -------------------------------------------

    #[test]
    fn new_constructs_with_fields() {
        let cp = Checkpoint::new(Bytes32::new([0xab; 32]), Slot::new(5));
        assert_eq!(cp.root.as_slice(), &[0xab; 32]);
        assert_eq!(cp.slot.get(), 5);
    }

    #[test]
    fn default_is_zeros() {
        let cp = Checkpoint::default();
        assert_eq!(cp.root, Bytes32::zero());
        assert_eq!(cp.slot, Slot::new(0));
    }

    // -- SSZ encode/decode --------------------------------------------------

    #[test]
    fn ssz_encode_layout_root_then_slot_le() {
        let mut root_arr = [0_u8; 32];
        for (i, b) in root_arr.iter_mut().enumerate() {
            *b = u8::try_from(i).unwrap();
        }
        let cp = Checkpoint::new(Bytes32::new(root_arr), Slot::new(0x1122_3344_5566_7788));
        let bytes = encode(&cp);
        assert_eq!(bytes.len(), SSZ_LEN);
        assert_eq!(&bytes[..BYTES32_LEN], &root_arr);
        assert_eq!(
            &bytes[BYTES32_LEN..],
            &0x1122_3344_5566_7788_u64.to_le_bytes()
        );
    }

    #[test]
    fn ssz_round_trip_zero_and_max() {
        for cp in [
            Checkpoint::default(),
            Checkpoint::new(Bytes32::new([0x55; 32]), Slot::new(u64::MAX)),
        ] {
            let back: Checkpoint = decode(&encode(&cp)).unwrap();
            assert_eq!(back, cp);
        }
    }

    #[test]
    fn ssz_decode_rejects_short_input() {
        let err = decode::<Checkpoint>(&[0_u8; 39]).unwrap_err();
        match err {
            SszError::Decode { source } => match source.0 {
                DecodeError::InvalidByteLength { len, expected } => {
                    assert_eq!(len, 39);
                    assert_eq!(expected, SSZ_LEN);
                }
                other => panic!("unexpected upstream variant: {other:?}"),
            },
            other => panic!("unexpected SszError variant: {other:?}"),
        }
    }

    #[test]
    fn ssz_decode_rejects_long_input() {
        let err = decode::<Checkpoint>(&[0_u8; 41]).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    #[test]
    fn ssz_error_converts_to_protocol_error() {
        // `?` flows SSZ failures through ProtocolError::Ssz via #[from].
        let result: Result<Checkpoint, ProtocolError> =
            decode::<Checkpoint>(&[0_u8; 0]).map_err(Into::into);
        assert!(matches!(result, Err(ProtocolError::Ssz(_))));
    }

    // -- HashTreeRoot --------------------------------------------------------

    #[test]
    fn hash_tree_root_is_pair_of_field_roots() {
        let root_arr = [0xaa_u8; 32];
        let cp = Checkpoint::new(Bytes32::new(root_arr), Slot::new(0xcafe_babe));
        let mut slot_chunk = [0_u8; 32];
        slot_chunk[..8].copy_from_slice(&0xcafe_babe_u64.to_le_bytes());
        assert_eq!(cp.hash_tree_root(), hash_pair(&root_arr, &slot_chunk));
    }

    #[test]
    fn hash_tree_root_zero_checkpoint_is_pair_of_zero_chunks() {
        assert_eq!(
            Checkpoint::default().hash_tree_root(),
            hash_pair(&[0_u8; 32], &[0_u8; 32])
        );
    }

    #[test]
    fn hash_tree_root_distinguishes_field_swaps() {
        // Swapping bytes between root and slot must NOT yield the same root.
        let mut root_arr = [0_u8; 32];
        root_arr[..8].copy_from_slice(&7_u64.to_le_bytes());
        let cp_a = Checkpoint::new(Bytes32::new(root_arr), Slot::new(0));
        let cp_b = Checkpoint::new(Bytes32::zero(), Slot::new(7));
        assert_ne!(cp_a.hash_tree_root(), cp_b.hash_tree_root());
    }

    // -- property tests ------------------------------------------------------

    proptest! {
        #[test]
        fn ssz_round_trips(
            root in proptest::array::uniform32(any::<u8>()),
            slot in any::<u64>(),
        ) {
            let cp = Checkpoint::new(Bytes32::new(root), Slot::new(slot));
            let back: Checkpoint = decode(&encode(&cp)).unwrap();
            prop_assert_eq!(back, cp);
        }

        #[test]
        fn hash_tree_root_is_deterministic(
            root in proptest::array::uniform32(any::<u8>()),
            slot in any::<u64>(),
        ) {
            let cp = Checkpoint::new(Bytes32::new(root), Slot::new(slot));
            prop_assert_eq!(cp.hash_tree_root(), cp.hash_tree_root());
        }
    }
}
