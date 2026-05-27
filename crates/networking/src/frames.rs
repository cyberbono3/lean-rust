//! Length-prefixed req/resp stream framing.
//!
//! Wire shape per frame:
//!
//! ```text
//! uvarint(uncompressed_ssz_length) || snappy_framed(ssz_bytes)
//! ```
//!
//! Multiple frames may be concatenated on a single stream — the
//! `BlocksByRoot` response shape is a sequence of one such frame per
//! `SignedBlock` chunk.
//! [`read_req_resp_frame`] therefore must consume exactly the bytes that
//! belong to one frame, leaving the cursor positioned at the start of the
//! next.
//!
//! # Why a hand-rolled Snappy chunk parser
//!
//! [`snap::read::FrameDecoder`] buffers its inner reader internally. After
//! it has produced `length` decoded bytes, the inner cursor has already
//! advanced past the last compressed chunk — a second `read_req_resp_frame`
//! call would start mid-chunk. We therefore parse Snappy framed chunks by
//! hand (one 4-byte header + one payload at a time), delegate decompression
//! to [`snap::raw::Decoder`], and validate the Castagnoli CRC32 with the
//! `crc32c` crate. This matches the byte-exact semantics of every other
//! conformant Snappy framed reader without surrendering frame boundaries.
//!
//! The [`crate::codecs`] one-shot variant uses [`snap::read::FrameDecoder`]
//! directly, since the slice boundary already coincides with the frame
//! boundary there.

use std::borrow::Cow;
use std::io::{self, Read, Write};

use snap::raw;
use snap::write::FrameEncoder;

use crate::error::NetworkingError;

const SNAPPY_STREAM_IDENTIFIER: u8 = 0xff;
const SNAPPY_CHUNK_COMPRESSED: u8 = 0x00;
const SNAPPY_CHUNK_UNCOMPRESSED: u8 = 0x01;
const SNAPPY_STREAM_MAGIC: &[u8; 6] = b"sNaPpY";
const SNAPPY_CHECKSUM_LEN: usize = 4;
const SNAPPY_CHUNK_HEADER_LEN: usize = 4;
const UVARINT_MAX_LEN: usize = 10;

// =============================================================================
// public surface
// =============================================================================

/// Writes one req/resp frame to `w`.
///
/// Emits `uvarint(ssz.len())` followed by the Snappy framed encoding of
/// `ssz`.
///
/// # Errors
/// [`NetworkingError::Io`] for sink failures during length or payload write.
pub fn write_req_resp_frame<W: Write>(w: &mut W, ssz: &[u8]) -> Result<(), NetworkingError> {
    let length = u64::try_from(ssz.len()).map_err(|_| NetworkingError::MalformedFrame {
        reason: "ssz length exceeds u64",
    })?;
    write_uvarint(w, length)?;

    let mut encoder = FrameEncoder::new(w);
    encoder.write_all(ssz)?;
    encoder
        .into_inner()
        .map_err(|e| NetworkingError::Io(e.into_error()))?;
    Ok(())
}

/// Reads one req/resp frame from `r`.
///
/// Reads the uvarint length prefix, validates it against `max_uncompressed`,
/// then decodes exactly the matching number of Snappy framed bytes. Pass
/// `None` to disable the length cap (only safe when an outer protocol
/// guarantees an upper bound). Returns:
///
/// - `Ok(Some(ssz_bytes))` — one decoded frame.
/// - `Ok(None)` — clean EOF before any byte was consumed (the response
///   stream is exhausted).
/// - `Err(_)` — malformed framing or a mid-frame I/O failure.
///
/// # Errors
/// [`NetworkingError::Io`] for stream I/O failures, [`NetworkingError::Snappy`]
/// for raw-decompression failures, [`NetworkingError::MalformedFrame`] for
/// structural violations (bad CRC, unknown chunk type, missing magic),
/// [`NetworkingError::FrameTooLarge`] when the declared length exceeds the
/// caller-supplied cap, and [`NetworkingError::UvarintOverflow`] for an
/// 11-byte length prefix.
pub fn read_req_resp_frame<R: Read>(
    r: &mut R,
    max_uncompressed: Option<u64>,
) -> Result<Option<Vec<u8>>, NetworkingError> {
    let Some(length) = read_uvarint_or_eof(r)? else {
        return Ok(None);
    };
    if let Some(max) = max_uncompressed {
        if length > max {
            return Err(NetworkingError::FrameTooLarge { length, max });
        }
    }
    read_snappy_frame_exact(r, length).map(Some)
}

// =============================================================================
// uvarint
// =============================================================================

