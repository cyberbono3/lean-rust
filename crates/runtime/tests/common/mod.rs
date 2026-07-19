//! Shared helpers for the chain integration tests.
//!
//! Thin wrapper over `runtime::duties::test_fixtures`, which owns the one
//! builder for validator secret key material (generate → write records → load
//! signer). Kept as a wrapper so these tests do not each manage a temp dir.

#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::Arc;

use crypto::PublicKey;
use parking_lot::Mutex;
use runtime::duties::test_fixtures::{signer_with_keys as build_signer, MIN_ACTIVE_EPOCHS};
use runtime::duties::LocalSigner;

/// Builds a [`LocalSigner`] holding freshly generated keys for `indices`, plus
/// the matching public keys (for signature verification).
///
/// These tests sign at epoch 0 only, so the minimum activation window suffices.
/// The signer holds its keys in memory after loading, so the backing temp dir is
/// dropped immediately.
#[must_use]
pub fn signer_with_keys(indices: &[u64]) -> (Arc<Mutex<LocalSigner>>, BTreeMap<u64, PublicKey>) {
    let dir = tempfile::tempdir().expect("tempdir");
    build_signer(dir.path(), indices, MIN_ACTIVE_EPOCHS)
}
