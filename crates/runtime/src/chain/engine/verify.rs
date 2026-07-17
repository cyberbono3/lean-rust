//! Signature verification at the import boundary — the runtime's READ side of
//! the leanSig sign/verify pair (the `duties/signer` module owns the write side).
//!
//! [`verify_positional`] pairs each attestation with its signature positionally
//! (`body.attestations` then the proposer attestation LAST), verifying each via
//! the injected [`Verifier`] port at epoch = `attestation.data.slot` over
//! `hash_tree_root(attestation)`. These inputs match the signer's inputs
//! byte-for-byte — a drift on either side breaks every cross-client signature
//! under the pinned production scheme.
//!
//! Verification lives HERE, at the runtime import boundary — never in
//! `protocol::state_transition` or `forkchoice` (see PROJECT-KNOWLEDGE.md →
//! `LAYER_RULE`). This module and `duties/signer` are the only `runtime` sites
//! that touch `crypto`.

use std::sync::Arc;

use crypto::{CryptoError, ProdScheme, MESSAGE_LENGTH};
use protocol::{Attestation, Validators};
use ssz::HashTreeRoot;
use types::{PublicKey, Signature};

/// Failure surface of the import-boundary verify gate.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VerifyError {
    /// The signature count does not equal `body.attestations.len() + 1`
    /// (the proposer attestation is always the extra element). Checked BEFORE
    /// any per-element verify, so a mismatch spends no crypto work.
    #[error("signature count {got} does not match expected {expected}")]
    LengthMismatch {
        /// `body.attestations.len() + 1`.
        expected: usize,
        /// The actual signature-list length.
        got: usize,
    },
    /// An attestation names a validator index outside the registry.
    #[error("validator {validator_id} out of range (registry len {len})")]
    ValidatorOutOfRange {
        /// The offending validator index.
        validator_id: u64,
        /// The validator-registry length.
        len: usize,
    },
    /// The attestation slot exceeds the leanSig epoch domain (`u32`,
    /// LIFETIME = `2^32`). Rejected, never truncated — a truncated epoch would
    /// verify against the wrong one-time-key slot.
    #[error("attestation slot {slot} exceeds the u32 epoch domain")]
    EpochOverflow {
        /// The offending slot value.
        slot: u64,
    },
    /// The underlying leanSig verification failed (bad signature, malformed
    /// bytes, or epoch out of the scheme lifetime).
    #[error("leanSig verification failed")]
    Crypto(#[from] CryptoError),
}

/// The verify port: one leanSig verification of `message` for `epoch` under
/// `public_key`. Mirrors [`crypto::verify`] exactly. `runtime` depends on this
/// trait (DIP); the leanSig-backed [`ProdVerifier`] is the adapter behind it,
/// and tests inject a hand-written fake.
pub trait Verifier {
    /// Returns `Ok(())` only when `signature` is valid for `message`/`epoch`
    /// under `public_key`.
    ///
    /// # Errors
    /// [`CryptoError`] when the signature does not verify or the bytes are
    /// malformed.
    fn verify(
        &self,
        public_key: &PublicKey,
        epoch: u32,
        message: &[u8; MESSAGE_LENGTH],
        signature: &Signature,
    ) -> Result<(), CryptoError>;
}

/// Production adapter binding the pinned [`ProdScheme`]. Injected at the
/// composition root in a later Part (once the full positional signature list is
/// assembled); the only place in `runtime` bound to a concrete scheme.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProdVerifier;

impl Verifier for ProdVerifier {
    fn verify(
        &self,
        public_key: &PublicKey,
        epoch: u32,
        message: &[u8; MESSAGE_LENGTH],
        signature: &Signature,
    ) -> Result<(), CryptoError> {
        crypto::verify::<ProdScheme>(public_key, epoch, message, signature)
    }
}

/// Convenience constructor for the boxed production verifier the Engine holds.
#[must_use]
pub fn prod_verifier() -> Arc<dyn Verifier + Send + Sync> {
    Arc::new(ProdVerifier)
}

