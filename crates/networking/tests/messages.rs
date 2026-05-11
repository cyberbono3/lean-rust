//! Replays cases from the lean-go networking message fixture.
//!
//! See `tests/data/PROVENANCE.md` for the validation scope: protocol-ID
//! string equality, `MAX_REQUEST_BLOCKS` value, and Rust-side codec
//! round-trip on the parsed payloads. Byte-parity vs lean-go's encoder is
//! out of reach — the fixture carries no encoded SSZ bytes.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::fs;

use protocol::{Block, BlockBody, Checkpoint, SignedBlock, Slot, ValidatorIndex};
use serde::Deserialize;
use ssz::{decode, encode};
use types::{Bytes32, Bytes4000};

use networking::{
    BlocksByRootRequest, BlocksByRootResponse, Status, BLOCKS_BY_ROOT_PROTOCOL_V1,
    MAX_REQUEST_BLOCKS, STATUS_PROTOCOL_V1,
};

const FIXTURE_PATH: &str = "tests/data/messages.json";

// =============================================================================
// JSON shape
// =============================================================================

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    output: Output,
}

#[derive(Deserialize)]
struct Output {
    protocol_id: String,
    payload_type: String,
    #[serde(default)]
    max_request_blocks: Option<usize>,
    payload: serde_json::Value,
}

#[derive(Deserialize)]
struct CheckpointJson {
    root: String,
    slot: u64,
}

#[derive(Deserialize)]
struct StatusJson {
    finalized: CheckpointJson,
    head: CheckpointJson,
}

#[derive(Deserialize)]
struct SignedBlockJson {
    message: BlockJson,
    signature: String,
}

#[derive(Deserialize)]
struct BlockJson {
    slot: u64,
    proposer_index: u64,
    parent_root: String,
    state_root: String,
    // The fixture carries an empty `body.attestations.data` array. We
    // decode the wrapper but ignore its contents — the round-trip is over
    // an empty `BlockBody`. Marked `_unused` to silence dead-code lints
    // while still requiring the field to be present in the JSON.
    #[serde(rename = "body")]
    _unused_body: serde_json::Value,
}

// =============================================================================
// JSON → typed Rust value
// =============================================================================

fn parse_bytes32(hex_str: &str) -> Bytes32 {
    let bytes = hex::decode(hex_str).expect("valid hex");
    assert_eq!(bytes.len(), 32, "root must decode to 32 bytes");
    let mut arr = [0_u8; 32];
    arr.copy_from_slice(&bytes);
    Bytes32::new(arr)
}

fn parse_checkpoint(cp: &CheckpointJson) -> Checkpoint {
    Checkpoint::new(parse_bytes32(&cp.root), Slot::new(cp.slot))
}

fn parse_status(payload: &serde_json::Value) -> Status {
    let parsed: StatusJson = serde_json::from_value(payload.clone()).unwrap();
    Status {
        finalized: parse_checkpoint(&parsed.finalized),
        head: parse_checkpoint(&parsed.head),
    }
}

fn parse_request(payload: &serde_json::Value) -> BlocksByRootRequest {
    let hex_roots: Vec<String> = serde_json::from_value(payload.clone()).unwrap();
    BlocksByRootRequest::new(hex_roots.iter().map(|h| parse_bytes32(h))).unwrap()
}

fn parse_signed_block(json: &SignedBlockJson) -> SignedBlock {
    // The lean-go fixture stores a 32-byte signature placeholder; the
    // Rust type uses `Bytes4000`. Pad the JSON bytes with trailing zeros
    // so we end up with a deterministic, zero-extended signature.
    let raw_sig = hex::decode(&json.signature).expect("valid signature hex");
    assert!(raw_sig.len() <= 4000, "fixture signature exceeds Bytes4000");
    let mut signature = [0_u8; 4000];
    signature[..raw_sig.len()].copy_from_slice(&raw_sig);

    SignedBlock {
        message: Block {
            slot: Slot::new(json.message.slot),
            proposer_index: ValidatorIndex::new(json.message.proposer_index),
            parent_root: parse_bytes32(&json.message.parent_root),
            state_root: parse_bytes32(&json.message.state_root),
            body: BlockBody::default(), // current fixture: no attestations
        },
        signature: Bytes4000::new(signature),
    }
}

fn parse_response(payload: &serde_json::Value) -> BlocksByRootResponse {
    let blocks: Vec<SignedBlockJson> = serde_json::from_value(payload.clone()).unwrap();
    BlocksByRootResponse::new(blocks.iter().map(parse_signed_block)).unwrap()
}

// =============================================================================
// Assertions
// =============================================================================

fn assert_protocol_id(case: &Case, expected: &str) {
    assert_eq!(
        case.output.protocol_id, expected,
        "protocol_id mismatch in case {}",
        case.id,
    );
}

fn assert_round_trip<T>(case_id: &str, value: &T)
where
    T: ssz::Encode + ssz::Decode + PartialEq + std::fmt::Debug,
{
    let bytes = encode(value);
    let back: T = decode(&bytes).expect("decode");
    assert_eq!(&back, value, "round-trip mismatch in case {case_id}");
}

// =============================================================================
// Test
// =============================================================================

#[test]
fn replay_lean_go_fixture() {
    let raw = fs::read_to_string(FIXTURE_PATH).expect("read fixture");
    let fixture: Fixture = serde_json::from_str(&raw).expect("parse fixture");
    assert!(
        !fixture.cases.is_empty(),
        "fixture must carry at least one case"
    );

    for case in &fixture.cases {
        match case.output.payload_type.as_str() {
            "Status" => {
                assert_protocol_id(case, STATUS_PROTOCOL_V1.as_str());
                let s = parse_status(&case.output.payload);
                assert_round_trip(&case.id, &s);
            }
            "BlocksByRootRequest" => {
                assert_protocol_id(case, BLOCKS_BY_ROOT_PROTOCOL_V1.as_str());
                assert_eq!(
                    case.output
                        .max_request_blocks
                        .expect("max_request_blocks present"),
                    MAX_REQUEST_BLOCKS,
                    "MAX_REQUEST_BLOCKS mismatch in case {}",
                    case.id,
                );
                let req = parse_request(&case.output.payload);
                assert_round_trip(&case.id, &req);
            }
            "BlocksByRootResponse" => {
                assert_protocol_id(case, BLOCKS_BY_ROOT_PROTOCOL_V1.as_str());
                let resp = parse_response(&case.output.payload);
                assert_round_trip(&case.id, &resp);
            }
            other => panic!("unknown payload_type {other} in case {}", case.id),
        }
    }
}
