//! Shared helpers for the chain integration tests.
//!
//! Builds a [`LocalSigner`] from freshly generated `ProdScheme` key material so
//! the production sign path can be exercised end-to-end. Keys use activation 0 /
//! 2 active epochs — the smallest window that can sign epoch 0, matching the
//! crypto crate's own tests — to keep the (CPU-heavy) key generation cheap.

#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::Arc;

use crypto::{ProdScheme, PublicKey};
use parking_lot::Mutex;
use protocol::ValidatorIndex;
use rand::rngs::StdRng;
use rand::SeedableRng;
use runtime::duties::LocalSigner;

/// Builds a [`LocalSigner`] holding freshly generated keys for `indices`, plus
/// the matching public keys (for signature verification). The signer holds the
/// keys in memory after loading, so the backing temp dir is dropped immediately.
#[must_use]
pub fn signer_with_keys(indices: &[u64]) -> (Arc<Mutex<LocalSigner>>, BTreeMap<u64, PublicKey>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut rng = StdRng::seed_from_u64(0x00C0_FFEE);
    let mut pubs = BTreeMap::new();
    for &i in indices {
        let (pk, sk) = crypto::generate::<ProdScheme, _>(&mut rng, 0, 2).expect("generate");
        std::fs::write(
            dir.path().join(format!("validator_{i}.ssz")),
            sk.to_record().to_ssz_bytes(),
        )
        .expect("write secret");
        pubs.insert(i, pk);
    }
    let signer = LocalSigner::load(dir.path(), indices.iter().map(|&i| ValidatorIndex::new(i)))
        .expect("load signer");
    (Arc::new(Mutex::new(signer)), pubs)
}
