//! Replays the SSZ wire-parity corpus through every networking codec.
//!
//! For each `<name>.<container>.ssz` fixture the test asserts three
//! invariants:
//!
//! 1. **SSZ byte-parity** — decode → re-encode reproduces the fixture bytes.
//! 2. **Snappy framed wire round-trip** — `encode_req_resp_wire` then
//!    `decode_req_resp_wire` recovers the original SSZ.
//! 3. **Length-prefixed stream round-trip** — `write_req_resp_frame` then
//!    `read_req_resp_frame` recovers the original SSZ and exhausts the
//!    cursor.
//!
//! See `tests/data/PROVENANCE.md` for the source of each fixture and the
//! list of containers it covers.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::fmt::Debug;
use std::io::Cursor;

use networking::{
    decode_req_resp, decode_req_resp_wire, encode_req_resp, encode_req_resp_wire,
    read_req_resp_frame, write_req_resp_frame, NetworkingError, Status,
};
use protocol::{Block, BlockBody, BlockHeader, Checkpoint, SignedBlock, SignedVote, State, Vote};
use ssz::{decode, encode, Decode, Encode};

// =============================================================================
// per-fixture entries
// =============================================================================

const FIXTURES: &[(&str, &[u8])] = &[
    (
        "empty.blockbody",
        include_bytes!("data/wire-parity/empty.blockbody.ssz"),
    ),
    (
        "two-votes.blockbody",
        include_bytes!("data/wire-parity/two-votes.blockbody.ssz"),
    ),
    (
        "genesis-4val.state",
        include_bytes!("data/wire-parity/genesis-4val.state.ssz"),
    ),
    (
        "genesis-anchor.checkpoint",
        include_bytes!("data/wire-parity/genesis-anchor.checkpoint.ssz"),
    ),
    (
        "slot12-justified.checkpoint",
        include_bytes!("data/wire-parity/slot12-justified.checkpoint.ssz"),
    ),
    (
        "slot1.blockheader",
        include_bytes!("data/wire-parity/slot1.blockheader.ssz"),
    ),
    (
        "slot1-empty.block",
        include_bytes!("data/wire-parity/slot1-empty.block.ssz"),
    ),
    (
        "slot1-empty.signedblock",
        include_bytes!("data/wire-parity/slot1-empty.signedblock.ssz"),
    ),
    (
        "slot7-vote.vote",
        include_bytes!("data/wire-parity/slot7-vote.vote.ssz"),
    ),
    (
        "validator3-vote.signedvote",
        include_bytes!("data/wire-parity/validator3-vote.signedvote.ssz"),
    ),
];

fn fixture(name: &str) -> &'static [u8] {
    FIXTURES
        .iter()
        .find(|(n, _)| *n == name)
        .map_or_else(|| panic!("unknown fixture {name}"), |(_, bytes)| *bytes)
}

// =============================================================================
// generic round-trip across all three layers
// =============================================================================

fn assert_round_trip<T>(name: &str, fixture: &[u8])
where
    T: Encode + Decode + PartialEq + Debug,
{
    // (1) SSZ byte-parity.
    let value: T = decode(fixture).expect("ssz decode");
    let re_encoded = encode(&value);
    assert_eq!(
        re_encoded, fixture,
        "{name}: ssz byte-parity (decode → encode mismatch)",
    );

    // (2) Snappy framed wire round-trip on raw SSZ bytes.
    let wire = encode_req_resp_wire(fixture);
    let unwrapped = decode_req_resp_wire(&wire).expect("framed decode");
    assert_eq!(unwrapped, fixture, "{name}: framed wire round-trip");

    // (3) Length-prefixed stream round-trip.
    let mut stream = Vec::new();
    write_req_resp_frame(&mut stream, fixture).expect("write frame");
    let mut cursor = Cursor::new(stream);
    let read_back = read_req_resp_frame(&mut cursor, None)
        .expect("read frame")
        .expect("frame present");
    assert_eq!(read_back, fixture, "{name}: stream round-trip");
    assert_eq!(
        read_req_resp_frame(&mut cursor, None).expect("post-frame eof"),
        None,
        "{name}: stream cursor must be exhausted",
    );

    // (4) Generic value-level codec sanity (skip Status here — covered
    // separately because it isn't in the fixture corpus).
    let value_back: T = decode_req_resp(&encode_req_resp(&value)).expect("value decode");
    assert_eq!(value_back, value, "{name}: typed req/resp round-trip");
}

