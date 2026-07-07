//! Chain-tick trigger metrics.
//!
//! Push-observation histograms observed ONLY at the runtime chain-tick
//! boundary. The `protocol` and `forkchoice` transition code stays free of
//! metrics/time/RNG; this type is the runtime-side home for the timing that
//! wraps their calls.
//!
//! Pure handle holder: it does NOT import the metrics `Recorder`. The node
//! composition root creates the histograms (`register_chain_histograms`) and
//! builds this via [`ChainMetrics::new`], mirroring `register_chain_gauges`.
//!
//! Default is a no-op (`None` handles): unit tests, the `engine_import` bench,
//! and any non-composition-root `Engine` observe nothing and export nothing.

use std::time::Duration;

use prometheus::Histogram;

/// Trigger histograms for the deferred-performance levers.
///
/// Wired (boundary-observable):
/// - `fork_choice_block_processing` → `lean_fork_choice_block_processing_time_seconds`.
/// - `state_transition` → `lean_state_transition_time_seconds`.
///
/// A per-slot process-slots split is intentionally NOT wired here: it measures
/// a sub-phase inside `protocol::State::state_transition` and cannot be observed
/// at the runtime boundary without adding timing inside `protocol`. The
/// whole-transition wall time is the coarse trigger.
#[derive(Clone, Default)]
pub struct ChainMetrics {
    fork_choice_block_processing: Option<Histogram>,
    state_transition: Option<Histogram>,
}

impl ChainMetrics {
    /// Builds a live handle set from pre-registered histograms. Called by the
    /// composition root (`register_chain_histograms`); tests and benches use
    /// [`ChainMetrics::default`] (all-`None`, no-op).
    #[must_use]
    pub fn new(fork_choice_block_processing: Histogram, state_transition: Histogram) -> Self {
        Self {
            fork_choice_block_processing: Some(fork_choice_block_processing),
            state_transition: Some(state_transition),
        }
    }

    /// Records one fork-choice recompute duration. No-op on the default handle.
    pub(crate) fn observe_fork_choice_block_processing(&self, elapsed: Duration) {
        if let Some(h) = &self.fork_choice_block_processing {
            h.observe(elapsed.as_secs_f64());
        }
    }

    /// Records one full state-transition duration. No-op on the default handle.
    pub(crate) fn observe_state_transition(&self, elapsed: Duration) {
        if let Some(h) = &self.state_transition {
            h.observe(elapsed.as_secs_f64());
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn default_chain_metrics_observe_is_noop() {
        let m = ChainMetrics::default();
        // Must not panic when the handles are absent.
        m.observe_state_transition(Duration::from_millis(1));
        m.observe_fork_choice_block_processing(Duration::from_millis(1));
    }
}
