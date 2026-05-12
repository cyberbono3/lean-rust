//! Process-level configuration for the runtime shell.

use std::time::Duration;

/// Default graceful-shutdown budget per service.
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Narrow configuration for the runtime [`Node`](crate::Node).
///
/// Only fields needed by the lifecycle spine itself live here. Service-
/// specific configuration (HTTP port, libp2p options, …) is owned by the
/// individual service implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeConfig {
    /// Graceful shutdown budget per service.
    ///
    /// [`Duration::ZERO`] disables the deadline: services may take as long
    /// as they need. Non-zero values bound the wait via
    /// [`tokio::time::timeout`].
    pub shutdown_timeout: Duration,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }
}