/// Writes an LEB128 (`PutUvarint`) length prefix.
#[allow(clippy::cast_possible_truncation)]
fn write_uvarint<W: Write>(w: &mut W, mut value: u64) -> Result<(), NetworkingError> {
    let mut buf = [0_u8; UVARINT_MAX_LEN];
    let mut i = 0;
    while value >= 0x80 {
        buf[i] = (value as u8) | 0x80;
        value >>= 7;
        i += 1;
    }
    buf[i] = value as u8;
    w.write_all(&buf[..=i])?;
    Ok(())
}

/// Reads an LEB128 uvarint. Returns `Ok(None)` on clean EOF before any
/// byte was consumed; otherwise the decoded value or an error.
fn read_uvarint_or_eof<R: Read>(r: &mut R) -> Result<Option<u64>, NetworkingError> {
    let mut acc: u64 = 0;
    let mut shift: u32 = 0;
    for i in 0..UVARINT_MAX_LEN {
        let byte = match read_one_byte(r)? {
            Some(b) => b,
            None if i == 0 => return Ok(None),
            None => return Err(NetworkingError::Io(io::ErrorKind::UnexpectedEof.into())),
        };
        if byte < 0x80 {
            if i == UVARINT_MAX_LEN - 1 && byte > 1 {
                return Err(NetworkingError::UvarintOverflow);
            }
            return Ok(Some(acc | (u64::from(byte) << shift)));
        }
        acc |= u64::from(byte & 0x7f) << shift;
        shift += 7;
    }
    Err(NetworkingError::UvarintOverflow)
}

/// Reads a single byte, returning `Ok(None)` on clean EOF.
fn read_one_byte<R: Read>(r: &mut R) -> Result<Option<u8>, NetworkingError> {
    let mut byte = [0_u8; 1];
    match r.read(&mut byte) {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(byte[0])),
        Err(err) => Err(NetworkingError::Io(err)),
    }
}

// =============================================================================
// Snappy chunk parser
// =============================================================================

/// Upper bound on the number of Snappy framed chunks we'll consume for
/// a single `read_snappy_frame_exact` call. A peer cannot keep us
/// spinning indefinitely on zero-progress chunks (skippable padding,
/// duplicate identifiers) without crossing this fault threshold.
const MAX_SNAPPY_CHUNK_ITERATIONS: usize = 4096;

/// Reads exactly `length` decoded bytes from `r` by walking Snappy framed
/// chunks until the accumulated payload size equals `length`.
fn read_snappy_frame_exact<R: Read>(r: &mut R, length: u64) -> Result<Vec<u8>, NetworkingError> {
    if length == 0 {
        return Ok(Vec::new());
    }
    let capacity = usize::try_from(length).map_err(|_| NetworkingError::MalformedFrame {
        reason: "uncompressed length exceeds usize",
    })?;
    // try_reserve_exact maps a failed allocation to a typed error
    // instead of aborting the process via the alloc-failure handler;
    // critical when `capacity` comes from an attacker-supplied uvarint.
    let mut decoded: Vec<u8> = Vec::new();
    decoded
        .try_reserve_exact(capacity)
        .map_err(|_| NetworkingError::MalformedFrame {
            reason: "snappy frame allocation exceeded available memory",
        })?;
    let mut remaining = length;
    let mut seen_identifier = false;
    let mut iterations: usize = 0;

    while remaining > 0 {
        iterations += 1;
        if iterations > MAX_SNAPPY_CHUNK_ITERATIONS {
            return Err(NetworkingError::MalformedFrame {
                reason: "snappy chunk iteration cap exceeded",
            });
        }
        // Each chunk read bounds its allocation against `remaining + chunk
        // overhead` so a peer cannot keep declaring max-size chunks beyond
        // what the outer length permits.
        let read_budget = remaining.saturating_add(SNAPPY_CHECKSUM_LEN as u64);
        let (chunk_type, payload) = read_snappy_chunk(r, read_budget)?;
        match chunk_type {
            SNAPPY_STREAM_IDENTIFIER => {
                if seen_identifier {
                    return Err(NetworkingError::MalformedFrame {
                        reason: "duplicate snappy stream identifier",
                    });
                }
                if payload.as_slice() != SNAPPY_STREAM_MAGIC {
                    return Err(NetworkingError::MalformedFrame {
                        reason: "invalid snappy stream identifier",
                    });
                }
                seen_identifier = true;
            }
            SNAPPY_CHUNK_COMPRESSED | SNAPPY_CHUNK_UNCOMPRESSED => {
                if !seen_identifier {
                    return Err(NetworkingError::MalformedFrame {
                        reason: "snappy data chunk before stream identifier",
                    });
                }
                let data = if chunk_type == SNAPPY_CHUNK_COMPRESSED {
                    Cow::Owned(decode_compressed_chunk(&payload)?)
                } else {
                    Cow::Borrowed(decode_uncompressed_chunk(&payload)?)
                };
                let data_len =
                    u64::try_from(data.len()).map_err(|_| NetworkingError::MalformedFrame {
                        reason: "snappy chunk length exceeds u64",
                    })?;
                if data_len > remaining {
                    return Err(NetworkingError::MalformedFrame {
                        reason: "snappy chunk decoded length exceeds remaining",
                    });
                }
                decoded.extend_from_slice(&data);
                remaining -= data_len;
            }
            0x02..=0x7f => {
                return Err(NetworkingError::MalformedFrame {
                    reason: "unsupported snappy chunk type",
                });
            }
            // 0x80..=0xfe — reserved skippable / padding.
            _ => {}
        }
    }
    Ok(decoded)
}

