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

use lean_wire::{
    decode_req_resp, decode_req_resp_wire, encode_req_resp, encode_req_resp_wire,
    read_req_resp_frame, write_req_resp_frame, NetworkingError, Status,
};
use protocol::{
    AttestationData, Block, BlockBody, BlockHeader, Checkpoint, SignedAttestation,
    SignedBlockWithAttestation, State,
};
use ssz::{decode, encode, Decode, Encode, HashTreeRoot};

// =============================================================================
// per-fixture entries
// =============================================================================

const FIXTURES: &[(&str, &[u8])] = &[
    (
        "empty.blockbody",
        include_bytes!("data/wire-parity/empty.blockbody.ssz"),
    ),
    (
        "two-attestations.blockbody",
        include_bytes!("data/synthetic/two-attestations.blockbody.ssz"),
    ),
    (
        "genesis-4val.state",
        include_bytes!("data/synthetic/genesis-4val.state.ssz"),
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
        include_bytes!("data/synthetic/slot1-empty.signedblock.ssz"),
    ),
    (
        "slot7.attestationdata",
        include_bytes!("data/wire-parity/slot7.attestationdata.ssz"),
    ),
    (
        "validator3.signedattestation",
        include_bytes!("data/synthetic/validator3.signedattestation.ssz"),
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
fn two_attestations_blockbody_round_trip() {
    assert_round_trip::<BlockBody>(
        "two-attestations.blockbody",
        fixture("two-attestations.blockbody"),
    );
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
    assert_round_trip::<SignedBlockWithAttestation>(
        "slot1-empty.signedblock",
        fixture("slot1-empty.signedblock"),
    );
}

#[test]
fn attestationdata_round_trip() {
    assert_round_trip::<AttestationData>("slot7.attestationdata", fixture("slot7.attestationdata"));
}

#[test]
fn signedattestation_round_trip() {
    assert_round_trip::<SignedAttestation>(
        "validator3.signedattestation",
        fixture("validator3.signedattestation"),
    );
}

/// Locks the hash-tree-roots of the `synthetic/` vectors to the values recorded
/// in `tests/data/PROVENANCE.md`.
///
/// These roots are self-generated, so they cannot prove the merkleization shape
/// is *correct* — only this workspace produced them, and a cross-client anchor
/// for the devnet-1 attestation containers does not exist until a live peer
/// supplies one. What they do is make the shape *fixed*: an accidental change to
/// field order, merkleization width, or padding fails here loudly instead of
/// passing silently and splitting consensus. Without this, the roots in
/// PROVENANCE are prose that nothing checks.
///
/// If this test fails, do not update the constants to match. Establish which
/// side is wrong first — a changed root means the wire shape moved.
#[test]
fn synthetic_vector_roots_are_pinned() {
    const SIGNED_ATTESTATION_ROOT: &str =
        "f698770b0bf6ae48b597bee138698b4829b5452d762f4ba9b2db56a32c18fbeb";
    const BLOCKBODY_ROOT: &str = "0a786852dc25250a5f62918d10bc7a2d19d448cd4b696f015d2ca3ad8942fe10";
    const SIGNED_BLOCK_ROOT: &str =
        "6210c7d3a20a8d046283fdbd2257543c3ee100f29342fa4c48d9095d19dfbf50";
    const GENESIS_STATE_ROOT: &str =
        "663a7142e12afccbb2bc78fc83c72bef1df8617bfaabd38a900486d0520bb05f";

    let signed: SignedAttestation =
        decode(fixture("validator3.signedattestation")).expect("decode signedattestation");
    assert_eq!(
        hex::encode(signed.hash_tree_root()),
        SIGNED_ATTESTATION_ROOT,
        "SignedAttestation root moved — the wire shape changed, or PROVENANCE is stale",
    );

    let body: BlockBody = decode(fixture("two-attestations.blockbody")).expect("decode blockbody");
    assert_eq!(
        hex::encode(body.hash_tree_root()),
        BLOCKBODY_ROOT,
        "BlockBody root moved — the wire shape changed, or PROVENANCE is stale",
    );

    let signed_block: SignedBlockWithAttestation =
        decode(fixture("slot1-empty.signedblock")).expect("decode signedblock");
    assert_eq!(
        hex::encode(signed_block.hash_tree_root()),
        SIGNED_BLOCK_ROOT,
        "SignedBlockWithAttestation root moved — the wire shape changed, or PROVENANCE is stale",
    );

    let state: State = decode(fixture("genesis-4val.state")).expect("decode state");
    assert_eq!(
        hex::encode(state.hash_tree_root()),
        GENESIS_STATE_ROOT,
        "State root moved — the wire shape changed, or PROVENANCE is stale",
    );
}

// =============================================================================
// non-fixture coverage: multi-chunk + Status + negative paths
// =============================================================================

#[test]
fn multi_chunk_stream_carries_independent_frames() {
    // Two SignedBlockWithAttestation frames written back-to-back — the BlocksByRoot
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
    let err = decode_req_resp::<SignedBlockWithAttestation>(&wire).unwrap_err();
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
    write_req_resp_frame(&mut stream, fixture("slot7.attestationdata")).unwrap();
    let truncated_len = stream.len() - 4;
    stream.truncate(truncated_len);
    let mut cursor = Cursor::new(stream);
    let err = read_req_resp_frame(&mut cursor, None).unwrap_err();
    assert!(matches!(err, NetworkingError::Io(_)), "got {err:?}");
}
