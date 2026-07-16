//! [`ValidatorIndex`] — `u64` newtype identifying a validator within the
//! registry — the [`Validator`] registry entry, the [`Validators`] registry
//! alias, and the round-robin proposer selection helper [`is_proposer`].

use ssz::merkleize::merkleize;
use ssz::{Decode, DecodeError, Encode, HashTreeRoot};
use types::PublicKey;

use crate::error::ProtocolError;
use crate::internal::{
    ensure_len, impl_u64_ssz_newtype, read_byte_array, PUBLIC_KEY_LEN, VALIDATOR_INDEX_LEN,
    VALIDATOR_SSZ_LEN,
};
use crate::slot::Slot;

/// Validator-registry index (`u64` newtype).
///
/// # Example
/// ```
/// use protocol::ValidatorIndex;
/// let v = ValidatorIndex::new(42);
/// assert_eq!(v.get(), 42);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValidatorIndex(u64);

impl_u64_ssz_newtype!(ValidatorIndex);

impl ValidatorIndex {
    /// Constructs a [`ValidatorIndex`] from a raw `u64`.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the underlying `u64`.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Reports whether `validator_index` is the round-robin proposer for `slot`
/// within a validator set of size `num_validators`.
///
/// The proposer for slot `s` is `s mod num_validators`.
///
/// # Errors
/// Returns [`ProtocolError::Invariant`] when `num_validators` is zero — the
/// modulo is undefined.
///
/// # Example
/// ```
/// use protocol::{is_proposer, Slot, ValidatorIndex};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// assert!(is_proposer(ValidatorIndex::new(2), Slot::new(2), 4)?);
/// assert!(!is_proposer(ValidatorIndex::new(1), Slot::new(2), 4)?);
/// # Ok(())
/// # }
/// ```
pub fn is_proposer(
    validator_index: ValidatorIndex,
    slot: Slot,
    num_validators: u64,
) -> Result<bool, ProtocolError> {
    if num_validators == 0 {
        return Err(ProtocolError::Invariant {
            context: "is_proposer",
            reason: "num_validators must be greater than zero",
        });
    }
    Ok(slot.get() % num_validators == validator_index.get())
}

/// Per-validator registry entry: post-quantum one-time-signature public key
/// plus the validator's registry index.
///
/// Fixed-size SSZ container — `pubkey` (52 bytes) then `index` (8 bytes LE), in
/// that order. The field order is committed by the hash-tree-root, so it must
/// match the consensus spec (`pubkey` before `index`). No serde derives —
/// domain purity.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Validator {
    /// XMSS one-time-signature public key (`Bytes52`).
    pub pubkey: PublicKey,
    /// Index of this validator within the registry.
    pub index: ValidatorIndex,
}

/// Bounded validator registry (`List[Validator, VALIDATOR_REGISTRY_LIMIT]`).
///
/// A naming alias, not a bounded newtype: the cap is enforced at the SSZ codec
/// and hash-tree-root sites via `VALIDATOR_REGISTRY_LIMIT`. Exists so downstream
/// code can name `&Validators`.
pub type Validators = Vec<Validator>;

impl Encode for Validator {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        VALIDATOR_SSZ_LEN
    }

    fn ssz_bytes_len(&self) -> usize {
        VALIDATOR_SSZ_LEN
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        // pubkey FIRST (field order is committed by the root), then the LE index.
        buf.extend_from_slice(self.pubkey.as_slice());
        self.index.ssz_append(buf);
    }
}

impl Decode for Validator {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        VALIDATOR_SSZ_LEN
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        ensure_len(bytes, VALIDATOR_SSZ_LEN)?;
        // Length verified above; both reads are in-bounds.
        let mut cursor = 0_usize;
        let pubkey = PublicKey::new(read_byte_array::<{ PUBLIC_KEY_LEN }>(bytes, &mut cursor));
        let index = ValidatorIndex::from_ssz_bytes(&bytes[cursor..cursor + VALIDATOR_INDEX_LEN])?;
        Ok(Self { pubkey, index })
    }
}

