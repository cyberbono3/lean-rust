//! Integration tests for [`runtime_api::MetricsService`].

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

mod metrics {
    use std::net::SocketAddr;
    use std::time::Duration;

    use runtime_api::{MetricsService, Recorder};
    use runtime_core::Service;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio_util::sync::CancellationToken;

    const LOOPBACK_EPHEMERAL: &str = "127.0.0.1:0";

    fn loopback() -> SocketAddr {
        LOOPBACK_EPHEMERAL.parse().unwrap()
    }

    async fn http_get(addr: SocketAddr, path: &str) -> (u16, Vec<(String, String)>, String) {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();

        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();

        let raw = std::str::from_utf8(&buf).unwrap();
        let status: u16 = raw.split_whitespace().nth(1).unwrap().parse().unwrap();
        let (headers, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
        let headers = headers
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
            .collect();

        (status, headers, body.to_owned())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn metrics_endpoint_returns_prometheus_text_with_injected_gauge() {
        let recorder = Recorder::new();
        recorder.gauge("lean_test_fixed_gauge", "Fixed gauge for tests.", || 42);
        let service = MetricsService::new(loopback(), recorder);

        service.start().await.unwrap();
        let addr = service.bound_addr().expect("service must be running");

        let (status, headers, body) = http_get(addr, "/metrics").await;

        assert_eq!(status, 200);
        assert!(
            headers.iter().any(|(name, value)| {
                name.eq_ignore_ascii_case("content-type") && value.starts_with("text/plain")
            }),
            "expected Prometheus text content type, got {headers:?}"
        );
        assert!(body.contains("# HELP lean_test_fixed_gauge Fixed gauge for tests."));
        assert!(body.contains("# TYPE lean_test_fixed_gauge gauge"));
        assert!(body.contains("lean_test_fixed_gauge 42"));

        service.stop(CancellationToken::new()).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_under_caller_cancel_aborts_server() {
        let service = MetricsService::new(loopback(), Recorder::new());

        service.start().await.unwrap();
        assert!(service.bound_addr().is_some());

        let cancel = CancellationToken::new();
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(2), service.stop(cancel))
            .await
            .expect("stop did not return within 2s")
            .unwrap();

        assert!(
            service.bound_addr().is_none(),
            "bound_addr cleared after stop"
        );
    }
}
