//! [`MetricsService`] — `runtime_core::Service` impl driving the
//! Prometheus metrics endpoint.
//!
//! Lifecycle mirrors `runtime/p2p::P2pService`'s state machine: build
//! → `Idle`, `start` binds the listener and spawns the serve task →
//! `Running`, `stop` cancels the token and joins the task → `Stopped`.

use std::net::SocketAddr;

use anyhow::anyhow;
use async_trait::async_trait;
use axum::Router;
use parking_lot::Mutex;
use runtime_core::{HttpsvcError, Server, Service};
use tokio::task::{JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;
use tracing::{debug, instrument, warn};

use super::{prometheus, Recorder};

/// `tokio::spawn` handle for the axum serve future.
type ServeJoin = JoinHandle<Result<(), HttpsvcError>>;

/// HTTP service serving Prometheus text exposition on `/metrics`.
///
/// Constructed with a [`Recorder`] and a listen address; `start`
/// resolves the OS-assigned port when `:0` is requested and exposes it
/// via [`Self::bound_addr`]. The service is single-shot per instance: a
/// `Stopped` service does not transition back to `Idle`.
pub struct MetricsService {
    recorder: Recorder,
    listen_addr: SocketAddr,
    state: Mutex<State>,
}

enum State {
    Idle,
    Running(RunningServer),
    Stopped,
    Transitioning,
}

impl State {
    fn bound_addr(&self) -> Option<SocketAddr> {
        match self {
            Self::Running(running) => Some(running.bound_addr),
            _ => None,
        }
    }

    fn take_idle(&mut self) -> anyhow::Result<()> {
        if matches!(*self, Self::Idle) {
            *self = Self::Transitioning;
            Ok(())
        } else {
            Err(anyhow!("metrics service already started"))
        }
    }

    fn install_running(&mut self, running: RunningServer) {
        *self = Self::Running(running);
    }

    fn restore_idle(&mut self) {
        *self = Self::Idle;
    }

    fn take_running(&mut self) -> Option<RunningServer> {
        match std::mem::replace(self, Self::Stopped) {
            Self::Running(running) => Some(running),
            other => {
                *self = other;
                None
            }
        }
    }

    fn status(&self) -> anyhow::Result<()> {
        match self {
            Self::Running(running) => {
                if running.is_finished() {
                    Err(anyhow!("metrics server task exited unexpectedly"))
                } else {
                    Ok(())
                }
            }
            Self::Idle => Err(anyhow!("metrics service not started")),
            Self::Stopped => Err(anyhow!("metrics service stopped")),
            Self::Transitioning => Err(anyhow!("metrics service mid-transition")),
        }
    }
}

struct RunningServer {
    cancel: CancellationToken,
    join: ServeJoin,
    bound_addr: SocketAddr,
}

impl RunningServer {
    fn spawn(server: Server, router: Router) -> Self {
        let bound_addr = server.local_addr();
        let cancel = CancellationToken::new();
        let join = tokio::spawn(server.serve(router, cancel.clone()));
        Self {
            cancel,
            join,
            bound_addr,
        }
    }

    fn cancel_and_take_join(self) -> ServeJoin {
        self.cancel.cancel();
        self.join
    }

    fn is_finished(&self) -> bool {
        self.join.is_finished()
    }
}

impl MetricsService {
    /// Constructs a service that will bind `listen_addr` at `start` and
    /// serve metrics from `recorder`.
    #[must_use]
    pub fn new(listen_addr: SocketAddr, recorder: Recorder) -> Self {
        Self {
            recorder,
            listen_addr,
            state: Mutex::new(State::Idle),
        }
    }

    /// Returns the listener's resolved address while the service is
    /// `Running`. Equal to `listen_addr` when a concrete port was
    /// configured, or the OS-assigned address when `:0` was used.
    /// Returns `None` before `start` and after `stop`.
    #[must_use]
    pub fn bound_addr(&self) -> Option<SocketAddr> {
        self.state.lock().bound_addr()
    }

    fn take_idle(&self) -> anyhow::Result<()> {
        self.state.lock().take_idle()
    }

    fn install_running(&self, running: RunningServer) {
        self.state.lock().install_running(running);
    }

    fn restore_idle(&self) {
        self.state.lock().restore_idle();
    }

    fn take_running(&self) -> Option<RunningServer> {
        self.state.lock().take_running()
    }

    fn build_router(&self) -> Router {
        prometheus::router(self.recorder.clone())
    }
}

#[async_trait]
impl Service for MetricsService {
    fn name(&self) -> &'static str {
        "runtime-api-metrics"
    }

    #[instrument(name = "metrics.start", skip(self), fields(listen_addr = %self.listen_addr))]
    async fn start(&self) -> anyhow::Result<()> {
        self.take_idle()?;

        let server = Server::bind(self.listen_addr)
            .await
            .inspect_err(|_| self.restore_idle())?;
        self.install_running(RunningServer::spawn(server, self.build_router()));
        Ok(())
    }

    #[instrument(name = "metrics.stop", skip_all, fields(listen_addr = %self.listen_addr))]
    async fn stop(&self, cancel: CancellationToken) -> anyhow::Result<()> {
        let Some(running) = self.take_running() else {
            debug!("stop called on non-running service");
            return Ok(());
        };
        let mut join = running.cancel_and_take_join();

        tokio::select! {
            res = &mut join => {
                handle_server_exit(res)
            }
            () = cancel.cancelled() => {
                warn!("shutdown cancel fired before metrics server drained; aborting");
                join.abort();
                Ok(())
            }
        }
    }

    async fn status(&self) -> anyhow::Result<()> {
        self.state.lock().status()
    }
}

fn handle_server_exit(res: Result<Result<(), HttpsvcError>, JoinError>) -> anyhow::Result<()> {
    match res {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(anyhow!("metrics server task failed: {err}")),
        Err(err) if err.is_panic() => Err(anyhow!("metrics server task panicked: {err}")),
        Err(err) => {
            debug!(%err, "metrics server task already cancelled");
            Ok(())
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn loopback() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    fn build_service() -> MetricsService {
        MetricsService::new(loopback(), Recorder::new())
    }

    async fn stop(service: &MetricsService) {
        service.stop(CancellationToken::new()).await.unwrap();
    }

    fn assert_err_contains(err: &anyhow::Error, needle: &str) {
        let msg = err.to_string();
        assert!(msg.contains(needle), "expected {needle:?} in {msg:?}");
    }

    #[tokio::test]
    async fn double_start_returns_already_started() {
        let service = build_service();
        service.start().await.unwrap();
        assert_err_contains(&service.start().await.unwrap_err(), "already started");
        stop(&service).await;
    }

    #[tokio::test]
    async fn stop_on_idle_is_noop() {
        stop(&build_service()).await;
    }

    #[tokio::test]
    async fn bound_addr_reflects_running_lifecycle() {
        let service = build_service();
        assert!(service.bound_addr().is_none(), "none before start");

        service.start().await.unwrap();
        let bound = service.bound_addr().expect("some while running");
        assert_ne!(bound.port(), 0, "OS-assigned port must be non-zero");

        stop(&service).await;
        assert!(service.bound_addr().is_none(), "none after stop");
    }

    #[tokio::test]
    async fn status_tracks_lifecycle_state() {
        let service = build_service();
        assert_err_contains(&service.status().await.unwrap_err(), "not started");

        service.start().await.unwrap();
        service.status().await.unwrap();

        stop(&service).await;
        assert_err_contains(&service.status().await.unwrap_err(), "stopped");
    }
}
