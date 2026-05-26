//! [`Engine`] type — the consensus execution boundary.
//!
//! Wraps [`forkchoice::Store`] in an `Arc<parking_lot::Mutex<_>>` so the engine
//! is `Send + Sync + Clone`: a cheap (refcount-bump) clone yields another
//! handle pointing at the same store. This is the canonical Rust shape for a
//! shareable mutable resource (compare `reqwest::Client`, `sqlx::Pool`).
//!
//! The engine surface is intentionally narrow:
//! - import paths: [`Engine::import_block`] / [`Engine::import_attestation`]
//!   (in [`crate::importer`]).
//! - production: [`Engine::produce_block`] / [`Engine::produce_attestation_vote`].
//! - read-through: [`Engine::head`] / [`Engine::has_block`] /
//!   [`Engine::safe_target`] / [`Engine::latest_finalized`] /
//!   [`Engine::with_store`].
//!
//! Issue-spec callers (`lean-chain` per #28) hold the only writer handle
//! into `import_*`; read-only subsystems (`runtime-api`, `runtime-p2p`) clone
//! the engine and use the read-through accessors.

use std::sync::Arc;

use forkchoice::{ForkchoiceError, ProducedBlock, ProducedVote, Store};
use parking_lot::{Mutex, MutexGuard};
use protocol::{Block, Checkpoint, Slot, State, ValidatorIndex};
use ssz::HashTreeRoot;
use tracing::{debug, info, warn};
use types::Bytes32;

use crate::error::EngineError;

/// Consensus execution boundary around a shared [`forkchoice::Store`].
///
/// Cloning an engine returns a new handle that points at the same store; all
/// handles serialize through the single `Mutex`.
#[derive(Clone)]
pub struct Engine {
    store: Arc<Mutex<Store>>,
}

impl Engine {
    /// Builds an engine from a trusted `(state, anchor_block)` pair.
    ///
    /// # Errors
    /// Forwards every variant raised by [`Store::from_anchor`].
    pub fn from_anchor(state: State, anchor_block: Block) -> Result<Self, ForkchoiceError> {
        let slot = anchor_block.slot;
        let validators = state.config.num_validators;
        let genesis_time = state.config.genesis_time;
        let anchor_root: Bytes32 = anchor_block.hash_tree_root().into();
        let state_root: Bytes32 = state.hash_tree_root().into();

        match Store::from_anchor(state, anchor_block) {
            Ok(store) => {
                info!(
                    slot = slot.get(),
                    validators,
                    genesis_time,
                    anchor_root = %anchor_root.to_hex(),
                    state_root = %state_root.to_hex(),
                    head_root = %store.head().to_hex(),
                    safe_target_root = %store.safe_target().to_hex(),
                    "engine constructed from anchor",
                );
                Ok(Self::wrap_store(store))
            }
            Err(err) => {
                warn!(
                    slot = slot.get(),
                    validators,
                    genesis_time,
                    anchor_root = %anchor_root.to_hex(),
                    state_root = %state_root.to_hex(),
                    %err,
                    "engine anchor rejected",
                );
                Err(err)
            }
        }
    }

    /// Builds an engine around an already-constructed [`Store`].
    #[must_use]
    pub fn from_store(store: Store) -> Self {
        debug!(
            head_root = %store.head().to_hex(),
            safe_target_root = %store.safe_target().to_hex(),
            "engine constructed from store",
        );
        Self::wrap_store(store)
    }

