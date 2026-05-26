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

use std::{net::SocketAddr, sync::Arc};

use lean_core::Service;
use protocol::{Checkpoint, Slot};
use runtime_api::{
    http::{ETH_V1_HEAD_PATH, FULL_HEAD_PATHS, HEAD_PATHS, LEAN_V0_HEAD_PATH},
    HttpService,
};
use storage::{HeadInfo, MemoryStore, Store};
use tokio_util::sync::CancellationToken;
use types::Bytes32;

mod support;

use support::{http_get, loopback, HttpResponse};

fn assert_json_response(response: &HttpResponse) {
    assert!(
        response.has_header_value_prefix("content-type", "application/json"),
        "expected JSON content type, got {:?}",
        response.headers
    );
}

async fn head_responses(addr: SocketAddr) -> Vec<(&'static str, HttpResponse)> {
    let mut responses = Vec::with_capacity(HEAD_PATHS.len());
    for path in HEAD_PATHS {
        responses.push((path, http_get(addr, path).await));
    }
    responses
}

fn assert_head_responses(
    responses: &[(&'static str, HttpResponse)],
    expected_status: u16,
    expected_body_for_path: impl Fn(&str) -> &'static str,
) {
    assert_eq!(responses.len(), HEAD_PATHS.len());
    for (path, response) in responses {
        let path = *path;
        assert_eq!(response.status, expected_status, "path {path}");
        assert_json_response(response);
        let expected_body = expected_body_for_path(path);
        assert_eq!(response.body, expected_body, "path {path}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_head_endpoint_returns_populated_info() {
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

    let expected_full = concat!(
        r#"{"head":{"root":"0x"#,
        "1111111111111111111111111111111111111111111111111111111111111111",
        r#"","slot":5},"finalized":{"root":"0x"#,
        "2222222222222222222222222222222222222222222222222222222222222222",
        r#"","slot":2}}"#,
    );
    let expected_ream = concat!(
        r#"{"head":"0x"#,
        "1111111111111111111111111111111111111111111111111111111111111111",
        r#""}"#,
    );
    let responses = head_responses(addr).await;
    assert_head_responses(&responses, 200, |path| {
        if path == LEAN_V0_HEAD_PATH {
            expected_ream
        } else {
            assert!(FULL_HEAD_PATHS.contains(&path), "unexpected path {path}");
            expected_full
        }
    });

    service.stop(CancellationToken::new()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_head_endpoint_returns_404_when_unset() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let service = HttpService::new(store, loopback());

    service.start().await.unwrap();
    let addr = service.bound_addr().expect("service must be running");

    let responses = head_responses(addr).await;
    assert_head_responses(&responses, 404, |_| r#"{"error":"head not yet set"}"#);

    service.stop(CancellationToken::new()).await.unwrap();
}

#[test]
fn lean_head_path_is_ream_compatible_endpoint() {
    assert_eq!(LEAN_V0_HEAD_PATH, "/lean/v0/head");
    assert_eq!(ETH_V1_HEAD_PATH, "/eth/v1/head");
}
