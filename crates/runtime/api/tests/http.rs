//! Integration tests for [`runtime_api::HttpService`].
//!
//! Each test binds an ephemeral loopback port via the service lifecycle,
//! reads `bound_addr()`, then drives a raw HTTP/1.1 request over
//! [`tokio::net::TcpStream`]. `Connection: close` keeps the response
//! read bounded by EOF so we never need to parse `Content-Length`.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use protocol::{Checkpoint, Slot};
use runtime_api::HttpService;
use runtime_core::Service;
use storage::{HeadInfo, MemoryStore, Store};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;
use types::Bytes32;

const LOOPBACK_EPHEMERAL: &str = "127.0.0.1:0";

fn loopback() -> SocketAddr {
    LOOPBACK_EPHEMERAL.parse().unwrap()
}

/// Issues a minimal HTTP/1.1 GET and returns `(status, body)`.
///
/// `Connection: close` makes the server flush + close on response end,
/// so `read_to_end` terminates without parsing `Content-Length`.
async fn http_get(addr: SocketAddr, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.unwrap();

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();

    let raw = std::str::from_utf8(&buf).unwrap();
    let status: u16 = raw.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn head_endpoint_returns_populated_info() {
    let store = Arc::new(MemoryStore::default());
    store
        .save_head(HeadInfo::new(
            Checkpoint::new(Bytes32::new([0x11; 32]), Slot::new(5)),
            Checkpoint::new(Bytes32::new([0x22; 32]), Slot::new(2)),
        ))
        .unwrap();
    let service = HttpService::new(store, loopback());

    service.start().await.unwrap();
    let addr = service.bound_addr().expect("service must be running");

    let (status, body) = http_get(addr, "/eth/v1/head").await;
    assert_eq!(status, 200);
    let expected = concat!(
        r#"{"head":{"root":"0x"#,
        "1111111111111111111111111111111111111111111111111111111111111111",
        r#"","slot":5},"finalized":{"root":"0x"#,
        "2222222222222222222222222222222222222222222222222222222222222222",
        r#"","slot":2}}"#,
    );
    assert_eq!(body, expected);

    service.stop(CancellationToken::new()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn head_endpoint_returns_404_when_unset() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let service = HttpService::new(store, loopback());

    service.start().await.unwrap();
    let addr = service.bound_addr().expect("service must be running");

    let (status, body) = http_get(addr, "/eth/v1/head").await;
    assert_eq!(status, 404);
    assert_eq!(body, r#"{"error":"head not yet set"}"#);

    service.stop(CancellationToken::new()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_under_caller_cancel_aborts_server() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let service = HttpService::new(store, loopback());

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
