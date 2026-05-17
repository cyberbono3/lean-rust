//! Contract tests for the local pq-devnet0 artifacts consumed by lean-rust.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use libp2p::identity::{secp256k1, Keypair};
use protocol::{ProtocolConfig, State, ValidatorIndex};
use runtime_duties::ValidatorAssignments;

const GENESIS_TIME: u64 = 1_778_169_008;
const EXPECTED_NODE1_PEER_ID: &str = "16Uiu2HAm4fSpFwKLAxCazVAVpsPuzmLGFYZbY8x1JNBWBDcaQ4wZ";

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn decode_hex_fixture(name: &str) -> Vec<u8> {
    let hex = std::fs::read_to_string(fixture_path(name)).expect("read hex fixture");
    hex::decode(hex.split_whitespace().collect::<String>()).expect("fixture must be valid hex")
}

fn read_u64_le(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("u64 field"))
}

fn decode_current_local_pq_genesis(bytes: &[u8]) -> State {
    // Current eth-beacon-genesis local-pq output is still the legacy 145-byte
    // state shape. Production support belongs in a later compatibility issue;
    // Issue #1 only pins the artifact contract and proves the fixture can be
    // mapped into the Rust domain state used by later tests.
    assert_eq!(
        bytes.len(),
        145,
        "unexpected local-pq genesis fixture length"
    );

    State {
        config: ProtocolConfig {
            num_validators: read_u64_le(bytes, 0),
            genesis_time: read_u64_le(bytes, 8),
        },
        ..State::default()
    }
}

#[test]
fn validators_2node_fixture_matches_local_pq_shape() {
    let assignments =
        ValidatorAssignments::load(fixture_path("validators-2node.yaml")).expect("load fixture");

    assert_eq!(assignments.total_validators(), 2);
    assert_eq!(
        assignments.group("ream_0").expect("ream group"),
        [ValidatorIndex::new(0)]
    );
    assert_eq!(
        assignments.group("leanrust_1").expect("lean-rust group"),
        [ValidatorIndex::new(1)]
    );
}

#[test]
fn genesis_2node_fixture_decodes_to_protocol_state() {
    let bytes = decode_hex_fixture("genesis-2node.ssz.hex");
    let state = decode_current_local_pq_genesis(&bytes);

    assert_eq!(state.config.num_validators, 2);
    assert_eq!(state.config.genesis_time, GENESIS_TIME);
    assert_eq!(state.slot.get(), 0);
}

#[test]
fn raw_secp256k1_node_key_derives_stable_peer_id() {
    let raw_key = std::fs::read_to_string(fixture_path("node1-secp256k1.key"))
        .expect("read secp256k1 key fixture");
    let mut bytes = hex::decode(raw_key.trim()).expect("fixture must be valid hex");
    let secret =
        secp256k1::SecretKey::try_from_bytes(&mut bytes).expect("fixture must be a secp key");
    let peer_id = Keypair::from(secp256k1::Keypair::from(secret))
        .public()
        .to_peer_id()
        .to_string();

    assert_eq!(peer_id, EXPECTED_NODE1_PEER_ID);
}

#[test]
fn bootnode_contract_uses_temporary_multiaddr_adapter() {
    let decision = include_str!("fixtures/README.md");
    assert!(decision.contains("genesis/bootnodes.rust.yaml"));
    assert!(decision.contains("rather than parsing ENR"));
}
