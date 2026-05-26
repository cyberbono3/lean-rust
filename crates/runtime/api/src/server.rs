//! Shared axum server lifecycle for runtime API endpoints.
//!
//! HTTP and Prometheus expose different routers and error surfaces, but
//! their listener lifecycle is identical: bind on `start`, spawn an axum
//! serve task, cancel and join on `stop`, and expose the OS-resolved
//! address while running. This module keeps that infrastructure
//! crate-private so endpoint modules stay focused on routing.

use std::net::SocketAddr;

use anyhow::anyhow;
use axum::Router;
use lean_core::{HttpsvcError, Server};
use parking_lot::Mutex;
use tokio::task::{JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

/// `tokio::spawn` handle for the axum serve future.
type ServeJoin = JoinHandle<Result<(), HttpsvcError>>;

/// Crate-private lifecycle wrapper for one bound axum endpoint.
pub(crate) struct EndpointServer {
    kind: &'static str,
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

    fn take_idle(&mut self, kind: &'static str) -> anyhow::Result<()> {
        match self {
            Self::Idle => {
                *self = Self::Transitioning;
                Ok(())
            }
            _ => Err(anyhow!("{kind} service already started")),
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

    fn status(&self, kind: &'static str) -> anyhow::Result<()> {
        match self {
            Self::Running(running) => {
                if running.is_finished() {
                    Err(anyhow!("{kind} server task exited unexpectedly"))
                } else {
                    Ok(())
                }
            }
            Self::Idle => Err(anyhow!("{kind} service not started")),
            Self::Stopped => Err(anyhow!("{kind} service stopped")),
            Self::Transitioning => Err(anyhow!("{kind} service mid-transition")),
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

impl EndpointServer {
    /// Constructs an idle endpoint server that will bind `listen_addr`
    /// when [`Self::start`] runs.
    pub(crate) fn new(kind: &'static str, listen_addr: SocketAddr) -> Self {
        Self {
            kind,
            listen_addr,
            state: Mutex::new(State::Idle),
        }
    }

    /// Configured listen address for tracing fields.
    #[must_use]
    pub(crate) const fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }

    /// Returns the listener's resolved address while running.
    #[must_use]
    pub(crate) fn bound_addr(&self) -> Option<SocketAddr> {
        self.state.lock().bound_addr()
    }

    /// Binds the listener and starts serving the router returned by
    /// `build_router`.
    ///
    /// # Errors
    ///
    /// Returns an error if the endpoint was already started or the
    /// listener cannot bind.
    pub(crate) async fn start(&self, build_router: impl FnOnce() -> Router) -> anyhow::Result<()> {
        self.take_idle()?;

        let server = Server::bind(self.listen_addr)
            .await
            .inspect_err(|_| self.restore_idle())?;
        self.install_running(RunningServer::spawn(server, build_router()));
        Ok(())
    }

    /// Cancels and joins the serve task.
    ///
    /// Stop is idempotent for idle/stopped endpoints.
    ///
    /// # Errors
    ///
    /// Returns an error if the serve task reports an HTTP-shell failure
    /// or panics before shutdown completes.
    pub(crate) async fn stop(&self, cancel: CancellationToken) -> anyhow::Result<()> {
        let Some(running) = self.take_running() else {
            debug!(service = self.kind, "stop called on non-running service");
            return Ok(());
        };
        let mut join = running.cancel_and_take_join();

        tokio::select! {
            res = &mut join => handle_server_exit(self.kind, res),
            () = cancel.cancelled() => {
                warn!(service = self.kind, "shutdown cancel fired before server drained; aborting");
                join.abort();
                let _ = join.await;
                Ok(())
            }
        }
    }

    /// Reports whether the serve task is currently running.
    pub(crate) fn status(&self) -> anyhow::Result<()> {
        self.state.lock().status(self.kind)
    }

    fn take_idle(&self) -> anyhow::Result<()> {
        self.state.lock().take_idle(self.kind)
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
}

fn handle_server_exit(
    kind: &'static str,
    res: Result<Result<(), HttpsvcError>, JoinError>,
) -> anyhow::Result<()> {
    match res {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(anyhow!("{kind} server task failed: {err}")),
        Err(err) if err.is_panic() => Err(anyhow!("{kind} server task panicked: {err}")),
        Err(err) => {
            debug!(service = kind, %err, "server task already cancelled");
            Ok(())
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use axum::routing::get;
    use std::net::TcpListener;
    use std::time::Duration;

    fn loopback() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    fn router() -> Router {
        Router::new().route("/health", get(|| async { "ok" }))
    }

    fn build_server() -> EndpointServer {
        EndpointServer::new("test", loopback())
    }

    async fn stop(server: &EndpointServer) {
        server.stop(CancellationToken::new()).await.unwrap();
    }

    fn assert_err_contains(err: &anyhow::Error, needle: &str) {
        let msg = err.to_string();
        assert!(msg.contains(needle), "expected {needle:?} in {msg:?}");
    }

    #[tokio::test]
    async fn double_start_returns_already_started() {
        let server = build_server();
        server.start(router).await.unwrap();

        assert_err_contains(&server.start(router).await.unwrap_err(), "already started");

        stop(&server).await;
    }

    #[tokio::test]
    async fn stop_on_idle_is_noop() {
        stop(&build_server()).await;
    }

    #[tokio::test]
    async fn bound_addr_reflects_running_lifecycle() {
        let server = build_server();
        assert!(server.bound_addr().is_none(), "none before start");

        server.start(router).await.unwrap();
        let bound = server.bound_addr().expect("some while running");
        assert_ne!(bound.port(), 0, "OS-assigned port must be non-zero");

        stop(&server).await;
        assert!(server.bound_addr().is_none(), "none after stop");
    }

    #[tokio::test]
    async fn status_tracks_lifecycle_state() {
        let server = build_server();
        assert_err_contains(&server.status().unwrap_err(), "not started");

        server.start(router).await.unwrap();
        server.status().unwrap();

        stop(&server).await;
        assert_err_contains(&server.status().unwrap_err(), "stopped");
    }

    #[tokio::test]
    async fn bind_failure_restores_idle() {
        let listener = TcpListener::bind(loopback()).unwrap();
        let occupied = listener.local_addr().unwrap();
        let server = EndpointServer::new("test", occupied);

        let err = server.start(router).await.unwrap_err();
        assert_err_contains(&err, "bind");

        drop(listener);
        server.start(router).await.unwrap();
        stop(&server).await;
    }

    #[tokio::test]
    async fn stop_under_caller_cancel_aborts_and_returns() {
        let server = build_server();
        server.start(router).await.unwrap();
        assert!(server.bound_addr().is_some());

        let cancel = CancellationToken::new();
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(2), server.stop(cancel))
            .await
            .expect("stop did not return within 2s")
            .unwrap();

        assert!(server.bound_addr().is_none());
    }
}
