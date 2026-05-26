//! Integration tests for the shared HTTP shell.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::net::SocketAddr;
use std::time::Duration;

use axum::{routing::get, Router};
use lean_core::{HttpsvcError, Server};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

const LOOPBACK_EPHEMERAL: &str = "127.0.0.1:0";

fn loopback() -> SocketAddr {
    LOOPBACK_EPHEMERAL.parse().unwrap()
}

fn health_router() -> Router {
    Router::new().route("/health", get(|| async { "ok" }))
}

/// Issues a minimal HTTP/1.1 request and returns the parsed status code.
///
/// `Connection: close` keeps the response read bounded by EOF so we never
/// need to parse `Content-Length`.
async fn http_get_status(addr: SocketAddr, path: &str) -> u16 {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.unwrap();

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();

    let head = std::str::from_utf8(&buf).unwrap();
    // Status line: "HTTP/1.1 200 OK\r\n..."
    head.split_whitespace().nth(1).unwrap().parse().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_route_returns_200() {
    let server = Server::bind(loopback()).await.unwrap();
    let addr = server.local_addr();
    let cancel = CancellationToken::new();

    let task = tokio::spawn({
        let cancel = cancel.clone();
        async move { server.serve(health_router(), cancel).await }
    });

    assert_eq!(http_get_status(addr, "/health").await, 200);

    cancel.cancel();
    task.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_token_shuts_down_within_2s() {
    let server = Server::bind(loopback()).await.unwrap();
    let cancel = CancellationToken::new();

    let task = tokio::spawn({
        let cancel = cancel.clone();
        async move { server.serve(health_router(), cancel).await }
    });

    cancel.cancel();

    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("server did not stop within 2s")
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn bind_collision_surfaces_as_bind_error() {
    let first = Server::bind(loopback()).await.unwrap();
    let occupied = first.local_addr();

    let err = Server::bind(occupied).await.unwrap_err();
    assert!(
        matches!(&err, HttpsvcError::Bind { addr, .. } if *addr == occupied),
        "expected HttpsvcError::Bind for {occupied}, got {err:?}",
    );
}
