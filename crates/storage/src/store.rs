//! Persistence trait + canonical-chain view exposed to runtime callers.

use protocol::{Checkpoint, SignedBlock, State};
use types::Bytes32;

use crate::error::StorageError;

/// Persisted canonical-chain view: the current `head` checkpoint and the
/// latest `finalized` checkpoint observed by the runtime.
///
/// All fields are `Copy`, so [`HeadInfo`] itself is `Copy` and round-trips
/// through the trait without clones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct HeadInfo {
    /// Current canonical head checkpoint.
    pub head: Checkpoint,
    /// Latest finalized checkpoint.
    pub finalized: Checkpoint,
}

impl HeadInfo {
    /// Constructs a [`HeadInfo`] from an explicit `(head, finalized)` pair.
    #[must_use]
    pub const fn new(head: Checkpoint, finalized: Checkpoint) -> Self {
        Self { head, finalized }
    }
}

/// Narrow persistence contract used by the runtime chain layer.
///
/// All methods take `&self`; adapters carry interior mutability via
/// [`RwLock`](parking_lot::RwLock) or equivalent. `Send + Sync` are
/// required so a single `Arc<dyn Store>` handle can be shared across
/// runtime services.
///
/// # Ownership
///
/// `save_*` methods take their payload by value — the adapter typically
/// keeps it without further allocation. `load_*` methods return owned
/// values (the adapter clones internally) so callers can use them past
/// the store's lock scope.
///
/// # Absent vs error
///
/// `load_*` and `has_block` return `Result<Option<T>, _>` /
/// `Result<bool, _>`. `Ok(None)` and `Ok(false)` mean "not present"; only
/// `Err(_)` signals a backend failure.
pub trait Store: Send + Sync {
    /// Persists `block` keyed by `root`. Overwrites any prior entry.
    ///
    /// # Errors
    /// Backend-specific failures via [`StorageError`].
    fn save_block(&self, root: Bytes32, block: SignedBlock) -> Result<(), StorageError>;

    /// Persists `state` keyed by `root`. Overwrites any prior entry.
    ///
    /// # Errors
    /// Backend-specific failures via [`StorageError`].
    fn save_state(&self, root: Bytes32, state: State) -> Result<(), StorageError>;

    /// Persists the current canonical chain view. Overwrites any prior
    /// head record.
    ///
    /// # Errors
    /// Backend-specific failures via [`StorageError`].
    fn save_head(&self, info: HeadInfo) -> Result<(), StorageError>;

    /// Atomically persists an accepted block, its post-state (both keyed by
    /// `block_root`), and the updated canonical `head` in one call.
    ///
    /// The head record is written only after the block and state succeed, so
    /// a mid-call backend failure never leaves [`Self::load_head`] pointing at
    /// a block or state that is absent from the store. This collapses the
    /// previous three-call `save_block` → `save_state` → `save_head` sequence
    /// (whose interleaving window let a crash strand the head ahead of its
    /// payload) into a single contract method.
    ///
    /// The default implementation performs the three writes in
    /// `block` → `state` → `head` order, propagating the first error with `?`.
    /// Adapters whose backend supports a single transaction (or a single lock,
    /// like [`crate::MemoryStore`]) SHOULD override this so all three writes
    /// commit together with no torn intermediate state observable to readers.
    ///
    /// # Errors
    /// Backend-specific failures via [`StorageError`]. On error the head
    /// record is guaranteed unchanged.
    fn save_accepted(
        &self,
        block_root: Bytes32,
        block: SignedBlock,
        state: State,
        head: HeadInfo,
    ) -> Result<(), StorageError> {
        self.save_block(block_root, block)?;
        self.save_state(block_root, state)?;
        self.save_head(head)?;
        Ok(())
    }

    /// Reports whether `root` is currently tracked.
    ///
    /// Adapters that can answer existence without materializing the full
    /// block (e.g. KV-store key probes) should implement this directly
    /// rather than falling back through `load_block`.
    ///
    /// # Errors
    /// Backend-specific failures via [`StorageError`].
    fn has_block(&self, root: &Bytes32) -> Result<bool, StorageError>;

    /// Resolves a persisted signed block by `root`.
    ///
    /// # Errors
    /// Backend-specific failures via [`StorageError`]. Returns
    /// `Ok(None)` for unknown roots — absence is not an error.
    fn load_block(&self, root: &Bytes32) -> Result<Option<SignedBlock>, StorageError>;

    /// Resolves a persisted post-state by `root`.
    ///
    /// # Errors
    /// Backend-specific failures via [`StorageError`]. Returns
    /// `Ok(None)` for unknown roots — absence is not an error.
    fn load_state(&self, root: &Bytes32) -> Result<Option<State>, StorageError>;

    /// Resolves the most recently persisted canonical chain view.
    ///
    /// # Errors
    /// Backend-specific failures via [`StorageError`]. Returns
    /// `Ok(None)` before the first `save_head` call.
    fn load_head(&self) -> Result<Option<HeadInfo>, StorageError>;
}
