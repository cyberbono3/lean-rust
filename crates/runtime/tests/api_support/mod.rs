use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const LOOPBACK_EPHEMERAL: &str = "127.0.0.1:0";

pub(crate) struct HttpResponse {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: String,
}

impl HttpResponse {
    pub(crate) fn has_header_value_prefix(&self, name: &str, prefix: &str) -> bool {
        self.headers.iter().any(|(candidate, value)| {
            candidate.eq_ignore_ascii_case(name) && value.starts_with(prefix)
        })
    }
}

pub(crate) fn loopback() -> SocketAddr {
    LOOPBACK_EPHEMERAL.parse().unwrap()
}

/// Issues a minimal HTTP/1.1 GET and returns the parsed response.
///
/// `Connection: close` makes the server flush + close on response end,
/// so `read_to_end` terminates without parsing `Content-Length`.
pub(crate) async fn http_get(addr: SocketAddr, path: &str) -> HttpResponse {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.unwrap();

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();

    let raw = std::str::from_utf8(&buf).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();

    HttpResponse {
        status,
        headers,
        body: body.to_owned(),
    }
}
