//! The verification surface, in wire types.
//!
//! Verification is stateless and takes no key state — which is why it is a free
//! function while signing hangs off the key.

use types::{PublicKey, Signature};

use crate::error::CryptoError;
use crate::scheme::{public_key_from_wire, signature_from_wire, SchemeWire};

/// Verifies `signature` over `message` for `epoch` under `public_key`.
///
/// Returns `Ok(())` only when the signature is valid. leanSig's `verify` returns
/// a bare `bool`; a `Result` is returned here so a caller cannot ignore the
/// outcome by accident — a `bool` is trivially droppable, a `Result` is
/// `#[must_use]`.
///
/// # Errors
///
/// - [`CryptoError::PublicKeyDecode`] / [`CryptoError::SignatureDecode`] when the
///   wire bytes are malformed.
/// - [`CryptoError::InvalidSignature`] when the signature does not verify, or
///   when `epoch` exceeds the scheme's lifetime.
pub fn verify<S: SchemeWire>(
    public_key: &PublicKey,
    epoch: u32,
    message: &[u8; leansig::MESSAGE_LENGTH],
    signature: &Signature,
) -> Result<(), CryptoError> {
    // leanSig asserts internally that the epoch is below the scheme lifetime.
    // For the production scheme that is unreachable (lifetime is 2^32 and epoch
    // is a u32), but this function is generic and public, so a consumer binding
    // a shorter-lifetime scheme would turn an attacker-supplied epoch into a
    // panic. Checking here means the guarantee does not depend on the caller's
    // choice of scheme.
    if u64::from(epoch) >= S::LIFETIME {
        return Err(CryptoError::InvalidSignature);
    }

    let pk = public_key_from_wire::<S>(public_key)?;
    let sig = signature_from_wire::<S>(signature)?;

    if S::verify(&pk, epoch, message, &sig) {
        Ok(())
    } else {
        Err(CryptoError::InvalidSignature)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::scheme::test_support::{test_key_pair, TestScheme};
    use crate::scheme::SchemeWire;
    use leansig::signature::SignatureScheme;

    const EPOCH: u32 = 3;
    const MESSAGE: [u8; 32] = [0xab; 32];

    #[test]
    fn test_sign_verify_round_trip() {
        let (wire_pk, mut sk) = test_key_pair();
        let sig = sk.sign(EPOCH, &MESSAGE).unwrap();
        assert!(verify::<TestScheme>(&wire_pk, EPOCH, &MESSAGE, &sig).is_ok());
    }

    #[test]
    fn test_tampered_message_rejected() {
        let (wire_pk, mut sk) = test_key_pair();
        let sig = sk.sign(EPOCH, &MESSAGE).unwrap();

        let mut tampered = MESSAGE;
        tampered[0] ^= 0x01;
        let err = verify::<TestScheme>(&wire_pk, EPOCH, &tampered, &sig).unwrap_err();
        assert!(matches!(err, CryptoError::InvalidSignature));
    }

    /// Flips a byte inside the payload, not in the padding.
    ///
    /// The distinction matters: tampering with the trailing padding is rejected
    /// with [`CryptoError::NonZeroPadding`], so a "flip the last byte" test would
    /// exercise padding validation rather than payload integrity.
    /// This test flips a payload byte to ensure payload tampering is rejected.
    #[test]
    fn test_tampered_signature_rejected() {
        let (wire_pk, mut sk) = test_key_pair();
        let sig = sk.sign(EPOCH, &MESSAGE).unwrap();

        let mut bytes = sig.0;
        bytes[TestScheme::PAYLOAD_LEN - 1] ^= 0x01;
        let tampered = Signature::new(bytes);

        // A flipped payload byte may fail to decode or decode and fail to
        // verify; both are rejections. Asserting a specific variant would
        // over-fit to the encoding.
        assert!(verify::<TestScheme>(&wire_pk, EPOCH, &MESSAGE, &tampered).is_err());
    }

    /// Tampering with the padding is rejected, not ignored.
    ///
    /// An earlier revision accepted this and pinned it as spec-mandated. That was
    /// wrong. The spec requires verification to *slice* the payload out, which is
    /// a ceiling on what must be accepted, not a floor: since no correct
    /// implementation emits non-zero padding, rejecting it is strictly narrower
    /// than the spec permits and costs nothing against honest peers.
    ///
    /// Accepting it made one logical signature into many valid containers, each
    /// with a distinct gossip message-id — so the duplicate cache missed and every
    /// variant was fully verified and re-forwarded. That is amplification on the
    /// most expensive operation in the protocol.
    #[test]
    fn test_tampered_padding_rejected() {
        let (wire_pk, mut sk) = test_key_pair();
        let sig = sk.sign(EPOCH, &MESSAGE).unwrap();

        let mut bytes = sig.0;
        bytes[TestScheme::PAYLOAD_LEN] ^= 0xff;
        let padded_tamper = Signature::new(bytes);

        let err = verify::<TestScheme>(&wire_pk, EPOCH, &MESSAGE, &padded_tamper).unwrap_err();
        assert!(matches!(err, CryptoError::NonZeroPadding));
    }

    /// A re-split payload is rejected rather than crashing the node.
    ///
    /// End-to-end regression for the remote-panic path: this exact input reached
    /// `index out of bounds: the len is 63 but the index is 63` inside leanSig
    /// before the layout check existed.
    #[test]
    fn test_rewritten_layout_rejected_not_panic() {
        let (wire_pk, mut sk) = test_key_pair();
        let sig = sk.sign(EPOCH, &MESSAGE).unwrap();

        let mut bytes = sig.0;
        let attacked: u32 = TestScheme::OFFSET_HASHES + 32;
        bytes[32..36].copy_from_slice(&attacked.to_le_bytes());

        let err =
            verify::<TestScheme>(&wire_pk, EPOCH, &MESSAGE, &Signature::new(bytes)).unwrap_err();
        assert!(matches!(err, CryptoError::MalformedLayout { .. }));
    }

    #[test]
    fn test_wrong_epoch_rejected() {
        let (wire_pk, mut sk) = test_key_pair();
        let sig = sk.sign(EPOCH, &MESSAGE).unwrap();
        let err = verify::<TestScheme>(&wire_pk, EPOCH + 1, &MESSAGE, &sig).unwrap_err();
        assert!(matches!(err, CryptoError::InvalidSignature));
    }

    #[test]
    fn test_wrong_public_key_rejected() {
        let (_pk, mut sk) = test_key_pair();
        let (wire_other, _other_sk) = test_key_pair();
        let sig = sk.sign(EPOCH, &MESSAGE).unwrap();
        let err = verify::<TestScheme>(&wire_other, EPOCH, &MESSAGE, &sig).unwrap_err();
        assert!(matches!(err, CryptoError::InvalidSignature));
    }

    /// An epoch at or beyond the scheme lifetime is rejected, not panicked on.
    #[test]
    fn test_epoch_beyond_lifetime_rejected() {
        let (wire_pk, mut sk) = test_key_pair();
        let sig = sk.sign(EPOCH, &MESSAGE).unwrap();

        let beyond = u32::try_from(TestScheme::LIFETIME).unwrap();
        let err = verify::<TestScheme>(&wire_pk, beyond, &MESSAGE, &sig).unwrap_err();
        assert!(matches!(err, CryptoError::InvalidSignature));
    }
}
