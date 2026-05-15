//! Prometheus text exposition for `/metrics`.

use axum::{
    extract::State, http::header::CONTENT_TYPE, response::IntoResponse, routing::get, Router,
};
use prometheus::{Registry, TextEncoder, TEXT_FORMAT};

use super::{error::MetricsError, recorder::Recorder};

/// Canonical mount path for the Prometheus scrape endpoint.
pub(crate) const PATH: &str = "/metrics";

/// Builds the Prometheus metrics router.
pub(crate) fn router(recorder: Recorder) -> Router {
    Router::new()
        .route(PATH, get(get_metrics))
        .with_state(recorder)
}

async fn get_metrics(State(recorder): State<Recorder>) -> Result<impl IntoResponse, MetricsError> {
    Ok(([(CONTENT_TYPE, TEXT_FORMAT)], render(&recorder)?))
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
