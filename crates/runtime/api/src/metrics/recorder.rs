//! Injected metrics providers.
//!
//! The recorder owns provider closures rather than concrete runtime
//! service handles. A composition root can adapt chain, p2p, duties, or
//! sync state into small closures without introducing compile-time
//! dependencies from `runtime-api` back into those crates.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use prometheus::{IntGauge, IntGaugeVec, Opts, Registry};

use super::error::MetricsError;

/// Provider for a single unsigned integer gauge.
pub type GaugeProvider = dyn Fn() -> u64 + Send + Sync + 'static;

/// Samples returned by a one-label gauge provider.
///
/// Each tuple is `(label_value, metric_value)`.
pub type LabeledGaugeSamples = Vec<(String, u64)>;

/// Provider for one-label unsigned integer gauge samples.
///
/// The tuple shape is `(label_value, metric_value)`. The metric
/// definition supplies the label name.
pub type LabeledGaugeProvider = dyn Fn() -> LabeledGaugeSamples + Send + Sync + 'static;

/// Runtime metrics recorder backed by injected provider closures.
#[derive(Clone)]
pub struct Recorder {
    metrics: Arc<RwLock<Vec<MetricDefinition>>>,
}

#[derive(Clone)]
enum MetricDefinition {
    Gauge {
        name: String,
        help: String,
        provider: Arc<GaugeProvider>,
    },
    LabeledGauge {
        name: String,
        help: String,
        label_name: String,
        provider: Arc<LabeledGaugeProvider>,
    },
}

impl Recorder {
    /// Constructs a recorder with baseline process gauges.
    #[must_use]
    pub fn new() -> Self {
        let recorder = Self {
            metrics: Arc::default(),
        };
        let start_time = unix_timestamp();
        recorder.gauge("lean_node_up", "Whether the Lean node process is up.", || 1);
        recorder.gauge(
            "lean_node_start_time_seconds",
            "Unix timestamp when the Lean node process started.",
            move || start_time,
        );
        recorder
    }

    /// Registers a single-value gauge provider.
    ///
    /// The provider is evaluated once per `/metrics` scrape.
    pub fn gauge<F>(&self, name: impl Into<String>, help: impl Into<String>, provider: F)
    where
        F: Fn() -> u64 + Send + Sync + 'static,
    {
        self.push_metric(MetricDefinition::gauge(name, help, provider));
    }

    /// Registers a one-label gauge provider.
    ///
    /// The provider is evaluated once per `/metrics` scrape and returns
    /// `(label_value, metric_value)` samples for the configured label
    /// name.
    pub fn labeled_gauge<F>(
        &self,
        name: impl Into<String>,
        help: impl Into<String>,
        label_name: impl Into<String>,
        provider: F,
    ) where
        F: Fn() -> LabeledGaugeSamples + Send + Sync + 'static,
    {
        self.push_metric(MetricDefinition::labeled_gauge(
            name, help, label_name, provider,
        ));
    }

    /// Registers the current provider snapshot into one Prometheus
    /// registry.
    ///
    /// # Errors
    ///
    /// Returns an error if Prometheus rejects a metric descriptor or a
    /// provider returns a value larger than `i64::MAX`.
    pub(crate) fn register_collectors(&self, registry: &Registry) -> Result<(), MetricsError> {
        self.snapshot_metrics()
            .iter()
            .try_for_each(|definition| definition.register(registry))
    }

    fn push_metric(&self, definition: MetricDefinition) {
        self.metrics.write().push(definition);
    }

    fn snapshot_metrics(&self) -> Vec<MetricDefinition> {
        self.metrics.read().clone()
    }
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricDefinition {
    fn gauge<F>(name: impl Into<String>, help: impl Into<String>, provider: F) -> Self
    where
        F: Fn() -> u64 + Send + Sync + 'static,
    {
        Self::Gauge {
            name: name.into(),
            help: help.into(),
            provider: Arc::new(provider),
        }
    }

    fn labeled_gauge<F>(
        name: impl Into<String>,
        help: impl Into<String>,
        label_name: impl Into<String>,
        provider: F,
    ) -> Self
    where
        F: Fn() -> LabeledGaugeSamples + Send + Sync + 'static,
    {
        Self::LabeledGauge {
            name: name.into(),
            help: help.into(),
            label_name: label_name.into(),
            provider: Arc::new(provider),
        }
    }

    fn register(&self, registry: &Registry) -> Result<(), MetricsError> {
        match self {
            Self::Gauge {
                name,
                help,
                provider,
            } => register_gauge(registry, name, help, (provider)()),
            Self::LabeledGauge {
                name,
                help,
                label_name,
                provider,
            } => register_labeled_gauge(registry, name, help, label_name, (provider)()),
        }
    }
}

fn register_gauge(
    registry: &Registry,
    name: &str,
    help: &str,
    raw_value: u64,
) -> Result<(), MetricsError> {
    let value = metric_value(name, raw_value)?;
    let gauge = IntGauge::with_opts(Opts::new(name, help))?;
    gauge.set(value);
    registry.register(Box::new(gauge))?;
    Ok(())
}

fn register_labeled_gauge(
    registry: &Registry,
    name: &str,
    help: &str,
    label_name: &str,
    samples: LabeledGaugeSamples,
) -> Result<(), MetricsError> {
    let gauge = IntGaugeVec::new(Opts::new(name.to_owned(), help), &[label_name])?;
    for (label, raw_value) in samples {
        let value = metric_value(name, raw_value)?;
        gauge
            .get_metric_with_label_values(&[label.as_str()])?
            .set(value);
    }
    registry.register(Box::new(gauge))?;
    Ok(())
}

fn metric_value(name: &str, value: u64) -> Result<i64, MetricsError> {
    i64::try_from(value).map_err(|_| MetricsError::ValueOutOfRange {
        name: name.to_owned(),
        value,
    })
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn collector_names(registry: &Registry) -> Vec<String> {
        registry
            .gather()
            .into_iter()
            .map(|family| family.get_name().to_owned())
            .collect()
    }

    #[track_caller]
    fn assert_has_collector(registry: &Registry, name: &str) {
        let names = collector_names(registry);
        assert!(
            names.iter().any(|candidate| candidate == name),
            "expected collector {name:?} in {names:?}"
        );
    }

    #[test]
    fn recorder_default_has_baseline_gauges() {
        let registry = Registry::new();
        Recorder::new().register_collectors(&registry).unwrap();

        assert_has_collector(&registry, "lean_node_up");
        assert_has_collector(&registry, "lean_node_start_time_seconds");
    }

    #[test]
    fn gauge_value_overflow_is_reported() {
        let recorder = Recorder::new();
        recorder.gauge("lean_too_large", "Too large.", || u64::MAX);

        let err = recorder
            .register_collectors(&Registry::new())
            .expect_err("overflow should fail");

        assert!(matches!(
            err,
            MetricsError::ValueOutOfRange {
                name,
                value: u64::MAX
            } if name == "lean_too_large"
        ));
    }
}
