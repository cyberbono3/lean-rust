//! Injected metrics providers.
//!
//! The recorder owns provider closures rather than concrete runtime
//! service handles. A composition root can adapt chain, p2p, duties, or
//! sync state into small closures without introducing compile-time
//! dependencies from `lean-api` back into those crates.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use prometheus::{Histogram, HistogramOpts, IntGauge, IntGaugeVec, Opts, Registry, TextEncoder};

use super::error::MetricsError;

/// Opaque handle to a registered push-observation histogram.
///
/// Wraps the backing `prometheus` collector so that implementation type does not
/// leak through this crate's public API. Cheap to clone (`Arc`-backed); a clone
/// injected into a producer records observations that the sibling clone held by
/// the recorder exports on scrape.
#[derive(Clone)]
pub struct ObservedHistogram {
    inner: Histogram,
}

impl ObservedHistogram {
    /// Records one observation, in seconds.
    pub fn observe(&self, elapsed: Duration) {
        self.inner.observe(elapsed.as_secs_f64());
    }
}

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

/// Mutable, boot-time builder for the metrics registry.
///
/// Composition roots register gauge providers here during node assembly,
/// then call [`Recorder::freeze`] to obtain an immutable [`FrozenRecorder`]
/// for the running service. Registration is single-pass and
/// single-threaded (the composition root), so the builder is a plain
/// `Vec` — no lock. The previous `Arc<RwLock<Vec<…>>>` paid a write lock
/// per registration and left registration possible at runtime; freezing
/// turns "no registration after boot" from a convention into a
/// type-level guarantee (the `register_*` methods exist only on
/// `Recorder`, not on `FrozenRecorder`).
#[derive(Default)]
pub struct Recorder {
    metrics: Vec<MetricDefinition>,
}

/// Immutable, cheaply cloneable snapshot of the registered metric
/// providers, consumed by the running metrics service.
///
/// Holds `Arc<[MetricDefinition]>` — no `RwLock`: the definition set is
/// fixed at [`Recorder::freeze`] time, so a `/metrics` scrape reads it
/// without any lock.
#[derive(Clone)]
pub struct FrozenRecorder {
    metrics: Arc<[MetricDefinition]>,
}

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
    /// Push-observation histogram. Unlike the gauge variants (sampled per
    /// scrape via a provider closure), the `Histogram` handle is cumulative
    /// and `Arc`-backed: the runtime chain-tick boundary holds a clone and
    /// calls `observe()` on it; this definition holds the sibling clone that
    /// the per-scrape `Registry` re-registers so `gather()` reads the
    /// accumulated state. No provider closure — state lives in the handle.
    Histogram { name: String, hist: Histogram },
}

impl Recorder {
    /// Constructs a recorder with baseline process gauges.
    #[must_use]
    pub fn new() -> Self {
        let mut recorder = Self::default();
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
    pub fn gauge<F>(&mut self, name: impl Into<String>, help: impl Into<String>, provider: F)
    where
        F: Fn() -> u64 + Send + Sync + 'static,
    {
        self.metrics
            .push(MetricDefinition::gauge(name, help, provider));
    }

    /// Registers a one-label gauge provider.
    ///
    /// The provider is evaluated once per `/metrics` scrape and returns
    /// `(label_value, metric_value)` samples for the configured label
    /// name.
    pub fn labeled_gauge<F>(
        &mut self,
        name: impl Into<String>,
        help: impl Into<String>,
        label_name: impl Into<String>,
        provider: F,
    ) where
        F: Fn() -> LabeledGaugeSamples + Send + Sync + 'static,
    {
        self.metrics.push(MetricDefinition::labeled_gauge(
            name, help, label_name, provider,
        ));
    }

    /// Registers a push-observation histogram and returns its opaque handle.
    ///
    /// Unlike the gauge variants (sampled per scrape via a provider closure), a
    /// histogram is cumulative. The returned [`ObservedHistogram`] is `Arc`-backed
    /// and cheap to clone: the composition root injects a clone into the runtime
    /// chain layer, which calls `observe()` at the chain-tick boundary, while the
    /// sibling clone stored here is re-registered into the ephemeral per-scrape
    /// registry so a `/metrics` scrape exports the accumulated observations. The
    /// handle wraps the backing `prometheus` collector so that implementation type
    /// does not leak through this crate's public API.
    ///
    /// # Errors
    /// [`MetricsError::Prometheus`] if Prometheus rejects the descriptor (e.g. an
    /// invalid metric name).
    pub fn histogram(
        &mut self,
        name: impl Into<String>,
        help: impl Into<String>,
        buckets: Vec<f64>,
    ) -> Result<ObservedHistogram, MetricsError> {
        let name = name.into();
        let opts = HistogramOpts::new(name.clone(), help.into()).buckets(buckets);
        let hist = Histogram::with_opts(opts)?;
        self.metrics.push(MetricDefinition::Histogram {
            name,
            hist: hist.clone(),
        });
        Ok(ObservedHistogram { inner: hist })
    }

    /// Consumes the builder and returns an immutable [`FrozenRecorder`],
    /// rejecting a duplicate metric name at this single boot-time gate
    /// rather than lazily at the first scrape (where Prometheus would
    /// reject the second collector). A duplicate is a composition-root
    /// wiring bug — failing `freeze` keeps the node from reaching HTTP
    /// listen.
    ///
    /// # Errors
    /// [`MetricsError::DuplicateMetric`] if two providers share a name.
    pub fn freeze(self) -> Result<FrozenRecorder, MetricsError> {
        let mut seen = HashSet::with_capacity(self.metrics.len());
        for definition in &self.metrics {
            if !seen.insert(definition.name()) {
                return Err(MetricsError::DuplicateMetric {
                    name: definition.name().to_owned(),
                });
            }
        }
        Ok(FrozenRecorder {
            metrics: Arc::from(self.metrics),
        })
    }
}

impl FrozenRecorder {
    /// Registers the frozen provider set into one Prometheus registry.
    ///
    /// # Errors
    ///
    /// Returns an error if Prometheus rejects a metric descriptor or a
    /// provider returns a value larger than `i64::MAX`.
    pub(crate) fn register_collectors(&self, registry: &Registry) -> Result<(), MetricsError> {
        self.metrics
            .iter()
            .try_for_each(|definition| definition.register(registry))
    }