// =============================================================================
// per-container tests
// =============================================================================

#[test]
fn empty_blockbody_round_trip() {
    assert_round_trip::<BlockBody>("empty.blockbody", fixture("empty.blockbody"));
}

#[test]
fn two_votes_blockbody_round_trip() {
    assert_round_trip::<BlockBody>("two-votes.blockbody", fixture("two-votes.blockbody"));
}

#[test]
fn genesis_state_round_trip() {
    assert_round_trip::<State>("genesis-4val.state", fixture("genesis-4val.state"));
}

#[test]
fn genesis_checkpoint_round_trip() {
    assert_round_trip::<Checkpoint>(
        "genesis-anchor.checkpoint",
        fixture("genesis-anchor.checkpoint"),
    );
}

#[test]
fn justified_checkpoint_round_trip() {
    assert_round_trip::<Checkpoint>(
        "slot12-justified.checkpoint",
        fixture("slot12-justified.checkpoint"),
    );
}

#[test]
fn blockheader_round_trip() {
    assert_round_trip::<BlockHeader>("slot1.blockheader", fixture("slot1.blockheader"));
}

#[test]
fn block_round_trip() {
    assert_round_trip::<Block>("slot1-empty.block", fixture("slot1-empty.block"));
}

#[test]
fn signed_block_round_trip() {
    assert_round_trip::<SignedBlock>(
        "slot1-empty.signedblock",
        fixture("slot1-empty.signedblock"),
    );
}

#[test]
fn vote_round_trip() {
    assert_round_trip::<Vote>("slot7-vote.vote", fixture("slot7-vote.vote"));
}

#[test]
fn signedvote_round_trip() {
    assert_round_trip::<SignedVote>(
        "validator3-vote.signedvote",
        fixture("validator3-vote.signedvote"),
    );
}

// =============================================================================
// non-fixture coverage: multi-chunk + Status + negative paths
// =============================================================================

#[test]
fn multi_chunk_stream_carries_independent_frames() {
    // Two SignedBlock frames written back-to-back — the BlocksByRoot
    // response shape libp2p will feed us.
    let a = fixture("slot1-empty.signedblock");
    let b = fixture("slot1-empty.signedblock");
    let mut stream = Vec::new();
    write_req_resp_frame(&mut stream, a).unwrap();
    write_req_resp_frame(&mut stream, b).unwrap();
    let mut cursor = Cursor::new(stream);
    assert_eq!(read_req_resp_frame(&mut cursor, None).unwrap().unwrap(), a);
    assert_eq!(read_req_resp_frame(&mut cursor, None).unwrap().unwrap(), b);
    assert_eq!(read_req_resp_frame(&mut cursor, None).unwrap(), None);
}

#[test]
fn generic_codec_round_trips_status() {
    let status = Status::default();
    let wire = encode_req_resp(&status);
    let back: Status = decode_req_resp(&wire).unwrap();
    assert_eq!(back, status);
}

#[test]
fn truncated_ssz_payload_surfaces_typed_error() {
    let mut bytes = fixture("slot1-empty.signedblock").to_vec();
    bytes.pop();
    let wire = encode_req_resp_wire(&bytes);
    let err = decode_req_resp::<SignedBlock>(&wire).unwrap_err();
    assert!(
        matches!(err, NetworkingError::Ssz(_)),
        "expected NetworkingError::Ssz, got {err:?}",
    );
}

#[test]
fn read_frame_rejects_length_over_cap() {
    let mut stream = Vec::new();
    write_req_resp_frame(&mut stream, fixture("slot1.blockheader")).unwrap();
    let mut cursor = Cursor::new(stream);
    let err = read_req_resp_frame(&mut cursor, Some(16)).unwrap_err();
    assert!(
        matches!(
            err,
            NetworkingError::FrameTooLarge {
                length: 112,
                max: 16
            }
        ),
        "got {err:?}",
    );
}

#[test]
fn short_eof_mid_frame_surfaces_io_error() {
    let mut stream = Vec::new();
    write_req_resp_frame(&mut stream, fixture("slot7-vote.vote")).unwrap();
    let truncated_len = stream.len() - 4;
    stream.truncate(truncated_len);
    let mut cursor = Cursor::new(stream);
    let err = read_req_resp_frame(&mut cursor, None).unwrap_err();
    assert!(matches!(err, NetworkingError::Io(_)), "got {err:?}");
}
