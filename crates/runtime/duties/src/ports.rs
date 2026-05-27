//! Port traits consumed by the duties [`Service`](super::Service).
//!
//! Following Decision 7 (Dependency Inversion): this module declares the
//! trait surface; impls live elsewhere.
//!
//! - [`Chain`] is satisfied by [`lean_chain::Service`] via the
//!   adapter `impl` in [`crate::chain_adapter`]. Tests in this crate
//!   use in-memory fakes.
//! - [`Publisher`] has no in-crate impl. The `node` crate provides the
//!   libp2p-backed adapter; tests in this crate use an in-memory
//!   `MockPublisher` test double (defined in `tests/scheduler.rs`).

use std::future::Future;

use lean_chain::ChainError;
use protocol::{SignedBlock, SignedVote, Slot, ValidatorIndex};
use thiserror::Error;

/// Failure surface for [`Publisher`] implementations.
///
/// Newtype around [`anyhow::Error`]: adapters wrap their
/// transport-specific error here and the duties scheduler treats every
/// publish failure uniformly (warn-log + record in `last_err`). The
/// `#[from]` impl gives `?`-friendly conversion from `anyhow::Error`
/// — adapters use `?` or `.into()` to construct.
///
/// If future adapters need to discriminate failure modes at the type
/// level, promote this to an enum at that point — single-variant
/// `#[non_exhaustive]` enums pretend extensibility they don't have.
#[derive(Debug, Error)]
#[error(transparent)]
pub struct PublishError(#[from] anyhow::Error);

/// Narrow chain-facing production surface required by the duties
/// scheduler.
///
/// Methods are declared with native `-> impl Future + Send` (RPITIT)
/// rather than `#[async_trait]`, so a call site no longer heap-allocates
/// a boxed `Future` per invocation (#27). The scheduler is generic over
/// the concrete `Chain` impl (it holds `Arc<C>`, not `Arc<dyn Chain>`),
/// which is what makes the un-boxed native form possible. `Send` is
/// required because the scheduler drives these futures from a spawned
/// worker task; `Send + Sync + 'static` on the trait keeps the `Arc<C>`
/// shareable across that task.
pub trait Chain: Send + Sync + 'static {
    /// Builds a locally authored block for `slot` proposed by
    /// `validator`. See [`lean_chain::Service::produce_block`] for the
    /// concrete persistence + state-refresh contract.
    ///
    /// # Errors
    /// Forwards every [`ChainError`] raised by the underlying service.
    fn produce_block(
        &self,
        slot: Slot,
        validator: ValidatorIndex,
    ) -> impl Future<Output = Result<SignedBlock, ChainError>> + Send;

    /// Builds a locally authored attestation for `slot` by `validator`.
    /// See [`lean_chain::Service::produce_attestation`] for the
    /// concrete own-vote re-import contract.
    ///
    /// # Errors
    /// Forwards every [`ChainError`] raised by the underlying service.
    fn produce_attestation(
        &self,
        slot: Slot,
        validator: ValidatorIndex,
    ) -> impl Future<Output = Result<SignedVote, ChainError>> + Send;
}

/// Outbound publish surface required by the duties scheduler.
///
/// Concrete impls live in the `node` crate (Issue #37). See the
/// module-level doc for the test-double placement. Like [`Chain`], the
/// methods use native `-> impl Future + Send` to avoid a boxed-future
/// allocation per publish (#27).
pub trait Publisher: Send + Sync + 'static {
    /// Publishes `block` to all interested peers.
    ///
    /// # Errors
    /// Per-call transport failures surface as [`PublishError`]. The
    /// scheduler warn-logs the failure and continues — a publish error
    /// is not a service-terminal condition.
    fn publish_block(
        &self,
        block: SignedBlock,
    ) -> impl Future<Output = Result<(), PublishError>> + Send;

    /// Publishes `vote` to all interested peers.
    ///
    /// # Errors
    /// As for [`Self::publish_block`].
    fn publish_attestation(
        &self,
        vote: SignedVote,
    ) -> impl Future<Output = Result<(), PublishError>> + Send;
}
