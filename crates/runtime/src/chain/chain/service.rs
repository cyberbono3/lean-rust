//! Chain [`Service`] — the single engine writer.
//!
//! Wraps [`crate::chain::engine::Engine`] + [`storage::Store`] and exposes async
//! `import_block` / `import_attestation` / `produce_block` /
//! `produce_attestation` / `tick_interval`, each funnelling through the
//! engine mutex. The self-driving consensus loop (`node` crate) drives the
//! forkchoice clock via [`Service::tick_interval`].
//!
//! See [`Service::import_block`] for the storage / engine divergence
//! contract on persistence failure.

// `refresh_snapshot` parks the snapshot `RwLock` write guard. Deny
// `await_holding_lock` so any future edit that holds a lock guard across an
// `.await` (which would stall the tokio worker thread) fails the build.
#![deny(clippy::await_holding_lock)]

use std::sync::Arc;

use crate::chain::engine::{AttestationImportResult, BlockImportResult, Engine, PersistPlan};
use async_trait::async_trait;
use lean_wire::Status;
use parking_lot::RwLock;
use protocol::{Checkpoint, SignedBlock, SignedVote, Slot, ValidatorIndex};
use ssz::HashTreeRoot;
use storage::HeadInfo;
use tokio_util::sync::CancellationToken;
use tracing::{debug, instrument, warn};
use types::{Bytes32, Bytes4000};

use super::cache::ChainSnapshot;
use super::error::ChainError;

/// Single-writer wrapper around [`Engine`] + [`storage::Store`].
///
/// # Concurrency
///
/// `import_block` and `import_attestation` serialize through the engine
/// mutex internally. Multiple callers may invoke them concurrently; the
/// engine is the funnel.
pub struct Service {
    engine: Engine,
    store: Arc<dyn storage::Store>,
    snapshot: Arc<RwLock<ChainSnapshot>>,
}

impl Service {
    /// Builds a service around `engine` and `store`. The initial snapshot
    /// is seeded from a single engine-lock acquisition so callers that
    /// clone the snapshot before `start` observe consistent state.
    #[must_use]
    pub fn new(engine: Engine, store: Arc<dyn storage::Store>) -> Self {
        let snapshot = Arc::new(RwLock::new(ChainSnapshot::from_engine(&engine)));
        Self {
            engine,
            store,
            snapshot,
        }
    }

    /// Returns a shared handle to the hot-read snapshot.
    ///
    /// Non-writer services (`lean-api`, `lean-p2p-host`) clone this
    /// handle and read through it instead of contending on the engine
    /// mutex.
    #[must_use]
    pub fn snapshot(&self) -> Arc<RwLock<ChainSnapshot>> {
        Arc::clone(&self.snapshot)
    }

    /// Imports `signed` through the engine. On [`BlockImportResult::Accepted`],
    /// persists the block, post-state, and head to storage and refreshes
    /// the snapshot.
    ///
    /// # Storage / engine divergence
    ///
    /// Persistence runs synchronously inside this call. If a `save_*`
    /// call fails after the engine has accepted the block, the engine
    /// in-memory state is ahead of storage: this method returns
    /// [`ChainError::Storage`] and the runtime cascade-stops. Recovery
    /// (replay-on-restart from the last persisted head) is tracked
    /// separately; it is intentionally out of scope here.
    ///
    /// # Errors
    /// - [`ChainError::Storage`] if any `save_*` call fails.
    /// - [`ChainError::PostStateMissing`] if the engine accepted the
    ///   block but the post-state has vanished by the time persistence
    ///   re-acquires the lock (engine invariant violation).
    #[instrument(level = "debug", skip_all, fields(slot = signed.message.slot.get()), err)]
    pub async fn import_block(&self, signed: SignedBlock) -> Result<BlockImportResult, ChainError> {
        let slot = signed.message.slot;
        // Import and capture the persist inputs under one engine-lock
        // acquisition, so no concurrent writer can shift the head/finalized
        // checkpoint between accept and capture.
        let (outcome, plan) = self.engine.import_block_capturing(signed);

        if let BlockImportResult::Accepted {
            block_root,
            head_root,
            ..
        } = &outcome
        {
            let plan = plan.ok_or(ChainError::PostStateMissing {
                block_root: *block_root,
            })?;
            self.persist_plan(plan)?;
            self.refresh_snapshot();
            debug!(
                slot = slot.get(),
                block_root = %block_root.to_hex(),
                head_root = %head_root.to_hex(),
                "chain accepted block persisted",
            );
        }
        Ok(outcome)
    }

