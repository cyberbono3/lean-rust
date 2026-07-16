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
        "slot7.attestationdata",
        include_bytes!("data/wire-parity/slot7.attestationdata.ssz"),
    ),
    (
        "validator3.signedattestation",
        include_bytes!("data/wire-parity/validator3.signedattestation.ssz"),
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

// =============================================================================
// devnet-1 wire-break fixture regeneration (SA2)
// =============================================================================
//
// `slot7.attestationdata.ssz` is byte-identical to the retired `slot7-vote.vote.ssz`
// (`AttestationData` shares `Vote`'s layout) and is preserved by `git mv`.
// `validator3.signedattestation.ssz` (4136 -> 3252) and `two-votes.blockbody.ssz`
// (8276 -> 6508) are regenerated from the canonical values below, faithful to the
// retired devnet-0 vectors: the same slot-7 attestation data, validator ids
// 3 and 1, and signature fills 0xa1 / 0xb2. Run once, then commit:
//
//   cargo test -p lean-wire --test parity regenerate_devnet1_fixtures -- --ignored --exact
mod regen {
    use protocol::{
        Attestation, AttestationData, Block, BlockBody, BlockSignatures, BlockWithAttestation,
        Checkpoint, SignedAttestation, SignedBlockWithAttestation, Slot, ValidatorIndex,
    };
    use ssz::test_support::regen_vector;
    use types::{Bytes32, Signature};

    fn slot1_empty_signed_block() -> SignedBlockWithAttestation {
        SignedBlockWithAttestation {
            message: BlockWithAttestation {
                block: Block {
                    slot: Slot::new(1),
                    proposer_index: ValidatorIndex::new(1),
                    parent_root: Bytes32::new([0x03; 32]),
                    state_root: Bytes32::new([0x04; 32]),
                    body: BlockBody::default(),
                },
                proposer_attestation: Attestation::default(),
            },
            signature: BlockSignatures::default(),
        }
    }

    fn slot7_data() -> AttestationData {
        AttestationData {
            slot: Slot::new(7),
            head: Checkpoint::new(Bytes32::new([0xab; 32]), Slot::new(12)),
            target: Checkpoint::new(Bytes32::new([0xab; 32]), Slot::new(12)),
            source: Checkpoint::new(Bytes32::zero(), Slot::ZERO),
        }
    }

    fn attestation(validator: u64) -> Attestation {
        Attestation {
            validator_id: ValidatorIndex::new(validator),
            data: slot7_data(),
        }
    }

    fn signed_attestation(validator: u64, sig_fill: u8) -> SignedAttestation {
        SignedAttestation {
            message: attestation(validator),
            signature: Signature::new([sig_fill; Signature::LEN]),
        }
    }

    #[test]
    #[ignore = "regeneration writes committed fixtures; run explicitly on a wire break"]
    fn regenerate_devnet1_fixtures() {
        let dir = "tests/data/wire-parity";

        // Byte-identical rename target — proves the AttestationData layout matches
        // the retired Vote bytes.
        let (data_bytes, _) =
            regen_vector(&format!("{dir}/slot7.attestationdata.ssz"), &slot7_data());
        assert_eq!(data_bytes.len(), 128);

        let (sa_bytes, _) = regen_vector(
            &format!("{dir}/validator3.signedattestation.ssz"),
            &signed_attestation(3, 0xa1),
        );
        assert_eq!(sa_bytes.len(), 3252);

        // Part 7: the block body now holds PLAIN attestations (signatures moved
        // to the block-signature list), so this fixture is regenerated again —
        // 2 * 136 + 4 offset = 276 bytes (was 6508 with SignedAttestation elements).
        let body = BlockBody {
            attestations: vec![attestation(3), attestation(1)],
        };
        let (body_bytes, _) = regen_vector(&format!("{dir}/two-votes.blockbody.ssz"), &body);
        assert_eq!(body_bytes.len(), 276);

        // Part 7: the block envelope changed (SignedBlock -> SignedBlockWithAttestation),
        // so this fixture is regenerated. 8 (two offsets) + 228 (message:
        // BlockWithAttestation = 140 fixed-part + 88 block) + 0 (empty signatures) = 236.
        let (signed_block_bytes, _) = regen_vector(
            &format!("{dir}/slot1-empty.signedblock.ssz"),
            &slot1_empty_signed_block(),
        );
        assert_eq!(signed_block_bytes.len(), 236);
    }
}
