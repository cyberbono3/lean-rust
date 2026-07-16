//! [`Engine::import_block`] and [`Engine::import_attestation`] — the network
//! side of the engine surface.
//!
//! Follows the upstream importer flow shape but uses
//! Rust sum-type results: failures land inside the `Rejected` variant of
//! the returned outcome instead of an `(outcome, error)` pair.
//!
//! ## Mutation invariants
//!
//! - `DuplicateBlock` / `MissingParent` return before any mutation; the store
//!   is byte-equal to its pre-call state.
//! - `Rejected` returns after [`protocol::State::state_transition`] but before
//!   `track_block`. `state_transition` is transactional (it computes the
//!   transition on a local clone and swaps only on success — see
//!   `crates/protocol/src/state.rs:762`), and `track_block` is the only
//!   subsequent mutator. So a `Rejected` arm also leaves the store byte-equal.

use std::time::Instant;

use forkchoice::Store;
use protocol::{SignedAttestation, SignedBlock, State};
use ssz::HashTreeRoot;
use types::Bytes32;

use super::error::EngineError;
use super::handle::{capture_persist_plan, Engine, PersistPlan};
use super::results::{AttestationImportResult, BlockImportResult};
use crate::chain::metrics::ChainMetrics;

impl Engine {
    /// Validates `signed_block`, runs the full state transition, and tracks
    /// the resulting `(block, post_state)` pair in the store. Refreshes the
    /// canonical head via `accept_new_votes` on success.
    ///
    /// Returns a structured outcome — see [`BlockImportResult`] for the four
    /// variants and their semantics. Engine never panics on this path.
    pub fn import_block(&self, signed_block: SignedBlock) -> BlockImportResult {
        // Plan-free import entry point: a thin wrapper over
        // [`Self::import_block_capturing`] that discards the persist plan, so
        // the two paths cannot drift. Production uses the capturing variant
        // directly (it needs the plan to persist atomically under the same
        // lock); this form serves tests, the `engine_import` bench, and any
        // caller that does not persist. The discarded capture is a cheap
        // Arc bump, so the extra work is negligible.
        self.import_block_capturing(signed_block).0
    }

    /// Imports `signed_block` and, on [`BlockImportResult::Accepted`], captures
    /// its persist inputs under the same lock acquisition. This closes the
    /// window between accept and capture that the two-call
    /// `import_block` + separate `with_store` capture left open: a concurrent
    /// writer could shift the head or finalized checkpoint between the two
    /// acquisitions.
    ///
    /// Returns the structured outcome plus an optional [`PersistPlan`]. The plan
    /// is `Some` only on `Accepted`; it is `None` for the non-accept outcomes,
    /// and (unreachably) `None` if a post-accept invariant is violated — the
    /// caller maps that to a storage-layer error.
    pub(crate) fn import_block_capturing(
        &self,
        signed_block: SignedBlock,
    ) -> (BlockImportResult, Option<PersistPlan>) {
        let block_root: Bytes32 = signed_block.message.hash_tree_root().into();
        let parent_root = signed_block.message.parent_root;
        let mut store = self.lock();

        if store.has_block(&block_root) {
            return (BlockImportResult::DuplicateBlock { block_root }, None);
        }
        // Deep-clone the parent post-state: the state transition mutates an
        // owned copy. (The post-state *capture* for persistence is the cheap
        // Arc bump; this parent clone is inherent to running the STF.)
        let Some(parent_state) = store.state(&parent_root).map(|s| State::clone(s)) else {
            return (
                BlockImportResult::MissingParent {
                    block_root,
                    parent_root,
                },
                None,
            );
        };

        // Clone the block once for the plan before `transition_and_track`
        // consumes it; the clone is dropped on the rejected path.
        let block_for_plan = signed_block.clone();
        match transition_and_track(&mut store, signed_block, parent_state, self.metrics()) {
            Ok(post_state_root) => {
                let head_root = store.head();
                let plan = capture_persist_plan(&store, block_root, head_root, block_for_plan);
                (
                    BlockImportResult::Accepted {
                        block_root,
                        parent_root,
                        post_state_root,
                        head_root,
                    },
                    plan,
                )
            }
            Err(error) => (
                BlockImportResult::Rejected {
                    block_root,
                    parent_root,
                    error,
                },
                None,
            ),
        }
    }

