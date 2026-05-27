//! Compatibility fixtures for the local devnet.
//!
//! This crate intentionally owns cross-client fixture contracts rather than
//! production runtime code. Runtime crates consume these contracts through
//! tests before the Docker devnet scripts are moved into this repo.

#![forbid(unsafe_code)]

pub mod stf;
pub mod storage;

use std::path::{Path, PathBuf};

/// Fixture file containing the raw secp256k1 key for the ream node.
pub const REAM_0_RAW_SECP256K1_KEY_FIXTURE: &str = "node0-secp256k1.key";

/// Fixture file containing the raw secp256k1 key for the lean-rust node.
pub const LEANRUST_1_RAW_SECP256K1_KEY_FIXTURE: &str = "node1-secp256k1.key";

/// Fixture file containing the Rust bootnodes adapter for the 2-node devnet.
pub const RUST_BOOTNODES_2NODE_FIXTURE: &str = "bootnodes-rust-2node.yaml";

/// Stable libp2p peer ID derived from [`REAM_0_RAW_SECP256K1_KEY_FIXTURE`].
pub const REAM_0_PEER_ID: &str = "16Uiu2HAm4NcdZ731PxafPpzi1sqcss24BGqCZ5eZXdCJDyTwzjVo";

/// Stable libp2p peer ID derived from [`LEANRUST_1_RAW_SECP256K1_KEY_FIXTURE`].
pub const LEANRUST_1_PEER_ID: &str = "16Uiu2HAm4fSpFwKLAxCazVAVpsPuzmLGFYZbY8x1JNBWBDcaQ4wZ";

/// Dialable address for the ream node before appending `/p2p/<peer-id>`.
pub const REAM_0_BOOTNODE_ADDR: &str = "/ip4/172.20.0.10/udp/9000/quic-v1";

/// Returns the path to a named devnet fixture.
#[must_use]
pub fn fixture_path(name: impl AsRef<Path>) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Returns the ream raw secp256k1 key fixture path.
#[must_use]
pub fn ream_0_raw_secp256k1_key_path() -> PathBuf {
    fixture_path(REAM_0_RAW_SECP256K1_KEY_FIXTURE)
}

/// Returns the lean-rust raw secp256k1 key fixture path.
#[must_use]
pub fn leanrust_1_raw_secp256k1_key_path() -> PathBuf {
    fixture_path(LEANRUST_1_RAW_SECP256K1_KEY_FIXTURE)
}

/// Returns the 2-node Rust bootnodes adapter fixture path.
#[must_use]
pub fn rust_bootnodes_2node_path() -> PathBuf {
    fixture_path(RUST_BOOTNODES_2NODE_FIXTURE)
}
