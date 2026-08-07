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

use forkchoice::ForkchoiceError;
use protocol::{SignedBlockWithAttestation, Slot, ValidatorIndex};
use runtime::chain::engine::test_fixtures::{
    engine_at_genesis, produce_signed_block, ENGINE_VALIDATORS,
};
use runtime::chain::engine::{Engine, EngineError};
use runtime::chain::{ChainError, Service};
use runtime::duties::test_fixtures::stub_signer;
use ssz::HashTreeRoot;
use storage::{MemoryStore, Store};
use types::Bytes32;

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
