//! [`Engine::import_block`] and [`Engine::import_attestation`] — the network
//! side of the engine surface.
//!
//! Votes reach fork choice through BOTH entry points, not only the second.
//! `import_attestation` carries a gossip vote into the pending pool, while
//! `import_block` folds the attestations a block carries in its BODY straight
//! into the known pool. That second path is what lets a node which receives only
//! blocks resolve the same head as one that also receives the gossip traffic.
//!
//! The envelope's `proposer_attestation` is deliberately NOT folded — it is
//! covered by neither the block root nor the state root, so it is peer-mutable
//! in a way body attestations are not. See `block_carried_votes`.
//!
//! Follows the upstream importer flow shape but uses
//! Rust sum-type results: failures land inside the `Rejected` variant of
//! the returned outcome instead of an `(outcome, error)` pair.
//!
//! ## Mutation invariants
//!
//! - `DuplicateBlock` return before any mutation; the store is byte-equal to its
//!   pre-call state.
//! - A `Rejected` from the cheap over-cap gate returns before the parent-state
//!   clone (so on the enforced path over-cap precedes `MissingParent`) and before
//!   any mutation; the store is byte-equal.
//! - `MissingParent` returns before any mutation; the store is byte-equal.
//! - A `Rejected` from the signature verify gate returns BEFORE
//!   [`protocol::State::state_transition`] runs (the gate is read-only over the
//!   parent state), so the store is trivially byte-equal.
//! - A `Rejected` from the state transition returns after
//!   [`protocol::State::state_transition`] but before `track_block`.
//!   `state_transition` is transactional (it computes the transition on a local
//!   clone at `crates/protocol/src/state.rs:834` and swaps at `:848`, only on
//!   success), and `track_block` is the only mutator reachable before this
//!   return. So this `Rejected` arm also leaves the store byte-equal.
//! - The on-chain vote fold runs only AFTER `track_block` has committed, and it
//!   returns no error. No rejection path can observe a partially folded pool,
//!   and no on-chain vote can produce a rejection.

use std::time::Instant;

use forkchoice::Store;
use protocol::{
    Attestation, BlockSignatures, BlockWithAttestation, SignedAttestation,
    SignedBlockWithAttestation, State, Validators,
};
use ssz::HashTreeRoot;
use tracing::warn;
use types::Bytes32;

use super::error::EngineError;
use super::handle::{capture_persist_plan, Engine, PersistPlan};
use super::results::{AttestationImportResult, BlockImportResult};
use super::verify::{pre_check_over_cap, verify_positional, VerifyError};
use crate::chain::metrics::ChainMetrics;

/// Whether an import entry point subjects the block to the import-boundary
/// signature gate. Named rather than a bare `bool` so the two call sites read as
/// a policy decision instead of a positional flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerifyPolicy {
    /// Live gossip: run the gate (subject to verifier presence and the flag).
    Enforce,
    /// Sync backfill: skip the gate unconditionally.
    Skip,
}

impl VerifyPolicy {
    /// `true` only for [`VerifyPolicy::Enforce`].
    fn enforces(self) -> bool {
        matches!(self, Self::Enforce)
    }
}

