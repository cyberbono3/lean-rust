//! Local validator signer — the one runtime owner of secret key material.
//!
//! [`LocalSigner`] loads each local validator's [`crypto::OtsKeyState`] record
//! (the `validator_<i>.ssz` files the offline keygen wrote) into a live
//! [`crypto::ProdSigningKey`], and produces real leanSig signatures over
//! `hash_tree_root(Attestation)` at epoch = `attestation.data.slot`.
//!
//! Signing lives HERE, at the runtime boundary — never in `forkchoice` or
//! `protocol::state_transition` (see PROJECT-KNOWLEDGE.md → `LAYER_RULE`). This
//! module is the only place in `runtime` that touches `crypto`'s signing surface.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crypto::{CryptoError, OtsKeyState, OtsKeyStateDecodeError, ProdSigningKey};
use protocol::{Attestation, ValidatorIndex};
use types::Signature;

use crate::signing_domain::{attestation_signing_inputs, EpochOverflow};

/// Errors raised while LOADING local secret key material at composition-root
/// startup. A load failure is fatal: a node configured to run a validator it has
/// no key for is misconfigured and must fail fast, never sign a placeholder.
#[derive(Debug, thiserror::Error)]
pub enum SignerLoadError {
    /// The `validator_<i>.ssz` secret record could not be read from disk.
    #[error("read secret key for validator {index} at {path:?}")]
    SecretFileRead {
        /// Validator index whose secret file could not be read.
        index: u64,
        /// Path the loader attempted to read.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// The on-disk bytes are not a valid [`OtsKeyState`] SSZ record.
    #[error("decode OtsKeyState for validator {index}")]
    KeyStateDecode {
        /// Validator index whose record failed to decode.
        index: u64,
        /// Underlying decode failure.
        #[source]
        source: OtsKeyStateDecodeError,
    },
    /// The record decoded but the signing key could not be restored from it
    /// (corrupt watermark / activation out of range).
    #[error("restore signing key for validator {index}")]
    KeyRestore {
        /// Validator index whose key could not be restored.
        index: u64,
        /// Underlying crypto failure.
        #[source]
        source: CryptoError,
    },
}

/// Errors raised while SIGNING at the runtime production boundary.
#[derive(Debug, thiserror::Error)]
pub enum SignError {
    /// No secret key is loaded for the requested validator.
    #[error("no signing key loaded for validator {validator_id}")]
    UnknownValidator {
        /// The validator index that has no loaded key.
        validator_id: u64,
    },
    /// The attestation slot exceeds the leanSig epoch domain. Raised by the
    /// shared [`attestation_signing_inputs`] derivation.
    #[error(transparent)]
    EpochOverflow(#[from] EpochOverflow),
    /// The underlying leanSig operation failed. Includes one-time-key reuse
    /// ([`CryptoError::EpochReused`]): it surfaces here rather than being
    /// swallowed, so a double-sign is a visible error, not a silent placeholder.
    #[error("leanSig signing failed")]
    Crypto(#[from] CryptoError),
}

/// Holds the local validators' live signing keys, keyed by index.
///
/// The map value is a [`crypto::ProdSigningKey`] (the pinned production scheme).
/// The composition root builds this and hands it to the chain service; signing
/// happens at the runtime boundary, outside the forkchoice store lock.
pub struct LocalSigner {
    keys: BTreeMap<ValidatorIndex, ProdSigningKey>,
}

impl LocalSigner {
    /// An empty signer holding no keys — used by observer nodes (no local
    /// validators). Any [`sign_attestation`](Self::sign_attestation) call returns
    /// [`SignError::UnknownValidator`], which an observer never triggers because
    /// it never produces messages.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            keys: BTreeMap::new(),
        }
    }