    /// Imports `signed` through the engine. On
    /// [`AttestationImportResult::Accepted`], refreshes the snapshot.
    ///
    /// # Errors
    /// This method is currently infallible at the infrastructure layer —
    /// the [`Result`] is preserved for symmetry with [`Self::import_block`]
    /// and to leave room for future side effects.
    #[instrument(level = "debug", skip_all, fields(validator = signed.validator_id.get()), err)]
    pub async fn import_attestation(
        &self,
        signed: SignedVote,
    ) -> Result<AttestationImportResult, ChainError> {
        let slot = signed.message.slot;
        let validator = signed.validator_id;
        let outcome = self.engine.import_attestation(signed);
        if let AttestationImportResult::Accepted { head_root, .. } = &outcome {
            self.refresh_snapshot();
            debug!(
                slot = slot.get(),
                validator = validator.get(),
                head_root = %head_root.to_hex(),
                "chain accepted attestation applied",
            );
        }
        Ok(outcome)
    }

    /// Advances the forkchoice clock by one interval and refreshes the hot
    /// [`ChainSnapshot`]. `has_proposal` reflects whether this node produced
    /// a block in the current slot's proposal interval; the engine uses it
    /// to decide whether post-proposal votes are accepted this tick.
    ///
    /// Replaces the deleted background tick loop: the self-driving consensus
    /// loop (`node` crate) now calls this once per interval with a truthful
    /// `has_proposal`.
    ///
    /// # Errors
    /// [`ChainError::Engine`] if the engine rejects the tick.
    #[instrument(level = "debug", skip_all, fields(has_proposal), err)]
    pub async fn tick_interval(&self, has_proposal: bool) -> Result<(), ChainError> {
        // `tick_interval` locks the engine synchronously and returns before
        // the snapshot refresh; no lock guard crosses the `.await` boundary.
        self.engine.tick_interval(has_proposal)?;
        self.refresh_snapshot();
        Ok(())
    }

    /// Builds one locally authored block via [`Engine::produce_block`],
    /// wraps it as a [`SignedBlock`] with a zero-filled signature
    /// placeholder, and persists block + post-state + head to storage.
    ///
    /// The engine has already tracked the produced block (its `track_block`
    /// step inside `produce_block`); persistence mirrors the
    /// [`Self::import_block`] sweep so storage stays consistent with
    /// engine state.
    ///
    /// # Errors
    /// - [`ChainError::Engine`] if [`Engine::produce_block`] rejects the
    ///   request (unauthorized proposer, missing head state, etc.).
    /// - [`ChainError::Storage`] / [`ChainError::PostStateMissing`] from
    ///   the shared persist sweep on the same conditions as
    ///   [`Self::import_block`].
    #[instrument(level = "debug", skip_all, fields(slot = slot.get(), validator = validator.get()), err)]
    pub async fn produce_block(
        &self,
        slot: Slot,
        validator: ValidatorIndex,
    ) -> Result<SignedBlock, ChainError> {
        // Produce and capture the persist inputs under one engine-lock
        // acquisition: the block, its post-state, the head, and the finalized
        // checkpoint all come from one consistent store snapshot, instead of
        // the prior three separate acquisitions (produce, head(), persist).
        let (signed, plan) = self.engine.produce_block_capturing(slot, validator)?;
        let block_root: Bytes32 = signed.message.hash_tree_root().into();
        let plan = plan.ok_or(ChainError::PostStateMissing { block_root })?;
        self.persist_plan(plan)?;
        self.refresh_snapshot();
        debug!(
            slot = slot.get(),
            validator = validator.get(),
            block_root = %block_root.to_hex(),
            "chain produced block persisted",
        );
        Ok(signed)
    }

