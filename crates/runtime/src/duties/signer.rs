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

use crypto::{seed_commitment, CryptoError, OtsKeyState, OtsKeyStateDecodeError, ProdSigningKey};
use protocol::{Attestation, ValidatorIndex};
use storage::StorageError;
use types::{OtsWatermark, Signature};

use crate::signing_domain::{attestation_signing_inputs, EpochOverflow};

/// `<secrets_dir>/validator_<index>.ssz` — the on-disk name of one validator's
/// [`OtsKeyState`] secret record.
///
/// The ONE home for this convention. The offline keygen writes these files
/// (`lean-cli::validator_keygen`) and [`LocalSigner::load`] reads them back; a
/// second spelling on either side would desync silently at runtime rather than
/// failing to compile, so both sides call this.
#[must_use]
pub fn validator_secret_path(secrets_dir: &Path, index: u64) -> PathBuf {
    secrets_dir.join(format!("validator_{index}.ssz"))
}

/// Errors raised while LOADING local secret key material at composition-root
/// startup. A load failure is fatal: a node configured to run a validator it has
/// no key for is misconfigured and must fail fast, never sign a placeholder.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
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
    /// The durable store could not be read while resuming the one-time-key
    /// watermark ([`LocalSigner::load_resuming`]).
    #[error("load persisted OTS watermark for validator {index}")]
    KeyStateLoad {
        /// Validator index whose persisted record could not be read.
        index: u64,
        /// Underlying storage failure.
        #[source]
        source: StorageError,
    },
    /// The persisted record and the on-disk secret record describe DIFFERENT
    /// keys (seed or activation window mismatch). Merging watermarks across
    /// different keys is meaningless, and silently preferring either side risks
    /// one-time-key reuse — the operator must reconcile (delete the stale side)
    /// before the node will sign.
    #[error(
        "persisted OTS key-state for validator {index} does not match the secret record \
         (seed or activation window differs); reconcile the store and the secrets dir"
    )]
    KeyStateMismatch {
        /// Validator index whose two records disagree.
        index: u64,
    },
}

/// Errors raised while SIGNING at the runtime production boundary.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
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
    /// Persisting the advanced one-time watermark failed, so the signature was
    /// withheld (persist-before-release — see `duties::ots_signer`). No key
    /// material leaked and no index was durably burned. Note the in-memory
    /// watermark HAS advanced, so re-signing the same epoch in-process surfaces
    /// [`CryptoError::EpochReused`], not a fresh signature; recovery is a restart
    /// + `load_resuming`, which resumes the older durable watermark.
    #[error("persist OTS watermark for validator {validator_id}")]
    Persist {
        /// Validator whose advanced record could not be persisted.
        validator_id: u64,
        /// Underlying storage failure.
        #[source]
        source: StorageError,
    },
}

/// Produces a validator's signature over one attestation.
///
/// The abstraction [`crate::chain::Service`] depends on, so the chain service is
/// coupled to the ACT of signing rather than to leanSig key material: production
/// injects [`LocalSigner`], tests inject a stub. Without this seam every test
/// that merely produces a block has to generate real `ProdScheme` keys, which is
/// CPU-heavy enough to force `#[ignore]`.
///
/// `&mut self` is load-bearing: a one-time-key implementation advances its
/// watermark on each sign, and the borrow checker then prevents two signers
/// sharing one key state.
///
/// `Send` is a supertrait so `dyn AttestationSigner` is `Send` — the chain
/// service holds one behind an `Arc<Mutex<..>>` and must stay `Send + Sync`.
pub trait AttestationSigner: Send {
    /// Signs `att` for its own `validator_id`.
    ///
    /// # Errors
    /// [`SignError`] if no key is loaded for the validator, the slot leaves the
    /// signature scheme's epoch domain, or the underlying scheme rejects the
    /// operation (including one-time-key reuse).
    fn sign_attestation(&mut self, att: &Attestation) -> Result<Signature, SignError>;
}

/// A signer whose one-time watermark can be snapshotted for persistence.
///
/// The requirement the durable guard (`duties::ots_signer`) places on its INNER
/// signer, split off [`AttestationSigner`] so the chain-facing seam stays
/// sign-only and a signer with no persistable key state (the test stubs) cannot
/// be wrapped in the guard by accident — the guard would then sign but never
/// persist a watermark.
pub trait PersistableSigner: AttestationSigner {
    /// Snapshots the crypto-free, **seed-free** [`OtsWatermark`] for `validator`
    /// after a sign, or `None` if no key is loaded for it. Backed by
    /// `crypto::SigningKey::to_watermark` in production ([`LocalSigner`]); the
    /// guard persists it, so no key material reaches the store.
    fn watermark_for(&self, validator: ValidatorIndex) -> Option<OtsWatermark>;
}

