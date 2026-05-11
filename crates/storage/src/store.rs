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
