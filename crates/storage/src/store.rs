//! Persistence trait + canonical-chain view exposed to runtime callers.

use protocol::{Checkpoint, SignedBlockWithAttestation, State, ValidatorIndex};
use thiserror::Error;
use types::{Bytes32, OtsWatermark};

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
    /// Constructs a [`HeadInfo`] from an explicit `(head, finalized)` pair
    /// without validation. Prefer [`Self::try_new`] at any seam that builds a
    /// `HeadInfo` from persisted or otherwise untrusted input.
    #[must_use]
    pub const fn new(head: Checkpoint, finalized: Checkpoint) -> Self {
        Self { head, finalized }
    }

    /// Validated constructor for the deserialization seam.
    ///
    /// Enforces the canonical-chain invariant `finalized.slot <= head.slot`:
    /// the finalized checkpoint is always an ancestor of (or equal to) the
    /// head, so it can never sit at a higher slot. Genesis — where `finalized`
    /// and `head` are the same zero-root checkpoint at slot 0 — and any other
    /// equal-slot pair are accepted.
    ///
    /// # Errors
    /// [`HeadInfoError::FinalizedAheadOfHead`] when `finalized.slot >
    /// head.slot`.
    pub fn try_new(head: Checkpoint, finalized: Checkpoint) -> Result<Self, HeadInfoError> {
        if finalized.slot > head.slot {
            return Err(HeadInfoError::FinalizedAheadOfHead {
                head: head.slot.get(),
                finalized: finalized.slot.get(),
            });
        }
        Ok(Self { head, finalized })
    }
}

/// Validation failure for [`HeadInfo::try_new`].
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum HeadInfoError {
    /// `finalized.slot` exceeds `head.slot`. The finalized checkpoint must be
    /// an ancestor of the head, so its slot can never be higher.
    #[error("finalized slot {finalized} is ahead of head slot {head}")]
    FinalizedAheadOfHead {
        /// Head checkpoint slot.
        head: u64,
        /// Finalized checkpoint slot (the out-of-range value).
        finalized: u64,
    },
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
    fn save_block(
        &self,
        root: Bytes32,
        block: SignedBlockWithAttestation,
    ) -> Result<(), StorageError>;

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

    /// Persists an accepted block, its post-state (both keyed by
    /// `block_root`), and the updated canonical `head` in one call, writing the
    /// head **last**.
    ///
    /// Head-last ordering is the load-bearing invariant: a mid-call backend
    /// failure never leaves [`Self::load_head`] pointing at a block or state
    /// that is absent from the store. Full atomicity (all-or-nothing across the
    /// three writes) is provided only by adapters that override this with a
    /// transaction or single lock — the default impl below is three ordered
    /// writes, not one atomic operation. This collapses the
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
        block: SignedBlockWithAttestation,
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

    /// Reports whether a post-state keyed by `root` is currently tracked.
    ///
    /// Symmetric with [`Self::has_block`]: adapters that can answer existence
    /// without materializing the full state (e.g. KV-store key probes) should
    /// implement this directly rather than falling back through
    /// [`Self::load_state`].
    ///
    /// # Errors
    /// Backend-specific failures via [`StorageError`].
    fn has_state(&self, root: &Bytes32) -> Result<bool, StorageError>;

    /// Resolves a persisted signed block by `root`.
    ///
    /// # Errors
    /// Backend-specific failures via [`StorageError`]. Returns
    /// `Ok(None)` for unknown roots — absence is not an error.
    fn load_block(
        &self,
        root: &Bytes32,
    ) -> Result<Option<SignedBlockWithAttestation>, StorageError>;

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

    /// Persists the crypto-free OTS `watermark` for `validator`.
    ///
    /// One record per validator: a later save overwrites the prior record for
    /// that same validator and leaves every other validator's record untouched.
    /// The payload is a `types`-owned byte blob ([`OtsWatermark`]) — a seed-free
    /// commitment plus the `next_index` watermark, so NO key material ever
    /// reaches the store (the seed stays in the operator's `0o600` secret file).
    /// No adapter links `crypto`, so this method never crosses the
    /// `storage`/`crypto` boundary.
    ///
    /// # Errors
    /// Backend-specific failures via [`StorageError`].
    fn save_ots_key_state(
        &self,
        validator: ValidatorIndex,
        watermark: OtsWatermark,
    ) -> Result<(), StorageError>;

    /// Resolves the persisted OTS watermark for `validator`, or `Ok(None)` when
    /// none was ever written (a fresh datadir — the normal first-boot path).
    ///
    /// Absent is not an error, mirroring [`Self::load_head`].
    ///
    /// # Errors
    /// Backend-specific failures via [`StorageError`], including a stored record
    /// that fails [`OtsWatermark`] decode.
    fn load_ots_key_state(
        &self,
        validator: ValidatorIndex,
    ) -> Result<Option<OtsWatermark>, StorageError>;
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use protocol::Slot;
    use types::Bytes32;

    use super::{Checkpoint, HeadInfo, HeadInfoError};

    fn cp(root_byte: u8, slot: u64) -> Checkpoint {
        Checkpoint::new(Bytes32::new([root_byte; 32]), Slot::new(slot))
    }

    #[test]
    fn try_new_accepts_valid_and_rejects_finalized_ahead_of_head() {
        // (head, finalized, expect_ok, label)
        let genesis = Checkpoint::new(Bytes32::zero(), Slot::new(0));
        let cases = [
            (genesis, genesis, true, "genesis: equal zero-root at slot 0"),
            (cp(0xaa, 5), cp(0xbb, 3), true, "finalized below head"),
            (
                cp(0xaa, 5),
                cp(0xbb, 5),
                true,
                "finalized equal to head slot",
            ),
            (cp(0xaa, 3), cp(0xbb, 5), false, "finalized ahead of head"),
        ];

        for (head, finalized, expect_ok, label) in cases {
            let result = HeadInfo::try_new(head, finalized);
            assert_eq!(result.is_ok(), expect_ok, "case: {label}");
            if expect_ok {
                let info = result.expect(label);
                assert_eq!(info.head, head, "case: {label}");
                assert_eq!(info.finalized, finalized, "case: {label}");
            } else {
                assert_eq!(
                    result.unwrap_err(),
                    HeadInfoError::FinalizedAheadOfHead {
                        head: head.slot.get(),
                        finalized: finalized.slot.get(),
                    },
                    "case: {label}"
                );
            }
        }
    }
}
