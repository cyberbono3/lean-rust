//! An attestation that reaches the fork-choice vote pool must still verify
//! against the signature it arrived with.
//!
//! The store resolves a genesis-placeholder source checkpoint for validation, and
//! that resolution must never reach storage: `source.root` is inside the signed
//! preimage, so a stored vote whose source was rewritten carries a signature over
//! bytes the attester never signed.
//!
//! This test lives in `runtime` rather than beside the fix because `forkchoice`
//! has no `crypto` dependency and the layer rules forbid adding one.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use crypto::{CryptoError, MESSAGE_LENGTH};
use protocol::{Attestation, AttestationData, Checkpoint, SignedAttestation, Slot, ValidatorIndex};
use runtime::chain::engine::test_fixtures::{engine_at_genesis, ENGINE_VALIDATORS};
use runtime::chain::engine::{AttestationImportResult, Verifier};
use runtime::signing_domain::attestation_signing_inputs;
use types::{Bytes32, PublicKey, Signature};

/// Accepts exactly one `(epoch, message, signature)` triple — the one captured
/// before the attestation entered the store — and rejects everything else.
///
/// The message is `hash_tree_root(attestation)`, so ANY single-byte change
/// anywhere in the attestation, `source.root` included, produces a different
/// message and a rejection. That is what makes this a signature check rather than
/// a field comparison.
struct PinnedVerifier {
    epoch: u32,
    message: [u8; MESSAGE_LENGTH],
    signature: Signature,
}

impl Verifier for PinnedVerifier {
    fn verify(
        &self,
        _public_key: &PublicKey,
        epoch: u32,
        message: &[u8; MESSAGE_LENGTH],
        signature: &Signature,
    ) -> Result<(), CryptoError> {
        if epoch == self.epoch && *message == self.message && *signature == self.signature {
            Ok(())
        } else {
            Err(CryptoError::InvalidSignature)
        }
    }
}

/// A slot-0 vote carrying the all-zero source checkpoint a peer emits when its
/// producer has not substituted the real genesis root. This is the only shape the
/// store's source resolution touches — a vote that did not trigger it would pass
/// before and after the fix, and would pin nothing.
fn genesis_placeholder_vote(anchor: Bytes32) -> SignedAttestation {
    let anchor_checkpoint = Checkpoint::new(anchor, Slot::ZERO);
    SignedAttestation {
        message: Attestation {
            validator_id: ValidatorIndex::new(0),
            data: AttestationData {
                slot: Slot::ZERO,
                head: anchor_checkpoint,
                target: anchor_checkpoint,
                source: Checkpoint::default(),
            },
        },
        // A recognisable pattern rather than `Signature::zero()`: an all-zero
        // signature cannot distinguish a preserved one from a dropped one.
        signature: Signature::new([0x5a; Signature::LEN]),
    }
}

#[test]
fn stored_attestation_signature_still_verifies() {
    let engine = engine_at_genesis(ENGINE_VALIDATORS);
    let before = genesis_placeholder_vote(engine.head());

    // Capture the signing inputs from the envelope as it arrives, through the
    // same derivation the signer and the import-boundary verifier both use.
    let (epoch_before, message_before) = attestation_signing_inputs(&before.message)
        .expect("a slot-0 attestation is inside the u32 epoch domain");
    let verifier = PinnedVerifier {
        epoch: epoch_before,
        message: message_before,
        signature: before.signature.clone(),
    };

    let outcome = engine.import_attestation(before.clone());
    assert!(
        matches!(outcome, AttestationImportResult::Accepted { .. }),
        "the genesis-placeholder source must still resolve and the vote be accepted, got {outcome:?}",
    );

    let stored = engine
        .with_store(|store| {
            store
                .latest_new_votes()
                .get(&ValidatorIndex::new(0))
                .cloned()
        })
        .expect("an accepted gossip vote must be in the pending pool");

    let (epoch, message) = attestation_signing_inputs(&stored.message)
        .expect("the stored attestation is inside the u32 epoch domain");
    verifier
        .verify(&PublicKey::default(), epoch, &message, &stored.signature)
        .expect(
            "the stored envelope must verify against the signature it arrived with — \
             a rewritten source root changes the attestation root and breaks this",
        );
}
