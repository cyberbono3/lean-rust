//! [`HttpService`] — `runtime_core::Service` impl driving the Lean
//! HTTP API.
//!
//! Lifecycle mirrors `runtime/p2p::P2pService`'s state machine: build
//! → `Idle`, `start` binds the listener and spawns the serve task →
//! `Running`, `stop` cancels the token and joins the task → `Stopped`.
//! On a caller-supplied cancel deadline during `stop`, the join handle
//! is aborted to guarantee no orphaned task outlives `stop`.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use axum::{routing::get, Router};
use parking_lot::Mutex;
use runtime_core::{HttpsvcError, Server, Service};
use storage::Store;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, instrument, warn};

use super::head;

/// `tokio::spawn` handle for the axum serve future.
type ServeJoin = JoinHandle<Result<(), HttpsvcError>>;

/// HTTP service serving the runtime's `/eth/v1/...` head endpoints.
///
/// Constructed with an `Arc<dyn storage::Store>` and a listen address;
/// `start` resolves the OS-assigned port (when `:0` is requested) and
/// exposes it via [`Self::bound_addr`]. The service is single-shot per
/// instance: a `Stopped` service does not transition back to `Idle`.
pub struct HttpService {
    store: Arc<dyn Store>,
    listen_addr: SocketAddr,
    state: Mutex<State>,
}

enum State {
    Idle,
    Running {
        cancel: CancellationToken,
        join: ServeJoin,
        bound_addr: SocketAddr,
    },
    Stopped,
    Transitioning,
}

impl HttpService {
    /// Constructs a service that will bind `listen_addr` at `start` and
    /// serve head queries against `store`.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, listen_addr: SocketAddr) -> Self {
        Self {
            store,
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
        match &*self.state.lock() {
            State::Running { bound_addr, .. } => Some(*bound_addr),
            _ => None,
        }
    }

    fn take_idle(&self) -> anyhow::Result<()> {
        let mut guard = self.state.lock();
        match std::mem::replace(&mut *guard, State::Transitioning) {
            State::Idle => Ok(()),
            other => {
                *guard = other;
                Err(anyhow!("http service already started"))
            }
        }
    }

    fn install_running(&self, cancel: CancellationToken, join: ServeJoin, bound_addr: SocketAddr) {
        *self.state.lock() = State::Running {
            cancel,
            join,
            bound_addr,
        };
    }

    fn restore_idle(&self) {
        *self.state.lock() = State::Idle;
    }

    fn take_running(&self) -> Option<(CancellationToken, ServeJoin)> {
        let mut guard = self.state.lock();
        match std::mem::replace(&mut *guard, State::Transitioning) {
            State::Running { cancel, join, .. } => {
                *guard = State::Stopped;
                Some((cancel, join))
            }
            other => {
                *guard = other;
                None
            }
        }
    }

    fn build_router(&self) -> Router {
        Router::new()
            .route(head::PATH, get(head::get_head))
            .with_state(Arc::clone(&self.store))
    }
}

#[async_trait]
impl Service for HttpService {
    fn name(&self) -> &'static str {
        "runtime-api-http"
    }

    #[instrument(name = "http.start", skip(self), fields(listen_addr = %self.listen_addr))]
    async fn start(&self) -> anyhow::Result<()> {
        self.take_idle()?;

        let server = Server::bind(self.listen_addr)
            .await
            .inspect_err(|_| self.restore_idle())?;
        let bound_addr = server.local_addr();
        let router = self.build_router();
        let cancel = CancellationToken::new();
        let join = tokio::spawn(server.serve(router, cancel.clone()));
        self.install_running(cancel, join, bound_addr);
        Ok(())
    }

    #[instrument(name = "http.stop", skip_all, fields(listen_addr = %self.listen_addr))]
    async fn stop(&self, cancel: CancellationToken) -> anyhow::Result<()> {
        let Some((task_cancel, mut join)) = self.take_running() else {
            debug!("stop called on non-running service");
            return Ok(());
        };
        task_cancel.cancel();

        tokio::select! {
            res = &mut join => {
                if let Err(err) = res {
                    if err.is_panic() {
                        return Err(anyhow!("http server task panicked: {err}"));
                    }
                    debug!(%err, "http server task already cancelled");
                }
                Ok(())
            }
            () = cancel.cancelled() => {
                warn!("shutdown cancel fired before http server drained; aborting");
                join.abort();
                Ok(())
            }
        }
    }

    async fn status(&self) -> anyhow::Result<()> {
        match &*self.state.lock() {
            State::Running { join, .. } => {
                if join.is_finished() {
                    Err(anyhow!("http server task exited unexpectedly"))
                } else {
                    Ok(())
                }
            }
            State::Idle => Err(anyhow!("http service not started")),
            State::Stopped => Err(anyhow!("http service stopped")),
            State::Transitioning => Err(anyhow!("http service mid-transition")),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use storage::MemoryStore;

    fn loopback() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    fn build_service() -> HttpService {
        HttpService::new(Arc::new(MemoryStore::default()), loopback())
    }

    async fn stop(service: &HttpService) {
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
