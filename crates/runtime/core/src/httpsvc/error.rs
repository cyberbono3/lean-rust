//! Error type for [`Server`](super::Server).

use std::io;
use std::net::SocketAddr;

use thiserror::Error;

/// Failures raised by [`Server`](super::Server).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HttpsvcError {
    /// `Server::bind` could not acquire the requested address (e.g.
    /// `EADDRINUSE`, or permission denied for a privileged port).
    #[error("bind {addr}: {source}")]
    Bind {
        /// Address that was requested.
        addr: SocketAddr,
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },

    /// `axum::serve` exited with an I/O error while accepting connections.
    #[error("serve: {0}")]
    Serve(#[source] io::Error),
}
