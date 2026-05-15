//! Lean HTTP API: `/eth/v1/...` head endpoints backed by
//! [`storage::Store`].
//!
//! Public surface:
//! - [`HttpService`] — `runtime_core::Service` implementation that
//!   binds the listener and serves the registered routes.
//! - [`HttpError`] — error type surfaced to clients as JSON.
//!
//! Wire-shape DTOs ([`store_snapshot`]) and the handler module
//! ([`head`]) stay crate-private — composition roots construct the
//! service, not the wire types.

pub(crate) mod error;
pub(crate) mod head;
pub(crate) mod service;
pub(crate) mod store_snapshot;

pub use error::HttpError;
pub use service::HttpService;