/// Reads one Snappy framed chunk: 1-byte type, 3-byte LE length, payload.
/// `max_payload_len` caps the per-chunk allocation against the caller's
/// outer length budget, preventing a peer from declaring a 16 MiB chunk
/// when only a few bytes are owed.
fn read_snappy_chunk<R: Read>(
    r: &mut R,
    max_payload_len: u64,
) -> Result<(u8, Vec<u8>), NetworkingError> {
    let mut header = [0_u8; SNAPPY_CHUNK_HEADER_LEN];
    r.read_exact(&mut header)?;
    let chunk_type = header[0];
    let len =
        usize::from(header[1]) | (usize::from(header[2]) << 8) | (usize::from(header[3]) << 16);
    if u64::try_from(len).unwrap_or(u64::MAX) > max_payload_len {
        return Err(NetworkingError::MalformedFrame {
            reason: "snappy chunk payload length exceeds caller budget",
        });
    }
    let mut payload: Vec<u8> = Vec::new();
    payload
        .try_reserve_exact(len)
        .map_err(|_| NetworkingError::MalformedFrame {
            reason: "snappy chunk allocation exceeded available memory",
        })?;
    payload.resize(len, 0);
    r.read_exact(&mut payload)?;
    Ok((chunk_type, payload))
}

fn decode_compressed_chunk(payload: &[u8]) -> Result<Vec<u8>, NetworkingError> {
    let (checksum, body) = split_checksum(payload)?;
    let decoded = raw::Decoder::new()
        .decompress_vec(body)
        .map_err(NetworkingError::Snappy)?;
    if snappy_masked_crc(&decoded) != checksum {
        return Err(NetworkingError::MalformedFrame {
            reason: "snappy chunk crc mismatch",
        });
    }
    Ok(decoded)
}

fn decode_uncompressed_chunk(payload: &[u8]) -> Result<&[u8], NetworkingError> {
    let (checksum, data) = split_checksum(payload)?;
    if snappy_masked_crc(data) != checksum {
        return Err(NetworkingError::MalformedFrame {
            reason: "snappy chunk crc mismatch",
        });
    }
    Ok(data)
}

fn split_checksum(payload: &[u8]) -> Result<(u32, &[u8]), NetworkingError> {
    let (head, rest) = payload.split_first_chunk::<SNAPPY_CHECKSUM_LEN>().ok_or(
        NetworkingError::MalformedFrame {
            reason: "snappy chunk missing checksum",
        },
    )?;
    Ok((u32::from_le_bytes(*head), rest))
}

