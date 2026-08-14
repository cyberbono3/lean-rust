//! Contract tests for the local pq-devnet0 artifacts consumed by lean-rust.

#![allow(clippy::expect_used, clippy::panic)]

use fixtures::{
    fixture_path, LEANRUST_1_PEER_ID, LEANRUST_1_RAW_SECP256K1_KEY_FIXTURE, REAM_0_BOOTNODE_ADDR,
    REAM_0_PEER_ID, REAM_0_RAW_SECP256K1_KEY_FIXTURE, RUST_BOOTNODES_2NODE_FIXTURE,
};
use libp2p::{
    identity::{secp256k1, Keypair},
    multiaddr::Protocol,
    Multiaddr,
};
use protocol::{stf::genesis_state, State, Validator, ValidatorIndex, Validators};
use runtime::duties::ValidatorAssignments;
use ssz::HashTreeRoot;
use types::PublicKey;

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

    // The compact interop format carries no validators tail, so the legacy
    // anchor has an EMPTY registry — the declared count is validated during
    // decode and discarded. A node refuses such a state as a chain anchor
    // (see lean-cli `validate_state_limits`); this test covers the decoder.
    assert_eq!(state.num_validators(), 0);
    assert_eq!(state.config.genesis_time, GENESIS_TIME);
    assert_eq!(state.slot.get(), 0);
    assert!(state.historical_block_hashes.is_empty());
    assert!(state.justified_slots.is_empty());
    assert_eq!(
        hex::encode(state.hash_tree_root()),
        // Moved when the in-state config dropped `num_validators`: the config
        // container went from two fields to one, so the state root changes.
        // The 145-byte devnet0 payload itself is unchanged.
        "9bcc325e28fd8fb4882da3406303fb2048600463806fc9819b7ed527115b6f58"
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

// =====================================================================
// Spec-parity: genesis hash-tree-root
// =====================================================================

/// A genesis root computed by the pinned leanSpec checkout for known inputs.
#[derive(serde::Deserialize)]
struct SpecGenesisVector {
    /// leanSpec revision the vector was generated from.
    spec_revision: String,
    genesis_time: u64,
    /// Hex-encoded validator pubkeys, in index order. Empty for the
    /// empty-registry vector.
    pubkeys: Vec<String>,
    /// Hex-encoded `hash_tree_root` of the spec's genesis state.
    expected_root: String,
}

impl SpecGenesisVector {
    fn load(name: &str) -> Self {
        let raw = std::fs::read(fixture_path(name)).expect("read spec vector fixture");
        serde_yaml::from_slice(&raw).expect("spec vector must be valid YAML")
    }

    fn validators(&self) -> Validators {
        self.pubkeys
            .iter()
            .enumerate()
            .map(|(i, hex)| {
                let index = u64::try_from(i).expect("fixture index fits u64");
                let pubkey =
                    PublicKey::try_from(hex.as_str()).expect("fixture pubkey must be valid hex");
                Validator::new(pubkey, ValidatorIndex::new(index))
            })
            .collect()
    }
}

/// The genesis state lean-rust synthesizes must hash to the root leanSpec
/// computes for the same inputs. This is the interop contract in its most
/// basic form: two clients that disagree here cannot agree on any head root.
///
/// Empty registry, deliberately. An `SSZList`'s empty root depends only on its
/// LIMIT, which matches on both sides, so this vector isolates the in-state
/// `Config` shape — the thing that changed — along with both checkpoints, the
/// block header, and the four remaining tails. The populated-registry form is
/// staged below.
#[test]
fn genesis_hash_tree_root_matches_spec_vector_empty_registry() {
    let vector = SpecGenesisVector::load("genesis-empty-registry-spec-vector.yaml");
    assert!(
        vector.pubkeys.is_empty(),
        "this vector must carry no validators"
    );

    let state = genesis_state(vector.genesis_time, Vec::new());

    assert_eq!(state.num_validators(), 0);
    assert_eq!(
        hex::encode(state.hash_tree_root()),
        vector.expected_root,
        "genesis root diverges from {}; peers would reject this genesis",
        vector.spec_revision,
    );
}

/// Populated-registry parity. Blocked on the `Validator` container gaining a
/// second pubkey: leanSpec's entry carries `attestation_pubkey`,
/// `proposal_pubkey` and `index` against our `pubkey` and `index`, so the
/// registry subtree merkleizes to a different width and the roots cannot match
/// yet. The vector is checked in so the work is staged rather than forgotten.
#[test]
#[ignore = "blocked on the dual-key Validator container; see the fixture header"]
fn genesis_hash_tree_root_matches_spec_vector_populated_registry() {
    let vector = SpecGenesisVector::load("genesis-2node-spec-vector.yaml");
    let state = genesis_state(vector.genesis_time, vector.validators());

    assert_eq!(state.num_validators(), 2);
    assert_eq!(
        hex::encode(state.hash_tree_root()),
        vector.expected_root,
        "genesis root diverges from {}",
        vector.spec_revision,
    );
}
