//! Integration tests for [`runtime_api::MetricsService`].

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

mod support;

mod metrics {
    use lean_core::Service;
    use runtime_api::{MetricsService, Recorder};
    use tokio_util::sync::CancellationToken;

    use crate::support::{http_get, loopback};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn metrics_endpoint_returns_prometheus_text_with_injected_gauge() {
        let recorder = Recorder::new();
        recorder.gauge("lean_test_fixed_gauge", "Fixed gauge for tests.", || 42);
        let service = MetricsService::new(loopback(), recorder);

        service.start().await.unwrap();
        let addr = service.bound_addr().expect("service must be running");

        let response = http_get(addr, "/metrics").await;

        assert_eq!(response.status, 200);
        assert!(
            response.has_header_value_prefix("content-type", "text/plain"),
            "expected Prometheus text content type, got {:?}",
            response.headers
        );
        assert!(response
            .body
            .contains("# HELP lean_test_fixed_gauge Fixed gauge for tests."));
        assert!(response.body.contains("# TYPE lean_test_fixed_gauge gauge"));
        assert!(response.body.contains("lean_test_fixed_gauge 42"));

        service.stop(CancellationToken::new()).await.unwrap();
    }
}
