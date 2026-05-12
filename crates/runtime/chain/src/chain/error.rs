//! Error type for the chain [`Service`](super::Service).

use thiserror::Error;
use types::Bytes32;

/// Failures raised by the chain service for infrastructure-level problems.
///
/// Logical import outcomes (`Accepted` / `DuplicateBlock` / `MissingParent`
/// / `Rejected`) are *not* errors — they flow through the import-result
/// sum types. `ChainError` is reserved for storage failures, engine-state
/// invariant violations, and forkchoice tick errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ChainError {
    /// A `storage::Store` call failed.
    #[error("storage: {0}")]
    Storage(#[from] storage::StorageError),

    /// The engine reported `Accepted` for `block_root` but the post-state
    /// was missing on the immediate follow-up read. Indicates an engine
    /// invariant violation, not a normal operating condition.
    #[error("post-state missing in engine after accepted import: block_root={block_root:?}")]
    PostStateMissing {
        /// Root of the block whose post-state could not be re-fetched.
        block_root: Bytes32,
    },

    /// `Engine::tick_interval` returned an error from the underlying
    /// forkchoice clock advance.
    #[error("engine tick: {0}")]
    Tick(#[from] engine::EngineError),
}
