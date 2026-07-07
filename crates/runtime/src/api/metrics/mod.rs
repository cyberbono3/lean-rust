//! Prometheus metrics API.
//!
//! Public surface:
//! - [`MetricsService`] — `crate::core::Service` implementation that
//!   binds the listener and serves `/metrics`.
//! - [`Recorder`] — registry of injected gauge providers. Composition
//!   roots adapt concrete runtime services into closures, keeping this
//!   crate decoupled from `lean-chain`, `lean-p2p-host`, and peers.
//! - [`MetricsError`] — error type surfaced to metrics clients.

pub(crate) mod error;
pub(crate) mod prometheus;
pub(crate) mod recorder;
pub(crate) mod service;

pub use error::MetricsError;
pub use recorder::{
    FrozenRecorder, GaugeProvider, LabeledGaugeProvider, LabeledGaugeSamples, ObservedHistogram,
    Recorder,
};
pub use service::MetricsService;