    fn wrap_store(store: Store) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
        }
    }

    /// Snapshots the canonical head root.
    #[must_use]
    pub fn head(&self) -> Bytes32 {
        self.lock().head()
    }

    /// Snapshots the safe attestation target root.
    #[must_use]
    pub fn safe_target(&self) -> Bytes32 {
        self.lock().safe_target()
    }

    /// Snapshots the latest finalized checkpoint.
    #[must_use]
    pub fn latest_finalized(&self) -> Checkpoint {
        self.lock().latest_finalized()
    }

    /// Reports whether `root` is tracked by the store.
    #[must_use]
    pub fn has_block(&self, root: &Bytes32) -> bool {
        self.lock().has_block(root)
    }

    /// Runs `f` with a shared reference to the locked store and returns its
    /// result. Use for read-only operations not covered by a dedicated
    /// accessor; the closure runs under the mutex, so keep it short.
    pub fn with_store<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Store) -> R,
    {
        f(&self.lock())
    }

    /// Delegates to [`Store::produce_block`].
    ///
    /// # Errors
    /// Forwards every variant raised by [`Store::produce_block`] (e.g.
    /// [`ForkchoiceError::UnauthorizedProposer`], [`ForkchoiceError::HeadStateNotFound`],
    /// or state-transition failures) via [`EngineError::Forkchoice`].
    pub fn produce_block(
        &self,
        slot: Slot,
        validator: ValidatorIndex,
    ) -> Result<ProducedBlock, EngineError> {
        match self.lock().produce_block(slot, validator) {
            Ok(produced) => {
                info!(
                    slot = slot.get(),
                    validator = validator.get(),
                    parent_root = %produced.parent_root.to_hex(),
                    block_root = %produced.root.to_hex(),
                    post_state_root = %produced.post_state_root.to_hex(),
                    attestations = produced.block.body.attestations.len(),
                    "engine block produced",
                );
                Ok(produced)
            }
            Err(err) => {
                warn!(
                    slot = slot.get(),
                    validator = validator.get(),
                    %err,
                    "engine block production failed",
                );
                Err(EngineError::from(err))
            }
        }
    }

    /// Delegates to [`Store::produce_attestation_vote`].
    ///
    /// # Errors
    /// Forwards every variant raised by [`Store::produce_attestation_vote`]
    /// via [`EngineError::Forkchoice`].
    pub fn produce_attestation_vote(&self, slot: Slot) -> Result<ProducedVote, EngineError> {
        match self.lock().produce_attestation_vote(slot) {
            Ok(produced) => {
                debug!(
                    slot = slot.get(),
                    head_root = %produced.head_root.to_hex(),
                    target_slot = produced.target.slot.get(),
                    target_root = %produced.target.root.to_hex(),
                    source_slot = produced.source.slot.get(),
                    source_root = %produced.source.root.to_hex(),
                    safe_target_root = %produced.safe_target.to_hex(),
                    "engine attestation vote produced",
                );
                Ok(produced)
            }
            Err(err) => {
                warn!(
                    slot = slot.get(),
                    %err,
                    "engine attestation vote production failed",
                );
                Err(EngineError::from(err))
            }
        }
    }

    /// Advances the forkchoice clock by one interval.
    ///
    /// `has_proposal` is the spec parameter to `Store::tick_interval`:
    /// `true` when the local node is the proposer for the slot that begins
    /// at this interval and has already gossiped a block, `false` otherwise.
    ///
    /// Mutating call — held under the engine mutex like the `import_*`
    /// paths. Reserved for `lean-chain` (the only writer); other
    /// subsystems clone the engine for read-through accessors.
    ///
    /// # Errors
    /// Forwards every variant raised by [`Store::tick_interval`] via
    /// [`EngineError::Forkchoice`].
    pub fn tick_interval(&self, has_proposal: bool) -> Result<(), EngineError> {
        let mut store = self.lock();
        match store.tick_interval(has_proposal) {
            Ok(()) => {
                debug!(
                    has_proposal,
                    head_root = %store.head().to_hex(),
                    safe_target_root = %store.safe_target().to_hex(),
                    "engine tick advanced",
                );
                Ok(())
            }
            Err(err) => {
                warn!(has_proposal, %err, "engine tick failed");
                Err(EngineError::from(err))
            }
        }
    }

    /// Acquires the store lock for the duration of the returned guard.
    ///
    /// Crate-private: external callers go through the public accessors or
    /// [`Self::with_store`]. The importer module uses this to take the lock
    /// once and hold it across the full import flow.
    pub(crate) fn lock(&self) -> MutexGuard<'_, Store> {
        self.store.lock()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use ssz::HashTreeRoot;
    use static_assertions::assert_impl_all;

    use crate::test_fixtures::{anchor_pair, engine_at_genesis};

    assert_impl_all!(Engine: Send, Sync, Clone);

    #[test]
    fn from_anchor_succeeds_at_genesis() {
        let (state, block) = anchor_pair(4);
        let anchor_root: Bytes32 = block.hash_tree_root().into();
        let engine = Engine::from_anchor(state, block).unwrap();
        assert_eq!(engine.head(), anchor_root);
        assert_eq!(engine.safe_target(), anchor_root);
        assert_eq!(
            engine.latest_finalized(),
            Checkpoint::new(anchor_root, Slot::ZERO)
        );
        assert!(engine.has_block(&anchor_root));
    }

    #[test]
    fn clone_shares_underlying_store() {
        let engine_a = engine_at_genesis(4);
        let engine_b = engine_a.clone();
        let produced = engine_a
            .produce_block(Slot::new(1), ValidatorIndex::new(1))
            .unwrap();
        assert!(engine_b.has_block(&produced.root));
        assert_eq!(engine_a.head(), engine_b.head());
    }

    #[test]
    fn produce_block_rejects_unauthorized_proposer() {
        let engine = engine_at_genesis(4);
        let err = engine
            .produce_block(Slot::new(1), ValidatorIndex::new(2))
            .unwrap_err();
        assert!(matches!(
            err,
            EngineError::Forkchoice(ForkchoiceError::UnauthorizedProposer { .. })
        ));
    }

    #[test]
    fn produce_attestation_vote_at_slot_1() {
        let engine = engine_at_genesis(4);
        let produced = engine.produce_attestation_vote(Slot::new(1)).unwrap();
        assert_eq!(produced.vote.slot, Slot::new(1));
        assert_eq!(produced.head_root, engine.head());
    }

    #[test]
    fn with_store_runs_closure_under_lock() {
        let engine = engine_at_genesis(4);
        assert_eq!(engine.with_store(Store::head), engine.head());
    }
}