impl Engine {
    /// Validates `signed_block`, runs the full state transition, and tracks
    /// the resulting `(block, post_state)` pair in the store. Refreshes the
    /// canonical head via `accept_new_votes` on success.
    ///
    /// Returns a structured outcome — see [`BlockImportResult`] for the four
    /// variants and their semantics. Engine never panics on this path.
    pub fn import_block(&self, signed_block: SignedBlockWithAttestation) -> BlockImportResult {
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
    ///
    /// Runs the import-boundary signature verify gate — active exactly when a
    /// verifier is injected via [`Engine::with_verifier`].
    pub(crate) fn import_block_capturing(
        &self,
        signed_block: SignedBlockWithAttestation,
    ) -> (BlockImportResult, Option<PersistPlan>) {
        self.import_block_capturing_inner(signed_block, VerifyPolicy::Enforce)
    }

    /// Sync-backfill variant: SKIPS the signature verify gate; live gossip uses
    /// [`Self::import_block_capturing`].
    ///
    /// `crate::chain::Service::import_block_synced` is the canonical statement
    /// of the trust boundary this opens — read it before routing anything else
    /// onto this entry.
    pub(crate) fn import_block_synced_capturing(
        &self,
        signed_block: SignedBlockWithAttestation,
    ) -> (BlockImportResult, Option<PersistPlan>) {
        self.import_block_capturing_inner(signed_block, VerifyPolicy::Skip)
    }

    /// Runs the import-boundary signature gate over `signed_block` against the
    /// parent post-state `validators`. A no-op (`Ok`) when no verifier is
    /// injected — the one way the gate stays inert. Read-only: it never touches
    /// the store.
    fn run_verify_gate(
        &self,
        signed_block: &SignedBlockWithAttestation,
        validators: &Validators,
    ) -> Result<(), VerifyError> {
        let Some(verifier) = self.verifier() else {
            return Ok(());
        };
        verify_positional(
            &signed_block.message.block.body.attestations,
            &signed_block.message.proposer_attestation,
            &signed_block.signature,
            validators,
            verifier,
        )
    }

    fn import_block_capturing_inner(
        &self,
        signed_block: SignedBlockWithAttestation,
        policy: VerifyPolicy,
    ) -> (BlockImportResult, Option<PersistPlan>) {
        let block_root: Bytes32 = signed_block.message.block.hash_tree_root().into();
        let parent_root = signed_block.message.block.parent_root;
        // One shape for every verify-stage rejection: gates below return through
        // this instead of re-spelling the (Rejected, None) pair.
        let reject = |error: VerifyError| {
            (
                BlockImportResult::Rejected {
                    block_root,
                    parent_root,
                    error: EngineError::Verify(error),
                },
                None,
            )
        };
        let mut store = self.lock();

        if store.has_block(&block_root) {
            return (BlockImportResult::DuplicateBlock { block_root }, None);
        }

        // Cheapest structural reject FIRST — the O(1) over-cap gate needs no parent
        // state, so it precedes the deep parent-state clone below (and any verify).
        // Runs on EVERY import policy, not only `Enforce`: it is verifier-independent
        // defense-in-depth, and the sync-backfill entry must not become the one
        // ingress that skips it (the policy gates only the expensive signature
        // verify below). Read-only, before any mutation, so a rejection leaves the
        // store byte-equal.
        if let Err(e) = pre_check_over_cap(
            &signed_block.signature,
            &signed_block.message.block.body.attestations,
        ) {
            return reject(e);
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

        // leanSig verify gate — BEFORE any mutation. Read-only over borrowed data, so
        // running it under the store lock is safe (no `&mut`, no `.await`); a rejection
        // returns with the store byte-equal. It needs `parent_state.validators` (just
        // materialized under this lock), so it follows the clone. Inert while no
        // verifier is injected; runs the full positional length/index/crypto check once
        // a verifier is wired (a later Part).
        if policy.enforces() {
            if let Err(e) = self.run_verify_gate(&signed_block, &parent_state.validators) {
                return reject(e);
            }
        }

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

    /// Validates `signed_vote` as a gossip attestation (the `is_from_block =
    /// false` branch of `Store::process_attestation`) and folds it into the
    /// pending-vote pool when newer than the existing entry.
    ///
    /// Returns a structured outcome — see [`AttestationImportResult`].
    pub fn import_attestation(&self, signed_vote: SignedAttestation) -> AttestationImportResult {
        let validator_id = signed_vote.message.validator_id;
        let mut store = self.lock();

        let changed = match store.process_attestation(signed_vote, false) {
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

/// Pairs each of the block's BODY attestations with the signature that belongs
/// to it, in the positional layout [`verify_positional`] checks — body
/// attestations first, index `i` to signature `i`.
///
/// # Why the proposer's attestation is NOT folded
///
/// `BlockWithAttestation::proposer_attestation` is deliberately excluded, and
/// the exclusion is a security boundary rather than an oversight.
///
/// `block_root` — the value that identifies a block, deduplicates it on import,
/// and anchors it in the store — is the hash-tree-root of the INNER
/// [`protocol::Block`] alone. `proposer_attestation` is a sibling field of that
/// block inside the envelope, so it enters no root the import path ever
/// computes. The state transition does not read it either: it processes
/// `block.body.attestations` and nothing else, so `state_root` cannot commit to
/// it. The one gate that would cover it — the import-boundary signature verify —
/// is inert whenever no verifier is injected, and the sync-backfill entry skips
/// it structurally.
///
/// Folding it would therefore connect a peer-controlled field, covered by
/// neither block identity nor the state root nor any active signature check,
/// directly to the vote pool that head resolution scores. Two envelopes
/// differing only in that field are indistinguishable to the duplicate check,
/// so a peer could take an honest block, substitute an arbitrary vote for any
/// validator index in the registry, and have it counted — while the honest copy
/// of the same block is then discarded as a duplicate. The on-chain branch also
/// evicts the victim's pending gossip vote, so the substitution deletes a real
/// vote as well as adding a forged one.
///
/// Body attestations carry none of that exposure: they are inside
/// `block.body`, hence inside `block_root`, and the state transition processes
/// them, so a tampered body changes `state_root` and the block is rejected
/// before this function is reached. Folding only the body keeps the invariant
/// that what identifies a block is exactly what the fold ingests.
///
/// The cost is that a proposer's own vote still reaches fork choice only on the
/// proposing node. That asymmetry closes on its own when the envelope is retired
/// and the proposer's vote becomes an ordinary body entry — at which point it is
/// covered by `block_root` and can be folded here for free.
///
/// # Signature pairing
///
/// The signature list is well-formed for the body when it holds at least
/// `body.len()` entries. Block production signs only the proposer's own
/// attestation and emits a ONE-element list regardless of body length, because
/// assembling the full positional list is a later change, so a short list is the
/// common case and each unpaired attestation falls back to a placeholder.
///
/// Fork-choice weight does not read the signature — the store scores the vote's
/// `data.head` checkpoint — so a placeholder costs nothing today. It will stop
/// being free once the positional list is assembled AND block production starts
/// reading the pooled signature, because a placeholder written here would then
/// be published in a produced block.
///
/// The paired signature is UNVERIFIED and, like the list length itself, is
/// peer-controlled: `BlockSignatures` is not covered by `block_root` either.
/// Nothing consumes the pooled signature today, so this is latent rather than
/// exploitable — but whichever change starts consuming it MUST verify it rather
/// than assume it was checked here.
///
/// Returns a lazy iterator rather than a `Vec` on purpose. A body at the
/// attestation cap turns ~136 bytes of wire data per attestation into a
/// 3252-byte `SignedAttestation`; collecting first would hold all of them live
/// across `track_block` on a peer-controlled path. Streaming keeps at most one
/// alive at a time, though every ACCEPTED vote is still retained by the store.
fn block_carried_votes<'a>(
    body: &'a [Attestation],
    signatures: &'a BlockSignatures,
) -> impl Iterator<Item = SignedAttestation> + 'a {
    // `BlockSignatures` derefs to `[Signature]`.
    body.iter()
        .enumerate()
        .map(move |(i, att)| SignedAttestation {
            message: *att,
            signature: signatures.get(i).cloned().unwrap_or_default(),
        })
}

/// Folds `votes` into the store's KNOWN vote pool — the `is_from_block == true`
/// branch of [`Store::process_attestation`], which is what the reference
/// specification's `on_block` does (`forkchoice/store.py:562-:569 @ 0c9528ac`:
/// block attestations go directly to the known payloads, which are the only ones
/// `update_head` reads).
///
/// A vote that fails validation is SKIPPED, never propagated. The reference
/// implementation applies no validity gate here at all, while this client's
/// [`Store::validate_attestation`] caps a vote's slot against the LOCAL store
/// clock. Propagating that error would let a node reject a block its own state
/// transition accepted, purely because the sender's clock is ahead — two honest
/// nodes would disagree about block validity, which is a permanent chain split.
/// So this function returns nothing: there is no path from a bad on-chain vote
/// to a `Rejected` import outcome.
///
/// Skipping is not free, and the cost is worth stating plainly. Because the slot
/// cap is a function of the local clock, two honest nodes whose clocks differ by
/// more than a slot fold DIFFERENT SUBSETS of the same block's attestations, and
/// can therefore compute different heads until the laggard catches up. That is a
/// soft, self-healing divergence, where propagating the error would be a hard,
/// permanent one — it is the better trade, not a free one, and it is accepted
/// deliberately here.
///
/// Reports one aggregated summary line per block rather than one line per
/// failure: a body may carry up to `MAX_ATTESTATIONS` entries, all of which a
/// peer can arrange to fail, and this runs while the caller holds the engine
/// store lock.
fn fold_block_attestations(store: &mut Store, votes: impl Iterator<Item = SignedAttestation>) {
    let mut skipped = 0_usize;
    let mut first_error = None;

    for vote in votes {
        let validator_id = vote.message.validator_id.get();
        if let Err(error) = store.process_attestation(vote, true) {
            skipped += 1;
            if first_error.is_none() {
                first_error = Some((validator_id, error));
            }
        }
    }

    if let Some((validator, error)) = first_error {
        warn!(
            skipped,
            first_validator = validator,
            first_error = %error,
            "on-chain attestations skipped; block import continues",
        );
    }
}

/// Runs the state transition, folds the block's own attestations into the
/// fork-choice vote pool, computes the post-state root, and tracks the
/// `(block, post_state)` pair in `store`. Refreshes the canonical head on
/// success. Returns the post-state root for the `Accepted` arm.
///
/// The fold sits between `track_block` and the head refresh, mirroring the
/// reference specification's `forkchoice/store.py:562-:574 @ 0c9528ac`, and
/// cannot fail — see [`fold_block_attestations`]. So this function's error paths
/// are unchanged: the same three `?` sites can return early, and no new one is
/// introduced.
///
/// Timing is observation-only: the two `Instant` reads never influence control
/// flow or the returned root. This function does not change the existing store
/// mutation behavior on error paths (e.g. an `accept_new_votes` error can occur
/// after `track_block` has already mutated the store).
fn transition_and_track(
    store: &mut Store,
    signed_block: SignedBlockWithAttestation,
    mut post_state: State,
    metrics: &ChainMetrics,
) -> Result<Bytes32, EngineError> {
    let stf_start = Instant::now();
    post_state.state_transition(&signed_block, true)?;
    let stf_elapsed = stf_start.elapsed();

    let post_state_root: Bytes32 = post_state.hash_tree_root().into();

    // Split the envelope: `track_block` consumes the block by value, but the
    // fold still needs the signature list. Only the body's attestation list is
    // cloned — one wire-size copy, versus holding a `SignedAttestation` per
    // attestation live across `track_block`. The clone cannot be avoided by
    // reading the block back out of the store, because the fold needs
    // `&mut store` and that borrow would conflict.
    //
    // The fold runs AFTER `track_block` for two reasons, neither of which is
    // "so the block's own roots resolve" — `validate_attestation` resolves only
    // `source.root` and `target.root`, and a body attestation cannot name its
    // containing block anyway (its root is not known until the body is fixed).
    // The actual reasons: `track_block` calls `adopt_post_state_checkpoints`,
    // which can advance `latest_justified`, and the fold's source resolution
    // reads exactly that; and folding earlier
    // would break the mutation invariant documented at the top of this file,
    // which states `track_block` is the only mutator reachable before the
    // state-transition rejection path returns.
    //
    // `proposer_attestation` is dropped here, NOT folded. It is covered by
    // neither `block_root` nor `state_root`, so folding it would make a
    // peer-controlled field a fork-choice input — see [`block_carried_votes`].
    let SignedBlockWithAttestation {
        message: BlockWithAttestation { block, .. },
        signature,
    } = signed_block;
    let body_attestations = block.body.attestations.clone();

    store.track_block(block, post_state)?;

    // The reference specification's `forkchoice/store.py:562-:574 @ 0c9528ac`
    // folds the block's attestations into the known pool and THEN refreshes the
    // head. The `accept_new_votes` below is that head refresh.
    fold_block_attestations(store, block_carried_votes(&body_attestations, &signature));

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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use forkchoice::ForkchoiceError;
    use protocol::{
        Attestation, AttestationData, Block, BlockBody, BlockSignatures, BlockWithAttestation,
        Checkpoint, Slot, ValidatorIndex, MAX_ATTESTATIONS,
    };
    use types::Signature;

    use super::super::test_fixtures::{engine_at_genesis, produce_signed_block, ENGINE_VALIDATORS};
    use super::super::verify::test_support::FakeVerifier;
    use std::sync::Arc;

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

    /// Builds a [`SignedBlockWithAttestation`] whose `parent_root` is `parent` and whose
    /// remaining fields are zero-filled. The signature payload is zero —
    /// engine never inspects it on the missing-parent / duplicate paths.
    fn orphan_signed_block(parent: Bytes32) -> SignedBlockWithAttestation {
        SignedBlockWithAttestation {
            message: BlockWithAttestation {
                block: Block {
                    slot: Slot::new(1),
                    proposer_index: ValidatorIndex::new(1),
                    parent_root: parent,
                    state_root: Bytes32::zero(),
                    body: BlockBody::default(),
                },
                proposer_attestation: Attestation::default(),
            },
            signature: BlockSignatures::default(),
        }
    }

    /// A within-cap signature list of `n` zero signatures.
    fn sigs(n: usize) -> BlockSignatures {
        core::iter::repeat_with(Signature::zero).take(n).collect()
    }

    // -- import_block: over-cap DoS pre-check --------------------------------

    #[test]
    fn over_cap_block_rejected_before_verify() {
        let producer = engine_at_genesis(ENGINE_VALIDATORS);
        let mut signed = produce_signed_block(&producer, Slot::new(1), ValidatorIndex::new(1));
        // Bloat the signature list past the cap; the inner block (and thus block_root
        // / parent) is untouched, so the block clears the missing-parent guard and
        // reaches the over-cap gate.
        signed.signature = sigs(MAX_ATTESTATIONS + 1);

        // Spy verifier scripted for ZERO calls: any invocation panics the fake.
        let fake = Arc::new(FakeVerifier::all_ok(0));
        let importer = engine_at_genesis(ENGINE_VALIDATORS).with_verifier(fake.clone());
        let before = StoreSnapshot::capture(&importer);

        let outcome = importer.import_block(signed);
        assert!(matches!(
            outcome,
            BlockImportResult::Rejected {
                error: EngineError::Verify(VerifyError::OverCap {
                    cap: MAX_ATTESTATIONS,
                    ..
                }),
                ..
            }
        ));
        // Over-cap short-circuits before any leanSig verify, and mutates nothing.
        assert_eq!(fake.call_count(), 0);
        assert_eq!(StoreSnapshot::capture(&importer), before);
    }

    #[test]
    fn over_cap_rejected_without_verifier() {
        let producer = engine_at_genesis(ENGINE_VALIDATORS);
        let mut signed = produce_signed_block(&producer, Slot::new(1), ValidatorIndex::new(1));
        signed.signature = sigs(MAX_ATTESTATIONS + 1);

        // Default engine: NO verifier injected — the over-cap gate is still active.
        let importer = engine_at_genesis(ENGINE_VALIDATORS);
        let before = StoreSnapshot::capture(&importer);

        assert!(matches!(
            importer.import_block(signed),
            BlockImportResult::Rejected {
                error: EngineError::Verify(VerifyError::OverCap { .. }),
                ..
            }
        ));
        assert_eq!(StoreSnapshot::capture(&importer), before);
    }

    // Note: that a within-cap, length-matched block clears the over-cap gate and
    // reaches the per-element verify is already proven by
    // `import_block_gossip_path_verifies_valid_signature` (which now also exercises the
    // over-cap gate on its accept path) — no separate over-cap-LSP test is needed.

    // -- import_block: happy path + duplicate -------------------------------

    #[test]
    fn import_block_accepts_then_returns_duplicate_block() {
        // Producer (engine_a) builds + tracks slot-1 block.
        let engine_a = engine_at_genesis(ENGINE_VALIDATORS);
        let signed = produce_signed_block(&engine_a, Slot::new(1), ValidatorIndex::new(1));
        let block_root: Bytes32 = signed.message.block.hash_tree_root().into();

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
        let block_root: Bytes32 = signed.message.block.hash_tree_root().into();

        let importer = engine_at_genesis(ENGINE_VALIDATORS);
        let (outcome, plan) = importer.import_block_capturing(signed);

        assert!(
            matches!(outcome, BlockImportResult::Accepted { block_root: r, .. } if r == block_root)
        );
        let plan = plan.expect("Accepted import must capture a persist plan");
        let (root, block, _state, head, _finalized) = plan.into_parts();
        assert_eq!(root, block_root);
        let persisted_root: Bytes32 = block.message.block.hash_tree_root().into();
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
        signed.message.block.state_root = Bytes32::new([0xff; 32]);

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
            signature: types::Signature::zero(),
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
        signed.message.block.state_root = Bytes32::new([0xff; 32]);

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

    // -- import-boundary verify gate ---------------------------------------

    /// A valid genesis-parented block at slot 1 whose `BlockSignatures` length
    /// matches `body.attestations.len() + 1`, so the strict length gate passes
    /// and every `(attestation, signature)` pair reaches the verifier. Returns
    /// the block plus its element count (`= body.len() + 1`).
    fn signed_block_len_matched() -> (SignedBlockWithAttestation, usize) {
        let producer = engine_at_genesis(ENGINE_VALIDATORS);
        let mut signed = produce_signed_block(&producer, Slot::new(1), ValidatorIndex::new(1));
        let elements = signed.message.block.body.attestations.len() + 1;
        signed.signature = sigs(elements);
        (signed, elements)
    }

    /// An importer engine with a populated validator registry and `fake`
    /// injected — injection is what enables the gate.
    fn gated_engine(fake: &Arc<FakeVerifier>) -> Engine {
        engine_at_genesis(ENGINE_VALIDATORS).with_verifier(fake.clone())
    }

    #[test]
    fn import_block_rejects_invalid_signature_when_verifier_injected() {
        let (signed, elements) = signed_block_len_matched();

        // The first element rejects → the gate short-circuits after one call.
        let fake = Arc::new(FakeVerifier::reject_nth(elements, 0));
        let importer = gated_engine(&fake);
        let pre = StoreSnapshot::capture(&importer);

        let outcome = importer.import_block(signed);
        assert!(matches!(
            outcome,
            BlockImportResult::Rejected {
                error: EngineError::Verify(_),
                ..
            }
        ));
        // The gate precedes state_transition → store byte-equal on rejection.
        assert_eq!(pre, StoreSnapshot::capture(&importer));
        assert_eq!(fake.call_count(), 1);
    }

    #[test]
    fn import_block_synced_skips_verify() {
        let (signed, elements) = signed_block_len_matched();

        // Verifier is injected and would reject — yet the synced entry skips.
        let fake = Arc::new(FakeVerifier::reject_nth(elements, 0));
        let importer = gated_engine(&fake);

        let (outcome, _plan) = importer.import_block_synced_capturing(signed);
        assert!(matches!(outcome, BlockImportResult::Accepted { .. }));
        assert_eq!(fake.call_count(), 0);
    }

    #[test]
    fn import_block_synced_rejects_over_cap() {
        // The synced entry skips only the SIGNATURE verify — the O(1) over-cap
        // structural bound runs on every import policy, so the sync-backfill
        // ingress is not the one path an over-cap block can slip through.
        let producer = engine_at_genesis(ENGINE_VALIDATORS);
        let mut signed = produce_signed_block(&producer, Slot::new(1), ValidatorIndex::new(1));
        signed.signature = sigs(MAX_ATTESTATIONS + 1);

        let importer = engine_at_genesis(ENGINE_VALIDATORS);
        let before = StoreSnapshot::capture(&importer);

        let (outcome, plan) = importer.import_block_synced_capturing(signed);
        assert!(matches!(
            outcome,
            BlockImportResult::Rejected {
                error: EngineError::Verify(VerifyError::OverCap { .. }),
                ..
            }
        ));
        assert!(plan.is_none());
        assert_eq!(StoreSnapshot::capture(&importer), before);
    }

    #[test]
    fn import_block_with_none_verifier_ignores_signature_length() {
        // PR-001 invariant: with NO verifier injected (the Engine default), the
        // gate is a no-op even for a block whose signature-list length would fail
        // the strict length check. Explicit guard so a future default-verifier
        // change cannot silently reject production blocks before the full
        // positional signature list is assembled (a later Part).
        let producer = engine_at_genesis(ENGINE_VALIDATORS);
        let mut signed = produce_signed_block(&producer, Slot::new(1), ValidatorIndex::new(1));
        // Deliberately mismatched vs body.len() + 1 (zero signatures).
        signed.signature = BlockSignatures::default();

        // No verifier injected — the Engine default.
        let importer = engine_at_genesis(ENGINE_VALIDATORS);
        assert!(matches!(
            importer.import_block(signed),
            BlockImportResult::Accepted { .. }
        ));
    }

    #[test]
    fn import_block_accepts_invalid_signature_when_gate_inert() {
        // The inert half of the #121 acceptance pair (the active half is
        // `import_block_rejects_invalid_signature_when_verifier_injected`).
        // Distinct from `import_block_with_none_verifier_ignores_signature_length`:
        // here the list length is CORRECT, so the strict length check would pass
        // and the block reaches the per-element verify. The signature bytes are
        // all-zero and would fail a real verify — but with no verifier injected
        // nothing ever inspects them, so the block is accepted.
        let (signed, _elements) = signed_block_len_matched();

        let importer = engine_at_genesis(ENGINE_VALIDATORS);
        assert!(matches!(
            importer.import_block(signed),
            BlockImportResult::Accepted { .. }
        ));
    }

    #[test]
    fn import_block_gossip_path_verifies_valid_signature() {
        let (signed, elements) = signed_block_len_matched();

        let fake = Arc::new(FakeVerifier::all_ok(elements));
        let importer = gated_engine(&fake);

        assert!(matches!(
            importer.import_block(signed),
            BlockImportResult::Accepted { .. }
        ));
        // The verifying path ran the gate once per positional element.
        assert_eq!(fake.call_count(), elements);
    }

    // -- block-carried attestation fold --------------------------------------

    /// A vote at slot 1 whose three checkpoints are all `cp`. Only `head` carries
    /// fork-choice weight; `validate_attestation` reads `source` and `target`.
    fn vote(validator: u64, cp: Checkpoint) -> Attestation {
        Attestation::new(
            ValidatorIndex::new(validator),
            AttestationData {
                slot: Slot::new(1),
                head: cp,
                target: cp,
                source: cp,
            },
        )
    }

    /// A signature filled with `byte`, distinguishable from every other one built
    /// this way. Pairing bugs that swap or shift elements fail loudly instead of
    /// passing by symmetry.
    fn tagged_sig(byte: u8) -> Signature {
        Signature::new([byte; Signature::LEN])
    }

    #[test]
    fn block_carried_votes_pairs_positionally_when_lengths_match() {
        let cp = Checkpoint::new(Bytes32::zero(), Slot::ZERO);
        let body = [vote(0, cp), vote(1, cp)];
        let signatures: BlockSignatures =
            [tagged_sig(0xa1), tagged_sig(0xb2)].into_iter().collect();

        let got: Vec<_> = block_carried_votes(&body, &signatures).collect();

        assert_eq!(got.len(), 2, "only the body is folded; the proposer is not");
        assert_eq!(got[0].message, body[0]);
        assert_eq!(got[0].signature, tagged_sig(0xa1));
        assert_eq!(got[1].message, body[1]);
        assert_eq!(got[1].signature, tagged_sig(0xb2));
    }

    #[test]
    fn block_carried_votes_falls_back_when_signature_list_is_short() {
        // One element is what `Service::produce_block` emits today, regardless of
        // body length. The first body attestation pairs with it; the rest fall
        // back to the placeholder.
        let cp = Checkpoint::new(Bytes32::zero(), Slot::ZERO);
        let body = [vote(0, cp), vote(1, cp)];
        let signatures: BlockSignatures = core::iter::once(tagged_sig(0xa1)).collect();

        let got: Vec<_> = block_carried_votes(&body, &signatures).collect();

        assert_eq!(got.len(), 2);
        assert_eq!(got[0].message, body[0]);
        assert_eq!(got[0].signature, tagged_sig(0xa1));
        assert_eq!(got[1].message, body[1]);
        assert_eq!(
            got[1].signature,
            Signature::default(),
            "an unpaired attestation takes the placeholder, never a mispaired signature",
        );
    }

    #[test]
    fn block_carried_votes_ignores_signatures_past_the_body() {
        // An over-long list is what a well-formed positional block looks like to
        // this helper: the trailing entry is the PROPOSER's signature, and since
        // the proposer attestation is not folded, its signature must be dropped
        // rather than mispaired onto a body attestation.
        let cp = Checkpoint::new(Bytes32::zero(), Slot::ZERO);
        let body = [vote(0, cp)];
        let signatures: BlockSignatures =
            [tagged_sig(0xa1), tagged_sig(0xff)].into_iter().collect();

        let got: Vec<_> = block_carried_votes(&body, &signatures).collect();

        assert_eq!(got.len(), 1, "one body attestation yields exactly one vote");
        assert_eq!(got[0].signature, tagged_sig(0xa1));
    }

    #[test]
    fn block_carried_votes_ignores_the_proposer_attestation() {
        // The security boundary this change turns on: `proposer_attestation` is
        // covered by neither `block_root` nor `state_root`, so it must never
        // become a fork-choice input. An EMPTY body yields NO votes even though
        // the envelope carries a proposer attestation and a signature for it.
        let body: [Attestation; 0] = [];
        let signatures: BlockSignatures = core::iter::once(tagged_sig(0xd4)).collect();

        let got: Vec<_> = block_carried_votes(&body, &signatures).collect();

        assert!(
            got.is_empty(),
            "an empty body must fold nothing; folding the proposer attestation \
             would make a peer-controlled field a fork-choice input",
        );
    }

    #[test]
    fn fold_writes_to_the_known_pool_not_the_pending_pool() {
        // Pins `is_from_block == true`, which no other test in this suite can
        // distinguish: every other one starts from an empty pending pool, and
        // `accept_new_votes` drains pending into known one statement after the
        // fold, so both settings leave identical observable state.
        //
        // The asymmetry this exploits: the on-chain branch guards the known pool
        // with `insert_if_newer`, so an OLDER vote loses. The gossip branch would
        // instead park the older vote in pending, and `accept_new_votes` merges
        // with `HashMap::extend`, which overwrites unconditionally — so the older
        // vote would clobber the newer one.
        let engine = engine_at_genesis(ENGINE_VALIDATORS);
        let anchor = engine.head();
        let anchor_cp = Checkpoint::new(anchor, Slot::ZERO);
        let validator = ValidatorIndex::new(0);

        // Seed the KNOWN pool with a slot-1 vote, directly rather than through
        // the fold, so the seed is unaffected by the flag under test. Slot 1 is
        // the newest admissible value: `validate_attestation` caps a vote at
        // `current_vote_slot() + 1`, which is 1 on a genesis engine.
        {
            let mut store = engine.lock();
            let newer = SignedAttestation {
                message: Attestation::new(
                    validator,
                    AttestationData {
                        slot: Slot::new(1),
                        head: anchor_cp,
                        target: anchor_cp,
                        source: anchor_cp,
                    },
                ),
                signature: Signature::default(),
            };
            assert!(store.process_attestation(newer, true).unwrap());
        }

        // Fold an OLDER vote (slot 0) for the same validator.
        {
            let mut store = engine.lock();
            let older = SignedAttestation {
                message: Attestation::new(
                    validator,
                    AttestationData {
                        slot: Slot::ZERO,
                        head: anchor_cp,
                        target: anchor_cp,
                        source: anchor_cp,
                    },
                ),
                signature: Signature::default(),
            };
            fold_block_attestations(&mut store, core::iter::once(older));
            store.accept_new_votes().unwrap();
        }

        let pooled_slot = engine.with_store(|s| {
            s.latest_known_votes()
                .get(&validator)
                .map(|sv| sv.message.data.slot)
        });
        assert_eq!(
            pooled_slot,
            Some(Slot::new(1)),
            "the on-chain branch must keep the NEWER known vote; seeing slot 0 \
             means the fold wrote to the pending pool and accept_new_votes then \
             clobbered the newer entry",
        );
    }

    #[test]
    fn invalid_on_chain_attestation_is_skipped_not_propagated() {
        // Decision 2, tested against the fold directly rather than through
        // `import_block`.
        //
        // Driving this through the import path is not possible, and the reason is
        // worth recording: `process_block_header` writes the block header — which
        // commits to the BODY root — into `state.latest_block_header`, and that
        // field is inside the state's own hash-tree-root. So ANY hand-edit to
        // `block.body.attestations` moves `state_root`, and the block is rejected
        // as `StateRootMismatch` long before the fold runs — whether or not the
        // STF would have tallied the vote. The body is structurally sealed
        // against tampering, which is precisely why folding it is safe and why
        // folding `proposer_attestation` would not be.
        //
        // That leaves the fold itself as the unit owning skip-on-invalid.
        let engine = engine_at_genesis(ENGINE_VALIDATORS);
        let anchor = engine.head();
        let anchor_cp = Checkpoint::new(anchor, Slot::ZERO);

        // First vote names an untracked target and fails `validate_attestation`;
        // the second is valid against the anchor.
        let bad = SignedAttestation {
            message: Attestation::new(
                ValidatorIndex::new(0),
                AttestationData {
                    slot: Slot::new(1),
                    head: anchor_cp,
                    target: Checkpoint::new(Bytes32::new([0xab; 32]), Slot::ZERO),
                    source: anchor_cp,
                },
            ),
            signature: Signature::default(),
        };
        let good = SignedAttestation {
            message: vote(1, anchor_cp),
            signature: Signature::default(),
        };

        {
            let mut store = engine.lock();
            // Returns `()` — there is no path from a bad on-chain vote to a
            // `Rejected` import outcome, and the signature is what guarantees it.
            fold_block_attestations(&mut store, [bad, good].into_iter());
        }

        // The bad vote was skipped and the good one still landed. A fold that
        // aborted on the first error would leave this at 0.
        assert_eq!(
            StoreSnapshot::capture(&engine).known_votes_len,
            1,
            "a valid vote must survive a preceding invalid one",
        );
    }

    #[test]
    fn block_carried_vote_reaches_the_vote_pool() {
        // The vote has to arrive in the body through the PRODUCTION path. A
        // hand-appended vote that fork choice would accept is also one the STF
        // tallies, which moves `state_root` and gets the block rejected before
        // the fold runs. Producing the block instead means the body holds exactly
        // what the producer's own pool contributed, and `state_root` is computed
        // over that body.
        let producer = engine_at_genesis(ENGINE_VALIDATORS);
        let anchor = producer.head();
        let anchor_cp = Checkpoint::new(anchor, Slot::ZERO);

        assert!(matches!(
            producer.import_attestation(SignedAttestation {
                message: vote(0, anchor_cp),
                signature: Signature::default(),
            }),
            AttestationImportResult::Accepted { .. }
        ));
        let signed = produce_signed_block(&producer, Slot::new(1), ValidatorIndex::new(1));
        assert!(
            !signed.message.block.body.attestations.is_empty(),
            "precondition: the produced block must actually carry the vote, or \
             the assertion below is vacuous",
        );

        // A fresh node that never saw the gossip vote.
        let importer = engine_at_genesis(ENGINE_VALIDATORS);
        let outcome = importer.import_block(signed);
        assert!(
            matches!(outcome, BlockImportResult::Accepted { .. }),
            "{outcome:?}",
        );

        // Without the fold this is 0: the carried vote reaches no pool at all.
        //
        // What this CANNOT prove is which pool the fold wrote to. `accept_new_votes`
        // runs one statement after the fold and drains pending into known, so both
        // settings of `is_from_block` leave the same observable state here — and
        // the same is true of both integration tests, which start from an empty
        // pending pool. `fold_writes_to_the_known_pool_not_the_pending_pool` below
        // is what actually pins that flag.
        assert_eq!(
            StoreSnapshot::capture(&importer).known_votes_len,
            1,
            "the block-carried vote must reach the vote pool; 0 means the fold never ran",
        );
    }
}
