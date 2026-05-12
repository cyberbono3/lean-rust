//! Lifecycle spine for the runtime shell.
//!
//! # Scope
//! - [`Service`] — async trait every runtime service implements
//!   (`chain`, `p2p`, `sync`, `duties`, `http`, `metrics`).
//! - [`Node`] — composition root holding up to six [`Service`] slots with
//!   ordered start (`chain → p2p → sync → duties → http → metrics`),
//!   reverse-ordered stop, and start-time unwinding on failure.
//! - [`NodeConfig`] — narrow process-level configuration (shutdown
//!   timeout).
//! - [`NodeError`] — typed lifecycle errors with the offending slot
//!   label preserved.
//!
//! - [`Server`] — shared HTTP shell that binds a TCP listener, serves an
//!   `axum::Router`, and terminates on a `CancellationToken`. Reused by
//!   `runtime/api` for the Lean HTTP API and Prometheus metrics.
//!
//! Service implementations (and the leanlog formatter) land in later
//! issues; this crate carries no business logic.

#![forbid(unsafe_code)]

mod config;
mod error;
mod httpsvc;
mod lifecycle;
mod node;
mod observability;
mod service;

pub use config::{NodeConfig, DEFAULT_SHUTDOWN_TIMEOUT};
pub use error::{NodeError, ServiceFailure};
pub use httpsvc::{HttpsvcError, Server};
pub use node::Node;
pub use observability::{
    init_tracing, FileSink, ParseVerbosityError, TracingGuard, TracingInitError, Verbosity,
};
pub use service::Service;