    /// Loads secret key material for exactly `local_indices` from
    /// `<secrets_dir>/validator_<i>.ssz`.
    ///
    /// STRICT: every requested index MUST resolve to a readable, decodable,
    /// restorable key, else the whole load fails — no partial signer, no silent
    /// gap.
    ///
    /// # Errors
    /// [`SignerLoadError`] on the first index whose file is unreadable, whose
    /// bytes fail [`OtsKeyState`] decode, or whose record fails
    /// [`from_record`](crypto::SigningKey::from_record).
    pub fn load(
        secrets_dir: &Path,
        local_indices: impl IntoIterator<Item = ValidatorIndex>,
    ) -> Result<Self, SignerLoadError> {
        let mut keys = BTreeMap::new();
        for index in local_indices {
            let idx = index.get();
            let path = secrets_dir.join(format!("validator_{idx}.ssz"));
            let bytes = std::fs::read(&path).map_err(|source| SignerLoadError::SecretFileRead {
                index: idx,
                path: path.clone(),
                source,
            })?;
            let record = OtsKeyState::from_ssz_bytes(&bytes)
                .map_err(|source| SignerLoadError::KeyStateDecode { index: idx, source })?;
            let key = ProdSigningKey::from_record(&record)
                .map_err(|source| SignerLoadError::KeyRestore { index: idx, source })?;
            keys.insert(index, key);
        }
        Ok(Self { keys })
    }

