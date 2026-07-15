//! XMSS key state: the secret key plus the guards that keep it one-time.
//!
//! leanSig's `sign` takes `&SecretKey` and will happily sign an epoch twice —
//! upstream delegates that discipline to the caller. This module is where the
//! project takes it on, so no consumer can reuse an epoch by accident.

use core::ops::Range;

use leansig::signature::{SignatureScheme, SignatureSchemeSecretKey};

use crate::error::CryptoError;

/// A secret key plus the watermark that keeps one-time keys one-time.
///
/// [`sign`](Self::sign) takes `&mut self` — not because leanSig requires it (it
/// does not), but because advancing the watermark is what prevents accidental
/// epoch reuse through a single wrapper. The borrow checker then enforces at
/// compile time that no two signers share one wrapper.
///
/// # Limitation
///
/// This is a guard against accident, not a proof. The watermark is bound to the
/// wrapper, not to the key material, and it is in-memory and per-process. Two
/// ways it does not hold:
///
/// - A restarted node that reloads key material starts with no watermark and can
///   re-sign an epoch it already signed.
/// - In-process, the secret key is serializable, so a caller can round-trip it
///   through bytes, wrap it in a second `SigningKey`, and start again from
///   `last_signed: None`.
///
/// Both have the same root cause: nothing binds the watermark to the key. Fixing
/// that needs persisted key state carrying its own watermark, which is out of
/// scope here.
pub struct SigningKey<S: SignatureScheme> {
    secret: S::SecretKey,
    last_signed: Option<u32>,
}

impl<S: SignatureScheme> SigningKey<S> {
    /// Wraps a freshly generated leanSig secret key.
    #[must_use]
    pub const fn new(secret: S::SecretKey) -> Self {
        Self {
            secret,
            last_signed: None,
        }
    }

    /// Returns the epoch interval this key was generated to cover.
    #[must_use]
    pub fn activation_interval(&self) -> Range<u64> {
        self.secret.get_activation_interval()
    }

    /// Returns the sub-interval this key is currently prepared to sign.
    #[must_use]
    pub fn prepared_interval(&self) -> Range<u64> {
        self.secret.get_prepared_interval()
    }

    /// Returns the highest epoch this key has signed, if any.
    #[must_use]
    pub const fn last_signed(&self) -> Option<u32> {
        self.last_signed
    }

    /// Advances the prepared window by one step.
    ///
    /// leanSig holds only a sliding window of the Merkle tree in memory, since a
    /// full 2^32 tree would need hundreds of gigabytes. This moves the window
    /// right; it is a no-op once the window reaches the end of the activation
    /// interval.
    pub fn advance(&mut self) {
        self.secret.advance_preparation();
    }

    /// Advances the prepared window until it covers `epoch`.
    ///
    /// # Errors
    ///
    /// - [`CryptoError::EpochNotActive`] when `epoch` lies outside the activation
    ///   interval — advancing could never reach it, so looping would spin
    ///   forever.
    /// - [`CryptoError::EpochNotPrepared`] when the window stops moving before
    ///   reaching `epoch`.
    pub fn prepare(&mut self, epoch: u32) -> Result<(), CryptoError> {
        let activation = self.activation_interval();
        let target = u64::from(epoch);
        if !activation.contains(&target) {
            return Err(CryptoError::EpochNotActive {
                epoch,
                start: activation.start,
                end: activation.end,
            });
        }

        while !self.prepared_interval().contains(&target) {
            let before = self.prepared_interval();
            self.advance();
            if self.prepared_interval() == before {
                // The window refused to move: it is already at the end of the
                // activation interval and `epoch` is unreachable. Without this
                // the loop would never terminate.
                return Err(CryptoError::EpochNotPrepared {
                    epoch,
                    start: before.start,
                    end: before.end,
                });
            }
        }
        Ok(())
    }

