//! [`request_response::Codec`] for the devnet0 `Status` +
//! `BlocksByRoot` protocols.
//!
//! Wire shape per message: `uvarint(ssz_len) || snappy_framed(ssz_bytes)`
//! ([`lean_wire::write_req_resp_frame`] /
//! [`lean_wire::read_req_resp_frame`]).
//! The substream half-closes after the sender finishes, so each codec
//! method reads to EOF and decodes the resulting buffer in one shot.
//!
//! The codec dispatches on the libp2p protocol id ([`STATUS_PROTOCOL_V1`]
//! / [`BLOCKS_BY_ROOT_PROTOCOL_V1`]) carried in the codec method's
//! `protocol` parameter, resolved once by [`RpcProtocol::resolve`];
//! unknown protocols surface as [`io::ErrorKind::Unsupported`].

use std::io::{self, Cursor};

use async_trait::async_trait;
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use lean_wire::{
    read_req_resp_frame, write_req_resp_frame, BlocksByRootRequest, BlocksByRootResponse, Status,
    BLOCKS_BY_ROOT_PROTOCOL_V1, STATUS_PROTOCOL_V1,
};
use libp2p::{request_response, StreamProtocol};
use protocol::SignedBlockWithAttestation;

/// Defensive upper bound on a single inbound req/resp stream payload.
///
/// A malicious peer that holds the substream open and keeps sending bytes
/// can OOM the process via `read_to_end` if no cap exists. Sized
/// generously enough for a full `BlocksByRoot` response (≤ `MAX_REQUEST_BLOCKS`
/// = 1024 `SignedBlockWithAttestation`s plus per-frame snappy / uvarint overhead) but
/// tight enough to fault-stop an attacker.
const MAX_RPC_STREAM_BYTES: u64 = 16 * 1024 * 1024;

/// The req/resp protocol a codec call is operating on.
///
/// The codec's four methods each receive the negotiated protocol id as a
/// string; resolving it in one place means a new protocol is added to
/// [`lean_wire::REQRESP_PROTOCOLS`] and matched here, not string-matched in
/// four methods that can drift apart.
///
/// `pub(super)` so the parent module's served-set pin can assert that the
/// codec resolves every protocol the host advertises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RpcProtocol {
    /// [`lean_wire::STATUS_PROTOCOL_V1`].
    Status,
    /// [`lean_wire::BLOCKS_BY_ROOT_PROTOCOL_V1`].
    BlocksByRoot,
}

impl RpcProtocol {
    /// Resolves a negotiated protocol id, or `None` if this client does not
    /// serve it.
    pub(super) fn resolve(protocol: &str) -> Option<Self> {
        if protocol == STATUS_PROTOCOL_V1.as_str() {
            Some(Self::Status)
        } else if protocol == BLOCKS_BY_ROOT_PROTOCOL_V1.as_str() {
            Some(Self::BlocksByRoot)
        } else {
            None
        }
    }
}

/// Outbound or inbound request payload on one of the devnet0 req/resp
/// protocols.
#[derive(Debug, Clone)]
pub enum RpcRequest {
    /// Initial handshake — sender advertises its `(finalized, head)`
    /// checkpoints.
    Status(Status),
    /// Request a list of blocks by their tree roots; bounded at
    /// [`lean_wire::MAX_REQUEST_BLOCKS`] at decode time.
    BlocksByRoot(BlocksByRootRequest),
}

/// Outbound or inbound response payload paired with one [`RpcRequest`].
#[derive(Debug, Clone)]
pub enum RpcResponse {
    /// Status response — receiver's `(finalized, head)` checkpoints.
    Status(Status),
    /// Response carrying ≤ `MAX_REQUEST_BLOCKS` signed blocks for the
    /// matching `BlocksByRoot` request.
    BlocksByRoot(BlocksByRootResponse),
}

/// Codec implementing [`request_response::Codec`] for the devnet0
/// `Status` + `BlocksByRoot` protocols.
#[derive(Debug, Default, Clone, Copy)]
pub struct SszSnappyCodec;

#[async_trait]
impl request_response::Codec for SszSnappyCodec {
    type Protocol = StreamProtocol;
    type Request = RpcRequest;
    type Response = RpcResponse;