    /// Signs `att` for its own `validator_id`: a leanSig signature over
    /// `hash_tree_root(att)` at epoch = `att.data.slot`.
    ///
    /// `&mut self` is load-bearing — the one-time-key index advances on each sign
    /// (Part 15 persists `next_index` afterwards). Re-signing the SAME epoch
    /// surfaces [`CryptoError::EpochReused`] via [`SignError::Crypto`] rather than
    /// burning a second index.
    ///
    /// # Errors
    /// - [`SignError::UnknownValidator`] if no key is loaded for `att.validator_id`.
    /// - [`SignError::EpochOverflow`] if `att.data.slot` exceeds the `u32`
    ///   epoch domain.
    /// - [`SignError::Crypto`] on any leanSig failure (not-active / not-prepared /
    ///   reused).
    pub(crate) fn sign_attestation(&mut self, att: &Attestation) -> Result<Signature, SignError> {
        let validator_id = att.validator_id;
        let key = self
            .keys
            .get_mut(&validator_id)
            .ok_or(SignError::UnknownValidator {
                validator_id: validator_id.get(),
            })?;
        let (epoch, message) = attestation_signing_inputs(att)?;
        // A reloaded key sits at its activation-start prepared window; advance it
        // to the target epoch before signing (no-op when already prepared).
        key.prepare(epoch)?;
        let signature = key.sign(epoch, &message)?;
        Ok(signature)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crypto::{ProdScheme, PublicKey};
    use protocol::{AttestationData, Slot};
    use rand::rngs::StdRng;
    use rand::SeedableRng;
    use ssz::HashTreeRoot;

    /// Generates a `ProdScheme` key per index (activation 0, 2 active epochs —
    /// the smallest window that can sign epoch 0, matching the crypto crate's own
    /// tests), writes each as `validator_<i>.ssz`, and returns the temp dir plus
    /// the matching public keys. `ProdScheme` keygen is CPU-heavy, so callers pass
    /// the minimum index set they need.
    fn make_secret_dir(indices: &[u64]) -> (tempfile::TempDir, BTreeMap<u64, PublicKey>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut rng = StdRng::seed_from_u64(42);
        let mut pubs = BTreeMap::new();
        for &i in indices {
            let (pk, sk) = crypto::generate::<ProdScheme, _>(&mut rng, 0, 2).expect("generate");
            let record = sk.to_record();
            std::fs::write(
                dir.path().join(format!("validator_{i}.ssz")),
                record.to_ssz_bytes(),
            )
            .expect("write secret");
            pubs.insert(i, pk);
        }
        (dir, pubs)
    }

    fn attestation(validator: u64, slot: u64) -> Attestation {
        Attestation {
            validator_id: ValidatorIndex::new(validator),
            data: AttestationData {
                slot: Slot::new(slot),
                ..Default::default()
            },
        }
    }

    #[test]
    #[ignore = "leanSig ProdScheme keygen is CPU-heavy; run explicitly with --ignored"]
    fn signer_signs_attestation_htr_at_data_slot_epoch() {
        let (dir, pubs) = make_secret_dir(&[0]);
        let mut signer = LocalSigner::load(dir.path(), [ValidatorIndex::new(0)]).unwrap();

        let att = attestation(0, 0);
        let sig = signer.sign_attestation(&att).unwrap();

        // Signed preimage is hash_tree_root(att) at epoch = data.slot (0).
        let msg = att.hash_tree_root();
        assert!(crypto::verify::<ProdScheme>(&pubs[&0], 0, &msg, &sig).is_ok());

        // Bound to the exact message: a different attestation does not verify.
        let other = attestation(0, 1);
        assert!(crypto::verify::<ProdScheme>(&pubs[&0], 0, &other.hash_tree_root(), &sig).is_err());
    }

    #[test]
    #[ignore = "leanSig ProdScheme keygen is CPU-heavy; run explicitly with --ignored"]
    fn signer_loads_secret_keystate_from_ssz() {
        let (dir, _pubs) = make_secret_dir(&[0, 2]);
        let mut signer =
            LocalSigner::load(dir.path(), [ValidatorIndex::new(0), ValidatorIndex::new(2)])
                .unwrap();

        assert!(signer.sign_attestation(&attestation(0, 0)).is_ok());
        assert!(signer.sign_attestation(&attestation(2, 0)).is_ok());
    }

    #[test]
    #[ignore = "leanSig ProdScheme keygen is CPU-heavy; run explicitly with --ignored"]
    fn signer_rejects_unknown_validator_and_epoch_overflow() {
        let (dir, _pubs) = make_secret_dir(&[0]);
        let mut signer = LocalSigner::load(dir.path(), [ValidatorIndex::new(0)]).unwrap();

        // Unknown validator — no key loaded, refused before any epoch check.
        let unknown = signer.sign_attestation(&attestation(9, 0)).unwrap_err();
        assert!(matches!(
            unknown,
            SignError::UnknownValidator { validator_id: 9 }
        ));

        // Epoch overflow — a known validator, but the slot exceeds u32::MAX.
        let big = u64::from(u32::MAX) + 1;
        let overflow = signer.sign_attestation(&attestation(0, big)).unwrap_err();
        assert!(matches!(overflow, SignError::EpochOverflow(e) if e.slot == big));
    }

    #[test]
    fn load_rejects_missing_secret_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        // `let Err(..) else` rather than `unwrap_err()`: the Ok type holds secret
        // keys and is deliberately not `Debug` (mirrors the crypto crate).
        let Err(err) = LocalSigner::load(dir.path(), [ValidatorIndex::new(0)]) else {
            panic!("load must fail when the secret file is missing");
        };
        assert!(matches!(
            err,
            SignerLoadError::SecretFileRead { index: 0, .. }
        ));
    }

    #[test]
    fn load_rejects_corrupt_keystate() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("validator_0.ssz"), b"garbage").expect("write");
        let Err(err) = LocalSigner::load(dir.path(), [ValidatorIndex::new(0)]) else {
            panic!("load must fail on a corrupt keystate");
        };
        assert!(matches!(
            err,
            SignerLoadError::KeyStateDecode { index: 0, .. }
        ));
    }

    #[test]
    fn empty_signer_signs_for_no_validator() {
        let mut signer = LocalSigner::empty();
        assert!(matches!(
            signer.sign_attestation(&attestation(0, 0)),
            Err(SignError::UnknownValidator { validator_id: 0 })
        ));
    }
}
