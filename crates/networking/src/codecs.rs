//! Snappy-wrapped req/resp + gossip byte transforms.
//!
//! Two wire formats live in this module:
//!
//! - **Req/resp wire bytes** — SSZ payload wrapped in Snappy *framed* stream
//!   encoding ("sNaPpY" magic + length-prefixed chunks with masked
//!   Castagnoli CRC32). Used for one-shot encode/decode on byte slices; the
//!   streaming variant lives in [`crate::frames`].
//! - **Gossip data** — SSZ payload wrapped in Snappy *block* (raw)
//!   compression. No framing, no checksum.
//!
//! The two formats are deliberately not interchangeable; feeding gossip
//! bytes to [`decode_req_resp_wire`] (or vice versa) returns
//! [`NetworkingError::Snappy`].
//!
//! [`encode_req_resp`] and [`decode_req_resp`] are generic over any
//! [`ssz::Encode`] / [`ssz::Decode`] type, so callers don't need a
//! hand-rolled wrapper per payload type.

use std::io::{self, Read, Write};

use snap::raw;
use snap::read::FrameDecoder;
use snap::write::FrameEncoder;
use ssz::{decode, encode, Decode, Encode};

use crate::error::NetworkingError;

// =============================================================================
// req/resp framed wire
// =============================================================================

/// Wraps `ssz` bytes in Snappy framed stream encoding.
///
/// # Errors
/// [`NetworkingError::Io`] if the framed-stream encoder reports an internal
/// I/O failure (in practice never, since the sink is a [`Vec`]).
pub fn encode_req_resp_wire(ssz_bytes: &[u8]) -> Result<Vec<u8>, NetworkingError> {
    let mut out = Vec::with_capacity(ssz_bytes.len());
    {
        let mut encoder = FrameEncoder::new(&mut out);
        encoder.write_all(ssz_bytes)?;
        encoder
            .into_inner()
            .map_err(|e| NetworkingError::Io(e.into_error()))?;
    }
    Ok(out)
}

/// Unwraps Snappy framed stream bytes into the underlying `ssz` bytes.
///
/// # Errors
/// [`NetworkingError::Snappy`] if `wire` is not a valid Snappy framed stream.
pub fn decode_req_resp_wire(wire: &[u8]) -> Result<Vec<u8>, NetworkingError> {
    let mut decoder = FrameDecoder::new(wire);
    let mut out = Vec::with_capacity(wire.len());
    decoder.read_to_end(&mut out).map_err(decoder_io_error)?;
    Ok(out)
}

/// Generic req/resp encoder: SSZ-encode `value`, wrap in Snappy framed bytes.
///
/// # Errors
/// Propagates [`encode_req_resp_wire`].
pub fn encode_req_resp<T: Encode>(value: &T) -> Result<Vec<u8>, NetworkingError> {
    encode_req_resp_wire(&encode(value))
}

/// Generic req/resp decoder: unwrap Snappy framed bytes, SSZ-decode into `T`.
///
/// # Errors
/// [`NetworkingError::Snappy`] for framing failures;
/// [`NetworkingError::Ssz`] for SSZ payload failures.
pub fn decode_req_resp<T: Decode>(wire: &[u8]) -> Result<T, NetworkingError> {
    Ok(decode::<T>(&decode_req_resp_wire(wire)?)?)
}

// =============================================================================
// gossip block compression
// =============================================================================

/// Snappy block-compresses `ssz_bytes` for gossipsub transport.
///
/// # Errors
/// [`NetworkingError::Snappy`] if `snap::raw::Encoder::compress_vec` rejects
/// the input (only `Error::TooBig` for inputs over 4 GiB).
pub fn encode_gossip_data(ssz_bytes: &[u8]) -> Result<Vec<u8>, NetworkingError> {
    raw::Encoder::new()
        .compress_vec(ssz_bytes)
        .map_err(NetworkingError::Snappy)
}

