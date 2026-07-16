//! Shared SSZ test helpers (SA2). Gated behind the `test-support` feature so
//! downstream crates opt in via a dev-dependency; never compiled into a
//! production build.
//
// This module is feature-gated, NOT `#[cfg(test)]`, so the workspace clippy
// denies (`unwrap_used`/`expect_used`/`panic` — root Cargo.toml, inherited via
// ssz `[lints] workspace = true`) DO apply here. Assertion helpers legitimately
// panic on failure, so allow those lints module-wide (mirrors the
// `#[cfg(test)] #[allow(...)]` pattern in vote.rs / internal.rs).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::missing_panics_doc
)]

use crate::{decode, encode, Decode, Encode, HashTreeRoot};

/// Asserts `decode(encode(value)) == value`. Returns the encoded bytes for
/// further assertions (length, layout).
#[track_caller]
pub fn assert_ssz_round_trip<T>(value: &T) -> Vec<u8>
where
    T: Encode + Decode + PartialEq + core::fmt::Debug,
{
    let bytes = encode(value);
    let back: T = decode(&bytes).expect("round-trip decode");
    assert_eq!(&back, value, "SSZ round-trip mismatch");
    bytes
}

/// Asserts a value's hash-tree-root equals `expected`.
#[track_caller]
pub fn assert_htr_eq<T: HashTreeRoot>(value: &T, expected: [u8; 32]) {
    assert_eq!(value.hash_tree_root(), expected, "hash_tree_root mismatch");
}

/// Regenerates a golden wire vector: encodes `value`, writes the bytes to
/// `path` (relative to the invoking crate's `CARGO_MANIFEST_DIR`), and returns
/// `(bytes, root)`.
///
/// Invoke ONLY from a `#[test]` gated behind `#[ignore]` or an env guard so a
/// normal CI run reads the committed file rather than rewriting it. The
/// enclosing `test-support` feature MUST stay a dev-dependency-only enable
/// workspace-wide so this `fs::write` never lands in a production build.
#[track_caller]
pub fn regen_vector<T: Encode + HashTreeRoot>(path: &str, value: &T) -> (Vec<u8>, [u8; 32]) {
    let bytes = encode(value);
    std::fs::write(path, &bytes).expect("write golden vector");
    (bytes, value.hash_tree_root())
}
