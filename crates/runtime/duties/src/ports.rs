//! Port traits consumed by the duties [`Service`](super::Service).
//!
//! Following Decision 7 (Dependency Inversion): this module declares the
//! trait surface; impls live elsewhere.
//!
//! - [`Chain`] is satisfied by [`runtime_chain::Service`] via the
//!   adapter `impl` in [`crate::chain_adapter`]. Tests in this crate
//!   use in-memory fakes.
//! - [`Publisher`] has no in-crate impl. The `node` crate provides the
//!   libp2p-backed adapter; tests in this crate use an in-memory
//!   `MockPublisher` test double (defined in `tests/scheduler.rs`).

use async_trait::async_trait;
use protocol::{SignedBlock, SignedVote, Slot, ValidatorIndex};
use runtime_chain::ChainError;
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
/// `Send + Sync + 'static` because the scheduler holds the chain port as
/// `Arc<dyn Chain>` and shares it with the spawned worker task.
#[async_trait]
pub trait Chain: Send + Sync + 'static {
    /// Builds a locally authored block for `slot` proposed by
    /// `validator`. See [`runtime_chain::Service::produce_block`] for the
    /// concrete persistence + state-refresh contract.
    ///
    /// # Errors
    /// Forwards every [`ChainError`] raised by the underlying service.
    async fn produce_block(
        &self,
        slot: Slot,
        validator: ValidatorIndex,
    ) -> Result<SignedBlock, ChainError>;

    /// Builds a locally authored attestation for `slot` by `validator`.
    /// See [`runtime_chain::Service::produce_attestation`] for the
    /// concrete own-vote re-import contract.
    ///
    /// # Errors
    /// Forwards every [`ChainError`] raised by the underlying service.
    async fn produce_attestation(
        &self,
        slot: Slot,
        validator: ValidatorIndex,
    ) -> Result<SignedVote, ChainError>;
}

/// Outbound publish surface required by the duties scheduler.
///
/// Concrete impls live in the `node` crate (Issue #37). See the
/// module-level doc for the test-double placement.
#[async_trait]
pub trait Publisher: Send + Sync + 'static {
    /// Publishes `block` to all interested peers.
    ///
    /// # Errors
    /// Per-call transport failures surface as [`PublishError`]. The
    /// scheduler warn-logs the failure and continues — a publish error
    /// is not a service-terminal condition.
    async fn publish_block(&self, block: SignedBlock) -> Result<(), PublishError>;

    /// Publishes `vote` to all interested peers.
    ///
    /// # Errors
    /// As for [`Self::publish_block`].
    async fn publish_attestation(&self, vote: SignedVote) -> Result<(), PublishError>;
}
