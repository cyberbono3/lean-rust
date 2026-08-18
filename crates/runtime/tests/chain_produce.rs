//! Integration tests for `Service::produce_block` and
//! `Service::produce_attestation`.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;

use crypto::{CryptoError, MESSAGE_LENGTH};
use forkchoice::ForkchoiceError;
use protocol::{
    Attestation, AttestationData, Checkpoint, SignedAttestation, SignedBlockWithAttestation, Slot,
    ValidatorIndex,
};
use runtime::chain::engine::test_fixtures::{
    engine_at_genesis, produce_signed_block, ENGINE_VALIDATORS,
};
use runtime::chain::engine::{
    AttestationImportResult, BlockImportResult, Engine, EngineError, Verifier,
};
use runtime::chain::{ChainError, Service};
use runtime::duties::test_fixtures::stub_signer;
use ssz::HashTreeRoot;
use storage::{MemoryStore, Store};
use types::{Bytes32, PublicKey, Signature};

/// The subject of every test here is what `produce_*` PERSISTS and how it moves
/// the head — never the signature bytes, which `chain_sign.rs` covers with real
/// key material. A stub signer therefore keeps this file out of CPU-heavy
/// `ProdScheme` keygen and in the default test suite.
fn fresh_service() -> (Service, Arc<MemoryStore>, Engine) {
    let engine = engine_at_genesis(ENGINE_VALIDATORS);
    let store = Arc::new(MemoryStore::new());
    let service = Service::with_signer(
        engine.clone(),
        Arc::clone(&store) as Arc<dyn Store>,
        stub_signer(),
    );
    (service, store, engine)
}

#[tokio::test]
async fn produce_block_persists_and_moves_head() {
    let (service, store, _engine) = fresh_service();
    let pre = service.snapshot();

    // Slot 1 round-robin proposer is validator 1 (slot % ENGINE_VALIDATORS).
    let signed = service
        .produce_block(Slot::ONE, ValidatorIndex::new(1))
        .await
        .unwrap();
    assert_eq!(signed.message.block.slot, Slot::ONE);

    let root: Bytes32 = signed.message.block.hash_tree_root().into();
    assert_eq!(signed.message.block.parent_root, pre.head_root);

    // Block + post-state persisted at produced root; head info written from
    // the live engine head after the produced block expands forkchoice.
    let saved_block = store.load_block(&root).unwrap().unwrap();
    assert_eq!(saved_block.message.block.slot, Slot::ONE);
    assert!(store.load_state(&root).unwrap().is_some());
    assert!(store.load_head().unwrap().is_some());

    // Read on demand: the produced block moved the head.
    let post = service.snapshot();
    assert_eq!(post.head_root, root);
}

#[tokio::test]
async fn produce_block_rejects_unauthorized_proposer() {
    // The engine rejects an unauthorized proposer BEFORE any signing happens,
    // so this path needs no key material: a non-signing `Service::new` keeps
    // the test out of the CPU-heavy `ProdScheme` keygen and in the default suite.
    let engine = engine_at_genesis(ENGINE_VALIDATORS);
    let store = Arc::new(MemoryStore::new());
    let service = Service::new(engine, Arc::clone(&store) as Arc<dyn Store>);

    // Slot 1 proposer is validator 1; validator 2 is unauthorized.
    let err = service
        .produce_block(Slot::ONE, ValidatorIndex::new(2))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            ChainError::Engine(EngineError::Forkchoice(
                ForkchoiceError::UnauthorizedProposer { .. }
            ))
        ),
        "expected UnauthorizedProposer, got {err:?}",
    );
}

#[tokio::test]
async fn produce_attestation_carries_validator_id_and_holds_head() {
    let (service, _store, _engine) = fresh_service();
    let pre = service.snapshot();

    let signed = service
        .produce_attestation(Slot::ONE, ValidatorIndex::new(0))
        .await
        .unwrap();
    assert_eq!(signed.message.validator_id, ValidatorIndex::new(0));
    assert_eq!(signed.message.data.slot, Slot::ONE);

    // Read on demand after the own vote was imported.
    let post = service.snapshot();
    assert_eq!(post.head_root, pre.head_root);
}

#[tokio::test]
async fn produce_attestation_reimports_early_vote_with_anchor_source() {
    // A fresh engine normalizes the genesis justified checkpoint to the
    // tracked anchor root, so early own votes should be importable instead
    // of failing with UnknownSourceBlock on the zero root.
    let (service, _store, engine) = fresh_service();

    let producer = engine_at_genesis(ENGINE_VALIDATORS);
    let block_1: SignedBlockWithAttestation =
        produce_signed_block(&producer, Slot::ONE, ValidatorIndex::new(1));
    let _ = service.import_block(block_1).await.unwrap();

    let own = service
        .produce_attestation(Slot::ONE, ValidatorIndex::new(0))
        .await
        .unwrap();
    assert_eq!(own.message.validator_id, ValidatorIndex::new(0));
    assert_eq!(own.message.data.slot, Slot::ONE);

    let (in_pending, in_known) = engine.with_store(|s| {
        (
            s.latest_new_votes().contains_key(&ValidatorIndex::new(0)),
            s.latest_known_votes().contains_key(&ValidatorIndex::new(0)),
        )
    });
    assert!(in_pending || in_known);
}

