//! Crypto-free, persistable OTS **watermark** record — the durable form.
//!
//! Distinct from [`crate::OtsKeyState`] (the secret-bearing on-disk keygen
//! record): the watermark carries NO seed, only a `key_commitment` (an opaque
//! 32-byte binding to the key's identity, computed where the seed lives) plus
//! the activation window and the monotonic `next_index`. The runtime's durable
//! OTS guard persists THIS through the store on every sign, so the store never
//! holds key material — the seed stays in the `0o600` secret file alone.
//!
//! Fixed-width so its SSZ encoding is a trivial concatenation needing no ssz
//! crate; the layout mirrors [`crate::OtsKeyState`] byte-for-byte (the
//! commitment occupies the seed slot), so both share one 56-byte codec shape.

/// SSZ-layout byte length: 32 (`key_commitment`) + 8 (`activation_epoch`) + 8
/// (`num_active_epochs`) + 8 (`next_index`).
pub const OTS_WATERMARK_SSZ_LEN: usize = 32 + 8 + 8 + 8;

/// Persistable, seed-free OTS watermark.
///
/// `next_index` is the monotonic watermark (highest-signed epoch + 1, `0` when
/// the key has never signed). `key_commitment` binds the record to one key
/// without disclosing it, so a resume can refuse merging a watermark from a
/// DIFFERENT key (rotated seed) while never storing the seed itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OtsWatermark {
    /// Opaque 32-byte binding to the signing key's identity (a hash of the
    /// seed, computed by the crypto adapter). NOT secret and NOT invertible to
    /// the seed — safe to persist in the shared chain store.
    pub key_commitment: [u8; 32],
    /// First epoch the key is active for (scheme-aligned).
    pub activation_epoch: u64,
    /// Number of active epochs from `activation_epoch` (scheme-aligned).
    pub num_active_epochs: u64,
    /// Highest-signed epoch + 1 (`0` = never signed). See [`crate::OtsKeyState`]
    /// for the monotonic-never-rewound invariant the runtime guard enforces.
    pub next_index: u64,
}

impl OtsWatermark {
    /// True when `self` and the given key identity (commitment + activation
    /// window) describe the SAME one-time key. The `next_index` watermark is
    /// deliberately excluded — two snapshots of one key differ only there.
    #[must_use]
    pub fn identifies(
        &self,
        key_commitment: &[u8; 32],
        activation_epoch: u64,
        num_active_epochs: u64,
    ) -> bool {
        self.key_commitment == *key_commitment
            && self.activation_epoch == activation_epoch
            && self.num_active_epochs == num_active_epochs
    }

    /// Encodes to the fixed layout:
    /// `key_commitment || activation_epoch || num_active_epochs || next_index`,
    /// each integer little-endian.
    #[must_use]
    pub fn to_ssz_bytes(&self) -> [u8; OTS_WATERMARK_SSZ_LEN] {
        let mut out = [0u8; OTS_WATERMARK_SSZ_LEN];
        out[0..32].copy_from_slice(&self.key_commitment);
        out[32..40].copy_from_slice(&self.activation_epoch.to_le_bytes());
        out[40..48].copy_from_slice(&self.num_active_epochs.to_le_bytes());
        out[48..56].copy_from_slice(&self.next_index.to_le_bytes());
        out
    }

    /// Inverse of [`to_ssz_bytes`](Self::to_ssz_bytes).
    ///
    /// # Errors
    ///
    /// [`OtsWatermarkDecodeError::Length`] when `bytes.len() != OTS_WATERMARK_SSZ_LEN`.
    pub fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, OtsWatermarkDecodeError> {
        if bytes.len() != OTS_WATERMARK_SSZ_LEN {
            return Err(OtsWatermarkDecodeError::Length {
                expected: OTS_WATERMARK_SSZ_LEN,
                actual: bytes.len(),
            });
        }
        let mut key_commitment = [0u8; 32];
        key_commitment.copy_from_slice(&bytes[0..32]);
        // Length verified above, so each 8-byte slice decodes; reuse the crate's
        // canonical LE decoder rather than re-implementing it.
        let field = |offset: usize| -> Result<u64, OtsWatermarkDecodeError> {
            crate::decode_u64_le(&bytes[offset..offset + 8]).map_err(|_| {
                OtsWatermarkDecodeError::Length {
                    expected: OTS_WATERMARK_SSZ_LEN,
                    actual: bytes.len(),
                }
            })
        };
        Ok(Self {
            key_commitment,
            activation_epoch: field(32)?,
            num_active_epochs: field(40)?,
            next_index: field(48)?,
        })
    }
}

/// Decode error for [`OtsWatermark::from_ssz_bytes`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OtsWatermarkDecodeError {
    /// Input length was not [`OTS_WATERMARK_SSZ_LEN`].
    #[error("ots watermark: expected {expected} bytes, got {actual}")]
    Length {
        /// Required length ([`OTS_WATERMARK_SSZ_LEN`]).
        expected: usize,
        /// Length actually supplied.
        actual: usize,
    },
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn sample() -> OtsWatermark {
        OtsWatermark {
            key_commitment: [9u8; 32],
            activation_epoch: 65_536,
            num_active_epochs: 262_144,
            next_index: 3,
        }
    }

    #[test]
    fn ssz_round_trips() {
        let wm = sample();
        let bytes = wm.to_ssz_bytes();
        assert_eq!(bytes.len(), OTS_WATERMARK_SSZ_LEN);
        assert_eq!(OtsWatermark::from_ssz_bytes(&bytes), Ok(wm));
    }

    #[test]
    fn from_ssz_bytes_rejects_wrong_length() {
        let err = OtsWatermark::from_ssz_bytes(&[0u8; 10]).unwrap_err();
        assert_eq!(
            err,
            OtsWatermarkDecodeError::Length {
                expected: 56,
                actual: 10,
            }
        );
    }

    #[test]
    fn identifies_ignores_watermark() {
        let wm = sample();
        assert!(
            wm.identifies(&[9u8; 32], 65_536, 262_144),
            "matching commitment + window is the same key regardless of next_index"
        );
    }

    #[test]
    fn identifies_rejects_mismatch() {
        let wm = sample();
        assert!(!wm.identifies(&[8u8; 32], 65_536, 262_144), "commitment");
        assert!(!wm.identifies(&[9u8; 32], 65_537, 262_144), "activation");
        assert!(!wm.identifies(&[9u8; 32], 65_536, 262_145), "window");
    }
}
