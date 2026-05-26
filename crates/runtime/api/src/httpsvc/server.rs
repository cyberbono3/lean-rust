//! The [`Server`] type — bound TCP listener that serves an `axum::Router`
//! until a [`CancellationToken`] fires.

use std::net::SocketAddr;

use axum::Router;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument};

use super::error::HttpsvcError;

/// Bound TCP listener ready to serve an `axum::Router`.
///
/// Construction (`bind`) and operation (`serve`) are split so callers can
/// observe the resolved [`local_addr`](Self::local_addr) — useful for
/// port-0 binds in tests — before driving the server to completion.
///
/// # Example
/// ```no_run
/// use axum::{routing::get, Router};
/// use lean_api::Server;
/// use tokio_util::sync::CancellationToken;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let server = Server::bind("127.0.0.1:0".parse()?).await?;
/// let router = Router::new().route("/health", get(|| async { "ok" }));
/// server.serve(router, CancellationToken::new()).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
#[must_use = "Server must be driven via `serve` or the listener is dropped"]
pub struct Server {
    listener: TcpListener,
    local_addr: SocketAddr,
}

impl Server {
    /// Binds a TCP listener for the shared HTTP shell.
    ///
    /// # Errors
    /// [`HttpsvcError::Bind`] if the OS rejects the address (e.g.
    /// `EADDRINUSE`, permission denied for a privileged port).
    #[instrument(level = "debug", fields(addr = %addr), err)]
    pub async fn bind(addr: SocketAddr) -> Result<Self, HttpsvcError> {
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|source| HttpsvcError::Bind { addr, source })?;
        let local_addr = listener
            .local_addr()
            .map_err(|source| HttpsvcError::Bind { addr, source })?;
        info!(%local_addr, "httpsvc bound");
        Ok(Self {
            listener,
            local_addr,
        })
    }

    /// Address the listener is bound to.
    ///
    /// When `bind` was called with a port-0 address, this returns the
    /// concrete port chosen by the kernel.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Drives `router` on the bound listener until `cancel` fires.
    ///
    /// Returns `Ok(())` on graceful shutdown.
    ///
    /// # Errors
    /// [`HttpsvcError::Serve`] if `axum::serve` returns an I/O error
    /// while accepting connections.
    #[instrument(level = "debug", skip_all, fields(local_addr = %self.local_addr), err)]
    pub async fn serve(
        self,
        router: Router,
        cancel: CancellationToken,
    ) -> Result<(), HttpsvcError> {
        let Self {
            listener,
            local_addr,
        } = self;
        axum::serve(listener, router)
            .with_graceful_shutdown(cancel.cancelled_owned())
            .await
            .map_err(HttpsvcError::Serve)?;
        info!(%local_addr, "httpsvc stopped");
        Ok(())
    }
}
