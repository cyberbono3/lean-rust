//! Engine-level error enum.
//!
//! Engine methods that mutate the store funnel forkchoice and state-transition
//! failures through this enum. Public `import_*` methods bury errors inside
//! the `Rejected` arm of their respective result type; `produce_*` methods
//! return `Result<_, EngineError>` directly.

use forkchoice::ForkchoiceError;
use protocol::StateTransitionError;

/// Failure surface exposed by [`crate::Engine`].
///
/// Engine never raises its own variants — both inner enums already cover the
/// states encountered along the import / produce paths. `#[from]` keeps the
/// boundary thin: callers pattern-match on the inner enum when they need
/// fine-grained discrimination.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EngineError {
    /// Forwarded from the underlying [`forkchoice::Store`].
    #[error(transparent)]
    Forkchoice(#[from] ForkchoiceError),

    /// Forwarded from [`protocol::State::state_transition`].
    #[error(transparent)]
    StateTransition(#[from] StateTransitionError),
}
