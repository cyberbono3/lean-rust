//! The [`Service`] async trait — the common lifecycle contract for every
//! runtime service.

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

/// Common lifecycle contract implemented by runtime services
/// (`chain`, `p2p`, `sync`, `duties`, `http`, `metrics`).
///
/// Implementations must be `Send + Sync + 'static` so a composition root
/// can hold them as `Arc<dyn Service>` without lifetime juggling. Per-
/// implementation typed errors are erased through [`anyhow::Error`]; the
/// composition root re-attaches the slot label (see
/// [`NodeError`](crate::NodeError)).
///
/// # Cancellation
///
/// `stop` receives a [`CancellationToken`] armed by the node with the
/// configured shutdown budget. Services that block on long-running work
/// should observe the token and short-circuit themselves; the node
/// additionally wraps each call in [`tokio::time::timeout`] so a
/// misbehaving service cannot block shutdown indefinitely.
///
/// # Example
/// ```
/// use std::sync::Arc;
/// use anyhow::Result;
/// use async_trait::async_trait;
/// use runtime_core::Service;
/// use tokio_util::sync::CancellationToken;
///
/// struct Noop;
///
/// #[async_trait]
/// impl Service for Noop {
///     fn name(&self) -> &'static str {
///         "noop"
///     }
///     async fn start(&self) -> Result<()> {
///         Ok(())
///     }
///     async fn stop(&self, _cancel: CancellationToken) -> Result<()> {
///         Ok(())
///     }
///     async fn status(&self) -> Result<()> {
///         Ok(())
///     }
/// }
///
/// // Witness: the trait is object-safe.
/// let _: Arc<dyn Service> = Arc::new(Noop);
/// ```
#[async_trait]
pub trait Service: Send + Sync + 'static {
    /// Service-chosen identifier used for log spans and structured fields.
    ///
    /// Independent from the slot label assigned by
    /// [`Node`](crate::Node) — services name themselves; the node names
    /// their slot.
    fn name(&self) -> &'static str;

    /// Brings the service up.
    ///
    /// # Errors
    /// Any failure that prevents the service from operating. The node
    /// unwinds previously-started services before propagating the error.
    async fn start(&self) -> anyhow::Result<()>;

    /// Brings the service down.
    ///
    /// `cancel` fires when the node's shutdown deadline elapses. Long-
    /// running teardown should select against `cancel.cancelled()` and
    /// short-circuit when it triggers.
    ///
    /// # Errors
    /// Any failure that prevents clean shutdown. The node continues
    /// stopping the remaining services and surfaces the first error.
    async fn stop(&self, cancel: CancellationToken) -> anyhow::Result<()>;

    /// Reports whether the service is healthy.
    ///
    /// # Errors
    /// Any condition that should fail an aggregated health check.
    async fn status(&self) -> anyhow::Result<()>;
}
