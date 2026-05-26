//! Lean HTTP API head endpoints backed by [`storage::Store`].
//!
//! Public surface:
//! - [`HttpService`] — `lean_core::Service` implementation that
//!   binds the listener and serves the registered routes.
//! - [`HttpError`] — error type surfaced to clients as JSON.
//! - [`HEAD_PATHS`] — mounted head endpoint paths.
//!
//! Wire-shape DTOs ([`store_snapshot`]) and the handler module
//! ([`head`]) stay crate-private — composition roots construct the
//! service, not the wire types.

pub(crate) mod error;
pub(crate) mod head;
pub(crate) mod service;
pub(crate) mod store_snapshot;

/// Ethereum-style head endpoint path.
pub const ETH_V1_HEAD_PATH: &str = "/eth/v1/head";

/// Local-pq compatibility head endpoint path.
pub const LEAN_V0_HEAD_PATH: &str = "/lean/v0/head";

/// lean-rust diagnostic head endpoint path.
pub const LEAN_V0_HEAD_FULL_PATH: &str = "/lean/v0/head/full";

/// All head endpoint paths served by [`HttpService`].
pub const HEAD_PATHS: [&str; 3] = [ETH_V1_HEAD_PATH, LEAN_V0_HEAD_PATH, LEAN_V0_HEAD_FULL_PATH];

/// Head endpoint paths that return the full diagnostic response.
pub const FULL_HEAD_PATHS: [&str; 2] = [ETH_V1_HEAD_PATH, LEAN_V0_HEAD_FULL_PATH];

pub use error::HttpError;
pub use service::HttpService;
