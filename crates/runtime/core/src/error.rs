//! Lifecycle error type emitted by [`Node`](crate::Node).

use thiserror::Error;

/// Errors raised by [`Node`](crate::Node) lifecycle transitions.
///
/// Each failure carries the slot label (`"chain"`, `"p2p"`, …) so callers
/// can match on which service in the composition root misbehaved without
/// type-erasing the underlying service-defined error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NodeError {
    /// A service's `start` method returned an error during `Node::start`.
    /// Already-started services are unwound before this error is returned.
    #[error("service '{service}' failed to start: {source}")]
    Start {
        /// Slot label of the offending service.
        service: &'static str,
        /// The underlying service-defined error.
        #[source]
        source: anyhow::Error,
    },

    /// A service's `stop` method returned an error during `Node::stop`.
    /// Best-effort: subsequent services in the reverse-stop sequence are
    /// still attempted; only the first failure surfaces here.
    #[error("service '{service}' failed to stop: {source}")]
    Stop {
        /// Slot label of the offending service.
        service: &'static str,
        /// The underlying service-defined error.
        #[source]
        source: anyhow::Error,
    },

    /// One or more services reported unhealthy in `Node::status`.
    #[error("node status reports {count} unhealthy service(s): {summary}")]
    Status {
        /// Number of unhealthy services.
        count: usize,
        /// Comma-joined list of unhealthy slot labels (for `Display`).
        summary: String,
        /// Per-service errors with their slot labels, in startup order.
        errors: Vec<(&'static str, anyhow::Error)>,
    },

    /// `Node::start` was called on an already-running node.
    #[error("node is already started")]
    AlreadyStarted,
}
