//! [`HttpService`] — `lean_core::Service` impl driving the Lean
//! HTTP API.
//!
//! Lifecycle mirrors `runtime/p2p::P2pService`'s state machine: build
//! → `Idle`, `start` binds the listener and spawns the serve task →
//! `Running`, `stop` cancels the token and joins the task → `Stopped`.
//! On a caller-supplied cancel deadline during `stop`, the join handle
//! is aborted to guarantee no orphaned task outlives `stop`.

use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use lean_core::Service;
use storage::Store;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use super::head;
use crate::server::EndpointServer;

/// HTTP service serving the runtime's head endpoints.
///
/// Constructed with an `Arc<dyn storage::Store>` and a listen address;
/// `start` resolves the OS-assigned port (when `:0` is requested) and
/// exposes it via [`Self::bound_addr`]. The service is single-shot per
/// instance: a `Stopped` service does not transition back to `Idle`.
pub struct HttpService {
    store: Arc<dyn Store>,
    server: EndpointServer,
}

impl HttpService {
    /// Constructs a service that will bind `listen_addr` at `start` and
    /// serve head queries against `store`.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, listen_addr: SocketAddr) -> Self {
        Self {
            store,
            server: EndpointServer::new("http", listen_addr),
        }
    }

    /// Returns the listener's resolved address while the service is
    /// `Running`. Equal to `listen_addr` when a concrete port was
    /// configured, or the OS-assigned address when `:0` was used.
    /// Returns `None` before `start` and after `stop`.
    #[must_use]
    pub fn bound_addr(&self) -> Option<SocketAddr> {
        self.server.bound_addr()
    }

    fn build_router(&self) -> Router {
        head::router().with_state(Arc::clone(&self.store))
    }
}

#[async_trait]
impl Service for HttpService {
    fn name(&self) -> &'static str {
        "runtime-api-http"
    }

    #[instrument(name = "http.start", skip(self), fields(listen_addr = %self.server.listen_addr()))]
    async fn start(&self) -> anyhow::Result<()> {
        self.server.start(|| self.build_router()).await
    }

    #[instrument(name = "http.stop", skip_all, fields(listen_addr = %self.server.listen_addr()))]
    async fn stop(&self, cancel: CancellationToken) -> anyhow::Result<()> {
        self.server.stop(cancel).await
    }

    async fn status(&self) -> anyhow::Result<()> {
        self.server.status()
    }
}