    async fn read_request<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        let buf = read_to_end(io).await?;
        match RpcProtocol::resolve(protocol.as_ref()) {
            Some(RpcProtocol::Status) => {
                Ok(RpcRequest::Status(decode_single_frame::<Status>(&buf)?))
            }
            Some(RpcProtocol::BlocksByRoot) => Ok(RpcRequest::BlocksByRoot(decode_single_frame::<
                BlocksByRootRequest,
            >(&buf)?)),
            None => Err(unknown_protocol(protocol.as_ref())),
        }
    }

    async fn read_response<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        let buf = read_to_end(io).await?;
        match RpcProtocol::resolve(protocol.as_ref()) {
            Some(RpcProtocol::Status) => {
                Ok(RpcResponse::Status(decode_single_frame::<Status>(&buf)?))
            }
            Some(RpcProtocol::BlocksByRoot) => Ok(RpcResponse::BlocksByRoot(
                decode_blocks_by_root_response(&buf)?,
            )),
            None => Err(unknown_protocol(protocol.as_ref())),
        }
    }

    async fn write_request<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
        req: Self::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let bytes = match (&req, RpcProtocol::resolve(protocol.as_ref())) {
            (RpcRequest::Status(s), Some(RpcProtocol::Status)) => encode_frame(s)?,
            (RpcRequest::BlocksByRoot(r), Some(RpcProtocol::BlocksByRoot)) => encode_frame(r)?,
            _ => return Err(protocol_mismatch(protocol.as_ref(), "request")),
        };
        write_framed(io, &bytes).await
    }

    async fn write_response<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
        res: Self::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let bytes = match (&res, RpcProtocol::resolve(protocol.as_ref())) {
            (RpcResponse::Status(s), Some(RpcProtocol::Status)) => encode_frame(s)?,
            (RpcResponse::BlocksByRoot(r), Some(RpcProtocol::BlocksByRoot)) => {
                encode_blocks_by_root_response(r)?
            }
            _ => return Err(protocol_mismatch(protocol.as_ref(), "response")),
        };
        write_framed(io, &bytes).await
    }
}

/// Writes `bytes` to the substream and signals end-of-message by
/// closing the write half. libp2p's req/resp framework relies on this
/// half-close to surface EOF to the peer's codec read.
async fn write_framed<T: AsyncWrite + Unpin + Send>(io: &mut T, bytes: &[u8]) -> io::Result<()> {
    io.write_all(bytes).await?;
    io.close().await
}

async fn read_to_end<T: AsyncRead + Unpin + Send>(io: &mut T) -> io::Result<Vec<u8>> {
    // Cap inbound payload to prevent OOM from a peer that keeps the
    // substream open and streams unbounded bytes. The frame-level uvarint
    // limit is enforced separately inside decode_*_frame; this is the
    // outer stream-level guard.
    //
    // `take(MAX + 1)` lets us distinguish a legitimate exact-cap read
    // (rare but allowed) from a peer overrunning the limit. Reading
    // strictly more than MAX bytes triggers the InvalidData error.
    let mut buf = Vec::new();
    let read = io
        .take(MAX_RPC_STREAM_BYTES + 1)
        .read_to_end(&mut buf)
        .await?;
    if u64::try_from(read).unwrap_or(u64::MAX) > MAX_RPC_STREAM_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "inbound rpc stream exceeded MAX_RPC_STREAM_BYTES ({MAX_RPC_STREAM_BYTES} bytes)"
            ),
        ));
    }
    Ok(buf)
}

fn encode_frame<T: ssz::Encode>(value: &T) -> io::Result<Vec<u8>> {
    let mut wire = Vec::new();
    write_req_resp_frame(&mut wire, &ssz::encode(value)).map_err(networking_err)?;
    Ok(wire)
}

fn encode_blocks_by_root_response(response: &BlocksByRootResponse) -> io::Result<Vec<u8>> {
    let mut wire = Vec::new();
    for block in response.blocks() {
        write_req_resp_frame(&mut wire, &ssz::encode(block)).map_err(networking_err)?;
    }
    Ok(wire)
}

fn decode_single_frame<T: ssz::Decode>(buf: &[u8]) -> io::Result<T> {
    let frames = decode_frames::<T>(buf)?;
    let frame_count = frames.len();
    let [frame] = frames.try_into().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected exactly one req/resp frame, got {frame_count}"),
        )
    })?;
    Ok(frame)
}