/// Snappy framing applies a masking transform to the raw CRC32C so a stored
/// checksum can't collide with the compressed payload's framing bytes.
fn snappy_masked_crc(bytes: &[u8]) -> u32 {
    let crc = crc32c::crc32c(bytes);
    crc.rotate_right(15).wrapping_add(0xa282_ead8)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn roundtrip_uvarint(value: u64, expected_len: usize) {
        let mut buf = Vec::new();
        write_uvarint(&mut buf, value).unwrap();
        assert_eq!(buf.len(), expected_len, "encoded length for {value}");
        let mut cursor = Cursor::new(buf);
        let decoded = read_uvarint_or_eof(&mut cursor).unwrap();
        assert_eq!(decoded, Some(value));
    }

    #[test]
    fn uvarint_round_trips_boundaries() {
        roundtrip_uvarint(0, 1);
        roundtrip_uvarint(127, 1);
        roundtrip_uvarint(128, 2);
        roundtrip_uvarint(16_383, 2);
        roundtrip_uvarint(16_384, 3);
        roundtrip_uvarint(u64::MAX, 10);
    }

    #[test]
    fn uvarint_eof_before_first_byte_is_none() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        assert_eq!(read_uvarint_or_eof(&mut cursor).unwrap(), None);
    }

    #[test]
    fn uvarint_eof_mid_value_is_error() {
        // Single continuation byte without a terminator.
        let mut cursor = Cursor::new(vec![0x80_u8]);
        let err = read_uvarint_or_eof(&mut cursor).unwrap_err();
        assert!(matches!(err, NetworkingError::Io(_)));
    }

    #[test]
    fn uvarint_overflow_rejects_11th_byte() {
        let buf = vec![0xff_u8; 11];
        let mut cursor = Cursor::new(buf);
        let err = read_uvarint_or_eof(&mut cursor).unwrap_err();
        assert!(matches!(err, NetworkingError::UvarintOverflow));
    }

    #[test]
    fn frame_round_trip_single() {
        let payload = b"single-frame payload";
        let mut buf = Vec::new();
        write_req_resp_frame(&mut buf, payload).unwrap();
        let mut cursor = Cursor::new(buf);
        let back = read_req_resp_frame(&mut cursor, None).unwrap().unwrap();
        assert_eq!(back, payload);
        assert_eq!(
            read_req_resp_frame(&mut cursor, None).unwrap(),
            None,
            "no more frames",
        );
    }

    #[test]
    fn frame_round_trip_multi_chunk() {
        let a = b"first chunk";
        let b = b"second chunk";
        let mut buf = Vec::new();
        write_req_resp_frame(&mut buf, a).unwrap();
        write_req_resp_frame(&mut buf, b).unwrap();
        let mut cursor = Cursor::new(buf);
        assert_eq!(read_req_resp_frame(&mut cursor, None).unwrap().unwrap(), a,);
        assert_eq!(read_req_resp_frame(&mut cursor, None).unwrap().unwrap(), b,);
        assert_eq!(read_req_resp_frame(&mut cursor, None).unwrap(), None);
    }

    #[test]
    fn frame_empty_payload_round_trips() {
        let mut buf = Vec::new();
        write_req_resp_frame(&mut buf, &[]).unwrap();
        let mut cursor = Cursor::new(buf);
        let back = read_req_resp_frame(&mut cursor, None).unwrap().unwrap();
        assert!(back.is_empty());
    }

    #[test]
    fn frame_rejects_length_over_cap() {
        let mut buf = Vec::new();
        write_req_resp_frame(&mut buf, b"abcdef").unwrap();
        let mut cursor = Cursor::new(buf);
        let err = read_req_resp_frame(&mut cursor, Some(3)).unwrap_err();
        assert!(
            matches!(err, NetworkingError::FrameTooLarge { length: 6, max: 3 }),
            "got {err:?}"
        );
    }

    #[test]
    fn frame_detects_crc_mismatch() {
        let mut buf = Vec::new();
        write_req_resp_frame(&mut buf, b"crc-test payload").unwrap();
        // Flip a bit near the tail of the snappy body — depending on
        // whether the flip lands inside the compressed bytes or inside
        // the CRC field, the reader returns either MalformedFrame (CRC
        // verifies after decompression) or Snappy (decompression fails
        // first).
        let flip_index = buf.len() - 1;
        buf[flip_index] ^= 0x01;
        let mut cursor = Cursor::new(buf);
        let err = read_req_resp_frame(&mut cursor, None).unwrap_err();
        assert!(
            matches!(
                err,
                NetworkingError::MalformedFrame { reason }
                    if reason.contains("crc mismatch")
            ) || matches!(err, NetworkingError::Snappy(_)),
            "got {err:?}",
        );
    }

    #[test]
    fn frame_detects_short_eof_mid_frame() {
        let mut buf = Vec::new();
        write_req_resp_frame(&mut buf, b"will be truncated").unwrap();
        // Drop the last 5 bytes to force ReadExact failure inside the
        // chunk parser.
        buf.truncate(buf.len() - 5);
        let mut cursor = Cursor::new(buf);
        let err = read_req_resp_frame(&mut cursor, None).unwrap_err();
        assert!(matches!(err, NetworkingError::Io(_)), "got {err:?}");
    }

    #[test]
    fn snappy_masked_crc_matches_reference_zero_byte() {
        // For a single zero byte the unmasked CRC32C is 0x527d5351.
        // Masked: rot_right(15) + 0xa282_ead8.
        let raw: u32 = 0x527d_5351;
        let expected = raw.rotate_right(15).wrapping_add(0xa282_ead8);
        assert_eq!(snappy_masked_crc(&[0]), expected);
    }
}
