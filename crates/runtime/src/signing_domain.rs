//! The leanSig signing domain for attestations: the `(epoch, message)` pair
//! that BOTH the sign side ([`crate::duties::signer`]) and the verify side
//! ([`crate::chain::engine`]) derive from an [`Attestation`].
//!
//! One home, one derivation. A signature is only valid when the verifier feeds
//! leanSig byte-identical inputs to the ones the signer used, so deriving the
//! epoch or the message twice is a correctness hazard, not just duplication:
//! a drift on either side silently breaks every cross-client signature under
//! the pinned production scheme.

use protocol::Attestation;
use ssz::HashTreeRoot;

/// The attestation slot exceeds the leanSig epoch domain (`u32`, LIFETIME =
/// `2^32`). Rejected, never truncated — a truncated epoch would sign or verify
/// against the wrong one-time-key slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("attestation slot {slot} exceeds the u32 epoch domain")]
pub struct EpochOverflow {
    /// The offending slot value.
    pub slot: u64,
}

/// Derives the leanSig inputs for `att`: the epoch (`att.data.slot` narrowed to
/// the `u32` domain) and the message preimage (`bytes(hash_tree_root(att))`).
///
/// [`Attestation::hash_tree_root`] returns `[u8; 32]`; unifying the return type
/// with `[u8; crypto::MESSAGE_LENGTH]` is a compile-time assertion that the
/// attestation-root width equals leanSig's message width — no cast, no fallible
/// conversion.
///
/// # Errors
/// [`EpochOverflow`] when `att.data.slot` exceeds `u32::MAX`. The narrowing is
/// NEVER a lossy `as` cast.
pub fn attestation_signing_inputs(
    att: &Attestation,
) -> Result<(u32, [u8; crypto::MESSAGE_LENGTH]), EpochOverflow> {
    let slot = att.data.slot.get();
    let epoch = u32::try_from(slot).map_err(|_| EpochOverflow { slot })?;
    Ok((epoch, att.hash_tree_root()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use protocol::{AttestationData, Slot, ValidatorIndex};

    fn att(slot: u64) -> Attestation {
        Attestation {
            validator_id: ValidatorIndex::new(0),
            data: AttestationData {
                slot: Slot::new(slot),
                ..AttestationData::default()
            },
        }
    }

    #[test]
    fn signing_inputs_are_slot_epoch_and_attestation_htr() {
        let a = att(7);
        let (epoch, message) = attestation_signing_inputs(&a).unwrap();
        assert_eq!(epoch, 7);
        assert_eq!(message, a.hash_tree_root());
    }

    #[test]
    fn signing_inputs_reject_slot_beyond_u32_domain() {
        let over = u64::from(u32::MAX) + 1;
        assert_eq!(
            attestation_signing_inputs(&att(over)),
            Err(EpochOverflow { slot: over })
        );
    }
}