fn decode_blocks_by_root_response(buf: &[u8]) -> io::Result<BlocksByRootResponse> {
    let blocks = decode_frames::<SignedBlockWithAttestation>(buf)?;
    BlocksByRootResponse::new(blocks)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))
}

fn decode_frames<T: ssz::Decode>(buf: &[u8]) -> io::Result<Vec<T>> {
    let mut cursor = Cursor::new(buf);
    let mut frames = Vec::new();
    while let Some(ssz_bytes) =
        read_req_resp_frame(&mut cursor, Some(MAX_RPC_STREAM_BYTES)).map_err(networking_err)?
    {
        frames.push(ssz::decode::<T>(&ssz_bytes).map_err(ssz_err)?);
    }
    Ok(frames)
}

fn networking_err(err: lean_wire::NetworkingError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}

fn ssz_err(err: ssz::SszError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}

fn unknown_protocol(name: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("unknown rpc protocol: {name}"),
    )
}

fn protocol_mismatch(name: &str, kind: &'static str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("rpc {kind} variant does not match protocol {name}"),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use futures::io::Cursor;
    use lean_wire::{BlocksByRootRequest, BlocksByRootResponse, Status};
    use libp2p::request_response::Codec;
    use protocol::SignedBlockWithAttestation;

    fn status_proto() -> StreamProtocol {
        StreamProtocol::new(STATUS_PROTOCOL_V1.as_str())
    }

    fn blocks_proto() -> StreamProtocol {
        StreamProtocol::new(BLOCKS_BY_ROOT_PROTOCOL_V1.as_str())
    }

    #[tokio::test]
    async fn status_request_round_trip() {
        let mut codec = SszSnappyCodec;
        let req = RpcRequest::Status(Status::default());

        let mut wire = Vec::new();
        codec
            .write_request(&status_proto(), &mut wire, req.clone())
            .await
            .unwrap();

        let mut reader = Cursor::new(&wire[..]);
        let decoded = codec
            .read_request(&status_proto(), &mut reader)
            .await
            .unwrap();
        match decoded {
            RpcRequest::Status(s) => assert_eq!(s, Status::default()),
            RpcRequest::BlocksByRoot(_) => panic!("expected Status, got BlocksByRoot"),
        }
    }

    #[tokio::test]
    async fn blocks_by_root_request_round_trip() {
        let mut codec = SszSnappyCodec;
        let req = RpcRequest::BlocksByRoot(BlocksByRootRequest::new(std::iter::empty()).unwrap());

        let mut wire = Vec::new();
        codec
            .write_request(&blocks_proto(), &mut wire, req)
            .await
            .unwrap();

        let mut reader = Cursor::new(&wire[..]);
        let decoded = codec
            .read_request(&blocks_proto(), &mut reader)
            .await
            .unwrap();
        match decoded {
            RpcRequest::BlocksByRoot(r) => assert!(r.is_empty()),
            RpcRequest::Status(_) => panic!("expected BlocksByRoot, got Status"),
        }
    }

    #[tokio::test]
    async fn blocks_by_root_response_round_trip() {
        let mut codec = SszSnappyCodec;
        let resp = RpcResponse::BlocksByRoot(
            BlocksByRootResponse::new(vec![SignedBlockWithAttestation::default()]).unwrap(),
        );

        let mut wire = Vec::new();
        codec
            .write_response(&blocks_proto(), &mut wire, resp)
            .await
            .unwrap();

        let mut reader = Cursor::new(&wire[..]);
        let decoded = codec
            .read_response(&blocks_proto(), &mut reader)
            .await
            .unwrap();
        match decoded {
            RpcResponse::BlocksByRoot(r) => assert_eq!(r.blocks().len(), 1),
            RpcResponse::Status(_) => panic!("expected BlocksByRoot, got Status"),
        }
    }

    #[tokio::test]
    async fn unknown_protocol_rejected() {
        let mut codec = SszSnappyCodec;
        let unknown = StreamProtocol::new("/lean/unknown/1");
        let mut reader = Cursor::new(&b""[..]);
        let err = codec.read_request(&unknown, &mut reader).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }

    #[tokio::test]
    async fn variant_protocol_mismatch_rejected() {
        let mut codec = SszSnappyCodec;
        let mut wire = Vec::new();
        // Status request on the blocks_by_root protocol → mismatch.
        let err = codec
            .write_request(
                &blocks_proto(),
                &mut wire,
                RpcRequest::Status(Status::default()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
