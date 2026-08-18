//! Integration tests for the REAL sign path on `Service::produce_block` and
//! `Service::produce_attestation` (Part 13). Uses genuine `ProdScheme` key
//! material so the produced signatures verify end-to-end.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

mod common;

use std::sync::Arc;

use protocol::{Slot, ValidatorIndex};
use runtime::chain::engine::test_fixtures::{engine_at_genesis, ENGINE_VALIDATORS};
use runtime::chain::{ChainError, Service};
use ssz::HashTreeRoot;
use storage::{MemoryStore, Store};
use types::Signature;

#[tokio::test]
#[ignore = "leanSig ProdScheme keygen is CPU-heavy; run explicitly with --ignored"]
async fn produce_attestation_emits_verifiable_signature() {
    let engine = engine_at_genesis(ENGINE_VALIDATORS);
    let store = Arc::new(MemoryStore::new());
    let (signer, pubs) = common::signer_with_keys(&[0]);
    let service = Service::with_signer(engine, Arc::clone(&store) as Arc<dyn Store>, signer);

    let att = service
        .produce_attestation(Slot::ONE, ValidatorIndex::new(0))
        .await
        .unwrap();

    // The signature verifies against the producing validator's public key over
    // hash_tree_root(attestation) at epoch = data.slot.
    let msg = att.message.hash_tree_root();
    let epoch = u32::try_from(att.message.data.slot.get()).unwrap();
    assert!(crypto::verify::<crypto::ProdScheme>(&pubs[&0], epoch, &msg, &att.signature).is_ok());
}

#[tokio::test]
#[ignore = "leanSig ProdScheme keygen is CPU-heavy; run explicitly with --ignored"]
async fn produce_block_signs_only_proposer_attestation() {
    let engine = engine_at_genesis(ENGINE_VALIDATORS);
    let store = Arc::new(MemoryStore::new());
    // Slot-1 round-robin proposer is validator 1.
    let (signer, pubs) = common::signer_with_keys(&[1]);
    let service = Service::with_signer(engine, Arc::clone(&store) as Arc<dyn Store>, signer);

    let blk = service
        .produce_block(Slot::ONE, ValidatorIndex::new(1))
        .await
        .unwrap();

    // One signature per body attestation, plus the proposer's own LAST. Nothing
    // seeds the pool here, so the body is empty and the list holds exactly the
    // proposer's signature — asserted against the RULE, not against the constant 1,
    // so this stops meaning something different the moment a body appears.
    assert_eq!(
        blk.signature.len(),
        blk.message.block.body.attestations.len() + 1
    );
    let proposer_att = blk.message.proposer_attestation;
    assert_eq!(proposer_att.validator_id, ValidatorIndex::new(1));

    let msg = proposer_att.hash_tree_root();
    let epoch = u32::try_from(proposer_att.data.slot.get()).unwrap();
    let proposer_sig = &blk.signature[blk.signature.len() - 1];
    assert!(crypto::verify::<crypto::ProdScheme>(&pubs[&1], epoch, &msg, proposer_sig).is_ok());
}

#[tokio::test]
#[ignore = "leanSig ProdScheme keygen is CPU-heavy; run explicitly with --ignored"]
async fn no_placeholder_signature_on_production_path() {
    let engine = engine_at_genesis(ENGINE_VALIDATORS);
    let store = Arc::new(MemoryStore::new());
    let (signer, pubs) = common::signer_with_keys(&[0, 1]);
    let service = Service::with_signer(engine, Arc::clone(&store) as Arc<dyn Store>, signer);

    // A loaded validator never yields an all-zero signature.
    let att = service
        .produce_attestation(Slot::ONE, ValidatorIndex::new(0))
        .await
        .unwrap();
    assert_ne!(
        att.signature,
        Signature::zero(),
        "attestation signature must not be a zero placeholder",
    );

    let blk = service
        .produce_block(Slot::ONE, ValidatorIndex::new(1))
        .await
        .unwrap();
    // The proposer's signature is the LAST element, not the first: this test
    // produces an attestation BEFORE the block, and `produce_attestation`
    // re-imports that vote, so the block carries it as a body attestation and
    // `signature[0]` is the ATTESTER's signature.
    //
    // Both facts are ASSERTED rather than asserted-in-prose: the body length pins
    // that validator 0's re-imported vote really was includable, and the signature
    // length pins the positional rule, so a future body change cannot silently
    // re-point the check below at another element.
    assert_eq!(
        blk.message.block.body.attestations.len(),
        1,
        "the attestation produced above must be includable, or this test is \
         asserting the positional rule against an empty body",
    );
    assert_eq!(
        blk.signature.len(),
        blk.message.block.body.attestations.len() + 1
    );
    assert_ne!(
        blk.signature[blk.signature.len() - 1],
        Signature::zero(),
        "block proposer signature must not be a zero placeholder",
    );

    // The headline claim of the positional list — that element i carries the
    // signature its ATTESTER made — is only proven where real key material is
    // loaded. `chain_produce.rs` covers length and pairing with an accept-all
    // verifier, which by construction cannot detect a wrong signature; this is the
    // one place both a real key and a non-empty body are available.
    let body_att = blk.message.block.body.attestations[0];
    assert_eq!(body_att.validator_id, ValidatorIndex::new(0));
    let body_msg = body_att.hash_tree_root();
    let body_epoch = u32::try_from(body_att.data.slot.get()).unwrap();
    assert!(
        crypto::verify::<crypto::ProdScheme>(&pubs[&0], body_epoch, &body_msg, &blk.signature[0])
            .is_ok(),
        "body signature 0 must verify under the attesting validator's key",
    );
}

/// The consensus loop SKIPS the slot's proposer in the attest pass precisely
/// because signing the same (validator, slot) twice reuses the one-time key.
/// This guards the crypto layer's rejection of that double-sign at the
/// `ChainService` boundary: if the loop skip (`attesters_exclude_the_slot_proposer`,
/// tested in the node crate) ever regressed, a proposer that also attested its
/// own slot would burn its one-time key — here the second sign errors instead.
#[tokio::test]
#[ignore = "leanSig ProdScheme keygen is CPU-heavy; run explicitly with --ignored"]
async fn proposer_reattesting_same_slot_hits_ots_reuse_guard() {
    let engine = engine_at_genesis(ENGINE_VALIDATORS);
    let store = Arc::new(MemoryStore::new());
    // Slot-1 round-robin proposer is validator 1.
    let (signer, _pubs) = common::signer_with_keys(&[1]);
    let service = Service::with_signer(engine, Arc::clone(&store) as Arc<dyn Store>, signer);

    // Validator 1 signs its own attestation once, as proposer at slot 1.
    service
        .produce_block(Slot::ONE, ValidatorIndex::new(1))
        .await
        .unwrap();
    // Re-attesting the SAME (validator 1, slot 1) would sign epoch 1 again →
    // the one-time-key guard rejects it (surfaced as ChainError::Sign).
    let err = service
        .produce_attestation(Slot::ONE, ValidatorIndex::new(1))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ChainError::Sign(_)),
        "re-signing the same (validator, slot) must be rejected, got {err:?}",
    );
}
