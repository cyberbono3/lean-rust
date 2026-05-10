//! Wire-parity check for [`statetransition::genesis_state`].
//!
//! Pins the SSZ encoding and hash-tree-root of `genesis_state(4, 1_700_000_000)`
//! against the canonical reference fixture in
//! `tests/fixtures/genesis-4val.state.{ssz,root.hex}`. Any divergence in the
//! state shape, field ordering, list/bitlist merkleization, or inner-config
//! layout fails this assertion.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use ssz::{decode, encode, HashTreeRoot};
use statetransition::genesis_state;

const NUM_VALIDATORS: u64 = 4;
const GENESIS_TIME: u64 = 1_700_000_000;

const FIXTURE_SSZ: &[u8] = include_bytes!("fixtures/genesis-4val.state.ssz");
const FIXTURE_ROOT_HEX: &str = include_str!("fixtures/genesis-4val.state.root.hex");

fn parse_root_hex(s: &str) -> [u8; 32] {
    let trimmed = s.trim();
    let mut out = [0_u8; 32];
    assert_eq!(
        trimmed.len(),
        64,
        "root.hex must encode 32 bytes (64 hex chars)"
    );
    for (i, byte) in out.iter_mut().enumerate() {
        let lo = i * 2;
        *byte =
            u8::from_str_radix(&trimmed[lo..lo + 2], 16).expect("root.hex must be lowercase hex");
    }
    out
}

#[test]
fn genesis_4val_state_hash_tree_root_matches_fixture() {
    let expected = parse_root_hex(FIXTURE_ROOT_HEX);
    let state = genesis_state(NUM_VALIDATORS, GENESIS_TIME);
    let actual = state.hash_tree_root();
    assert_eq!(
        hex_lower(&actual),
        hex_lower(&expected),
        "genesis_state HTR mismatch"
    );
}

#[test]
fn genesis_4val_state_ssz_encoding_matches_fixture() {
    let state = genesis_state(NUM_VALIDATORS, GENESIS_TIME);
    let actual = encode(&state);
    assert_eq!(
        actual,
        FIXTURE_SSZ,
        "genesis_state SSZ encoding mismatch (got {} bytes, fixture {} bytes)",
        actual.len(),
        FIXTURE_SSZ.len()
    );
}

#[test]
fn genesis_4val_state_ssz_round_trip_matches_constructor() {
    let state = genesis_state(NUM_VALIDATORS, GENESIS_TIME);
    let decoded = decode::<protocol::State>(FIXTURE_SSZ).expect("fixture must decode");
    assert_eq!(decoded, state);
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