/// Snappy block-decompresses gossipsub data into the underlying `ssz` bytes.
///
/// # Errors
/// [`NetworkingError::Snappy`] if `data` is not valid Snappy block output.
pub fn decode_gossip_data(data: &[u8]) -> Result<Vec<u8>, NetworkingError> {
    raw::Decoder::new()
        .decompress_vec(data)
        .map_err(NetworkingError::Snappy)
}

/// Generic gossip encoder: SSZ-encode `value`, Snappy-block-compress.
///
/// # Errors
/// Propagates [`encode_gossip_data`].
pub fn encode_gossip<T: Encode>(value: &T) -> Result<Vec<u8>, NetworkingError> {
    encode_gossip_data(&encode(value))
}

/// Generic gossip decoder: Snappy-block-decompress, SSZ-decode into `T`.
///
/// # Errors
/// [`NetworkingError::Snappy`] for decompression failures;
/// [`NetworkingError::Ssz`] for SSZ payload failures.
pub fn decode_gossip<T: Decode>(data: &[u8]) -> Result<T, NetworkingError> {
    Ok(decode::<T>(&decode_gossip_data(data)?)?)
}

// =============================================================================
// helpers
// =============================================================================

/// Maps an `io::Error` from [`FrameDecoder`] to either a Snappy or I/O variant.
///
/// `snap::read::FrameDecoder` reports protocol-level framing failures by
/// wrapping a [`snap::Error`] inside an [`io::Error`]. Surface that as the
/// typed [`NetworkingError::Snappy`] so tests can `matches!` on it; fall
/// back to [`NetworkingError::Io`] for genuine I/O errors.
fn decoder_io_error(err: io::Error) -> NetworkingError {
    let kind = err.kind();
    let Some(inner) = err.into_inner() else {
        return NetworkingError::Io(io::Error::from(kind));
    };
    match inner.downcast::<snap::Error>() {
        Ok(snap_err) => NetworkingError::Snappy(*snap_err),
        Err(other) => NetworkingError::Io(io::Error::new(kind, other)),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn req_resp_wire_round_trips() {
        let ssz = b"hello world".as_slice();
        let wire = encode_req_resp_wire(ssz).unwrap();
        assert_ne!(wire, ssz, "wire should differ from raw ssz");
        let back = decode_req_resp_wire(&wire).unwrap();
        assert_eq!(back, ssz);
    }

    #[test]
    fn req_resp_wire_handles_empty_payload() {
        let wire = encode_req_resp_wire(&[]).unwrap();
        let back = decode_req_resp_wire(&wire).unwrap();
        assert!(back.is_empty());
    }

    #[test]
    fn gossip_data_round_trips() {
        let ssz = b"signed block placeholder".as_slice();
        let data = encode_gossip_data(ssz).unwrap();
        let back = decode_gossip_data(&data).unwrap();
        assert_eq!(back, ssz);
    }

    #[test]
    fn req_resp_wire_rejects_gossip_bytes() {
        let gossip = encode_gossip_data(b"payload").unwrap();
        let err = decode_req_resp_wire(&gossip).unwrap_err();
        assert!(
            matches!(err, NetworkingError::Snappy(_)),
            "expected Snappy error, got {err:?}"
        );
    }

    #[test]
    fn gossip_rejects_req_resp_bytes() {
        let wire = encode_req_resp_wire(b"payload").unwrap();
        let err = decode_gossip_data(&wire).unwrap_err();
        assert!(
            matches!(err, NetworkingError::Snappy(_)),
            "expected Snappy error, got {err:?}"
        );
    }

    #[test]
    fn req_resp_rejects_truncated_wire() {
        let wire = encode_req_resp_wire(b"payload").unwrap();
        let err = decode_req_resp_wire(&wire[..wire.len() / 2]).unwrap_err();
        assert!(
            matches!(err, NetworkingError::Snappy(_) | NetworkingError::Io(_)),
            "expected framing failure, got {err:?}"
        );
    }
}
