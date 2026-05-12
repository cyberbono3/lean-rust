//! Chain [`Service`] — the single engine writer.
//!
//! Wraps [`engine::Engine`] + [`storage::Store`] and exposes async
//! `import_block` / `import_attestation`. Spawns a background tick loop
//! on `start` that advances the forkchoice clock every
//! `config::SECONDS_PER_INTERVAL`.
//!
//! # Storage / engine divergence
//!
//! `import_block` persists `(block, state, head)` synchronously inside the
//! same call that accepted the block. If a `save_*` call fails after the
//! engine has accepted the block, the engine in-memory state is ahead of
//! storage: this service surfaces [`ChainError::Storage`] and the
//! containing runtime cascade-stops. Replay-on-restart — re-feeding any
//! undrained blocks through the engine from the last persisted head — is
//! tracked separately (see issue #36); it is intentionally out of scope
//! here.

use std::sync::Arc;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use engine::{AttestationImportResult, BlockImportResult, Engine};
use parking_lot::{Mutex, RwLock};
use protocol::{Checkpoint, SignedBlock, SignedVote, Slot};
use storage::HeadInfo;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::instrument;
use types::Bytes32;

use super::cache::ChainSnapshot;
use super::error::ChainError;
use super::tick;

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
    tick_task: Mutex<Option<JoinHandle<()>>>,
    tick_cancel: Mutex<Option<CancellationToken>>,
}

impl Service {
    /// Builds a service around `engine` and `store`. The initial snapshot
    /// is seeded from a single engine-lock acquisition so callers that
    /// clone the snapshot before `start` observe consistent state.
    #[must_use]
    pub fn new(engine: Engine, store: Arc<dyn storage::Store>) -> Self {
        let mut initial = ChainSnapshot::default();
        initial.refresh(&engine);
        Self {
            engine,
            store,
            snapshot: Arc::new(RwLock::new(initial)),
            tick_task: Mutex::new(None),
            tick_cancel: Mutex::new(None),
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
            self.snapshot.write().refresh(&self.engine);
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
            self.snapshot.write().refresh(&self.engine);
        }
        Ok(outcome)
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

#[async_trait]
impl runtime_core::Service for Service {
    fn name(&self) -> &'static str {
        "chain"
    }

    async fn start(&self) -> anyhow::Result<()> {
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(tick::run_tick_loop(
            self.engine.clone(),
            Arc::clone(&self.snapshot),
            cancel.clone(),
        ));
        *self.tick_cancel.lock() = Some(cancel);
        *self.tick_task.lock() = Some(handle);
        Ok(())
    }

    async fn stop(&self, _cancel: CancellationToken) -> anyhow::Result<()> {
        if let Some(c) = self.tick_cancel.lock().take() {
            c.cancel();
        }
        let handle = self.tick_task.lock().take();
        if let Some(h) = handle {
            h.await.context("chain tick task panicked")?;
        }
        Ok(())
    }

    async fn status(&self) -> anyhow::Result<()> {
        let task = self.tick_task.lock();
        match task.as_ref() {
            Some(h) if h.is_finished() => Err(anyhow!("chain tick task exited prematurely")),
            Some(_) => Ok(()),
            None => Err(anyhow!("chain service not started")),
        }
    }
}
