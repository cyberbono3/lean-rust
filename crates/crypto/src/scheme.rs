//! Scheme binding and wire conversions shared by `sign` and `verify`.
//!
//! One home for the leanSig setup: duplicating it across the signer and the
//! verifier is the failure this module exists to prevent.

use leansig::serialization::Serializable;
use leansig::signature::SignatureScheme;
use types::{PublicKey, Signature};

use crate::error::CryptoError;
use crate::key_state::SigningKey;

/// The interop-pinned production scheme.
///
/// The revision and these parameters are an interop contract: every client on
/// the network must agree on them, or signatures do not verify across clients.
/// Parameters `LIFETIME = 2^32`, `DIM = 64`, `BASE = 8`, `TARGET_SUM = 375`.
pub type ProdScheme = leansig::signature::generalized_xmss::instantiations_poseidon_top_level::lifetime_2_to_the_32::hashing_optimized::SIGTopLevelTargetSumLifetime32Dim64Base8;

/// A scheme whose signature payload length is known, so a padded wire container
/// can be sliced back to it.
///
/// This trait exists because the wire container is padded and the payload length
/// cannot be recovered from the bytes. [`Signature`] is 3116 bytes of which only
/// the leading `PAYLOAD_LEN` are real; the rest is zero padding. Decoding must
/// slice to exactly `PAYLOAD_LEN` first — handing the full container to leanSig's
/// decoder fails, because its trailing `hashes` field is defined as "the rest of
/// the buffer" and would swallow the padding.
///
/// Why the length cannot simply be derived from the bytes: the SSZ offsets locate
/// the *start* of each variable field, never the end of the last one; and the
/// payload may legitimately end in zero bytes, so trimming trailing zeros is not
/// sound either. It is a per-scheme constant, and this is where it lives.
///
/// This is not a port trait. `leansig::signature::SignatureScheme` is the port;
/// this carries only wire metadata leanSig does not expose.
///
/// It is **sealed**: only this crate can implement it. The trait has to be `pub`
/// because it appears as a bound on [`crate::verify`] and
/// [`SigningKey::sign`](crate::SigningKey::sign), and Rust requires a public
/// item's bounds be at least as visible as the item. Sealing keeps that a
/// technicality rather than an extension point — a downstream implementor could
/// otherwise declare a wrong `PAYLOAD_LEN` and silently corrupt every signature
/// it round-trips.
pub trait SchemeWire: SignatureScheme + sealed::Sealed {
    /// Length of this scheme's [`Serializable::to_bytes`] output — the payload
    /// inside the padded container.
    ///
    /// Asserted against a real signature by `test_payload_len_matches_reality`.
    /// A wrong value here is a silent interop break, so it is never assumed.
    const PAYLOAD_LEN: usize;
}

/// Seals [`SchemeWire`] against outside implementors.
mod sealed {
    /// Implemented only for the schemes this crate blesses.
    pub trait Sealed {}
}

impl sealed::Sealed for ProdScheme {}

impl SchemeWire for ProdScheme {
    // 4 (offset) + 28 (rho) + 4 (offset) + 1028 (path: 4 + 32 siblings x 32)
    // + 2048 (64 chain hashes x 32).
    const PAYLOAD_LEN: usize = 3112;
}

/// Generates a key pair for `S`, active over `num_active_epochs` from
/// `activation_epoch`.
///
/// Returns the public key already encoded into its wire newtype, and the secret
/// key wrapped in a [`SigningKey`] so the one-time discipline applies from the
/// first signature.
///
/// This is the only supported way to obtain a [`SigningKey`]: it is what keeps
/// leanSig's own key types off this crate's public surface.
///
/// # Errors
///
/// [`CryptoError::PublicKeyLength`] when the generated public key does not match
/// the wire width — a stale interop parameter, not a per-key condition.
pub fn generate<S: SchemeWire, R: rand::Rng>(
    rng: &mut R,
    activation_epoch: usize,
    num_active_epochs: usize,
) -> Result<(PublicKey, SigningKey<S>), CryptoError> {
    let (pk, sk) = S::key_gen(rng, activation_epoch, num_active_epochs);
    let wire_pk = public_key_to_wire::<S>(&pk)?;
    Ok((wire_pk, SigningKey::new(sk)))
}

