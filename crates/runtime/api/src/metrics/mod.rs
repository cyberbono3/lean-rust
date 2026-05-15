//! Prometheus metrics API.
//!
//! Public surface:
//! - [`MetricsService`] — `runtime_core::Service` implementation that
//!   binds the listener and serves `/metrics`.
//! - [`Recorder`] — registry of injected gauge providers. Composition
//!   roots adapt concrete runtime services into closures, keeping this
//!   crate decoupled from `runtime-chain`, `runtime-p2p`, and peers.
//! - [`MetricsError`] — error type surfaced to metrics clients.

pub(crate) mod error;
pub(crate) mod prometheus;
pub(crate) mod recorder;
pub(crate) mod service;

pub use error::MetricsError;
pub use recorder::{GaugeProvider, LabeledGaugeProvider, LabeledGaugeSamples, Recorder};
pub use service::MetricsService;
