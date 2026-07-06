//! Error type for the chain [`Service`](super::Service).

use crate::chain::engine::EngineError;
use thiserror::Error;
use types::Bytes32;

/// Failures raised by the chain service for infrastructure-level problems.
///
/// Logical import outcomes (`Accepted` / `DuplicateBlock` / `MissingParent`
/// / `Rejected`) are *not* errors — they flow through the import-result
/// sum types. `ChainError` is reserved for storage failures, engine-state
/// invariant violations, and production-path failures (where the engine
/// surfaces `Result<_, EngineError>` directly).
///
/// Forkchoice tick failures surface as [`ChainError::Engine`] from
/// [`super::Service::tick_interval`]; the self-driving consensus loop
/// warn-logs and continues rather than escalating a single tick failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ChainError {
    /// A `storage::Store` call failed.
    #[error("storage: {0}")]
    Storage(#[from] storage::StorageError),

    /// The engine reported `Accepted` for `block_root` but the post-state
    /// was missing on the immediate follow-up read. Indicates an engine
    /// invariant violation, not a normal operating condition.
    #[error("post-state missing in engine after accepted import: block_root={}", block_root.to_hex())]
    PostStateMissing {
        /// Root of the block whose post-state could not be re-fetched.
        block_root: Bytes32,
    },

    /// The engine refused a `produce_block` / `produce_attestation_vote`
    /// call. Surfaces the underlying [`EngineError`] (forkchoice or
    /// state-transition) — duties callers warn-log and continue.
    #[error("engine: {0}")]
    Engine(#[from] EngineError),

    /// The engine's claimed head root resolved to no block on the
    /// immediate follow-up read inside the persist sweep. Indicates a
    /// head/track race or invariant violation; refusing the persist
    /// (rather than silently writing `HeadInfo { slot: 0 }`) keeps the
    /// on-disk state consistent with the engine.
    #[error("engine claimed head root has no corresponding block: head_root={}", head_root.to_hex())]
    HeadBlockMissing {
        /// Root the engine returned from `head()` but had no block for.
        head_root: Bytes32,
    },
}