    /// Validates `signed_attestation` as a gossip attestation (the `is_from_block =
    /// false` branch of `Store::process_attestation`) and folds it into the
    /// pending-vote pool when newer than the existing entry.
    ///
    /// Returns a structured outcome — see [`AttestationImportResult`].
    pub fn import_attestation(&self, signed_attestation: SignedAttestation) -> AttestationImportResult {
        let validator_id = signed_attestation.message.validator_id;
        let mut store = self.lock();

        let changed = match store.process_attestation(signed_attestation, false) {
            Ok(changed) => changed,
            Err(e) => {
                return AttestationImportResult::Rejected {
                    validator_id,
                    error: e.into(),
                };
            }
        };
        let head_root = store.head();
        let safe_target_root = store.safe_target();
        if changed {
            AttestationImportResult::Accepted {
                validator_id,
                head_root,
                safe_target_root,
            }
        } else {
            AttestationImportResult::Ignored {
                validator_id,
                head_root,
                safe_target_root,
            }
        }
    }
}

/// Runs the state transition, computes the post-state root, and tracks the
/// `(block, post_state)` pair in `store`. Refreshes the canonical head on
/// success. Returns the post-state root for the `Accepted` arm.
///
/// Timing is observation-only: the two `Instant` reads never influence control
/// flow or the returned root. This function does not change the existing store
/// mutation behavior on error paths (e.g. an `accept_new_votes` error can occur
/// after `track_block` has already mutated the store).
fn transition_and_track(
    store: &mut Store,
    signed_block: SignedBlock,
    mut post_state: State,
    metrics: &ChainMetrics,
) -> Result<Bytes32, EngineError> {
    let stf_start = Instant::now();
    post_state.state_transition(&signed_block, true)?;
    let stf_elapsed = stf_start.elapsed();

    let post_state_root: Bytes32 = post_state.hash_tree_root().into();
    store.track_block(signed_block.message, post_state)?;

    let fc_start = Instant::now();
    store.accept_new_votes()?;
    let fc_elapsed = fc_start.elapsed();

    // Observe both trigger histograms only once the import reaches success. The
    // `?` on state_transition / track_block / accept_new_votes returns early on
    // any failure, so a block that reaches Rejected records no sample. (One edge:
    // if accept_new_votes fails after track_block has already committed the block,
    // the sample is skipped — a slight undercount tracked with the store-
    // consistency follow-up, not a spurious count.)
    metrics.observe_state_transition(stf_elapsed);
    metrics.observe_fork_choice_block_processing(fc_elapsed);

    Ok(post_state_root)
}

// Fixtures here still build the deprecated `Bytes4000` placeholder. `expect`
// rather than `allow` so it retires itself when the fixture moves to
// `Signature`.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[expect(deprecated)]
mod tests {
    use super::*;
    use forkchoice::ForkchoiceError;
    use protocol::{
        Attestation, AttestationData, Block, BlockBody, Checkpoint, Slot, ValidatorIndex,
    };
    use types::{Bytes4000, Signature};

    use super::super::test_fixtures::{engine_at_genesis, produce_signed_block, ENGINE_VALIDATORS};

    /// Snapshot of store fields that must remain byte-equal across a
    /// no-mutation branch (`DuplicateBlock` / `MissingParent` / `Rejected`).
    #[derive(Debug, PartialEq, Eq)]
    struct StoreSnapshot {
        head: Bytes32,
        safe_target: Bytes32,
        block_order_len: usize,
        known_votes_len: usize,
        new_votes_len: usize,
    }

    impl StoreSnapshot {
        fn capture(engine: &Engine) -> Self {
            engine.with_store(|s| Self {
                head: s.head(),
                safe_target: s.safe_target(),
                block_order_len: s.block_order().len(),
                known_votes_len: s.latest_known_votes().len(),
                new_votes_len: s.latest_new_votes().len(),
            })
        }
    }

    /// Builds a [`SignedBlock`] whose `parent_root` is `parent` and whose
    /// remaining fields are zero-filled. The signature payload is zero —
    /// engine never inspects it on the missing-parent / duplicate paths.
    fn orphan_signed_block(parent: Bytes32) -> SignedBlock {
        SignedBlock {
            message: Block {
                slot: Slot::new(1),
                proposer_index: ValidatorIndex::new(1),
                parent_root: parent,
                state_root: Bytes32::zero(),
                body: BlockBody::default(),
            },
            signature: Bytes4000::new([0; 4000]),
        }
    }

