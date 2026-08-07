//! Crypto-free, persistable OTS key-state record (SA5).
//!
//! Holds a reproducible RNG seed plus the parameters needed to regenerate a
//! validator signing key, and a monotonic watermark. Crypto-free so `storage`
//! can persist it without a `crypto` dependency; fixed-width so its SSZ encoding
//! is a trivial concatenation needing no ssz crate.
//!
//! The leanSig secret key is a variable-size, materialized Merkle tree with no
//! fixed width, so it is deliberately NOT stored here. The 32-byte `seed`
//! regenerates the identical key deterministically (leanSig `key_gen` is a
//! reproducible function of a seeded RNG).

use core::fmt;

/// SSZ-layout byte length: 32 (seed) + 8 (`activation_epoch`) + 8
/// (`num_active_epochs`) + 8 (`next_index`).
pub const OTS_KEY_STATE_SSZ_LEN: usize = crate::ots_record_codec::OTS_RECORD_SSZ_LEN;

/// Persistable OTS key state.
///
/// `next_index` is the monotonic watermark (highest-signed epoch + 1, `0` when
/// the key has never signed); reloading through the crypto adapter restores it
/// so a one-time key cannot be reused across a restart.
#[derive(Clone, PartialEq, Eq)]
pub struct OtsKeyState {
    /// Reproducible RNG seed the signing key was generated from.
    pub seed: [u8; 32],
    /// First epoch the key is active for (scheme-aligned).
    pub activation_epoch: u64,
    /// Number of active epochs from `activation_epoch` (scheme-aligned).
    pub num_active_epochs: u64,
    /// Highest-signed epoch + 1 (`0` = never signed).
    ///
    /// The one-time-key watermark: reconstructing through the crypto adapter
    /// refuses to sign any epoch at or below `next_index - 1`. This is a plain
    /// deserialization target and cannot enforce monotonicity on its own. The
    /// "monotonic, never rewound" invariant across restarts is owned by the
    /// runtime sign-boundary guard (the composition-root OTS signer): it persists
    /// only post-sign (advanced) records and, on reload, must reject a watermark
    /// below the one already in force. The durable key-value store faithfully
    /// records whatever the guard hands it and does NOT itself compare watermarks
    /// (a blind overwrite, like every other `save_*`), so it is the guard's job to
    /// ensure a lower `next_index` never reaches `save_ots_key_state`.
    pub next_index: u64,
}

/// Hand-written so the seed is never printed.
impl fmt::Debug for OtsKeyState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OtsKeyState")
            .field("seed", &"<redacted>")
            .field("activation_epoch", &self.activation_epoch)
            .field("num_active_epochs", &self.num_active_epochs)
            .field("next_index", &self.next_index)
            .finish()
    }
}

/// Decode error for [`OtsKeyState::from_ssz_bytes`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OtsKeyStateDecodeError {
    /// Input length was not [`OTS_KEY_STATE_SSZ_LEN`].
    #[error("ots key-state: expected {expected} bytes, got {actual}")]
    Length {
        /// Required length ([`OTS_KEY_STATE_SSZ_LEN`]).
        expected: usize,
        /// Length actually supplied.
        actual: usize,
    },
}

impl OtsKeyState {
    /// Encodes to the fixed SSZ container layout:
    /// `seed || activation_epoch || num_active_epochs || next_index`, each
    /// integer little-endian. The byte work lives in
    /// [`crate::ots_record_codec`], shared with [`crate::OtsWatermark`].
    #[must_use]
    pub fn to_ssz_bytes(&self) -> [u8; OTS_KEY_STATE_SSZ_LEN] {
        crate::ots_record_codec::encode(
            &self.seed,
            self.activation_epoch,
            self.num_active_epochs,
            self.next_index,
        )
    }

    /// Inverse of [`to_ssz_bytes`](Self::to_ssz_bytes).
    ///
    /// # Errors
    ///
    /// [`OtsKeyStateDecodeError::Length`] when `bytes.len() != OTS_KEY_STATE_SSZ_LEN`.
    pub fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, OtsKeyStateDecodeError> {
        let (seed, activation_epoch, num_active_epochs, next_index) =
            crate::ots_record_codec::decode(bytes).ok_or(OtsKeyStateDecodeError::Length {
                expected: OTS_KEY_STATE_SSZ_LEN,
                actual: bytes.len(),
            })?;
        Ok(Self {
            seed,
            activation_epoch,
            num_active_epochs,
            next_index,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn sample() -> OtsKeyState {
        OtsKeyState {
            seed: [7u8; 32],
            activation_epoch: 65_536,
            num_active_epochs: 262_144,
            next_index: 3,
        }
    }

    #[test]
    fn ssz_round_trips() {
        let ks = sample();
        let bytes = ks.to_ssz_bytes();
        assert_eq!(bytes.len(), OTS_KEY_STATE_SSZ_LEN);
        assert_eq!(OtsKeyState::from_ssz_bytes(&bytes), Ok(ks));
    }

    #[test]
    fn from_ssz_bytes_rejects_wrong_length() {
        let err = OtsKeyState::from_ssz_bytes(&[0u8; 10]).unwrap_err();
        assert_eq!(
            err,
            OtsKeyStateDecodeError::Length {
                expected: 56,
                actual: 10,
            }
        );
    }

    #[test]
    fn debug_does_not_leak_seed() {
        let rendered = format!("{:?}", sample());
        assert!(rendered.contains("<redacted>"));
        assert!(
            !rendered.contains("7, 7, 7"),
            "seed bytes leaked into Debug"
        );
    }
}
