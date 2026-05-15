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

use std::sync::Arc;

use protocol::{Checkpoint, Slot};
use runtime_api::HttpService;
use runtime_core::Service;
use storage::{HeadInfo, MemoryStore, Store};
use tokio_util::sync::CancellationToken;
use types::Bytes32;

mod support;

use support::{http_get, loopback};

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

    let response = http_get(addr, "/eth/v1/head").await;
    assert_eq!(response.status, 200);
    assert!(
        response.has_header_value_prefix("content-type", "application/json"),
        "expected JSON content type, got {:?}",
        response.headers
    );
    let expected = concat!(
        r#"{"head":{"root":"0x"#,
        "1111111111111111111111111111111111111111111111111111111111111111",
        r#"","slot":5},"finalized":{"root":"0x"#,
        "2222222222222222222222222222222222222222222222222222222222222222",
        r#"","slot":2}}"#,
    );
    assert_eq!(response.body, expected);

    service.stop(CancellationToken::new()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn head_endpoint_returns_404_when_unset() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let service = HttpService::new(store, loopback());

    service.start().await.unwrap();
    let addr = service.bound_addr().expect("service must be running");

    let response = http_get(addr, "/eth/v1/head").await;
    assert_eq!(response.status, 404);
    assert!(
        response.has_header_value_prefix("content-type", "application/json"),
        "expected JSON content type, got {:?}",
        response.headers
    );
    assert_eq!(response.body, r#"{"error":"head not yet set"}"#);

    service.stop(CancellationToken::new()).await.unwrap();
}
