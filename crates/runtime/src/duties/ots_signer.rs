//! Durable one-time-signature guard at the runtime sign boundary (Part 15).
//!
//! [`OtsSigner`] wraps the local signer behind the narrow [`AttestationSigner`]
//! seam and persists each validator's advanced watermark through
//! [`storage::Store`] before a signature is released, so an OTS leaf is never
//! signed twice across a restart or a self-sync. The in-memory monotonic guard
//! and the leanSig algorithm stay in `crypto`; the byte record stays in `types`;
//! the durable KV stays in `storage`. This module owns only the persist-before-
//! release ordering (sign → advance the index → persist the record → release the
//! signature), keyed per [`ValidatorIndex`]. The stronger reserve-before-sign
//! ordering (persist the consumed index *before* the crypto sign) is the Part-3
//! hardening — see [`OtsSigner::sign_own_duty`].
//!
//! Scope note: until Part 3 wires [`OtsSigner::sign_own_duty`] into the
//! production sign sites (`produce_block` / `produce_attestation`), the whole
//! guard cluster is reachable only from tests — see the `#[allow(dead_code)]` on
//! the `mod ots_signer;` declaration in `duties/mod.rs`, which Part 3 removes.

use std::sync::Arc;

use protocol::{Attestation, ValidatorIndex};
use storage::{StorageError, Store};
use types::{OtsKeyState, Signature};

// NOTE (Part-13 decoupling): the seam speaks the Part-15-owned [`SignRejected`]
// rather than Part 13's `SignError`, so this module compiles + tests on a base
// that does not yet carry `duties::signer`. Part 3's `impl AttestationSigner for
// LocalSigner` projects `SignError` onto `SignRejected` (add `impl
// From<SignError> for SignRejected` there). Do NOT import the Part-13 module here.

/// Part-15-owned sign-failure surface for the [`AttestationSigner`] seam.
///
/// Decouples the guard from Part 13's `SignError`: the reuse / backward / other
/// failures the inner signer detects are projected onto these variants at the
/// seam boundary (Part 3). Reuse stays a first-class variant so a reuse test can
/// assert it without the Part-13 type.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum SignRejected {
    /// The inner signer refused a one-time-key reuse / backward epoch for this
    /// validator (projected from `CryptoError::EpochReused` / `EpochNotActive`).
    #[error("one-time-key reuse or backward epoch for validator {validator}")]
    Reuse {
        /// Validator whose sign was refused for reuse / backward epoch.
        validator: u64,
    },
    /// Any other inner-sign failure (unknown validator, epoch overflow, leanSig
    /// error), carried as an opaque message so the seam stays Part-13-agnostic.
    ///
    /// SECURITY (Part 3): when `impl From<SignError> for SignRejected` populates
    /// this `message` by projecting the inner signer's error, it MUST NEVER embed
    /// key material (seed / secret bytes) — the string flows into `OtsError`'s
    /// `Debug`/`Display`. Preserve `OtsKeyState`'s redaction discipline at the seam.
    #[error("sign refused: {message}")]
    Other {
        /// Message projected from the inner signer's error.
        message: String,
    },
}

/// The narrow signing seam [`OtsSigner`] wraps.
///
/// Production impl: Part 13's `LocalSigner` (real leanSig, advances the in-memory
/// one-time index and rejects reuse / backward, mapped to [`SignRejected`]). Test
/// impl: a hand-written fake (no `ProdScheme` keygen) — the persist-ordering
/// logic is exercised entirely through this trait.
pub(crate) trait AttestationSigner {
    /// Signs `att` for its own `validator_id`, advancing the in-memory one-time
    /// index. Mirrors `LocalSigner::sign_attestation`, projected to
    /// [`SignRejected`].
    fn sign_attestation(&mut self, att: &Attestation) -> Result<Signature, SignRejected>;

    /// Snapshots the crypto-free persistable record for `validator` after a sign,
    /// or `None` if no key is loaded for it. Backed by
    /// `crypto::SigningKey::to_record` in production.
    fn record_for(&self, validator: ValidatorIndex) -> Option<OtsKeyState>;
}

/// Errors raised while signing-with-persistence at the runtime boundary.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum OtsError {
    /// Persisting the advanced watermark failed; no signature is returned, so a
    /// crash-equivalent never leaks a used-but-unpersisted index.
    #[error("persist OTS key-state for validator {validator}")]
    Persist {
        /// Validator whose record could not be persisted.
        validator: u64,
        /// Underlying storage failure.
        #[source]
        source: StorageError,
    },
    /// The inner signer refused — includes one-time-key reuse and backward-epoch
    /// (projected via [`SignRejected`]), so a double-sign is a visible error,
    /// never a silent second index burn.
    #[error(transparent)]
    Sign(#[from] SignRejected),
    /// The signer produced a signature but exposed no record for the validator
    /// (invariant violation: a loaded key must yield a record).
    #[error("no OTS key-state record for validator {validator}")]
    UnknownValidator {
        /// Validator with a missing post-sign record.
        validator: u64,
    },
}

/// Signs own-duty attestations and persists the advanced one-time index before
/// releasing the signature. Constructed at the composition root with the local
/// signer and the durable store; keyed per [`ValidatorIndex`] via the record home.
pub(crate) struct OtsSigner {
    inner: Box<dyn AttestationSigner + Send>,
    store: Arc<dyn Store>,
}

