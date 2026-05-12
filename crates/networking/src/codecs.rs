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
//!
//! Every `encode_*` here returns [`Vec<u8>`] directly: `io::Write` into
//! [`Vec`] is infallible by construction, and `snap::raw::Encoder` only
//! errors on inputs exceeding 4 GiB (impossible for SSZ payloads in this
//! crate, which are bounded far below that).

// Justification: every `.expect(_)` below collapses a documented
// infallible code path — `io::Write` into `Vec<u8>` cannot fail, and the
// Snappy block encoder only rejects inputs >4 GiB.
#![allow(clippy::expect_used)]

use std::io::{self, Read, Write};

use snap::raw;
use snap::read::FrameDecoder;
use snap::write::FrameEncoder;
use ssz::{decode, encode, Decode, Encode};

use crate::error::NetworkingError;

const INFALLIBLE_VEC_WRITE: &str = "io::Write into Vec<u8> is infallible";
const INFALLIBLE_SNAPPY_BLOCK: &str = "Snappy block encoder rejects only >4 GiB inputs";

// =============================================================================
// req/resp framed wire
// =============================================================================

/// Wraps `ssz_bytes` in Snappy framed stream encoding.
///
/// # Panics
/// Statically infallible — `io::Write` into [`Vec<u8>`] cannot fail.
#[must_use]
pub fn encode_req_resp_wire(ssz_bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ssz_bytes.len());
    let mut encoder = FrameEncoder::new(&mut out);
    encoder.write_all(ssz_bytes).expect(INFALLIBLE_VEC_WRITE);
    encoder.into_inner().expect(INFALLIBLE_VEC_WRITE);
    out
}

/// Unwraps Snappy framed stream bytes into the underlying SSZ bytes.
///
/// # Errors
/// [`NetworkingError::Snappy`] if `wire` is not a valid Snappy framed stream.
pub fn decode_req_resp_wire(wire: &[u8]) -> Result<Vec<u8>, NetworkingError> {
    let mut out = Vec::with_capacity(wire.len());
    FrameDecoder::new(wire)
        .read_to_end(&mut out)
        .map_err(snap_or_io)?;
    Ok(out)
}

/// SSZ-encode `value`, then wrap in Snappy framed bytes.
///
/// # Panics
/// Statically infallible — see [`encode_req_resp_wire`].
#[must_use]
pub fn encode_req_resp<T: Encode>(value: &T) -> Vec<u8> {
    encode_req_resp_wire(&encode(value))
}

/// Unwrap Snappy framed bytes, then SSZ-decode into `T`.
///
/// # Errors
/// [`NetworkingError::Snappy`] for framing failures;
/// [`NetworkingError::Ssz`] for SSZ payload failures.
pub fn decode_req_resp<T: Decode>(wire: &[u8]) -> Result<T, NetworkingError> {
    let ssz_bytes = decode_req_resp_wire(wire)?;
    decode(&ssz_bytes).map_err(Into::into)
}

// =============================================================================
// gossip block compression
// =============================================================================

/// Snappy-block-compresses `ssz_bytes` for gossipsub transport.
///
/// # Panics
/// On inputs exceeding 4 GiB, which exceeds every SSZ payload bound in
/// this crate by several orders of magnitude.
#[must_use]
pub fn encode_gossip_data(ssz_bytes: &[u8]) -> Vec<u8> {
    raw::Encoder::new()
        .compress_vec(ssz_bytes)
        .expect(INFALLIBLE_SNAPPY_BLOCK)
}

/// Snappy-block-decompresses gossipsub data into the underlying SSZ bytes.
///
/// # Errors
/// [`NetworkingError::Snappy`] if `data` is not valid Snappy block output.
pub fn decode_gossip_data(data: &[u8]) -> Result<Vec<u8>, NetworkingError> {
    raw::Decoder::new()
        .decompress_vec(data)
        .map_err(NetworkingError::Snappy)
}

/// SSZ-encode `value`, then Snappy-block-compress.
///
/// # Panics
/// See [`encode_gossip_data`].
#[must_use]
pub fn encode_gossip<T: Encode>(value: &T) -> Vec<u8> {
    encode_gossip_data(&encode(value))
}

/// Snappy-block-decompress, then SSZ-decode into `T`.
///
/// # Errors
/// [`NetworkingError::Snappy`] for decompression failures;
/// [`NetworkingError::Ssz`] for SSZ payload failures.
pub fn decode_gossip<T: Decode>(data: &[u8]) -> Result<T, NetworkingError> {
    let ssz_bytes = decode_gossip_data(data)?;
    decode(&ssz_bytes).map_err(Into::into)
}

// =============================================================================
// helpers
// =============================================================================

/// Routes a [`FrameDecoder`] failure to the matching [`NetworkingError`] arm.
///
/// `snap::read::FrameDecoder` reports protocol-level framing failures by
/// wrapping a [`snap::Error`] inside an [`io::Error`]. Surface that as the
/// typed [`NetworkingError::Snappy`] so tests can `matches!` on it; fall
/// back to [`NetworkingError::Io`] for genuine I/O errors.
fn snap_or_io(err: io::Error) -> NetworkingError {
    err.downcast::<snap::Error>()
        .map_or_else(NetworkingError::Io, NetworkingError::Snappy)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn req_resp_wire_round_trips() {
        let ssz: &[u8] = b"hello world";
        let wire = encode_req_resp_wire(ssz);
        assert_ne!(wire, ssz, "wire should differ from raw ssz");
        assert_eq!(decode_req_resp_wire(&wire).unwrap(), ssz);
    }

    #[test]
    fn req_resp_wire_handles_empty_payload() {
        let wire = encode_req_resp_wire(&[]);
        assert!(decode_req_resp_wire(&wire).unwrap().is_empty());
    }

    #[test]
    fn gossip_data_round_trips() {
        let ssz: &[u8] = b"signed block placeholder";
        let data = encode_gossip_data(ssz);
        assert_eq!(decode_gossip_data(&data).unwrap(), ssz);
    }

    #[test]
    fn req_resp_wire_rejects_gossip_bytes() {
        let gossip = encode_gossip_data(b"payload");
        let err = decode_req_resp_wire(&gossip).unwrap_err();
        assert!(
            matches!(err, NetworkingError::Snappy(_)),
            "expected Snappy error, got {err:?}"
        );
    }

    #[test]
    fn gossip_rejects_req_resp_bytes() {
        let wire = encode_req_resp_wire(b"payload");
        let err = decode_gossip_data(&wire).unwrap_err();
        assert!(
            matches!(err, NetworkingError::Snappy(_)),
            "expected Snappy error, got {err:?}"
        );
    }

    #[test]
    fn req_resp_rejects_truncated_wire() {
        let wire = encode_req_resp_wire(b"payload");
        let err = decode_req_resp_wire(&wire[..wire.len() / 2]).unwrap_err();
        assert!(
            matches!(err, NetworkingError::Snappy(_) | NetworkingError::Io(_)),
            "expected framing failure, got {err:?}"
        );
    }
}
