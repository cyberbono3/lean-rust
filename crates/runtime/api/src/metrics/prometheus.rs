//! Prometheus text exposition for `/metrics`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::State, http::header::CONTENT_TYPE, response::IntoResponse, routing::get, Router,
};
use parking_lot::Mutex;
use prometheus::{Registry, TextEncoder, TEXT_FORMAT};

use super::{error::MetricsError, recorder::Recorder};

/// Canonical mount path for the Prometheus scrape endpoint.
pub(crate) const PATH: &str = "/metrics";

/// How long a rendered `/metrics` body is reused before the next request
/// re-runs the providers. Bounds the per-second cost of a `/metrics`
/// flood: an unauthenticated attacker that hits the endpoint N times per
/// second now triggers at most one full render and N-1 cache hits,
/// instead of N full renders. Prometheus scrape intervals are typically
/// 5–15 seconds, so a 1-second TTL is invisible to legitimate scrapers.
const RENDER_CACHE_TTL: Duration = Duration::from_secs(1);

#[derive(Clone)]
struct EndpointState {
    recorder: Recorder,
    cache: Arc<Mutex<Option<(Instant, String)>>>,
}

/// Builds the Prometheus metrics router.
pub(crate) fn router(recorder: Recorder) -> Router {
    let state = EndpointState {
        recorder,
        cache: Arc::new(Mutex::new(None)),
    };
    Router::new()
        .route(PATH, get(get_metrics))
        .with_state(state)
}

async fn get_metrics(
    State(state): State<EndpointState>,
) -> Result<impl IntoResponse, MetricsError> {
    Ok((
        [(CONTENT_TYPE, TEXT_FORMAT)],
        render_cached(&state.recorder, &state.cache)?,
    ))
}

fn render_cached(
    recorder: &Recorder,
    cache: &Mutex<Option<(Instant, String)>>,
) -> Result<String, MetricsError> {
    // Hold the cache lock across `render()`. The previous code dropped
    // the lock before rendering, so a burst of scrapes arriving after
    // the TTL expired all observed a stale entry and all re-rendered in
    // parallel (a thundering herd that defeats the cache). Holding the
    // lock collapses a concurrent burst to a single render: the first
    // caller renders while the rest block, then observe the fresh entry.
    // `render` is synchronous and CPU-only (gather + encode), so no
    // `.await` is held across the lock.
    let mut guard = cache.lock();
    if let Some((stamp, body)) = guard.as_ref() {
        if stamp.elapsed() < RENDER_CACHE_TTL {
            return Ok(body.clone());
        }
    }
    let body = render(recorder)?;
    *guard = Some((Instant::now(), body.clone()));
    Ok(body)
}

fn render(recorder: &Recorder) -> Result<String, MetricsError> {
    let registry = Registry::new();
    recorder.register_collectors(&registry)?;

    let body = TextEncoder::new().encode_to_string(&registry.gather())?;
    Ok(body)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn render_with(configure: impl FnOnce(&Recorder)) -> String {
        let recorder = Recorder::new();
        configure(&recorder);
        render(&recorder).unwrap()
    }

    #[track_caller]
    fn assert_contains(body: &str, needle: &str) {
        assert!(
            body.contains(needle),
            "expected metrics body to contain {needle:?}:\n{body}"
        );
    }

    #[test]
    fn render_includes_simple_gauge_provider() {
        let body = render_with(|recorder| {
            recorder.gauge("lean_test_fixed_gauge", "Fixed gauge for tests.", || 42);
        });

        assert_contains(&body, "# TYPE lean_test_fixed_gauge gauge");
        assert_contains(&body, "lean_test_fixed_gauge 42");
    }

    #[test]
    fn concurrent_render_collapses_to_single_render() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier};
        use std::thread;

        const THREADS: usize = 32;

        // A provider that counts how many times `render` runs.
        let renders = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&renders);
        let recorder = Recorder::new();
        recorder.gauge("lean_test_render_count", "Counts renders.", move || {
            counter.fetch_add(1, Ordering::SeqCst) as u64
        });

        // All threads share one empty cache and race `render_cached`.
        let cache: Arc<Mutex<Option<(Instant, String)>>> = Arc::new(Mutex::new(None));
        let barrier = Arc::new(Barrier::new(THREADS));

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let recorder = recorder.clone();
                let cache = Arc::clone(&cache);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    render_cached(&recorder, &cache).expect("render")
                })
            })
            .collect();
        for h in handles {
            h.join().expect("thread");
        }

        // Holding the lock across render means the burst collapses to a
        // single render; without the fix each thread would re-render.
        assert_eq!(renders.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn render_includes_labeled_gauge_provider() {
        let body = render_with(|recorder| {
            recorder.labeled_gauge(
                "lean_test_labeled_gauge",
                "Labeled gauge for tests.",
                "role",
                || vec![("primary".to_owned(), 7)],
            );
        });

        assert_contains(&body, r#"lean_test_labeled_gauge{role="primary"} 7"#);
    }
}