/// Encodes a scheme public key into the wire newtype.
///
/// The public key is fixed-length and exactly fills its container, so unlike the
/// signature there is no padding here.
pub(crate) fn public_key_to_wire<S: SignatureScheme>(
    pk: &S::PublicKey,
) -> Result<PublicKey, CryptoError> {
    let bytes = pk.to_bytes();
    let arr: [u8; PublicKey::LEN] =
        bytes
            .as_slice()
            .try_into()
            .map_err(|_| CryptoError::PublicKeyLength {
                got: bytes.len(),
                expected: PublicKey::LEN,
            })?;
    Ok(PublicKey::new(arr))
}

/// Decodes a scheme public key from the wire newtype.
pub(crate) fn public_key_from_wire<S: SignatureScheme>(
    pk: &PublicKey,
) -> Result<S::PublicKey, CryptoError> {
    S::PublicKey::from_bytes(pk.as_slice()).map_err(|err| CryptoError::PublicKeyDecode {
        detail: format!("{err:?}"),
    })
}

/// Encodes a scheme signature into the padded wire container.
///
/// The container is deliberately larger than the payload; the consensus spec
/// defines the remainder as right-hand zero padding and slices it off before
/// verifying. Padding is therefore the specified behaviour, not a workaround for
/// a size mismatch.
///
/// Note the padding is not authenticated — see [`signature_from_wire`].
pub(crate) fn signature_to_wire<S: SchemeWire>(
    sig: &S::Signature,
) -> Result<Signature, CryptoError> {
    let bytes = sig.to_bytes();

    // The payload length is a per-scheme constant, so disagreeing with it means
    // PAYLOAD_LEN is stale against the pinned revision — not that this signature
    // is unusual. Fail loudly rather than pad a wrong-length payload into a
    // right-sized container, which no length check downstream could catch.
    if bytes.len() != S::PAYLOAD_LEN {
        return Err(CryptoError::PayloadLength {
            got: bytes.len(),
            expected: S::PAYLOAD_LEN,
        });
    }

    // Unreachable while PAYLOAD_LEN <= Signature::LEN, which holds for both
    // current schemes. Kept as the guard for a future scheme whose payload does
    // not fit the container: without it, that case would silently panic in the
    // copy below rather than report.
    if bytes.len() > Signature::LEN {
        return Err(CryptoError::PayloadTooLong {
            got: bytes.len(),
            capacity: Signature::LEN,
        });
    }

    let mut padded = [0_u8; Signature::LEN];
    padded[..bytes.len()].copy_from_slice(&bytes);
    Ok(Signature::new(padded))
}

