//! Durable one-time-signature guard at the runtime sign boundary (Part 15).
//!
//! [`OtsSigner`] wraps the local signer behind the [`AttestationSigner`] seam
//! and persists each validator's advanced watermark through [`storage::Store`]
//! before a signature is released, so an OTS leaf is never signed twice across
//! a restart or a self-sync. The in-memory monotonic guard and the leanSig
//! algorithm stay in `crypto`; the byte record stays in `types`; the durable KV
//! stays in `storage`. This module owns only the persist-before-release
//! ordering (sign → advance the index → persist the record → release the
//! signature), keyed per validator index. Only the seed-free [`OtsWatermark`]
//! is persisted, so key material never reaches the store. The stronger
//! reserve-before-sign ordering (persist the consumed index *before* the crypto
//! sign) remains a later hardening — see the [`AttestationSigner`] impl on
//! [`OtsSigner`].
//!
//! Wiring: the composition root (`node::devnet`) constructs
//! `OtsSigner::new(Box::new(local_signer), store)` and injects it as the chain
//! service's [`AttestationSigner`], so every production sign
//! (`produce_block` / `produce_attestation`) flows through the guard. The
//! matching read side is `LocalSigner::load_resuming`, which resumes the
//! watermark from the store at startup.

use std::sync::Arc;

use protocol::Attestation;
use storage::Store;
use types::Signature;

use super::signer::{AttestationSigner, PersistableSigner, SignError};

/// Signs own-duty attestations and persists the advanced one-time index before
/// releasing the signature. Constructed at the composition root with the local
/// signer and the durable store; keyed per validator index via the record home.
pub struct OtsSigner {
    inner: Box<dyn PersistableSigner>,
    store: Arc<dyn Store>,
}

impl OtsSigner {
    /// Builds the guard over `inner` (the local signer) and `store` (the durable
    /// persistence sink). `inner` is a [`PersistableSigner`] by construction: a
    /// signer with no persistable key state cannot be wrapped, so the guard can
    /// never sign without a watermark to persist.
    #[must_use]
    pub fn new(inner: Box<dyn PersistableSigner>, store: Arc<dyn Store>) -> Self {
        Self { inner, store }
    }
}

