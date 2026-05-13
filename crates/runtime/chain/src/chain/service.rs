//! Chain [`Service`] — the single engine writer.
//!
//! Wraps [`engine::Engine`] + [`storage::Store`] and exposes async
//! `import_block` / `import_attestation`. Spawns a background tick loop
//! on `start` that advances the forkchoice clock every
//! `config::SECONDS_PER_INTERVAL`.
//!
//! See [`Service::import_block`] for the storage / engine divergence
//! contract on persistence failure.

use std::sync::Arc;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use engine::{AttestationImportResult, BlockImportResult, Engine};
use networking::Status;
use parking_lot::{Mutex, RwLock};
use protocol::{Checkpoint, SignedBlock, SignedVote, Slot, ValidatorIndex};
use storage::HeadInfo;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{instrument, warn};
use types::{Bytes32, Bytes4000};

use super::cache::ChainSnapshot;
use super::error::ChainError;
use super::tick;

/// Handle to the running tick task: the spawned `JoinHandle` and the
/// `CancellationToken` that triggers its loop exit. Held as
/// `Mutex<Option<TickHandle>>` so the two fields are always in lockstep
/// (both present while running, both gone after `stop`).
struct TickHandle {
    task: JoinHandle<()>,
    cancel: CancellationToken,
}

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
    tick: Mutex<Option<TickHandle>>,
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
            tick: Mutex::new(None),
        }
    }

    /// Returns a shared handle to the hot-read snapshot.
    ///
    /// Non-writer services (`runtime/api`, `runtime/p2p`) clone this
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
        let to_persist = signed.clone();
        let outcome = self.engine.import_block(signed);

        if let BlockImportResult::Accepted {
            block_root,
            head_root,
            ..
        } = &outcome
        {
            self.persist_accepted(*block_root, *head_root, to_persist)?;
            self.refresh_snapshot();
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
        let outcome = self.engine.import_attestation(signed);
        if matches!(outcome, AttestationImportResult::Accepted { .. }) {
            self.refresh_snapshot();
        }
        Ok(outcome)
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
        let produced = self.engine.produce_block(slot, validator)?;
        let signed = SignedBlock {
            message: produced.block,
            signature: Bytes4000::new([0; 4000]),
        };
        let head_root = self.engine.head();
        self.persist_accepted(produced.root, head_root, signed.clone())?;
        self.refresh_snapshot();
        Ok(signed)
    }

    /// Builds one locally authored attestation via
    /// [`Engine::produce_attestation_vote`], wraps it as a [`SignedVote`]
    /// with a zero-filled signature placeholder, and re-imports the vote
    /// locally so it lands in the engine's `latest_known_votes` pool.
    ///
    /// The local re-import is load-bearing: without it, this validator's
    /// own attestations only reach peers via gossip, and the next produced
    /// block would omit them — quorum on a small devnet can stall. Mirror
    /// of the upstream Go fix at `lean-go/runtime/chain/service.go`
    /// (`PR105 Phase 8`).
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
        // and the engine returns `Rejected`. lean-go behaves the same and
        // warn-logs; we mirror that and continue.
        let outcome = self.engine.import_attestation(signed.clone());
        if matches!(outcome, AttestationImportResult::Rejected { .. }) {
            warn!(
                ?outcome,
                slot = slot.get(),
                validator = validator.get(),
                "own-attestation re-import rejected (vote still propagates to peers)",
            );
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

    /// One-shot persistence sweep for an accepted block. All three
    /// `save_*` calls run before the snapshot refresh so a partial
    /// failure leaves storage maximally consistent with what was
    /// recorded.
    fn persist_accepted(
        &self,
        block_root: Bytes32,
        head_root: Bytes32,
        signed: SignedBlock,
    ) -> Result<(), ChainError> {
        let (post_state_opt, head_slot_opt, finalized) = self.engine.with_store(|s| {
            (
                s.state(&block_root).cloned(),
                s.block(&head_root).map(|b| b.slot),
                s.latest_finalized(),
            )
        });
        let post_state = post_state_opt.ok_or(ChainError::PostStateMissing { block_root })?;
        let head_slot = head_slot_opt.unwrap_or(Slot::ZERO);

        self.store.save_block(block_root, signed)?;
        self.store.save_state(block_root, post_state)?;
        self.store.save_head(HeadInfo::new(
            Checkpoint::new(head_root, head_slot),
            finalized,
        ))?;
        Ok(())
    }
}

impl Drop for Service {
    /// Best-effort cleanup if a caller drops the service without going
    /// through [`runtime_core::Service::stop`]: cancel the tick token so
    /// the spawned task exits on its next iteration. We cannot await the
    /// join here, so the task detaches; cancellation guarantees it does
    /// not loop forever holding `Arc` clones of the snapshot and engine.
    fn drop(&mut self) {
        // `get_mut` skips locking: `&mut self` proves no aliasing.
        if let Some(handle) = self.tick.get_mut().take() {
            handle.cancel.cancel();
        }
    }
}

#[async_trait]
impl runtime_core::Service for Service {
    fn name(&self) -> &'static str {
        "chain"
    }

    #[instrument(level = "info", name = "chain.start", skip_all, err)]
    async fn start(&self) -> anyhow::Result<()> {
        let mut slot = self.tick.lock();
        if slot.is_some() {
            return Err(anyhow!("chain service is already running"));
        }
        let cancel = CancellationToken::new();
        let task = tokio::spawn(tick::run_tick_loop(
            self.engine.clone(),
            Arc::clone(&self.snapshot),
            cancel.clone(),
        ));
        *slot = Some(TickHandle { task, cancel });
        Ok(())
    }

    #[instrument(level = "info", name = "chain.stop", skip_all, err)]
    async fn stop(&self, cancel: CancellationToken) -> anyhow::Result<()> {
        let Some(TickHandle {
            mut task,
            cancel: tick_cancel,
        }) = self.tick.lock().take()
        else {
            return Ok(());
        };
        tick_cancel.cancel();

        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                task.abort();
                // Drain so the task fully transitions; the `JoinError::Cancelled`
                // it produces here is expected and discarded.
                let _ = task.await;
                Err(anyhow!("chain tick task did not stop within shutdown budget"))
            }
            join = &mut task => {
                join.context("chain tick task panicked")?;
                Ok(())
            }
        }
    }

    async fn status(&self) -> anyhow::Result<()> {
        match self.tick.lock().as_ref() {
            None => Err(anyhow!("chain service is not running")),
            Some(h) if h.task.is_finished() => Err(anyhow!("chain tick task exited prematurely")),
            Some(_) => Ok(()),
        }
    }
}

// Adapter `impl` blocks for the Tier-6 services that drive this
// chain Service live in the consuming crates (orphan rule: each
// trait is defined in the same crate as its adapter):
//   - `runtime-sync::chain_adapter`    impl sync::Chain for Service
//   - `runtime-duties::chain_adapter`  impl duties::Chain for Service
