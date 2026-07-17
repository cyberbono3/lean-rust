//! SA5 conversions between an in-memory [`SigningKey`] and the crypto-free,
//! persistable [`OtsKeyState`] record.
//!
//! The record stores a reproducible seed rather than the variable-size leanSig
//! secret, plus the monotonic `next_index` watermark. [`SigningKey::from_record`]
//! reconstructs the key AND restores the watermark, so a reload across a restart
//! cannot re-enable one-time-key reuse — the discipline the [`SigningKey`] guards
//! in memory (see `key_state`), extended across persistence.

use types::OtsKeyState;

use crate::error::CryptoError;
use crate::key_state::SigningKey;
use crate::scheme::{generate_from_seed, SchemeWire};

impl<S: SchemeWire> SigningKey<S> {
    /// Snapshots this key as a crypto-free record.
    ///
    /// `next_index = last_signed + 1` (`0` when the key has never signed).
    ///
    /// Persists the scheme-ALIGNED activation window: leanSig expands/aligns a
    /// requested window, and [`activation_interval`](Self::activation_interval)
    /// returns the expanded range. A misaligned request is therefore stored in
    /// its aligned form; `from_record` re-feeds it and leanSig's expansion is
    /// idempotent on aligned input, so the key round-trips.
    #[must_use]
    pub fn to_record(&self) -> OtsKeyState {
        let activation = self.activation_interval();
        OtsKeyState {
            seed: self.seed(),
            activation_epoch: activation.start,
            num_active_epochs: activation.end - activation.start,
            next_index: self.last_signed().map_or(0, |epoch| u64::from(epoch) + 1),
        }
    }

