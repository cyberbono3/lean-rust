//! Stub [`request_response::Codec`] for the devnet0 req/resp protocols.
//!
//! The codec advertises [`STATUS_PROTOCOL_V1`] and
//! [`BLOCKS_BY_ROOT_PROTOCOL_V1`] (see [`crate::host::behaviour`]), but
//! its read/write methods short-circuit with
//! [`io::ErrorKind::Unsupported`] until the handler logic lands.
//!
//! Stub status:
//! - `read_request` / `read_response` return
//!   [`io::ErrorKind::Unsupported`].
//! - `write_request` / `write_response` return
//!   [`io::ErrorKind::Unsupported`].
//!
//! Inbound peers see the protocol on identify but every exchange
//! errors out — sufficient for the construction smoke tests in this
//! crate while keeping the wire surface available for later handler
//! work.

use std::io;

use async_trait::async_trait;
use futures::{AsyncRead, AsyncWrite};
use libp2p::{request_response, StreamProtocol};

const ERR_READ_REQ: &str = "ssz_snappy request decoder not yet implemented";
const ERR_READ_RES: &str = "ssz_snappy response decoder not yet implemented";
const ERR_WRITE_REQ: &str = "ssz_snappy request encoder not yet implemented";
const ERR_WRITE_RES: &str = "ssz_snappy response encoder not yet implemented";

#[inline]
fn unsupported(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, msg)
}

/// Concrete [`request_response::Codec`] used by the host behaviour.
#[derive(Debug, Default, Clone, Copy)]
pub struct SszSnappyCodec;

#[async_trait]
impl request_response::Codec for SszSnappyCodec {
    type Protocol = StreamProtocol;
    type Request = Vec<u8>;
    type Response = Vec<u8>;

    async fn read_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        _io: &mut T,
    ) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        Err(unsupported(ERR_READ_REQ))
    }

    async fn read_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        _io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        Err(unsupported(ERR_READ_RES))
    }

    async fn write_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        _io: &mut T,
        _req: Self::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        Err(unsupported(ERR_WRITE_REQ))
    }

    async fn write_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        _io: &mut T,
        _res: Self::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        Err(unsupported(ERR_WRITE_RES))
    }
}