    /// Renders the frozen metric set to Prometheus text exposition — the same
    /// bytes the `/metrics` endpoint serves. Builds a fresh registry, registers
    /// the providers, gathers, and encodes.
    ///
    /// `pub` (but `#[doc(hidden)]`) so tests across the runtime crate AND the
    /// node crate (devnet composition tests) can assert on exposition output
    /// without reaching into the private HTTP render path. Not part of the
    /// documented public API — the `/metrics` endpoint is the supported surface.
    ///
    /// # Errors
    /// [`MetricsError`] on descriptor rejection or encode failure.
    #[doc(hidden)]
    pub fn encode(&self) -> Result<String, MetricsError> {
        let registry = Registry::new();
        self.register_collectors(&registry)?;
        Ok(TextEncoder::new().encode_to_string(&registry.gather())?)
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

    /// The metric's exposition name — the dedup key checked by
    /// [`Recorder::freeze`].
    fn name(&self) -> &str {
        match self {
            Self::Gauge { name, .. }
            | Self::LabeledGauge { name, .. }
            | Self::Histogram { name, .. } => name,
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
            // The handle is cumulative and lives outside the ephemeral
            // registry; register a clone so `gather()` sees it this scrape.
            Self::Histogram { hist, .. } => {
                registry.register(Box::new(hist.clone()))?;
                Ok(())
            }
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
    let gauge = IntGaugeVec::new(Opts::new(name, help), &[label_name])?;
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
        Recorder::new()
            .freeze()
            .unwrap()
            .register_collectors(&registry)
            .unwrap();

        assert_has_collector(&registry, "lean_node_up");
        assert_has_collector(&registry, "lean_node_start_time_seconds");
    }

    #[test]
    fn gauge_value_overflow_is_reported() {
        let mut recorder = Recorder::new();
        recorder.gauge("lean_too_large", "Too large.", || u64::MAX);

        let err = recorder
            .freeze()
            .unwrap()
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

    #[test]
    fn freeze_rejects_duplicate_metric_name() {
        let mut recorder = Recorder::new();
        recorder.gauge("lean_dup", "First.", || 1);
        recorder.gauge("lean_dup", "Second.", || 2);

        // `FrozenRecorder` holds provider closures and is not `Debug`,
        // so use `.err()` rather than `expect_err`.
        let err = recorder
            .freeze()
            .err()
            .expect("duplicate name should fail freeze");
        assert!(
            matches!(&err, MetricsError::DuplicateMetric { name } if name == "lean_dup"),
            "got {err:?}",
        );
    }

    #[test]
    fn freeze_accepts_distinct_names() {
        let mut recorder = Recorder::new();
        recorder.gauge("lean_a", "A.", || 1);
        recorder.gauge("lean_b", "B.", || 2);
        assert!(recorder.freeze().is_ok());
    }

    #[test]
    fn histogram_renders_help_type_and_buckets() {
        let mut recorder = Recorder::new();
        let h = recorder
            .histogram("lean_test_latency_seconds", "Test latency.", vec![0.1, 1.0])
            .unwrap();
        h.observe(Duration::from_millis(50));

        let body = recorder.freeze().unwrap().encode().unwrap();
        assert!(body.contains("# TYPE lean_test_latency_seconds histogram"));
        assert!(body.contains("lean_test_latency_seconds_bucket{le=\"0.1\"}"));
        assert!(body.contains("lean_test_latency_seconds_count 1"));
    }

    #[test]
    fn duplicate_histogram_name_is_reported() {
        let mut recorder = Recorder::new();
        recorder
            .histogram("lean_dup_hist", "First.", vec![1.0])
            .unwrap();
        recorder
            .histogram("lean_dup_hist", "Second.", vec![1.0])
            .unwrap();

        let err = recorder
            .freeze()
            .err()
            .expect("duplicate histogram name should fail freeze");
        assert!(
            matches!(&err, MetricsError::DuplicateMetric { name } if name == "lean_dup_hist"),
            "got {err:?}",
        );
    }

    #[test]
    fn histogram_observations_accumulate_across_scrapes() {
        // The handle is `Arc`-backed, so counts persist across the per-scrape
        // `Registry` rebuild inside `encode()`.
        let mut recorder = Recorder::new();
        let h = recorder
            .histogram("lean_acc_seconds", "Acc.", vec![1.0])
            .unwrap();
        let frozen = recorder.freeze().unwrap();

        h.observe(Duration::from_millis(200));
        assert!(frozen
            .encode()
            .unwrap()
            .contains("lean_acc_seconds_count 1"));
        h.observe(Duration::from_millis(300));
        assert!(frozen
            .encode()
            .unwrap()
            .contains("lean_acc_seconds_count 2"));
    }
}