    /// Reconstructs a signing key from a record, restoring the one-time-key
    /// watermark so a reload cannot re-enable reuse.
    ///
    /// Regenerates the key deterministically from the record's seed and sets the
    /// watermark to `next_index - 1`, so any epoch at or below the last one signed
    /// before the record was taken is refused after reload.
    ///
    /// The reloaded key sits at its activation-start prepared window (like any
    /// freshly generated key). The caller advances it with
    /// [`prepare`](SigningKey::prepare) to the epoch it actually intends to sign —
    /// this deliberately does NOT eagerly walk the window to `next_index`, which
    /// for a key deep in its lifetime would be a long, blocking advance on every
    /// reload.
    ///
    /// # Errors
    ///
    /// - [`CryptoError::ActivationOutOfRange`] when the record's window does not
    ///   fit the scheme's lifetime (via `generate_from_seed`).
    /// - [`CryptoError::EpochNotActive`] when a nonzero `next_index` falls outside
    ///   `(activation_epoch, activation_end]` — a value at/below the window start
    ///   would rewind the watermark (re-enabling reuse), and one past the end
    ///   describes a key that could never sign; both are refused.
    pub fn from_record(record: &OtsKeyState) -> Result<Self, CryptoError> {
        let activation_epoch = usize::try_from(record.activation_epoch).map_err(|_| {
            CryptoError::ActivationOutOfRange {
                activation_epoch: usize::MAX,
                num_active_epochs: 0,
                lifetime: S::LIFETIME,
            }
        })?;
        let num_active_epochs = usize::try_from(record.num_active_epochs).map_err(|_| {
            CryptoError::ActivationOutOfRange {
                activation_epoch,
                num_active_epochs: usize::MAX,
                lifetime: S::LIFETIME,
            }
        })?;

        let (_pk, mut signing_key) =
            generate_from_seed::<S>(record.seed, activation_epoch, num_active_epochs)?;

        if record.next_index > 0 {
            // A valid nonzero watermark falls within `(activation_epoch, activation_end]`:
            // `last_signed = next_index - 1` must be a real epoch inside the window.
            // `next_index == activation_end` is the legitimate exhausted-key case. A value
            // at/below the window start (impossible from `to_record`) would rewind the
            // watermark to before any signable epoch, and one past the end describes a key
            // that could never sign — both are corrupt records and are refused rather than
            // returned as a silently unsignable or replay-enabling key.
            let activation_end = record.activation_epoch + record.num_active_epochs;
            if record.next_index <= record.activation_epoch || record.next_index > activation_end {
                return Err(CryptoError::EpochNotActive {
                    epoch: u32::try_from(record.next_index).unwrap_or(u32::MAX),
                    start: record.activation_epoch,
                    end: activation_end,
                });
            }
            let last_signed =
                u32::try_from(record.next_index - 1).map_err(|_| CryptoError::EpochNotActive {
                    epoch: u32::MAX,
                    start: record.activation_epoch,
                    end: activation_end,
                })?;
            signing_key.set_last_signed(last_signed);
        }

        Ok(signing_key)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::scheme::generate_from_seed;
    use crate::scheme::test_support::TestScheme;
    use crate::verify::verify;

    const SEED: [u8; 32] = [9u8; 32];
    const MSG: [u8; 32] = [0xab; 32];

    #[test]
    fn same_seed_reproduces_identical_pubkey() {
        let (pk_a, _a) = generate_from_seed::<TestScheme>(SEED, 0, 64).unwrap();
        let (pk_b, _b) = generate_from_seed::<TestScheme>(SEED, 0, 64).unwrap();
        assert_eq!(pk_a, pk_b);
    }

    #[test]
    fn to_from_record_round_trips_and_signs() {
        let (pk, sk) = generate_from_seed::<TestScheme>(SEED, 0, 64).unwrap();
        let record = sk.to_record();
        assert_eq!(record.next_index, 0);

        let mut reloaded = SigningKey::<TestScheme>::from_record(&record).unwrap();
        let start = u32::try_from(reloaded.activation_interval().start).unwrap();
        let sig = reloaded.sign(start, &MSG).unwrap();
        assert!(verify::<TestScheme>(&pk, start, &MSG, &sig).is_ok());
    }

    #[test]
    fn reload_after_sign_refuses_replay() {
        let (_pk, mut sk) = generate_from_seed::<TestScheme>(SEED, 0, 64).unwrap();
        sk.sign(5, &MSG).unwrap();

        let record = sk.to_record();
        assert_eq!(record.next_index, 6); // last_signed 5 -> next_index 6

        let mut reloaded = SigningKey::<TestScheme>::from_record(&record).unwrap();
        // Watermark preserved across reload: signing epoch <= 5 is refused.
        let Err(err) = reloaded.sign(5, &MSG) else {
            panic!("replay of epoch 5 must be refused after reload");
        };
        assert!(matches!(
            err,
            CryptoError::EpochReused {
                epoch: 5,
                last_signed: 5
            }
        ));
        reloaded.sign(6, &MSG).unwrap(); // forward epoch still works
    }

    #[test]
    fn signing_key_debug_redacts_seed() {
        let (_pk, sk) = generate_from_seed::<TestScheme>(SEED, 0, 64).unwrap();
        let rendered = format!("{sk:?}");
        assert!(rendered.contains("<redacted>"));
        // SEED is all 0x09; its byte-list rendering must not appear.
        assert!(
            !rendered.contains("9, 9, 9"),
            "seed bytes leaked into Debug"
        );
    }

    #[test]
    fn reload_never_signed_nonzero_activation_signs_at_start() {
        let (pk, sk) = generate_from_seed::<TestScheme>(SEED, 64, 64).unwrap();
        let record = sk.to_record();
        assert_eq!(record.next_index, 0);

        let mut reloaded = SigningKey::<TestScheme>::from_record(&record).unwrap();
        let start = u32::try_from(reloaded.activation_interval().start).unwrap();
        let sig = reloaded.sign(start, &MSG).unwrap();
        assert!(verify::<TestScheme>(&pk, start, &MSG, &sig).is_ok());
    }

    #[test]
    fn reload_exhausted_key_does_not_panic_and_refuses_signing() {
        let (_pk, sk) = generate_from_seed::<TestScheme>(SEED, 0, 64).unwrap();
        let end = sk.activation_interval().end;

        let mut record = sk.to_record();
        record.next_index = end; // exhausted: no signable epoch remains

        let mut reloaded = SigningKey::<TestScheme>::from_record(&record).unwrap();
        let last = u32::try_from(end - 1).unwrap();
        assert!(reloaded.sign(last, &MSG).is_err()); // <= watermark -> refused
    }

    #[test]
    fn misaligned_window_round_trips_via_reload_and_verifies() {
        let (pk, sk) = generate_from_seed::<TestScheme>(SEED, 7, 20).unwrap();
        // Reload through from_record (not a bare re-derive) and prove the reloaded
        // key still produces a signature that verifies under the original pubkey —
        // exercising expand-idempotence on the SIGNING path, not just pubkey equality.
        let mut reloaded = SigningKey::<TestScheme>::from_record(&sk.to_record()).unwrap();
        let start = u32::try_from(reloaded.activation_interval().start).unwrap();
        let sig = reloaded.sign(start, &MSG).unwrap();
        assert!(verify::<TestScheme>(&pk, start, &MSG, &sig).is_ok());
    }

    #[test]
    fn from_record_rejects_next_index_past_window_end() {
        let (_pk, sk) = generate_from_seed::<TestScheme>(SEED, 0, 64).unwrap();
        let end = sk.activation_interval().end;

        let mut record = sk.to_record();
        record.next_index = end + 1; // past the window end -> corrupt record

        let err = SigningKey::<TestScheme>::from_record(&record).unwrap_err();
        assert!(matches!(err, CryptoError::EpochNotActive { .. }));
    }

    #[test]
    fn from_record_rejects_nonzero_next_index_at_or_below_window_start() {
        // A nonzero watermark at/below the activation start is impossible from
        // to_record and would rewind the guard on reload — refuse it so a stale or
        // corrupt record cannot re-enable one-time-key reuse.
        let (_pk, sk) = generate_from_seed::<TestScheme>(SEED, 64, 64).unwrap();
        let start = sk.activation_interval().start;
        assert!(start > 0, "test needs a non-zero activation start");
        let mut record = sk.to_record();

        record.next_index = start; // == window start: last_signed would be start-1 (below window)
        assert!(matches!(
            SigningKey::<TestScheme>::from_record(&record),
            Err(CryptoError::EpochNotActive { .. })
        ));

        record.next_index = 1; // well below the window
        assert!(matches!(
            SigningKey::<TestScheme>::from_record(&record),
            Err(CryptoError::EpochNotActive { .. })
        ));
    }
}