    // -- import_block: happy path + duplicate -------------------------------

    #[test]
    fn import_block_accepts_then_returns_duplicate_block() {
        // Producer (engine_a) builds + tracks slot-1 block.
        let engine_a = engine_at_genesis(ENGINE_VALIDATORS);
        let signed = produce_signed_block(&engine_a, Slot::new(1), ValidatorIndex::new(1));
        let block_root: Bytes32 = signed.message.hash_tree_root().into();

        // Importer (engine_b) is a fresh handle anchored at the same genesis.
        let engine_b = engine_at_genesis(ENGINE_VALIDATORS);

        let BlockImportResult::Accepted {
            block_root: accepted_root,
            head_root,
            ..
        } = engine_b.import_block(signed.clone())
        else {
            panic!("expected Accepted on first import");
        };
        assert_eq!(accepted_root, block_root);
        assert_eq!(head_root, engine_b.head());

        // AC #1: importing the same block twice → DuplicateBlock.
        assert!(matches!(
            engine_b.import_block(signed),
            BlockImportResult::DuplicateBlock { block_root: r } if r == block_root
        ));
    }

    // -- import_block_capturing: captures plan on accept -------------------

    #[test]
    fn import_block_capturing_accepts_and_captures_plan() {
        let producer = engine_at_genesis(ENGINE_VALIDATORS);
        let signed = produce_signed_block(&producer, Slot::new(1), ValidatorIndex::new(1));
        let block_root: Bytes32 = signed.message.hash_tree_root().into();

        let importer = engine_at_genesis(ENGINE_VALIDATORS);
        let (outcome, plan) = importer.import_block_capturing(signed);

        assert!(
            matches!(outcome, BlockImportResult::Accepted { block_root: r, .. } if r == block_root)
        );
        let plan = plan.expect("Accepted import must capture a persist plan");
        let (root, block, _state, head, _finalized) = plan.into_parts();
        assert_eq!(root, block_root);
        let persisted_root: Bytes32 = block.message.hash_tree_root().into();
        assert_eq!(persisted_root, block_root);
        // Head checkpoint captured under the same lock matches the live head.
        assert_eq!(head.root, importer.head());
    }

    #[test]
    fn import_block_capturing_yields_no_plan_on_duplicate() {
        let producer = engine_at_genesis(ENGINE_VALIDATORS);
        let signed = produce_signed_block(&producer, Slot::new(1), ValidatorIndex::new(1));

        let importer = engine_at_genesis(ENGINE_VALIDATORS);
        let _ = importer.import_block_capturing(signed.clone());
        let (outcome, plan) = importer.import_block_capturing(signed);

        assert!(matches!(outcome, BlockImportResult::DuplicateBlock { .. }));
        assert!(plan.is_none(), "duplicate import must not capture a plan");
    }

    // -- import_block: missing parent does not mutate ----------------------

    #[test]
    fn import_block_missing_parent_leaves_store_byte_equal() {
        let engine = engine_at_genesis(ENGINE_VALIDATORS);
        let pre = StoreSnapshot::capture(&engine);

        let bogus_parent = Bytes32::new([0xaa; 32]);
        let outcome = engine.import_block(orphan_signed_block(bogus_parent));
        let BlockImportResult::MissingParent { parent_root, .. } = outcome else {
            panic!("expected MissingParent, got {outcome:?}");
        };
        assert_eq!(parent_root, bogus_parent);

        // AC #2: state snapshot identical.
        assert_eq!(pre, StoreSnapshot::capture(&engine));
    }

    // -- import_block: state-root mismatch returns Rejected ----------------

    #[test]
    fn import_block_state_root_mismatch_returns_rejected() {
        let producer = engine_at_genesis(ENGINE_VALIDATORS);
        let mut signed = produce_signed_block(&producer, Slot::new(1), ValidatorIndex::new(1));
        signed.message.state_root = Bytes32::new([0xff; 32]);

        let importer = engine_at_genesis(ENGINE_VALIDATORS);
        let pre = StoreSnapshot::capture(&importer);
        let outcome = importer.import_block(signed);
        assert!(matches!(
            outcome,
            BlockImportResult::Rejected {
                error: EngineError::StateTransition(_),
                ..
            }
        ));
        // Rejection must also leave the store byte-equal.
        assert_eq!(pre, StoreSnapshot::capture(&importer));
    }