/// Verifies every `(attestation, signature)` pair positionally: the body
/// attestations first, the proposer attestation LAST. Strict length equality is
/// checked before any verification runs; the first failing element
/// short-circuits.
///
/// `validators` is the parent post-state registry; `validator_id` indexes it.
///
/// # Errors
/// [`VerifyError`] on a length mismatch, an out-of-range validator, a slot that
/// overflows the `u32` epoch domain, or a failed leanSig verification.
pub(crate) fn verify_positional<V: Verifier + ?Sized>(
    body_attestations: &[Attestation],
    proposer_attestation: &Attestation,
    signatures: &[Signature],
    validators: &Validators,
    verifier: &V,
) -> Result<(), VerifyError> {
    let expected = body_attestations.len() + 1;
    if signatures.len() != expected {
        return Err(VerifyError::LengthMismatch {
            expected,
            got: signatures.len(),
        });
    }

    body_attestations
        .iter()
        .chain(core::iter::once(proposer_attestation))
        .zip(signatures)
        .try_for_each(|(att, sig)| verify_one(att, sig, validators, verifier))
}

fn verify_one<V: Verifier + ?Sized>(
    att: &Attestation,
    sig: &Signature,
    validators: &Validators,
    verifier: &V,
) -> Result<(), VerifyError> {
    let validator_id = att.validator_id.get();
    // `usize::try_from` is infallible on 64-bit targets; the `get` bound-check
    // below is the real range guard.
    let idx = usize::try_from(validator_id).unwrap_or(usize::MAX);
    let validator = validators
        .get(idx)
        .ok_or(VerifyError::ValidatorOutOfRange {
            validator_id,
            len: validators.len(),
        })?;

    let slot = att.data.slot.get();
    let epoch = u32::try_from(slot).map_err(|_| VerifyError::EpochOverflow { slot })?;

    let message = att.hash_tree_root();
    verifier
        .verify(&validator.pubkey, epoch, &message, sig)
        .map_err(VerifyError::Crypto)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
pub(crate) mod test_support {
    //! Shared `Verifier` test double, reused by the `importer` gate tests.

    use std::collections::VecDeque;
    use std::sync::Mutex;

    use crypto::{CryptoError, MESSAGE_LENGTH};
    use types::{PublicKey, Signature};

    use super::Verifier;

    /// Hand-written `Verifier` double (per testing.md — no `mockall`). Records
    /// each call's `(epoch, message)` and returns scripted results in order.
    /// `Send + Sync` so it injects as `Arc<dyn Verifier + Send + Sync>`.
    pub(crate) struct FakeVerifier {
        calls: Mutex<Vec<(u32, [u8; MESSAGE_LENGTH])>>,
        script: Mutex<VecDeque<Result<(), CryptoError>>>,
    }

    impl FakeVerifier {
        /// A fake that returns `Ok` for the first `n` calls.
        pub(crate) fn all_ok(n: usize) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                script: Mutex::new((0..n).map(|_| Ok(())).collect()),
            }
        }

        /// A fake of `n` scripted calls where call index `bad` rejects with
        /// [`CryptoError::InvalidSignature`] and the rest return `Ok`.
        pub(crate) fn reject_nth(n: usize, bad: usize) -> Self {
            let script = (0..n)
                .map(|i| {
                    if i == bad {
                        Err(CryptoError::InvalidSignature)
                    } else {
                        Ok(())
                    }
                })
                .collect();
            Self {
                calls: Mutex::new(Vec::new()),
                script: Mutex::new(script),
            }
        }

        /// The recorded `(epoch, message)` of every call, in order.
        pub(crate) fn calls(&self) -> Vec<(u32, [u8; MESSAGE_LENGTH])> {
            self.calls.lock().expect("fake verifier lock").clone()
        }

        /// How many times `verify` was invoked.
        pub(crate) fn call_count(&self) -> usize {
            self.calls.lock().expect("fake verifier lock").len()
        }
    }

    impl Verifier for FakeVerifier {
        fn verify(
            &self,
            _public_key: &PublicKey,
            epoch: u32,
            message: &[u8; MESSAGE_LENGTH],
            _signature: &Signature,
        ) -> Result<(), CryptoError> {
            self.calls
                .lock()
                .expect("fake verifier lock")
                .push((epoch, *message));
            self.script
                .lock()
                .expect("fake verifier lock")
                .pop_front()
                .unwrap_or(Ok(()))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::test_support::FakeVerifier;
    use super::*;
    use protocol::{AttestationData, Slot, Validator, ValidatorIndex};

    /// Builds an attestation for `validator_id` at `slot` (other fields default).
    fn att(validator_id: u64, slot: u64) -> Attestation {
        Attestation {
            validator_id: ValidatorIndex::new(validator_id),
            data: AttestationData {
                slot: Slot::new(slot),
                ..AttestationData::default()
            },
        }
    }

    /// A registry of `n` validators (pubkeys default — the fake ignores them).
    fn validators(n: u64) -> Validators {
        (0..n)
            .map(|i| Validator {
                pubkey: PublicKey::default(),
                index: ValidatorIndex::new(i),
            })
            .collect()
    }

    fn zero_sigs(n: usize) -> Vec<Signature> {
        vec![Signature::zero(); n]
    }

    #[test]
    fn verify_positional_accepts_valid_block() {
        let body = vec![att(0, 1), att(1, 2)];
        let proposer = att(2, 3);
        let sigs = zero_sigs(3);
        let vals = validators(4);
        let fake = FakeVerifier::all_ok(3);

        assert!(verify_positional(&body, &proposer, &sigs, &vals, &fake).is_ok());
        // Called once per element, in positional order (body first, proposer last).
        assert_eq!(fake.call_count(), 3);
        let epochs: Vec<u32> = fake.calls().iter().map(|(e, _)| *e).collect();
        assert_eq!(epochs, vec![1, 2, 3]);
    }

    #[test]
    fn verify_positional_rejects_length_mismatch() {
        let body = vec![att(0, 1), att(1, 2)];
        let proposer = att(2, 3);
        let vals = validators(4);

        // Too few (expected 3, got 2): the gate fires before any verify.
        let fake = FakeVerifier::all_ok(3);
        assert!(matches!(
            verify_positional(&body, &proposer, &zero_sigs(2), &vals, &fake),
            Err(VerifyError::LengthMismatch {
                expected: 3,
                got: 2
            })
        ));
        assert_eq!(fake.call_count(), 0);

        // Too many (got 4): same.
        assert!(matches!(
            verify_positional(&body, &proposer, &zero_sigs(4), &vals, &fake),
            Err(VerifyError::LengthMismatch {
                expected: 3,
                got: 4
            })
        ));
        assert_eq!(fake.call_count(), 0);
    }

    #[test]
    fn verify_positional_rejects_out_of_range_validator() {
        // First body element names validator 5; registry has only 3.
        let body = vec![att(5, 1)];
        let proposer = att(0, 2);
        let sigs = zero_sigs(2);
        let vals = validators(3);
        let fake = FakeVerifier::all_ok(2);

        assert!(matches!(
            verify_positional(&body, &proposer, &sigs, &vals, &fake),
            Err(VerifyError::ValidatorOutOfRange {
                validator_id: 5,
                len: 3
            })
        ));
        // Out-of-range short-circuits before any verify.
        assert_eq!(fake.call_count(), 0);
    }

    #[test]
    fn verify_positional_propagates_crypto_error() {
        let body = vec![att(0, 1), att(1, 2)];
        let proposer = att(2, 3);
        let sigs = zero_sigs(3);
        let vals = validators(4);
        // 2nd element (index 1) rejects.
        let fake = FakeVerifier::reject_nth(3, 1);

        let err = verify_positional(&body, &proposer, &sigs, &vals, &fake).unwrap_err();
        assert!(matches!(err, VerifyError::Crypto(_)));
        // Short-circuit: the 3rd element is never visited.
        assert_eq!(fake.call_count(), 2);
    }

    #[test]
    fn verify_positional_epoch_from_slot_and_msg_is_htr() {
        let body = vec![att(0, 7)];
        let proposer = att(1, 9);
        let sigs = zero_sigs(2);
        let vals = validators(4);
        let fake = FakeVerifier::all_ok(2);

        verify_positional(&body, &proposer, &sigs, &vals, &fake).unwrap();
        let calls = fake.calls();
        // Body element: epoch == data.slot, message == hash_tree_root(att).
        assert_eq!(calls[0].0, 7);
        assert_eq!(calls[0].1, body[0].hash_tree_root());
        // Proposer element last.
        assert_eq!(calls[1].0, 9);
        assert_eq!(calls[1].1, proposer.hash_tree_root());
    }

    #[test]
    fn verify_positional_rejects_epoch_overflow() {
        let over = u64::from(u32::MAX) + 1;
        let body: Vec<Attestation> = Vec::new();
        let proposer = att(0, over);
        let sigs = zero_sigs(1);
        let vals = validators(4);
        let fake = FakeVerifier::all_ok(1);

        assert!(matches!(
            verify_positional(&body, &proposer, &sigs, &vals, &fake),
            Err(VerifyError::EpochOverflow { slot }) if slot == over
        ));
        // Overflow is detected before the verify call.
        assert_eq!(fake.call_count(), 0);
    }
}
