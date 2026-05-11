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
//!   [`Engine::safe_target`] / [`Engine::with_store`].
//!
//! Issue-spec callers (`runtime-chain` per #28) hold the only writer handle
//! into `import_*`; read-only subsystems (`runtime-api`, `runtime-p2p`) clone
//! the engine and use the read-through accessors.

use std::sync::Arc;

use forkchoice::{ForkchoiceError, ProducedBlock, ProducedVote, Store};
use parking_lot::Mutex;
use protocol::{Block, Slot, State, ValidatorIndex};
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
        let store = Store::from_anchor(state, anchor_block)?;
        Ok(Self::from_store(store))
    }

    /// Builds an engine around an already-constructed [`Store`].
    #[must_use]
    pub fn from_store(store: Store) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
        }
    }

    /// Snapshots the canonical head root.
    #[must_use]
    pub fn head(&self) -> Bytes32 {
        self.store.lock().head()
    }

    /// Snapshots the safe attestation target root.
    #[must_use]
    pub fn safe_target(&self) -> Bytes32 {
        self.store.lock().safe_target()
    }

    /// Reports whether `root` is tracked by the store.
    #[must_use]
    pub fn has_block(&self, root: &Bytes32) -> bool {
        self.store.lock().has_block(root)
    }

    /// Runs `f` with a shared reference to the locked store.
    ///
    /// Use this for read-only operations not covered by a dedicated accessor.
    /// The closure runs under the mutex, so keep it short and avoid blocking
    /// I/O inside `f`.
    pub fn with_store<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Store) -> R,
    {
        f(&self.store.lock())
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
        Ok(self.store.lock().produce_block(slot, validator)?)
    }

    /// Delegates to [`Store::produce_attestation_vote`].
    ///
    /// # Errors
    /// Forwards every variant raised by [`Store::produce_attestation_vote`]
    /// via [`EngineError::Forkchoice`].
    pub fn produce_attestation_vote(&self, slot: Slot) -> Result<ProducedVote, EngineError> {
        Ok(self.store.lock().produce_attestation_vote(slot)?)
    }

    /// Internal: returns a clone of the `Arc<Mutex<Store>>` so the importer
    /// module can acquire the same lock as the public API without exposing
    /// the inner type.
    pub(crate) fn store_handle(&self) -> &Mutex<Store> {
        &self.store
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use static_assertions::assert_impl_all;

    use crate::test_fixtures::{anchor_pair, engine_at_genesis};

    assert_impl_all!(Engine: Send, Sync, Clone);

    #[test]
    fn from_anchor_succeeds_at_genesis() {
        let (state, block) = anchor_pair(4);
        let anchor_root: Bytes32 = {
            use ssz::HashTreeRoot;
            block.hash_tree_root().into()
        };
        let engine = Engine::from_anchor(state, block).unwrap();
        assert_eq!(engine.head(), anchor_root);
        assert_eq!(engine.safe_target(), anchor_root);
        assert!(engine.has_block(&anchor_root));
    }

    #[test]
    fn clone_shares_underlying_store() {
        let engine_a = engine_at_genesis(4);
        let engine_b = engine_a.clone();
        // Produce a block via handle A; handle B observes it.
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
        let head = engine.with_store(forkchoice::Store::head);
        assert_eq!(head, engine.head());
    }
}