    // -- import_attestation: rejection path --------------------------------

    #[test]
    fn import_attestation_unknown_target_returns_rejected() {
        let engine = engine_at_genesis(ENGINE_VALIDATORS);
        let anchor_root = engine.head();

        // Vote targets a root that the store does not track.
        let bogus = Bytes32::new([0xbb; 32]);
        let source = Checkpoint::new(anchor_root, Slot::ZERO);
        let target = Checkpoint::new(bogus, Slot::new(1));
        let sv = SignedAttestation {
            message: Attestation {
                validator_id: ValidatorIndex::new(0),
                data: AttestationData {
                    slot: Slot::new(1),
                    head: target,
                    target,
                    source,
                },
            },
            signature: Signature::zero(),
        };
        assert!(matches!(
            engine.import_attestation(sv),
            AttestationImportResult::Rejected {
                error: EngineError::Forkchoice(ForkchoiceError::UnknownTargetBlock { .. }),
                ..
            }
        ));
    }

    // -- trigger metrics: observe-on-success at the chain-tick boundary -----

    /// Builds a recorder with the two trigger histograms registered and a
    /// matching [`ChainMetrics`] handle set. Assembled inline because
    /// `register_chain_histograms` lives in the node crate.
    fn metrics_with_recorder() -> (crate::api::metrics::Recorder, ChainMetrics) {
        let mut recorder = crate::api::metrics::Recorder::new();
        let fc = recorder
            .histogram(
                "lean_fork_choice_block_processing_time_seconds",
                "fc",
                vec![1.0],
            )
            .unwrap();
        let stf = recorder
            .histogram("lean_state_transition_time_seconds", "stf", vec![1.0])
            .unwrap();
        let metrics = ChainMetrics::new(fc, stf);
        (recorder, metrics)
    }

    #[test]
    fn import_with_metrics_records_stf_and_fork_choice() {
        let (recorder, metrics) = metrics_with_recorder();
        let producer = engine_at_genesis(ENGINE_VALIDATORS);
        let signed = produce_signed_block(&producer, Slot::new(1), ValidatorIndex::new(1));
        let importer = engine_at_genesis(ENGINE_VALIDATORS).with_metrics(metrics);

        assert!(matches!(
            importer.import_block(signed),
            BlockImportResult::Accepted { .. }
        ));

        let body = recorder.freeze().unwrap().encode().unwrap();
        assert!(body.contains("lean_state_transition_time_seconds_count 1"));
        assert!(body.contains("lean_fork_choice_block_processing_time_seconds_count 1"));
    }

    #[test]
    fn rejected_import_does_not_observe_state_transition() {
        let (recorder, metrics) = metrics_with_recorder();
        let producer = engine_at_genesis(ENGINE_VALIDATORS);
        let mut signed = produce_signed_block(&producer, Slot::new(1), ValidatorIndex::new(1));
        // Corrupt the committed state root so the transition is rejected.
        signed.message.state_root = Bytes32::new([0xff; 32]);

        let importer = engine_at_genesis(ENGINE_VALIDATORS).with_metrics(metrics);
        assert!(matches!(
            importer.import_block(signed),
            BlockImportResult::Rejected {
                error: EngineError::StateTransition(_),
                ..
            }
        ));

        // Observe-on-success: a rejected import bumps neither histogram.
        let body = recorder.freeze().unwrap().encode().unwrap();
        assert!(body.contains("lean_state_transition_time_seconds_count 0"));
        assert!(body.contains("lean_fork_choice_block_processing_time_seconds_count 0"));
    }

    // -- AC #3 (produce_block validity) ------------------------------------

    #[test]
    fn produce_block_via_engine_returns_valid_block() {
        let engine = engine_at_genesis(ENGINE_VALIDATORS);
        let anchor_root = engine.head();
        let produced = engine
            .produce_block(Slot::new(1), ValidatorIndex::new(1))
            .unwrap();
        assert_eq!(produced.parent_root, anchor_root);
        assert_eq!(produced.block.slot, Slot::new(1));
        assert_eq!(produced.block.proposer_index, ValidatorIndex::new(1));
        assert!(produced.block.body.attestations.len() <= protocol::MAX_ATTESTATIONS);
        let recomputed: Bytes32 = produced.post_state.hash_tree_root().into();
        assert_eq!(produced.block.state_root, recomputed);
    }
}
