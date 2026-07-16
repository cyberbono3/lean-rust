//! Verifies the concrete `chain::Service` surface the sync `Loop` calls
//! (the former `sync::Chain` port collapsed to it), via the real engine +
//! a `MemoryStore`. Covers `local_status` snapshot semantics and
//! `has_block` consistency before / after `import_block`.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;

use protocol::{Slot, ValidatorIndex};
use runtime::chain::engine::test_fixtures::{
    engine_at_genesis, produce_signed_block, ENGINE_VALIDATORS,
};
use runtime::chain::engine::BlockImportResult;
use runtime::chain::Service;
use ssz::HashTreeRoot;
use storage::MemoryStore;
use types::Bytes32;

fn build_service() -> Service {
    let engine = engine_at_genesis(ENGINE_VALIDATORS);
    Service::new(engine, Arc::new(MemoryStore::new()))
}

#[tokio::test(flavor = "current_thread")]
async fn local_status_reflects_engine_head_after_import() {
    let svc = build_service();
    let pre = svc.local_status();

    // Drive one accepted import and re-read.
    let producer = sibling_engine();
    let signed = produce_signed_block(&producer, Slot::new(1), ValidatorIndex::new(1));
    let outcome = Service::import_block(&svc, signed).await.unwrap();
    let head_root_after_import = match outcome {
        BlockImportResult::Accepted { head_root, .. } => head_root,
        other => panic!("expected Accepted, got {other:?}"),
    };

    let post = svc.local_status();
    // Snapshot reflects the engine-reported head after import.
    assert_eq!(post.head.root, head_root_after_import);
    // Genesis finalized checkpoint did not move.
    assert_eq!(post.finalized, pre.finalized);
}

#[tokio::test(flavor = "current_thread")]
async fn has_block_reports_true_after_import() {
    let svc = build_service();
    let producer = sibling_engine();
    let signed = produce_signed_block(&producer, Slot::new(1), ValidatorIndex::new(1));
    let block_root: Bytes32 = signed.message.block.hash_tree_root().into();

    assert!(!svc.has_block(&block_root).unwrap());
    let _ = Service::import_block(&svc, signed).await.unwrap();
    assert!(svc.has_block(&block_root).unwrap());
}

/// `Engine` access through the service requires a public hook; for tests
/// we sidestep by producing the block via a fresh sibling engine at the
/// same genesis (deterministic — no state divergence before import).
fn sibling_engine() -> runtime::chain::engine::Engine {
    engine_at_genesis(ENGINE_VALIDATORS)
}
