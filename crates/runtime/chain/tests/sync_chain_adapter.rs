//! Verifies `sync::Chain for chain::Service` via the real engine + a
//! `MemoryStore`. Covers `local_status` snapshot semantics and
//! `has_block` consistency before / after `import_block`.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;

use engine::test_fixtures::{engine_at_genesis, produce_signed_block, ENGINE_VALIDATORS};
use engine::BlockImportResult;
use protocol::{Slot, ValidatorIndex};
use runtime_chain::sync::Chain as SyncChain;
use runtime_chain::Service;
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
    let pre = SyncChain::local_status(&svc).await.unwrap();

    // Drive one accepted import and re-read.
    let signed = produce_signed_block(svc_engine(&svc), Slot::new(1), ValidatorIndex::new(1));
    let outcome = Service::import_block(&svc, signed).await.unwrap();
    let head_root_after_import = match outcome {
        BlockImportResult::Accepted { head_root, .. } => head_root,
        other => panic!("expected Accepted, got {other:?}"),
    };

    let post = SyncChain::local_status(&svc).await.unwrap();
    // Snapshot reflects the engine-reported head after import.
    assert_eq!(post.head.root, head_root_after_import);
    // Genesis finalized checkpoint did not move.
    assert_eq!(post.finalized, pre.finalized);
}

#[tokio::test(flavor = "current_thread")]
async fn has_block_reports_true_after_import() {
    let svc = build_service();
    let signed = produce_signed_block(svc_engine(&svc), Slot::new(1), ValidatorIndex::new(1));
    let block_root: Bytes32 = signed.message.hash_tree_root().into();

    assert!(!SyncChain::has_block(&svc, block_root).await.unwrap());
    let _ = Service::import_block(&svc, signed).await.unwrap();
    assert!(SyncChain::has_block(&svc, block_root).await.unwrap());
}

/// `Engine` access through the service requires a public hook; for tests
/// we sidestep by producing the block via a fresh sibling engine at the
/// same genesis (deterministic — no state divergence before import).
fn svc_engine(_svc: &Service) -> &'static engine::Engine {
    use std::sync::OnceLock;
    static E: OnceLock<engine::Engine> = OnceLock::new();
    E.get_or_init(|| engine_at_genesis(ENGINE_VALIDATORS))
}