    /// Signs `message` for `epoch`, refusing any epoch already signed.
    ///
    /// `pub(crate)`, not `pub`: it returns leanSig's own signature type, and a
    /// crate whose stated purpose is that callers never see upstream's shapes
    /// must not hand one out. [`SigningKey::sign`](Self::sign) is the public
    /// surface.
    ///
    /// # Errors
    ///
    /// - [`CryptoError::EpochReused`] when `epoch <= last_signed` — the guard
    ///   that keeps the one-time key one-time.
    /// - [`CryptoError::EpochNotPrepared`] when the prepared window does not
    ///   cover `epoch`.
    /// - [`CryptoError::Signing`] when leanSig's encoder fails.
    pub(crate) fn sign_raw(
        &mut self,
        epoch: u32,
        message: &[u8; leansig::MESSAGE_LENGTH],
    ) -> Result<S::Signature, CryptoError> {
        if let Some(last) = self.last_signed {
            if epoch <= last {
                return Err(CryptoError::EpochReused {
                    epoch,
                    last_signed: last,
                });
            }
        }

        // Checked before calling `S::sign`, and load-bearing rather than
        // stylistic: leanSig asserts internally that the epoch is within the
        // prepared window, so this ordering is what keeps that assert
        // unreachable. Do not move it below the sign call.
        let prepared = self.prepared_interval();
        if !prepared.contains(&u64::from(epoch)) {
            return Err(CryptoError::EpochNotPrepared {
                epoch,
                start: prepared.start,
                end: prepared.end,
            });
        }

        let signature = S::sign(&self.secret, epoch, message).map_err(CryptoError::Signing)?;
        self.last_signed = Some(epoch);
        Ok(signature)
    }
}

/// Hand-written on purpose: the derived `Debug` would print the secret key.
impl<S: SignatureScheme> core::fmt::Debug for SigningKey<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SigningKey")
            .field("secret", &"<redacted>")
            .field("last_signed", &self.last_signed)
            .finish()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::scheme::test_support::test_key_pair;

    // leanSig's signature type does not implement `Debug`, so `unwrap_err()` —
    // which needs `T: Debug` to render the Ok side — will not compile here.
    // `let ... else` asserts the same thing without the bound.

    #[test]
    fn test_sign_same_epoch_twice_rejected() {
        let (_pk, mut sk) = test_key_pair();
        sk.sign_raw(1, &[1_u8; 32]).unwrap();

        let Err(err) = sk.sign_raw(1, &[2_u8; 32]) else {
            panic!("signing epoch 1 twice must be refused");
        };
        assert!(matches!(
            err,
            CryptoError::EpochReused {
                epoch: 1,
                last_signed: 1
            }
        ));
    }

    #[test]
    fn test_sign_earlier_epoch_rejected() {
        let (_pk, mut sk) = test_key_pair();
        sk.sign_raw(5, &[1_u8; 32]).unwrap();

        let Err(err) = sk.sign_raw(4, &[1_u8; 32]) else {
            panic!("signing an earlier epoch must be refused");
        };
        assert!(matches!(
            err,
            CryptoError::EpochReused {
                epoch: 4,
                last_signed: 5
            }
        ));
    }

    #[test]
    fn test_advance_moves_prepared_window() {
        let (_pk, mut sk) = test_key_pair();
        let before = sk.prepared_interval();
        sk.advance();
        assert!(sk.prepared_interval().start >= before.start);
    }

    #[test]
    fn test_prepare_rejects_epoch_outside_activation() {
        let (_pk, mut sk) = test_key_pair();
        let end = sk.activation_interval().end;
        let unreachable = u32::try_from(end).unwrap();
        let err = sk.prepare(unreachable).unwrap_err();
        assert!(matches!(err, CryptoError::EpochNotActive { .. }));
    }

    #[test]
    fn test_debug_does_not_leak_secret() {
        let (_pk, sk) = test_key_pair();
        let rendered = format!("{sk:?}");
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn test_last_signed_watermark_advances() {
        let (_pk, mut sk) = test_key_pair();
        assert_eq!(sk.last_signed(), None);
        sk.sign_raw(3, &[1_u8; 32]).unwrap();
        assert_eq!(sk.last_signed(), Some(3));
    }
}
