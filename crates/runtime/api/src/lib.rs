//! Lean HTTP API for the runtime shell.
//!
//! Read-only adapter over [`storage::Store`]. The crate hosts the
//! serde-tagged wire shapes for the runtime; domain types in `protocol`
//! and `storage` stay framework-free per the workspace architecture
//! rules.
//!
//! # Scope
//! - [`HttpService`] — [`lean_core::Service`] implementation that
//!   serves head endpoints, backed by an
//!   `Arc<dyn storage::Store>` injected at construction.
//! - [`http::HEAD_PATHS`] — mounted head endpoint paths.
//! - [`HttpError`] — public error surface returned to clients.
//! - [`MetricsService`] — [`lean_core::Service`] implementation that
//!   serves Prometheus text exposition on `/metrics`, backed by
//!   injected provider closures.
//!
//! # Architecture
//! Runtime data sources stay injected through narrow traits or
//! closures. There are no compile-time references to `engine`,
//! `forkchoice`, `statetransition`, `runtime/chain`, or `runtime/p2p` —
//! composition happens at the `node` crate (Issue #37).

#![forbid(unsafe_code)]

mod server;

pub mod http;
pub mod metrics;

pub use http::{HttpError, HttpService};
pub use metrics::{
    GaugeProvider, LabeledGaugeProvider, LabeledGaugeSamples, MetricsError, MetricsService,
    Recorder,
};