/// Holds the local validators' live signing keys, keyed by index.
///
/// The map value is a [`crypto::ProdSigningKey`] (the pinned production scheme).
/// The composition root builds this and hands it to the chain service; signing
/// happens at the runtime boundary, outside the forkchoice store lock.
///
/// The production implementation of [`AttestationSigner`].
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
        Self::load_with(secrets_dir, local_indices, |_, record| Ok(record))
    }

    /// Like [`load`](Self::load), but additionally consults `store` for each
    /// validator's persisted one-time watermark and resumes from whichever
    /// record — file or store — has advanced further.
    ///
    /// The keygen-written file carries `next_index = 0` forever (nothing
    /// rewrites it), while the runtime persists the advanced record on every
    /// sign (`duties::ots_signer`). Resuming from the file alone would rewind
    /// the watermark to fresh-keygen state and re-enable one-time-key reuse
    /// after a restart; taking the further-advanced record is fail-safe in both
    /// directions — worst case it skips unused leaves, it never reuses one.
    ///
    /// # Errors
    /// Everything [`load`](Self::load) raises, plus
    /// [`SignerLoadError::KeyStateLoad`] if the store read fails and
    /// [`SignerLoadError::KeyStateMismatch`] if the two records describe
    /// different keys (seed / activation window disagree).
    pub fn load_resuming(
        secrets_dir: &Path,
        local_indices: impl IntoIterator<Item = ValidatorIndex>,
        store: &dyn storage::Store,
    ) -> Result<Self, SignerLoadError> {
        Self::load_with(secrets_dir, local_indices, |index, file_record| {
            let stored = store.load_ots_key_state(index).map_err(|source| {
                SignerLoadError::KeyStateLoad {
                    index: index.get(),
                    source,
                }
            })?;
            resolve_record(index, file_record, stored)
        })
    }

    /// The one load loop both constructors share: reads + decodes each
    /// validator's secret record, lets `resolve` pick the record to restore
    /// from (identity for [`load`](Self::load), the store merge for
    /// [`load_resuming`](Self::load_resuming)), and restores the signing key.
    fn load_with(
        secrets_dir: &Path,
        local_indices: impl IntoIterator<Item = ValidatorIndex>,
        mut resolve: impl FnMut(ValidatorIndex, OtsKeyState) -> Result<OtsKeyState, SignerLoadError>,
    ) -> Result<Self, SignerLoadError> {
        let mut keys = BTreeMap::new();
        for index in local_indices {
            let file_record = read_secret_record(secrets_dir, index)?;
            let record = resolve(index, file_record)?;
            let key = ProdSigningKey::from_record(&record).map_err(|source| {
                SignerLoadError::KeyRestore {
                    index: index.get(),
                    source,
                }
            })?;
            keys.insert(index, key);
        }
        Ok(Self { keys })
    }
}

/// Reads and decodes `<secrets_dir>/validator_<i>.ssz` for one validator.
fn read_secret_record(
    secrets_dir: &Path,
    index: ValidatorIndex,
) -> Result<OtsKeyState, SignerLoadError> {
    let idx = index.get();
    let path = validator_secret_path(secrets_dir, idx);
    let bytes = std::fs::read(&path).map_err(|source| SignerLoadError::SecretFileRead {
        index: idx,
        path: path.clone(),
        source,
    })?;
    OtsKeyState::from_ssz_bytes(&bytes)
        .map_err(|source| SignerLoadError::KeyStateDecode { index: idx, source })
}

/// Merges the on-disk secret record with the store-persisted watermark, keeping
/// the seed from the file and the further-advanced `next_index`.
///
/// The seed lives ONLY in the file record; the store holds a seed-free
/// [`OtsWatermark`]. The two must describe the SAME key — the file seed's
/// [`seed_commitment`] must equal the stored commitment, and the activation
/// windows must agree — otherwise the merge is refused loudly, since silently
/// preferring either side of a mismatched pair risks resurrecting a stale key or
/// rewinding a watermark. The returned record always carries the FILE seed (so
/// `from_record` regenerates the real key) with `next_index` advanced to the
/// higher of the two watermarks: fail-safe in both directions — worst case it
/// skips unused leaves, it never reuses one. With no stored watermark (first
/// boot, or an in-memory store), the file record stands alone.
fn resolve_record(
    index: ValidatorIndex,
    file: OtsKeyState,
    stored: Option<OtsWatermark>,
) -> Result<OtsKeyState, SignerLoadError> {
    let Some(stored) = stored else {
        return Ok(file);
    };
    let same_key = stored.identifies(
        &seed_commitment(file.seed),
        file.activation_epoch,
        file.num_active_epochs,
    );
    if !same_key {
        return Err(SignerLoadError::KeyStateMismatch { index: index.get() });
    }
    Ok(OtsKeyState {
        next_index: file.next_index.max(stored.next_index),
        ..file
    })
}

