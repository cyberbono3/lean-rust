//! [`ValidatorIndex`] — `u64` newtype identifying a validator within the
//! registry — plus the round-robin proposer selection helper [`is_proposer`].

use crate::error::ProtocolError;
use crate::internal::impl_u64_ssz_newtype;
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
    }
}