impl AttestationSigner for OtsSigner {
    /// Signs `att` for its own duty and persists the advanced watermark before
    /// the signature is returned. This is what makes the guard hold on the
    /// production paths: the chain service holds the seam, the composition root
    /// injects [`OtsSigner`], and neither knows about persistence.
    ///
    /// Order (persist-after-sign): the in-memory sign advances the one-time
    /// index (and rejects reuse / backward via [`SignError`]); the advanced,
    /// seed-free watermark is then persisted; only on a successful persist is the
    /// signature released. A persist failure returns [`SignError::Persist`] and
    /// no signature, so a crash-equivalent never leaks a used-but-unpersisted
    /// index.
    ///
    /// Later hardening (`NEEDS_VALIDATION`): reserve-before-sign — persist
    /// `next_index = epoch + 1` before the crypto sign.
    ///
    /// # Errors
    /// - Any inner-sign failure, unchanged (reuse / backward / other).
    /// - [`SignError::Persist`] if the advanced watermark cannot be persisted.
    /// - [`SignError::UnknownValidator`] if the signer exposes no watermark
    ///   post-sign (invariant violation: a loaded key must yield one).
    fn sign_attestation(&mut self, att: &Attestation) -> Result<Signature, SignError> {
        let validator = att.validator_id;
        let signature = self.inner.sign_attestation(att)?;
        let watermark = self
            .inner
            .watermark_for(validator)
            .ok_or(SignError::UnknownValidator {
                validator_id: validator.get(),
            })?;
        self.store
            .save_ots_key_state(validator, watermark)
            .map_err(|source| SignError::Persist {
                validator_id: validator.get(),
                source,
            })?;
        Ok(signature)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;

    use crypto::CryptoError;
    use parking_lot::Mutex;
    use protocol::{SignedBlockWithAttestation, State, ValidatorIndex};
    use storage::{HeadInfo, StorageError};
    use types::{Bytes32, OtsWatermark};

    /// In-test signer: advances a per-validator sign count (which drives the
    /// exposed watermark's `next_index`) and optionally refuses a repeat sign to
    /// model one-time-key reuse. No `ProdScheme` keygen.
    struct FakeSigner {
        signed: BTreeMap<ValidatorIndex, u64>,
        reject_repeats: bool,
        // When false, `watermark_for` returns `None` even after a successful sign —
        // models the invariant-violation path guarded by `SignError::UnknownValidator`.
        expose_record: bool,
    }

    impl FakeSigner {
        /// A signer that signs every request and exposes a monotonic record.
        fn new() -> Self {
            Self {
                signed: BTreeMap::new(),
                reject_repeats: false,
                expose_record: true,
            }
        }

        /// Like [`Self::new`], but refuses a second sign for a validator it has
        /// already signed — models one-time-key reuse.
        fn reject_repeats() -> Self {
            Self {
                reject_repeats: true,
                ..Self::new()
            }
        }

        /// Like [`Self::new`], but `watermark_for` returns `None` after a
        /// successful sign — models the invariant violation guarded by
        /// [`SignError::UnknownValidator`].
        fn without_record() -> Self {
            Self {
                expose_record: false,
                ..Self::new()
            }
        }
    }

    impl AttestationSigner for FakeSigner {
        fn sign_attestation(&mut self, att: &Attestation) -> Result<Signature, SignError> {
            let validator = att.validator_id;
            if self.reject_repeats && self.signed.contains_key(&validator) {
                return Err(SignError::Crypto(CryptoError::EpochReused {
                    epoch: 0,
                    last_signed: 0,
                }));
            }
            *self.signed.entry(validator).or_insert(0) += 1;
            Ok(Signature::zero())
        }
    }

    impl PersistableSigner for FakeSigner {
        fn watermark_for(&self, validator: ValidatorIndex) -> Option<OtsWatermark> {
            if !self.expose_record {
                return None;
            }
            self.signed.get(&validator).map(|&n| OtsWatermark {
                key_commitment: [0u8; 32],
                // Exact window values are immaterial to these tests; `262_144` is
                // 2^18, the resolved activation epoch, kept for realism.
                activation_epoch: 262_144,
                num_active_epochs: 1_024,
                next_index: n,
            })
        }
    }

    /// In-test store: a real per-validator OTS map plus a `fail_save` switch to
    /// model a persistence failure. Every other `Store` method is a trivial stub
    /// (never exercised by these tests); `save_accepted` uses the trait default.
    #[derive(Default)]
    struct FakeStore {
        ots: Mutex<BTreeMap<ValidatorIndex, OtsWatermark>>,
        fail_save: bool,
        fail_load: bool,
    }

    impl FakeStore {
        /// A store whose `save_ots_key_state` always fails — models a persistence
        /// failure (the crash-equivalent path).
        fn failing() -> Self {
            Self {
                fail_save: true,
                ..Self::default()
            }
        }

        /// A store whose `load_ots_key_state` always fails — models a store-read
        /// failure while resuming the watermark at startup.
        fn failing_load() -> Self {
            Self {
                fail_load: true,
                ..Self::default()
            }
        }
    }

    impl Store for FakeStore {
        fn save_ots_key_state(
            &self,
            validator: ValidatorIndex,
            watermark: OtsWatermark,
        ) -> Result<(), StorageError> {
            if self.fail_save {
                return Err(StorageError::Backend {
                    message: "forced save failure".to_owned(),
                });
            }
            self.ots.lock().insert(validator, watermark);
            Ok(())
        }

        fn load_ots_key_state(
            &self,
            validator: ValidatorIndex,
        ) -> Result<Option<OtsWatermark>, StorageError> {
            if self.fail_load {
                return Err(StorageError::Backend {
                    message: "forced load failure".to_owned(),
                });
            }
            Ok(self.ots.lock().get(&validator).cloned())
        }

        fn save_block(
            &self,
            _root: Bytes32,
            _block: SignedBlockWithAttestation,
        ) -> Result<(), StorageError> {
            Ok(())
        }
        fn save_state(&self, _root: Bytes32, _state: State) -> Result<(), StorageError> {
            Ok(())
        }
        fn save_head(&self, _info: HeadInfo) -> Result<(), StorageError> {
            Ok(())
        }
        fn has_block(&self, _root: &Bytes32) -> Result<bool, StorageError> {
            Ok(false)
        }
        fn has_state(&self, _root: &Bytes32) -> Result<bool, StorageError> {
            Ok(false)
        }
        fn load_block(
            &self,
            _root: &Bytes32,
        ) -> Result<Option<SignedBlockWithAttestation>, StorageError> {
            Ok(None)
        }
        fn load_state(&self, _root: &Bytes32) -> Result<Option<State>, StorageError> {
            Ok(None)
        }
        fn load_head(&self) -> Result<Option<HeadInfo>, StorageError> {
            Ok(None)
        }
    }

    /// A default attestation tagged with `validator` — the guard only reads
    /// `validator_id`, so the remaining fields are left at their defaults.
    fn attestation(validator: u64) -> Attestation {
        Attestation {
            validator_id: ValidatorIndex::new(validator),
            ..Default::default()
        }
    }

    #[test]
    fn sign_persists_advance_before_return() {
        let store = Arc::new(FakeStore::default());
        let mut signer = OtsSigner::new(
            Box::new(FakeSigner::new()),
            Arc::clone(&store) as Arc<dyn Store>,
        );

        let sig = signer.sign_attestation(&attestation(0)).unwrap();
        assert_eq!(sig, Signature::zero());

        // The advanced record is already durable by the time the signature is
        // returned (persist-before-release).
        let record = store
            .load_ots_key_state(ValidatorIndex::new(0))
            .unwrap()
            .expect("record persisted");
        assert_eq!(record.next_index, 1);
    }

    #[test]
    fn sign_returns_no_signature_on_persist_failure() {
        let store = Arc::new(FakeStore::failing());
        let mut signer = OtsSigner::new(
            Box::new(FakeSigner::new()),
            Arc::clone(&store) as Arc<dyn Store>,
        );

        // A crash-equivalent: the in-memory index advanced, but persistence
        // failed, so the caller gets Err and NO signature.
        let err = signer.sign_attestation(&attestation(0)).unwrap_err();
        assert!(matches!(
            err,
            SignError::Persist {
                validator_id: 0,
                ..
            }
        ));

        // No durable leak: the failed persist left no record behind.
        assert_eq!(
            store.load_ots_key_state(ValidatorIndex::new(0)).unwrap(),
            None
        );
    }

    #[test]
    fn sign_returns_unknown_validator_when_record_absent() {
        let store = Arc::new(FakeStore::default());
        let mut signer = OtsSigner::new(
            Box::new(FakeSigner::without_record()),
            Arc::clone(&store) as Arc<dyn Store>,
        );

        // Fail-closed: a signer that produces a signature but exposes no record
        // yields Err and persists nothing (an invariant violation, never a
        // silently-unpersisted advance).
        let err = signer.sign_attestation(&attestation(0)).unwrap_err();
        assert!(matches!(
            err,
            SignError::UnknownValidator { validator_id: 0 }
        ));
        assert_eq!(
            store.load_ots_key_state(ValidatorIndex::new(0)).unwrap(),
            None
        );
    }

    #[test]
    fn sign_rejects_epoch_reuse_without_double_advance() {
        let store = Arc::new(FakeStore::default());
        let mut signer = OtsSigner::new(
            Box::new(FakeSigner::reject_repeats()),
            Arc::clone(&store) as Arc<dyn Store>,
        );

        signer.sign_attestation(&attestation(0)).unwrap();
        let err = signer.sign_attestation(&attestation(0)).unwrap_err();
        assert!(matches!(
            err,
            SignError::Crypto(CryptoError::EpochReused { .. })
        ));

        // The rejected reuse did not persist a second, advanced record.
        let record = store
            .load_ots_key_state(ValidatorIndex::new(0))
            .unwrap()
            .expect("first record persisted");
        assert_eq!(record.next_index, 1);
    }

    #[test]
    fn sign_is_independent_per_validator() {
        let store = Arc::new(FakeStore::default());
        let mut signer = OtsSigner::new(
            Box::new(FakeSigner::new()),
            Arc::clone(&store) as Arc<dyn Store>,
        );

        signer.sign_attestation(&attestation(0)).unwrap();
        signer.sign_attestation(&attestation(1)).unwrap();

        // Two independent records; one validator's advance does not touch the other.
        assert_eq!(
            store
                .load_ots_key_state(ValidatorIndex::new(0))
                .unwrap()
                .unwrap()
                .next_index,
            1
        );
        assert_eq!(
            store
                .load_ots_key_state(ValidatorIndex::new(1))
                .unwrap()
                .unwrap()
                .next_index,
            1
        );
    }

    #[test]
    fn load_resuming_surfaces_store_read_failure() {
        use super::super::signer::{validator_secret_path, LocalSigner, SignerLoadError};
        use types::OtsKeyState;

        // A SYNTHETIC secret record (no keygen): `read_secret_record` decodes it,
        // then the failing store read short-circuits before any key
        // regeneration, so this exercises the `KeyStateLoad` path cheaply.
        let dir = tempfile::tempdir().expect("tempdir");
        let record = OtsKeyState {
            seed: [1u8; 32],
            activation_epoch: 0,
            num_active_epochs: 1_024,
            next_index: 0,
        };
        std::fs::write(validator_secret_path(dir.path(), 0), record.to_ssz_bytes())
            .expect("write synthetic secret");

        let store = FakeStore::failing_load();
        // `let Err(..) else`: the Ok type holds secret keys and is not `Debug`.
        let Err(err) = LocalSigner::load_resuming(dir.path(), [ValidatorIndex::new(0)], &store)
        else {
            panic!("a store read failure while resuming must surface");
        };
        assert!(matches!(
            err,
            SignerLoadError::KeyStateLoad { index: 0, .. }
        ));
    }

    /// Full restart round-trip over REAL leanSig keys: sign through the guard,
    /// "restart" by reloading the signer via `load_resuming` against the same
    /// store, and confirm the watermark survived — the same epoch is refused,
    /// the next epoch signs.
    #[test]
    #[ignore = "leanSig ProdScheme keygen is CPU-heavy; run explicitly with --ignored"]
    fn restart_resumes_watermark_from_store() {
        use super::super::signer::LocalSigner;
        use super::super::test_fixtures::{write_validator_secrets, MIN_ACTIVE_EPOCHS};
        use protocol::{AttestationData, Slot};

        let dir = tempfile::tempdir().expect("tempdir");
        let _ = write_validator_secrets(dir.path(), &[0], MIN_ACTIVE_EPOCHS);
        let store: Arc<dyn Store> = Arc::new(FakeStore::default());

        let att = |slot: u64| Attestation {
            validator_id: ValidatorIndex::new(0),
            data: AttestationData {
                slot: Slot::new(slot),
                ..Default::default()
            },
        };

        // First run: sign epoch 0 through the guard; the advance persists.
        let local = LocalSigner::load(dir.path(), [ValidatorIndex::new(0)]).unwrap();
        let mut signer = OtsSigner::new(Box::new(local), Arc::clone(&store));
        signer.sign_attestation(&att(0)).unwrap();

        // "Restart": the secrets file still says next_index = 0, but the store
        // carries the advance — load_resuming takes the further-advanced record.
        let local =
            LocalSigner::load_resuming(dir.path(), [ValidatorIndex::new(0)], store.as_ref())
                .unwrap();
        let mut signer = OtsSigner::new(Box::new(local), Arc::clone(&store));

        // Same epoch again → refused as reuse (the watermark survived the restart).
        let err = signer.sign_attestation(&att(0)).unwrap_err();
        assert!(matches!(
            err,
            SignError::Crypto(CryptoError::EpochReused { .. })
        ));

        // The next epoch is fresh and signs.
        signer.sign_attestation(&att(1)).unwrap();
    }
}