// -- positional signature list ------------------------------------------

/// A `Verifier` that accepts every element. The subject of the two tests below
/// is the LIST — its length and its positional pairing — not the crypto, which
/// `chain_sign.rs` covers with real key material.
struct AcceptAll;

impl Verifier for AcceptAll {
    fn verify(
        &self,
        _public_key: &PublicKey,
        _epoch: u32,
        _message: &[u8; MESSAGE_LENGTH],
        _signature: &Signature,
    ) -> Result<(), CryptoError> {
        Ok(())
    }
}

/// A signature distinguishable per validator, so a test can assert WHICH
/// signature landed at a position rather than only how many there are.
fn marked_signature(validator: u64) -> Signature {
    let mut bytes = [0u8; Signature::LEN];
    bytes[0] = u8::try_from(validator + 1).unwrap();
    Signature::new(bytes)
}

/// Seeds the pool with one distinctly-signed vote per validator except the slot-1
/// proposer, all voting for the anchor. The votes are includable: the anchor is
/// tracked and is the justified source, so `produce_block` folds them into the body.
///
/// Each import is ASSERTED, not discarded: `AttestationImportResult` is
/// `#[must_use]`, and a silently `Rejected` vote would just shrink the body while
/// the per-index loop below still passed.
async fn seed_anchor_votes(service: &Service, anchor: Bytes32) {
    let anchor_cp = Checkpoint::new(anchor, Slot::ZERO);
    for v in 0..ENGINE_VALIDATORS {
        if v == 1 {
            continue; // the slot-1 proposer votes via the block envelope
        }
        let signed = SignedAttestation {
            message: Attestation::new(
                ValidatorIndex::new(v),
                AttestationData {
                    slot: Slot::ONE,
                    head: anchor_cp,
                    target: anchor_cp,
                    source: anchor_cp,
                },
            ),
            signature: marked_signature(v),
        };
        assert!(
            matches!(
                service.import_attestation(signed).await.unwrap(),
                AttestationImportResult::Accepted { .. }
            ),
            "fixture precondition: validator {v}'s vote must be admitted to the pool",
        );
    }
}

#[tokio::test]
async fn produce_block_emits_positional_signature_list() {
    let (service, _store, engine) = fresh_service();
    let anchor = engine.head();
    seed_anchor_votes(&service, anchor).await;

    let signed = service
        .produce_block(Slot::ONE, ValidatorIndex::new(1))
        .await
        .unwrap();

    let body = &signed.message.block.body.attestations;
    assert!(
        !body.is_empty(),
        "fixture precondition: the produced block must carry body attestations, \
         otherwise the length assertion below is vacuous",
    );
    // The criterion: one signature per body attestation, plus the proposer's.
    assert_eq!(signed.signature.len(), body.len() + 1);

    // Positional pairing: element i is the signature the pooled vote for
    // `body.attestations[i]` carried. Body order is `HashMap::values()` order, so
    // this keys on the validator, never on position.
    for (i, att) in body.iter().enumerate() {
        assert_eq!(
            signed.signature[i],
            marked_signature(att.validator_id.get()),
            "signature {i} does not belong to the attestation at the same index",
        );
    }
    // The proposer's own signature is LAST — the layout the verify side pairs
    // `proposer_attestation` against. The stub signer emits `Signature::zero()`;
    // the per-index assertions above are what rule out a `[proposer] ++ body`
    // ordering, since a zero is also what an unfilled slot looks like.
    assert_eq!(signed.signature[body.len()], Signature::zero());
}

#[tokio::test]
async fn produced_block_verifies_locally() {
    let (service, _store, engine) = fresh_service();
    let anchor = engine.head();
    seed_anchor_votes(&service, anchor).await;

    let signed = service
        .produce_block(Slot::ONE, ValidatorIndex::new(1))
        .await
        .unwrap();
    assert!(!signed.message.block.body.attestations.is_empty());

    // Round-trip through the real import-boundary gate on a peer engine with a
    // verifier injected: the produced block must survive the length and index
    // checks it will meet on every armed node. Before the positional list was
    // assembled this returned Rejected with a length mismatch.
    let importer = engine_at_genesis(ENGINE_VALIDATORS).with_verifier(Arc::new(AcceptAll));
    assert!(
        matches!(
            importer.import_block(signed),
            BlockImportResult::Accepted { .. }
        ),
        "a locally produced block must pass local verification",
    );
}
