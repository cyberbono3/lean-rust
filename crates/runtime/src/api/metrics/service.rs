//! [`MetricsService`] — `crate::core::Service` impl driving the
//! Prometheus metrics endpoint.
//!
//! Lifecycle mirrors `lean-p2p-host::P2pService`'s state machine: build
//! → `Idle`, `start` binds the listener and spawns the serve task →
//! `Running`, `stop` cancels the token and joins the task → `Stopped`.

use std::net::SocketAddr;

use crate::core::Service;
use async_trait::async_trait;
use axum::Router;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use super::{prometheus, FrozenRecorder};
use crate::api::server::EndpointServer;

/// HTTP service serving Prometheus text exposition on `/metrics`.
///
/// Constructed with a [`Recorder`] and a listen address; `start`
/// resolves the OS-assigned port when `:0` is requested and exposes it
/// via [`Self::bound_addr`]. The service is single-shot per instance: a
/// `Stopped` service does not transition back to `Idle`.
pub struct MetricsService {
    recorder: FrozenRecorder,
    server: EndpointServer,
}

impl MetricsService {
    /// Constructs a service that will bind `listen_addr` at `start` and
    /// serve metrics from the frozen `recorder`.
    #[must_use]
    pub fn new(listen_addr: SocketAddr, recorder: FrozenRecorder) -> Self {
        Self {
            recorder,
            server: EndpointServer::new("metrics", listen_addr),
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
        prometheus::router(self.recorder.clone())
    }
}

#[async_trait]
impl Service for MetricsService {
    fn name(&self) -> &'static str {
        "lean-api-metrics"
    }

    #[instrument(name = "metrics.start", skip(self), fields(listen_addr = %self.server.listen_addr()))]
    async fn start(&self) -> anyhow::Result<()> {
        self.server.start(|| self.build_router()).await
    }

    #[instrument(name = "metrics.stop", skip_all, fields(listen_addr = %self.server.listen_addr()))]
    async fn stop(&self, cancel: CancellationToken) -> anyhow::Result<()> {
        self.server.stop(cancel).await
    }

    async fn status(&self) -> anyhow::Result<()> {
        self.server.status()
    }
}
