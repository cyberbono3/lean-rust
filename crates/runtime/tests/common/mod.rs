//! Shared helpers for the chain integration tests.
//!
//! Thin wrapper over `runtime::duties::test_fixtures`, which owns the one
//! builder for validator secret key material (generate → write records → load
//! signer). Kept as a wrapper so these tests do not each manage a temp dir.

// Only `expect_used`: the `.expect("tempdir")` below is the sole suppression this
// file needs. `dead_code` was dropped (one declaring binary, `chain_sign.rs`,
// which uses the single helper at four sites) and `unwrap_used` with it — there
// is no `.unwrap()` here. Add a token back only when a helper actually needs one.
// Reference the call by identifier, not line number: the number rots on reflow.
#![allow(clippy::expect_used)]

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
pub(crate) fn signer_with_keys(
    indices: &[u64],
) -> (Arc<Mutex<LocalSigner>>, BTreeMap<u64, PublicKey>) {
    let dir = tempfile::tempdir().expect("tempdir");
    build_signer(dir.path(), indices, MIN_ACTIVE_EPOCHS)
}
