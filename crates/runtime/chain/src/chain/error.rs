//! Error type for the chain [`Service`](super::Service).

use crate::engine::EngineError;
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
/// Forkchoice tick failures are deliberately *not* part of this enum: the
/// tick loop logs and continues (see [`super::tick::run_tick_loop`]). If
/// a future revision needs to escalate consecutive tick failures, add the
/// variant in the same PR that introduces the escalation policy.
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
}
