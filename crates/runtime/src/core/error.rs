//! Lifecycle error type emitted by [`Node`](crate::core::Node).
//!
//! Each variant preserves the slot label of the offending service so
//! callers can route on it without unpacking the service-defined error
//! (which is type-erased through [`anyhow::Error`]).

use thiserror::Error;

/// A single service's failure: slot label paired with the service-defined
/// error.
///
/// Used as the inner payload for [`NodeError::StartFailed`],
/// [`NodeError::StopFailed`], and the per-service items in
/// [`NodeError::ServicesUnhealthy`]. Implements [`std::error::Error`] via
/// `#[derive(Error)]` so it participates in `?` chains and
/// [`std::error::Error::source`] walks.
#[derive(Debug, Error)]
#[error("service '{service}': {source}")]
pub struct ServiceFailure {
    /// Slot label of the offending service (`"chain"`, `"p2p"`, …).
    pub service: &'static str,
    /// The underlying service-defined error.
    #[source]
    pub source: anyhow::Error,
}

/// Errors raised by [`Node`](crate::core::Node) lifecycle transitions.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NodeError {
    /// A service's `start` method returned an error during `Node::start`.
    /// Already-started services are unwound before this error is returned.
    #[error("failed to start: {0}")]
    StartFailed(#[source] ServiceFailure),

    /// A service's `stop` method returned an error during `Node::stop`.
    /// Best-effort: subsequent services in the reverse-stop sequence are
    /// still attempted; only the first failure surfaces here.
    #[error("failed to stop: {0}")]
    StopFailed(#[source] ServiceFailure),

    /// One or more services reported unhealthy in `Node::status`.
    #[error(
        "node status reports {} unhealthy service(s): {}",
        errors.len(),
        errors.iter().map(|f| f.service).collect::<Vec<_>>().join(", ")
    )]
    ServicesUnhealthy {
        /// Per-service failures, in startup order.
        errors: Vec<ServiceFailure>,
    },

    /// `Node::start` was called on an already-running node.
    #[error("node is already started")]
    AlreadyStarted,
}
