//! Integration tests for `Service::import_attestation`.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;

use protocol::{Attestation, AttestationData, Checkpoint, SignedAttestation, Slot, ValidatorIndex};
use runtime::chain::engine::test_fixtures::{engine_at_genesis, ENGINE_VALIDATORS};
use runtime::chain::engine::AttestationImportResult;
use runtime::chain::Service;
use storage::MemoryStore;
use types::{Bytes32, Signature};

fn fresh_service() -> Service {
    let engine = engine_at_genesis(ENGINE_VALIDATORS);
    Service::new(engine, Arc::new(MemoryStore::new()))
}

fn vote(head: Checkpoint, target: Checkpoint, source: Checkpoint) -> SignedAttestation {
    SignedAttestation {
        message: Attestation {
            validator_id: ValidatorIndex::new(0),
            data: AttestationData {
                slot: Slot::ONE,
                head,
                target,
                source,
            },
        },
        signature: Signature::new([0; Signature::LEN]),
    }
}

#[tokio::test]
async fn accepted_vote_leaves_head_at_anchor() {
    let service = fresh_service();
    let anchor = service.snapshot().head_root;

    // A vote whose head/target/source all reference the genesis anchor
    // (the only block tracked by a fresh engine) is structurally valid
    // and lands as a gossip-pool insert for validator 0.
    let anchor_ckpt = Checkpoint::new(anchor, Slot::ZERO);
    let outcome = service
        .import_attestation(vote(anchor_ckpt, anchor_ckpt, anchor_ckpt))
        .await
        .unwrap();
    assert!(
        matches!(outcome, AttestationImportResult::Accepted { .. }),
        "expected Accepted, got {outcome:?}",
    );
    // Read on demand: head_root still matches anchor (no forkchoice movement
    // on a single vote at slot 1).
    assert_eq!(service.snapshot().head_root, anchor);
}

#[tokio::test]
async fn rejected_vote_leaves_state_unchanged() {
    let service = fresh_service();
    let pre = service.snapshot();

    let anchor_ckpt = Checkpoint::new(pre.head_root, Slot::ZERO);
    let bogus = Bytes32::new([0xbb; 32]);
    let bogus_target = Checkpoint::new(bogus, Slot::ONE);
    let outcome = service
        .import_attestation(vote(bogus_target, bogus_target, anchor_ckpt))
        .await
        .unwrap();
    assert!(
        matches!(outcome, AttestationImportResult::Rejected { .. }),
        "expected Rejected, got {outcome:?}",
    );

    let post = service.snapshot();
    assert_eq!(pre, post);
}