impl OtsSigner {
    /// Builds the guard over `inner` (the local signer) and `store` (the durable
    /// persistence sink).
    pub(crate) fn new(inner: Box<dyn AttestationSigner + Send>, store: Arc<dyn Store>) -> Self {
        Self { inner, store }
    }

    /// Signs `att` for its own duty and persists the advanced watermark before
    /// the signature is returned.
    ///
    /// Order (shape-(a) baseline — see plan OQ6): the in-memory sign advances the
    /// one-time index (and rejects reuse / backward via [`SignRejected`]); the
    /// advanced record is then persisted; only on a successful persist is the
    /// signature released. A persist failure returns [`OtsError::Persist`] and no
    /// signature.
    ///
    /// Part-3 hardening (`NEEDS_VALIDATION`): reserve-before-sign — persist
    /// `next_index = epoch + 1` before the crypto sign — once the Part-13
    /// `SigningKey` surface is on this branch (plan OQ6 shape (b)). Parts 1–2
    /// alone do NOT provide cross-restart reuse safety.
    ///
    /// # Errors
    /// - [`OtsError::Sign`] on any inner-sign failure (reuse / backward / other).
    /// - [`OtsError::Persist`] if the advanced record cannot be persisted.
    /// - [`OtsError::UnknownValidator`] if the signer exposes no record post-sign.
    pub(crate) fn sign_own_duty(&mut self, att: &Attestation) -> Result<Signature, OtsError> {
        let validator = att.validator_id;
        let signature = self.inner.sign_attestation(att)?;
        let record = self
            .inner
            .record_for(validator)
            .ok_or(OtsError::UnknownValidator {
                validator: validator.get(),
            })?;
        self.store
            .save_ots_key_state(validator, record)
            .map_err(|source| OtsError::Persist {
                validator: validator.get(),
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

    use parking_lot::Mutex;
    use protocol::{SignedBlockWithAttestation, State};
    use storage::HeadInfo;
    use types::Bytes32;

    /// In-test signer: advances a per-validator sign count (which drives the
    /// exposed record's `next_index`) and optionally refuses a repeat sign to
    /// model one-time-key reuse. No `ProdScheme` keygen.
    struct FakeSigner {
        signed: BTreeMap<ValidatorIndex, u64>,
        reject_repeats: bool,
        // When false, `record_for` returns `None` even after a successful sign —
        // models the invariant-violation path guarded by `OtsError::UnknownValidator`.
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

        /// Like [`Self::new`], but `record_for` returns `None` after a successful
        /// sign — models the invariant violation guarded by
        /// [`OtsError::UnknownValidator`].
        fn without_record() -> Self {
            Self {
                expose_record: false,
                ..Self::new()
            }
        }
    }

    impl AttestationSigner for FakeSigner {
        fn sign_attestation(&mut self, att: &Attestation) -> Result<Signature, SignRejected> {
            let validator = att.validator_id;
            if self.reject_repeats && self.signed.contains_key(&validator) {
                return Err(SignRejected::Reuse {
                    validator: validator.get(),
                });
            }
            *self.signed.entry(validator).or_insert(0) += 1;
            Ok(Signature::zero())
        }

        fn record_for(&self, validator: ValidatorIndex) -> Option<OtsKeyState> {
            if !self.expose_record {
                return None;
            }
            self.signed.get(&validator).map(|&n| OtsKeyState {
                seed: [0u8; 32],
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
        ots: Mutex<BTreeMap<ValidatorIndex, OtsKeyState>>,
        fail_save: bool,
    }

    impl FakeStore {
        /// A store whose `save_ots_key_state` always fails — models a persistence
        /// failure (the crash-equivalent path).
        fn failing() -> Self {
            Self {
                ots: Mutex::new(BTreeMap::new()),
                fail_save: true,
            }
        }
    }

    impl Store for FakeStore {
        fn save_ots_key_state(
            &self,
            validator: ValidatorIndex,
            record: OtsKeyState,
        ) -> Result<(), StorageError> {
            if self.fail_save {
                return Err(StorageError::Backend {
                    message: "forced save failure".to_owned(),
                });
            }
            self.ots.lock().insert(validator, record);
            Ok(())
        }

        fn load_ots_key_state(
            &self,
            validator: ValidatorIndex,
        ) -> Result<Option<OtsKeyState>, StorageError> {
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

        let sig = signer.sign_own_duty(&attestation(0)).unwrap();
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
        let err = signer.sign_own_duty(&attestation(0)).unwrap_err();
        assert!(matches!(err, OtsError::Persist { validator: 0, .. }));

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
        let err = signer.sign_own_duty(&attestation(0)).unwrap_err();
        assert!(matches!(err, OtsError::UnknownValidator { validator: 0 }));
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

        signer.sign_own_duty(&attestation(0)).unwrap();
        let err = signer.sign_own_duty(&attestation(0)).unwrap_err();
        assert!(matches!(
            err,
            OtsError::Sign(SignRejected::Reuse { validator: 0 })
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

        signer.sign_own_duty(&attestation(0)).unwrap();
        signer.sign_own_duty(&attestation(1)).unwrap();

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
}
