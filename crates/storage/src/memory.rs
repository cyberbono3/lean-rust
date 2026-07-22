//! In-memory [`Store`] adapter.
//!
//! [`MemoryStore`] keeps blocks, post-states, and the canonical-chain view
//! behind one [`parking_lot::RwLock`] so concurrent readers parallelize and
//! writers exclude. All operations are infallible — the adapter never
//! returns [`StorageError`].

use std::collections::{BTreeMap, HashMap};

use parking_lot::RwLock;
use protocol::{SignedBlockWithAttestation, State, ValidatorIndex};
use types::{Bytes32, OtsWatermark};

use crate::error::StorageError;
use crate::store::{HeadInfo, Store};

/// In-memory persistence adapter. Construct with [`Self::new`] or
/// [`Default::default`]; share across services via `Arc<MemoryStore>` or
/// `Arc<dyn Store>`.
pub struct MemoryStore {
    inner: RwLock<Inner>,
}

#[derive(Default)]
struct Inner {
    blocks: HashMap<Bytes32, SignedBlockWithAttestation>,
    states: HashMap<Bytes32, State>,
    head: Option<HeadInfo>,
    // One OTS key-state per validator; `BTreeMap` matches the local signer's
    // keyset ordering and keeps records independent across validators.
    ots_key_states: BTreeMap<ValidatorIndex, OtsWatermark>,
}

impl MemoryStore {
    /// Constructs an empty in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner::default()),
        }
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Store for MemoryStore {
    fn save_block(
        &self,
        root: Bytes32,
        block: SignedBlockWithAttestation,
    ) -> Result<(), StorageError> {
        self.inner.write().blocks.insert(root, block);
        Ok(())
    }

    fn save_state(&self, root: Bytes32, state: State) -> Result<(), StorageError> {
        self.inner.write().states.insert(root, state);
        Ok(())
    }

    fn save_head(&self, info: HeadInfo) -> Result<(), StorageError> {
        self.inner.write().head = Some(info);
        Ok(())
    }

    /// Atomic override: block, state, and head all land under one `write()`
    /// acquisition, so concurrent readers never observe a head that points at
    /// a not-yet-inserted block or state.
    fn save_accepted(
        &self,
        block_root: Bytes32,
        block: SignedBlockWithAttestation,
        state: State,
        head: HeadInfo,
    ) -> Result<(), StorageError> {
        let mut inner = self.inner.write();
        inner.blocks.insert(block_root, block);
        inner.states.insert(block_root, state);
        inner.head = Some(head);
        Ok(())
    }

    fn has_block(&self, root: &Bytes32) -> Result<bool, StorageError> {
        Ok(self.inner.read().blocks.contains_key(root))
    }

    fn has_state(&self, root: &Bytes32) -> Result<bool, StorageError> {
        Ok(self.inner.read().states.contains_key(root))
    }

    fn load_block(
        &self,
        root: &Bytes32,
    ) -> Result<Option<SignedBlockWithAttestation>, StorageError> {
        Ok(self.inner.read().blocks.get(root).cloned())
    }

    fn load_state(&self, root: &Bytes32) -> Result<Option<State>, StorageError> {
        Ok(self.inner.read().states.get(root).cloned())
    }

    fn load_head(&self) -> Result<Option<HeadInfo>, StorageError> {
        Ok(self.inner.read().head)
    }

    fn save_ots_key_state(
        &self,
        validator: ValidatorIndex,
        watermark: OtsWatermark,
    ) -> Result<(), StorageError> {
        self.inner
            .write()
            .ots_key_states
            .insert(validator, watermark);
        Ok(())
    }

    fn load_ots_key_state(
        &self,
        validator: ValidatorIndex,
    ) -> Result<Option<OtsWatermark>, StorageError> {
        Ok(self.inner.read().ots_key_states.get(&validator).cloned())
    }
}
