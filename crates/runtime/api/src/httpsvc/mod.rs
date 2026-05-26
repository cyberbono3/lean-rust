//! Shared HTTP shell.
//!
//! Binds a TCP listener and serves an `axum::Router`, terminating on a
//! [`CancellationToken`](tokio_util::sync::CancellationToken). Reused by
//! `runtime/api` for both the Lean HTTP API and the Prometheus metrics
//! endpoint.
//!
//! `axum` is intentionally confined to this module (and `runtime/api`)
//! so the rest of the workspace stays framework-agnostic.

mod error;
mod server;

pub use error::HttpsvcError;
pub use server::Server;