impl HashTreeRoot for Validator {
    fn hash_tree_root(&self) -> [u8; 32] {
        // Container with exactly 2 fields → merkleize at width 2.
        merkleize(&[self.pubkey.hash_tree_root(), self.index.hash_tree_root()])
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use ssz::{decode, encode, HashTreeRoot};

    // -- Construction + accessors -------------------------------------------

    #[test]
    fn new_round_trips_to_get() {
        assert_eq!(ValidatorIndex::new(42).get(), 42);
    }

    #[test]
    fn default_is_zero() {
        assert_eq!(ValidatorIndex::default().get(), 0);
    }

    #[test]
    fn from_into_u64() {
        let v: ValidatorIndex = 99_u64.into();
        let raw: u64 = v.into();
        assert_eq!(raw, 99);
    }

    #[test]
    fn display_is_decimal() {
        assert_eq!(format!("{}", ValidatorIndex::new(7)), "7");
    }

    // -- SSZ encode/decode round-trip ---------------------------------------

    #[test]
    fn ssz_round_trip_boundary_values() {
        for original in [ValidatorIndex::new(0), ValidatorIndex::new(u64::MAX)] {
            let bytes = encode(&original);
            assert_eq!(bytes.len(), 8);
            let back: ValidatorIndex = decode(&bytes).unwrap();
            assert_eq!(back, original);
        }
    }

    // -- HashTreeRoot --------------------------------------------------------

    #[test]
    fn hash_tree_root_is_le_chunk() {
        let root = ValidatorIndex::new(0xdead_beef).hash_tree_root();
        assert_eq!(&root[..8], &0xdead_beef_u64.to_le_bytes());
        assert!(root[8..].iter().all(|&b| b == 0));
    }

    // -- Validator container -------------------------------------------------

    #[test]
    fn validator_field_order_is_pubkey_then_index() {
        assert_eq!(VALIDATOR_SSZ_LEN, 60);
        let mut pubkey_bytes = [0_u8; PUBLIC_KEY_LEN];
        for (i, b) in pubkey_bytes.iter_mut().enumerate() {
            *b = u8::try_from(i).unwrap();
        }
        let v = Validator {
            pubkey: PublicKey::new(pubkey_bytes),
            index: ValidatorIndex::new(0x1122_3344_5566_7788),
        };
        let bytes = encode(&v);
        assert_eq!(bytes.len(), VALIDATOR_SSZ_LEN);
        // pubkey FIRST, then the 8-byte LE index.
        assert_eq!(&bytes[..PUBLIC_KEY_LEN], &pubkey_bytes);
        assert_eq!(
            &bytes[PUBLIC_KEY_LEN..],
            &0x1122_3344_5566_7788_u64.to_le_bytes()
        );
    }

    #[test]
    fn validator_ssz_round_trip_boundary_values() {
        for (pk, idx) in [
            ([0x00_u8; PUBLIC_KEY_LEN], ValidatorIndex::new(0)),
            ([0xff_u8; PUBLIC_KEY_LEN], ValidatorIndex::new(u64::MAX)),
        ] {
            let v = Validator {
                pubkey: PublicKey::new(pk),
                index: idx,
            };
            let bytes = encode(&v);
            assert_eq!(bytes.len(), 60);
            let back: Validator = decode(&bytes).unwrap();
            assert_eq!(back, v);
        }
    }

    #[test]
    fn validator_decode_rejects_wrong_length() {
        assert!(decode::<Validator>(&[0_u8; VALIDATOR_SSZ_LEN - 1]).is_err());
        assert!(decode::<Validator>(&[0_u8; VALIDATOR_SSZ_LEN + 1]).is_err());
    }

    #[test]
    fn validator_default_index_is_zero() {
        let v = Validator::default();
        assert_eq!(v.index, ValidatorIndex::default());
        assert_eq!(v.pubkey, PublicKey::new([0_u8; PUBLIC_KEY_LEN]));
    }

    #[test]
    fn validator_htr_two_leaf_shape_and_field_sensitivity() {
        let base = Validator {
            pubkey: PublicKey::new([0x11; PUBLIC_KEY_LEN]),
            index: ValidatorIndex::new(7),
        };
        assert_eq!(
            base.hash_tree_root(),
            merkleize(&[base.pubkey.hash_tree_root(), base.index.hash_tree_root()])
        );

        let mut pubkey_changed = base.clone();
        pubkey_changed.pubkey = PublicKey::new([0x22; PUBLIC_KEY_LEN]);
        assert_ne!(pubkey_changed.hash_tree_root(), base.hash_tree_root());

        let mut index_changed = base.clone();
        index_changed.index = ValidatorIndex::new(8);
        assert_ne!(index_changed.hash_tree_root(), base.hash_tree_root());
    }

    // -- is_proposer ---------------------------------------------------------

    #[test]
    fn is_proposer_matrix_round_robin() {
        let counts = [1_u64, 2, 5, 10, 100, 1000];
        for count in counts {
            let limit = (count * 2).min(20);
            for slot_n in 0..limit {
                let slot = Slot::new(slot_n);
                let expected = ValidatorIndex::new(slot_n % count);
                assert!(
                    is_proposer(expected, slot, count).unwrap(),
                    "{expected} should be proposer for slot {slot} with {count} validators"
                );
                if count > 1 {
                    let other = ValidatorIndex::new((slot_n + 1) % count);
                    assert!(
                        !is_proposer(other, slot, count).unwrap(),
                        "{other} should NOT be proposer for slot {slot} with {count} validators"
                    );
                }
            }
        }
    }

    #[test]
    fn is_proposer_zero_validators_returns_invariant_error() {
        let err = is_proposer(ValidatorIndex::new(0), Slot::new(0), 0).unwrap_err();
        assert!(matches!(
            err,
            ProtocolError::Invariant {
                context: "is_proposer",
                ..
            }
        ));
    }

    // -- property tests ------------------------------------------------------

    proptest! {
        #[test]
        fn ssz_round_trips(value in any::<u64>()) {
            let v = ValidatorIndex::new(value);
            let back: ValidatorIndex = decode(&encode(&v)).unwrap();
            prop_assert_eq!(back, v);
        }

        #[test]
        fn is_proposer_unique_winner(slot_n in 0_u64..1_000, count in 1_u64..=64) {
            let slot = Slot::new(slot_n);
            let mut winners = 0_u64;
            for i in 0..count {
                if is_proposer(ValidatorIndex::new(i), slot, count).unwrap() {
                    winners += 1;
                }
            }
            prop_assert_eq!(winners, 1);
        }

        #[test]
        fn validator_ssz_round_trips(
            pubkey_bytes in proptest::collection::vec(any::<u8>(), PUBLIC_KEY_LEN),
            index in any::<u64>(),
        ) {
            let mut pk = [0_u8; PUBLIC_KEY_LEN];
            pk.copy_from_slice(&pubkey_bytes);
            let v = Validator {
                pubkey: PublicKey::new(pk),
                index: ValidatorIndex::new(index),
            };
            let back: Validator = decode(&encode(&v)).unwrap();
            prop_assert_eq!(back, v);
        }
    }
}
