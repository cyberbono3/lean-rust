//! Contract tests for the local pq-devnet0 artifacts consumed by lean-rust.

#![allow(clippy::expect_used, clippy::panic)]

use libp2p::{
    identity::{secp256k1, Keypair},
    multiaddr::Protocol,
    Multiaddr,
};
use pq_devnet_0::{
    fixture_path, LEANRUST_1_PEER_ID, LEANRUST_1_RAW_SECP256K1_KEY_FIXTURE, REAM_0_BOOTNODE_ADDR,
    REAM_0_PEER_ID, REAM_0_RAW_SECP256K1_KEY_FIXTURE, RUST_BOOTNODES_2NODE_FIXTURE,
};
use protocol::{State, ValidatorIndex};
use runtime_duties::ValidatorAssignments;
use ssz::HashTreeRoot;

const GENESIS_TIME: u64 = 1_778_169_008;

fn decode_hex_fixture(name: &str) -> Vec<u8> {
    let hex = std::fs::read_to_string(fixture_path(name)).expect("read hex fixture");
    hex::decode(hex.split_whitespace().collect::<String>()).expect("fixture must be valid hex")
}

fn derive_peer_id_from_raw_key(name: &str) -> String {
    let raw_key = std::fs::read_to_string(fixture_path(name)).expect("read secp256k1 key fixture");
    let mut bytes = hex::decode(raw_key.trim()).expect("fixture must be valid hex");
    let secret =
        secp256k1::SecretKey::try_from_bytes(&mut bytes).expect("fixture must be a secp key");
    Keypair::from(secp256k1::Keypair::from(secret))
        .public()
        .to_peer_id()
        .to_string()
}

fn parse_bootnode_entry(entry: &str) -> (Multiaddr, String) {
    let mut addr = entry.parse::<Multiaddr>().expect("adapter multiaddr");
    let peer_id = match addr.pop() {
        Some(Protocol::P2p(peer_id)) => peer_id.to_string(),
        other => panic!("expected terminal /p2p peer id, got {other:?}"),
    };

    (addr, peer_id)
}

fn decode_current_local_pq_genesis(bytes: &[u8]) -> State {
    // Current eth-beacon-genesis local-pq output is the compact 145-byte Ream
    // leanchain state shape. Production startup supports this through the
    // protocol adapter used here.
    assert_eq!(
        bytes.len(),
        145,
        "unexpected local-pq genesis fixture length"
    );

    State::from_ream_legacy_ssz_bytes(bytes).expect("fixture must decode")
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
    assert!(state.historical_block_hashes.is_empty());
    assert!(state.justified_slots.is_empty());
    assert_eq!(
        hex::encode(state.hash_tree_root()),
        "8d30f4011dddd48e95d246ba5438c131864e1c8184b30844687a30728fc2461e"
    );
}

#[test]
fn raw_secp256k1_node_keys_derive_stable_peer_ids() {
    for (fixture, expected_peer_id) in [
        (REAM_0_RAW_SECP256K1_KEY_FIXTURE, REAM_0_PEER_ID),
        (LEANRUST_1_RAW_SECP256K1_KEY_FIXTURE, LEANRUST_1_PEER_ID),
    ] {
        assert_eq!(derive_peer_id_from_raw_key(fixture), expected_peer_id);
    }
}

#[test]
fn bootnodes_rust_adapter_fixture_is_remote_ream_multiaddr() {
    let raw = std::fs::read(fixture_path(RUST_BOOTNODES_2NODE_FIXTURE))
        .expect("read bootnodes adapter fixture");
    let entries: Vec<String> = serde_yaml::from_slice(&raw).expect("adapter must be YAML list");
    let [entry] = entries.as_slice() else {
        panic!("expected exactly one Rust bootnode entry, got {entries:?}");
    };

    let (addr, peer_id) = parse_bootnode_entry(entry);

    assert_eq!(addr.to_string(), REAM_0_BOOTNODE_ADDR);
    assert_eq!(peer_id, REAM_0_PEER_ID);
    assert_eq!(
        peer_id,
        derive_peer_id_from_raw_key(REAM_0_RAW_SECP256K1_KEY_FIXTURE)
    );
    assert_ne!(peer_id, LEANRUST_1_PEER_ID);
}

#[test]
fn bootnode_contract_uses_temporary_multiaddr_adapter() {
    let decision = include_str!("fixtures/README.md");
    assert!(decision.contains("genesis/bootnodes.rust.yaml"));
    assert!(decision.contains("rather than parsing ENR"));
}
