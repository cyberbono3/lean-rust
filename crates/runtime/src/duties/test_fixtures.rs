//! Sign-path test fixtures — the one builder for validator secret key material.
//!
//! `ProdScheme` keygen is CPU-heavy, and the setup (generate → write
//! `validator_<i>.ssz` records → load a [`LocalSigner`]) was previously
//! copy-pasted into this crate's unit tests, its integration tests, and the
//! `node` composition-root tests. One home keeps the record encoding, the file
//! naming, and the activation window from drifting between them.
//!
//! Gated behind `cfg(test)` in-crate and the `test-fixtures` feature for
//! downstream test crates, exactly like [`crate::chain::engine::test_fixtures`].

// Test-only setup: a failure here is a broken fixture, not a runtime condition,
// so it panics rather than threading a Result through every call site. Mirrors
// the allow-set on `chain::engine::test_fixtures`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc
)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use crypto::{ProdScheme, PublicKey};
use parking_lot::Mutex;
use protocol::ValidatorIndex;
use rand::rngs::StdRng;
use rand::SeedableRng;

use super::signer::{validator_secret_path, LocalSigner};

/// The smallest active window that can sign epoch 0 — matching the crypto
/// crate's own tests. Use this for load-only fixtures, which never sign.
pub const MIN_ACTIVE_EPOCHS: usize = 2;

/// Fixed RNG seed: fixture key material is reproducible across runs, so a
/// failure is replayable rather than dependent on entropy.
const FIXTURE_SEED: u64 = 0x00C0_FFEE;

/// Writes a `validator_<i>.ssz` secret record for each of `indices` into `dir`
/// (activation epoch 0), returning the matching public keys for verification.
///
/// `num_active_epochs` must cover every epoch the caller signs at — a key is
/// only signable inside its activation window. Load-only callers pass
/// [`MIN_ACTIVE_EPOCHS`]; callers that drive block production need enough
/// epochs for the slots they reach (each validator signs once per slot).
///
/// # Panics
/// On any keygen, directory-creation, or file-write failure — this is test-only
/// setup, where a failure is a broken fixture rather than a runtime condition.
#[must_use]
pub fn write_validator_secrets(
    dir: &Path,
    indices: &[u64],
    num_active_epochs: usize,
) -> BTreeMap<u64, PublicKey> {
    std::fs::create_dir_all(dir).expect("create secrets dir");
    let mut rng = StdRng::seed_from_u64(FIXTURE_SEED);
    let mut pubkeys = BTreeMap::new();
    for &index in indices {
        let (pubkey, key) =
            crypto::generate::<ProdScheme, _>(&mut rng, 0, num_active_epochs).expect("generate");
        std::fs::write(
            validator_secret_path(dir, index),
            key.to_record().to_ssz_bytes(),
        )
        .expect("write validator secret");
        pubkeys.insert(index, pubkey);
    }
    pubkeys
}

/// Writes secrets for `indices` into `dir` (see [`write_validator_secrets`]) and
/// loads them into a [`LocalSigner`] wrapped for [`crate::chain::Service`].
///
/// The signer holds its keys in memory after loading, so a caller passing a
/// temp dir may drop it as soon as this returns.
///
/// # Panics
/// On fixture-setup failure, or if the just-written records fail to load.
#[must_use]
pub fn signer_with_keys(
    dir: &Path,
    indices: &[u64],
    num_active_epochs: usize,
) -> (Arc<Mutex<LocalSigner>>, BTreeMap<u64, PublicKey>) {
    let pubkeys = write_validator_secrets(dir, indices, num_active_epochs);
    let signer = LocalSigner::load(dir, indices.iter().map(|&i| ValidatorIndex::new(i)))
        .expect("load signer over just-written secrets");
    (Arc::new(Mutex::new(signer)), pubkeys)
}
