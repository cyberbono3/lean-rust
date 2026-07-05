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
//! Tracing-subscriber setup lives in the sibling `observability`
//! module; the shared axum-server shell lives in `api::httpsvc`.
//! This module carries no business logic.

mod config;
mod error;
mod lifecycle;
mod node;
mod service;

pub use config::{NodeConfig, DEFAULT_SHUTDOWN_TIMEOUT};
pub use error::{NodeError, ServiceFailure};
pub use node::Node;
pub use service::Service;
