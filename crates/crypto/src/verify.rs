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
    /// The distinction matters: the container's trailing bytes are padding that
    /// verification slices off, so flipping one there is a no-op (pinned by
    /// `test_tampered_padding_is_ignored`). A tamper test that targeted the last
    /// byte of the container would assert a rejection that cannot happen.
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

    /// The padding region is not authenticated — mutating it does not invalidate
    /// the signature.
    ///
    /// This is a property of the spec's wire format, not a defect introduced
    /// here: the container is fixed-width and verification slices to the payload
    /// length, discarding the rest without inspection. Pinned deliberately so
    /// the behaviour is a recorded decision rather than a surprise, and so a
    /// future change that starts authenticating the padding breaks this test
    /// loudly.
    #[test]
    fn test_tampered_padding_is_ignored() {
        let (wire_pk, mut sk) = test_key_pair();
        let sig = sk.sign(EPOCH, &MESSAGE).unwrap();

        let mut bytes = sig.0;
        bytes[TestScheme::PAYLOAD_LEN] ^= 0xff;
        bytes[Signature::LEN - 1] ^= 0xff;
        let padded_tamper = Signature::new(bytes);

        assert!(verify::<TestScheme>(&wire_pk, EPOCH, &MESSAGE, &padded_tamper).is_ok());
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