impl AttestationSigner for LocalSigner {
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
    fn sign_attestation(&mut self, att: &Attestation) -> Result<Signature, SignError> {
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

impl PersistableSigner for LocalSigner {
    /// Exposes the seed-free watermark for `validator` — the durable-guard seam
    /// (`duties::ots_signer`) snapshots this after each sign to persist the
    /// advanced watermark without ever writing key material to the store.
    fn watermark_for(&self, validator: ValidatorIndex) -> Option<OtsWatermark> {
        self.keys.get(&validator).map(ProdSigningKey::to_watermark)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::super::test_fixtures::{write_validator_secrets, MIN_ACTIVE_EPOCHS};
    use super::*;
    use crypto::{ProdScheme, PublicKey};
    use protocol::{AttestationData, Slot};
    use ssz::HashTreeRoot;

    /// Writes a `validator_<i>.ssz` record per index into a fresh temp dir and
    /// returns the dir (kept alive by the caller) plus the matching public keys.
    ///
    /// The generation itself lives in `duties::test_fixtures`; this only owns the
    /// temp dir. `ProdScheme` keygen is CPU-heavy, so callers pass the minimum
    /// index set they need. These tests sign at epoch 0 only.
    fn make_secret_dir(indices: &[u64]) -> (tempfile::TempDir, BTreeMap<u64, PublicKey>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pubs = write_validator_secrets(dir.path(), indices, MIN_ACTIVE_EPOCHS);
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

    /// `resolve_record` merge rules — pure record selection, no keygen.
    mod resolve_record {
        use super::super::resolve_record;
        use super::*;
        use crypto::seed_commitment;
        use types::OtsWatermark;

        /// A file (secret) record — carries the raw seed.
        fn record(seed_byte: u8, next_index: u64) -> OtsKeyState {
            OtsKeyState {
                seed: [seed_byte; 32],
                activation_epoch: 0,
                num_active_epochs: 1_024,
                next_index,
            }
        }

        /// A stored watermark for the key generated from `[seed_byte; 32]`
        /// (its commitment), matching `record`'s window.
        fn watermark(seed_byte: u8, next_index: u64) -> OtsWatermark {
            OtsWatermark {
                key_commitment: seed_commitment([seed_byte; 32]),
                activation_epoch: 0,
                num_active_epochs: 1_024,
                next_index,
            }
        }

        #[test]
        fn no_stored_record_uses_file() {
            let got = resolve_record(ValidatorIndex::new(0), record(1, 0), None).unwrap();
            assert_eq!(got.next_index, 0);
        }

        #[test]
        fn stored_ahead_of_file_wins() {
            // The restart case: the file is frozen at keygen (0), the store
            // carries the advance. Resuming from the store is what prevents
            // one-time-key reuse.
            let got = resolve_record(ValidatorIndex::new(0), record(1, 0), Some(watermark(1, 5)))
                .unwrap();
            assert_eq!(got.next_index, 5);
            // The regenerated key uses the FILE seed, never the store's bytes.
            assert_eq!(got.seed, [1u8; 32]);
        }

        #[test]
        fn file_ahead_of_stored_wins() {
            // Symmetric: never rewind, whichever side is behind.
            let got = resolve_record(ValidatorIndex::new(0), record(1, 7), Some(watermark(1, 3)))
                .unwrap();
            assert_eq!(got.next_index, 7);
        }

        #[test]
        fn equal_watermarks_are_fine() {
            let got = resolve_record(ValidatorIndex::new(0), record(1, 4), Some(watermark(1, 4)))
                .unwrap();
            assert_eq!(got.next_index, 4);
        }

        #[test]
        fn seed_mismatch_is_refused() {
            // A stored watermark from a DIFFERENT key (rotated seed → different
            // commitment) must not be merged — fail loud, never silently pick a side.
            let err = resolve_record(ValidatorIndex::new(3), record(1, 0), Some(watermark(2, 5)))
                .unwrap_err();
            assert!(matches!(
                err,
                SignerLoadError::KeyStateMismatch { index: 3 }
            ));
        }

        #[test]
        fn window_mismatch_is_refused() {
            let mut stored = watermark(1, 5);
            stored.num_active_epochs = 2_048;
            let err =
                resolve_record(ValidatorIndex::new(0), record(1, 0), Some(stored)).unwrap_err();
            assert!(matches!(
                err,
                SignerLoadError::KeyStateMismatch { index: 0 }
            ));
        }
    }
}