/// Decodes a scheme signature from the padded wire container.
///
/// Slices to `S::PAYLOAD_LEN` before decoding — see [`SchemeWire`] for why the
/// length cannot be recovered from the bytes.
///
/// # Padding is not authenticated
///
/// Bytes beyond `PAYLOAD_LEN` are discarded without inspection, so mutating them
/// in transit does not invalidate the signature. That follows from the spec's
/// fixed-container-plus-right-slice design and is not something this adapter can
/// change unilaterally without breaking interop. Callers must not treat the
/// padding region as carrying meaning.
pub(crate) fn signature_from_wire<S: SchemeWire>(
    sig: &Signature,
) -> Result<S::Signature, CryptoError> {
    let payload = sig
        .as_slice()
        .get(..S::PAYLOAD_LEN)
        .ok_or(CryptoError::PayloadTooLong {
            got: S::PAYLOAD_LEN,
            capacity: Signature::LEN,
        })?;

    S::Signature::from_bytes(payload).map_err(|err| CryptoError::SignatureDecode {
        detail: format!("{err:?}"),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
pub(crate) mod test_support {
    use super::SchemeWire;
    use crate::key_state::SigningKey;
    use leansig::signature::SignatureScheme;

    /// Cheap scheme used by every test in this crate.
    ///
    /// Production keygen is ~2s against this scheme's ~12ms. The adapter logic
    /// under test is identical; the schemes differ only in `LOG_LIFETIME` (8 vs
    /// 32), which is the entire 768-byte gap between their payload lengths.
    pub type TestScheme = leansig::signature::generalized_xmss::instantiations_poseidon_top_level::lifetime_2_to_the_8::SIGTopLevelTargetSumLifetime8Dim64Base8;

    impl super::sealed::Sealed for TestScheme {}

    impl SchemeWire for TestScheme {
        // Same body as production with an 8-deep co-path instead of 32:
        // 4 + 28 + 4 + (4 + 8 x 32) + 2048.
        const PAYLOAD_LEN: usize = 2344;
    }

    /// Generates a test key pair active over the whole test-scheme lifetime.
    ///
    /// The one keygen call site for tests — no per-test copy-paste. Returns the
    /// wire public key, which is what every caller here actually wants.
    pub fn test_key_pair() -> (types::PublicKey, SigningKey<TestScheme>) {
        let mut rng = rand::rng();
        // The test scheme's lifetime is 2^8, so this conversion cannot fail on
        // any target; `try_into` states that rather than asserting it with a cast.
        let lifetime = usize::try_from(<TestScheme as SignatureScheme>::LIFETIME)
            .expect("test scheme lifetime fits usize");
        super::generate::<TestScheme, _>(&mut rng, 0, lifetime).unwrap()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::test_support::*;
    use super::*;

    /// The production public key exactly fills its container — no padding.
    ///
    /// Measured through `Serializable::to_bytes`, not through
    /// `public_key_to_wire`: that conversion enforces the width itself, so
    /// routing the measurement through it would assert its own precondition and
    /// could never observe a mismatch.
    #[test]
    fn test_prod_scheme_public_key_fills_container_exactly() {
        let mut rng = rand::rng();
        let (pk, _sk) = ProdScheme::key_gen(&mut rng, 0, 2);
        assert_eq!(pk.to_bytes().len(), PublicKey::LEN);
    }

    /// `PAYLOAD_LEN` must equal what the pinned revision actually produces.
    ///
    /// The load-bearing test in this crate. `PAYLOAD_LEN` is a hardcoded constant
    /// that decoding slices on; if it drifts from reality — an upstream revision
    /// bump, a parameter change — every signature silently decodes from the wrong
    /// byte range. No length check downstream catches that, because the container
    /// stays 3116 either way. Measured on the raw `sign` output, before any
    /// conversion, so it cannot be tautological.
    ///
    /// Two active epochs is the smallest window that can sign epoch 0.
    #[test]
    fn test_payload_len_matches_reality() {
        let mut rng = rand::rng();
        let (_pk, sk) = ProdScheme::key_gen(&mut rng, 0, 2);
        let sig = ProdScheme::sign(&sk, 0, &[7_u8; 32]).unwrap();
        assert_eq!(
            sig.to_bytes().len(),
            ProdScheme::PAYLOAD_LEN,
            "ProdScheme::PAYLOAD_LEN is stale against the pinned leanSig revision",
        );
    }

    /// The same assertion for the test scheme, which every other test relies on.
    #[test]
    fn test_scheme_payload_len_matches_reality() {
        let (_pk, mut sk) = test_key_pair();
        let sig = sk.sign_raw(0, &[7_u8; 32]).unwrap();
        assert_eq!(sig.to_bytes().len(), TestScheme::PAYLOAD_LEN);
    }

    /// The payload fits the container, and the remainder is zero padding — the
    /// shape the spec's right-slice depends on.
    #[test]
    fn test_signature_pads_into_container() {
        let (_pk, mut sk) = test_key_pair();
        let sig = sk.sign(0, &[7_u8; 32]).unwrap();

        assert_eq!(sig.as_slice().len(), Signature::LEN);
        assert!(
            sig.as_slice()[TestScheme::PAYLOAD_LEN..]
                .iter()
                .all(|b| *b == 0),
            "padding must be zeros",
        );
    }

    /// Round-tripping through the padded container is lossless — this is what
    /// proves the pad and slice halves agree.
    #[test]
    fn test_wire_round_trip_is_lossless() {
        let (_pk, mut sk) = test_key_pair();
        let raw = sk.sign_raw(0, &[7_u8; 32]).unwrap();
        let wire = signature_to_wire::<TestScheme>(&raw).unwrap();
        let back = signature_from_wire::<TestScheme>(&wire).unwrap();
        assert_eq!(back.to_bytes(), raw.to_bytes());
    }
}