    /// Builds one locally authored attestation via
    /// [`Engine::produce_attestation_vote`], wraps it as a [`SignedVote`]
    /// with a zero-filled signature placeholder, and re-imports the vote
    /// locally so it lands in the engine's `latest_known_votes` pool.
    ///
    /// The local re-import is load-bearing: without it, this validator's
    /// own attestations only reach peers via gossip, and the next produced
    /// block would omit them — quorum on a small devnet can stall. Mirrors
    /// the upstream chain-service fix for the same stall.
    ///
    /// # Errors
    /// [`ChainError::Engine`] if [`Engine::produce_attestation_vote`]
    /// rejects the request.
    #[instrument(level = "debug", skip_all, fields(slot = slot.get(), validator = validator.get()), err)]
    pub async fn produce_attestation(
        &self,
        slot: Slot,
        validator: ValidatorIndex,
    ) -> Result<SignedVote, ChainError> {
        let produced = self.engine.produce_attestation_vote(slot)?;
        let signed = SignedVote {
            validator_id: validator,
            message: produced.vote,
            signature: Bytes4000::new([0; 4000]),
        };
        // Best-effort re-import: when `latest_justified` is still the
        // zero-sentinel (e.g. fresh anchor before the first justified
        // checkpoint), the produced vote's source.root is unresolvable
        // and the engine returns `Rejected`. The upstream client behaves the
        // same and warn-logs; we mirror that and continue.
        let outcome = self.engine.import_attestation(signed.clone());
        match &outcome {
            AttestationImportResult::Accepted { head_root, .. } => {
                debug!(
                    slot = slot.get(),
                    validator = validator.get(),
                    head_root = %head_root.to_hex(),
                    "chain own attestation reimported",
                );
            }
            AttestationImportResult::Rejected { .. } => {
                warn!(
                    ?outcome,
                    slot = slot.get(),
                    validator = validator.get(),
                    "own-attestation re-import rejected (vote still propagates to peers)",
                );
            }
            _ => {
                debug!(
                    ?outcome,
                    slot = slot.get(),
                    validator = validator.get(),
                    "own-attestation re-import outcome",
                );
            }
        }
        self.refresh_snapshot();
        Ok(signed)
    }

    /// Returns the local node's current [`Status`] for the peer-handshake.
    ///
    /// Backed by the cached [`ChainSnapshot`]: the value is eventually
    /// consistent with engine state (refreshed after each `Accepted`
    /// import and each tick). Acceptable for sync — the protocol
    /// tolerates a one-tick handshake lag.
    #[must_use]
    pub fn local_status(&self) -> Status {
        let snap = *self.snapshot.read();
        let head = Checkpoint::new(snap.head_root, Slot::new(snap.current_slot));
        Status {
            finalized: snap.latest_finalized,
            head,
        }
    }

    /// Reports whether `root` is already known to local storage.
    ///
    /// # Errors
    /// [`ChainError::Storage`] when the backing store call fails.
    pub fn has_block(&self, root: &Bytes32) -> Result<bool, ChainError> {
        Ok(self.store.has_block(root)?)
    }

    /// Replaces the cached snapshot with a fresh capture of engine state.
    /// One central edit point if the refresh policy ever becomes
    /// conditional (e.g. "only refresh when head moved").
    fn refresh_snapshot(&self) {
        *self.snapshot.write() = ChainSnapshot::from_engine(&self.engine);
    }

    /// Commits an engine-captured [`PersistPlan`] to storage.
    ///
    /// The plan was materialized atomically under the engine lock (head,
    /// post-state, and finalized checkpoint from one consistent snapshot), so
    /// this method only decomposes it and issues the single atomic
    /// [`storage::Store::save_accepted`] write: block + post-state + head
    /// commit together, and a mid-persist failure can never strand the head
    /// ahead of its block or state.
    fn persist_plan(&self, plan: PersistPlan) -> Result<(), ChainError> {
        let (block_root, block, post_state, head, finalized) = plan.into_parts();
        // The engine lock is already released here, so unwrapping the Arc (and
        // deep-cloning only if the store still shares it) happens off the hot
        // path — the under-lock cost was just the refcount bump in capture.
        self.store.save_accepted(
            block_root,
            block,
            Arc::unwrap_or_clone(post_state),
            HeadInfo::new(head, finalized),
        )?;
        Ok(())
    }
}

#[async_trait]
impl crate::core::Service for Service {
    fn name(&self) -> &'static str {
        "chain"
    }

    /// No-op: the chain service no longer owns a driving loop. The
    /// self-driving consensus loop (`node` crate) advances the engine via
    /// [`Service::tick_interval`]; the chain service only funnels engine
    /// mutations under the single writer lock.
    async fn start(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// No-op: nothing to tear down (no owned task).
    async fn stop(&self, _cancel: CancellationToken) -> anyhow::Result<()> {
        Ok(())
    }

    /// Always healthy: the chain service is a passive engine funnel with no
    /// background task to observe.
    async fn status(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

// Adapter `impl` blocks for the Tier-6 services that drive this
// chain Service live in the consuming crates (orphan rule: each
// trait is defined in the same crate as its adapter):
//   - `lean-sync::chain_adapter`    impl sync::Chain for Service
//   - `lean-duties::chain_adapter`  impl duties::Chain for Service
