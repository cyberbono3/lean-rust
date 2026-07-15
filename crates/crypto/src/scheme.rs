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

    /// Canonical value of the signature payload's first SSZ offset field.
    ///
    /// `4 (this offset) + 28 (rho) + 4 (the hashes offset)`. Both current schemes
    /// use `RAND_LEN_FE = 7`, so both have the same value; it is an associated
    /// constant rather than a shared one so a scheme with different randomness
    /// cannot inherit a wrong default.
    const OFFSET_PATH: u32 = 36;

    /// Canonical value of the signature payload's second SSZ offset field.
    ///
    /// `OFFSET_PATH + 4 (the co-path's own inner offset) + LOG_LIFETIME * 32`.
    /// Pinning it is what makes the co-path and hash-list lengths canonical —
    /// see [`signature_from_wire`].
    const OFFSET_HASHES: u32;
}

/// Reads a little-endian `u32` at `offset`, or `None` if out of range.
fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    let raw: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(raw))
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
    // 36 + 4 + 32 siblings x 32.
    const OFFSET_HASHES: u32 = 1064;
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
/// - [`CryptoError::ActivationOutOfRange`] when the requested window does not fit
///   the scheme's lifetime.
/// - [`CryptoError::PublicKeyLength`] when the generated public key does not match
///   the wire width — a stale interop parameter, not a per-key condition.
pub fn generate<S: SchemeWire, R: rand::Rng>(
    rng: &mut R,
    activation_epoch: usize,
    num_active_epochs: usize,
) -> Result<(PublicKey, SigningKey<S>), CryptoError> {
    // leanSig asserts `activation_epoch + num_active_epochs <= LIFETIME` and
    // computes that sum in `usize`. Passing the arguments through unchecked would
    // turn a caller's bad input into either an overflow panic or — worse — a
    // release-mode wraparound that satisfies the assert and generates a key over
    // a garbage window. A `pub fn` returning `Result` must not abort the process
    // on bad arguments, so the bound is checked here with checked arithmetic.
    let end = activation_epoch.checked_add(num_active_epochs).ok_or(
        CryptoError::ActivationOutOfRange {
            activation_epoch,
            num_active_epochs,
            lifetime: S::LIFETIME,
        },
    )?;

    if u64::try_from(end).map_or(true, |end| end > S::LIFETIME) {
        return Err(CryptoError::ActivationOutOfRange {
            activation_epoch,
            num_active_epochs,
            lifetime: S::LIFETIME,
        });
    }

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
/// Every byte of the container is constrained before anything is decoded. That
/// is not defensive tidiness — both checks below close real attacks.
///
/// # Why the layout is validated
///
/// leanSig's decoder validates only that the SSZ offsets are monotonic and
/// in-bounds. It does **not** check that the co-path has `LOG_LIFETIME` siblings
/// or that the hash list has `DIMENSION` entries — but its `verify` assumes both,
/// indexing `hashes[chain_index]` for `chain_index in 0..DIMENSION` and asserting
/// on the co-path depth. Those are `assert!` and an unchecked index, so they fire
/// in release builds.
///
/// An attacker who observes one valid signature can rewrite the second offset
/// field to re-split the same payload — a 33-sibling co-path leaves 63 hashes —
/// and every node that verifies it panics on the out-of-bounds index. The
/// container stays exactly `Signature::LEN`, so no length check anywhere catches
/// it. Pinning both offsets to their canonical values makes the co-path and hash
/// lengths canonical too, which is the only lever this crate has: leanSig's
/// fields are private, so the lengths cannot be checked after decoding.
///
/// # Why the padding must be zero
///
/// The spec defines the region beyond the payload as right-hand zero padding and
/// slices it off before verifying, so accepting arbitrary bytes there would make
/// one logical signature into `2^(8*padding)` distinct containers that all
/// verify. Distinct bytes mean distinct gossip message-ids, so the duplicate
/// cache misses and each variant is fully verified and re-forwarded — mesh
/// amplification on the most expensive operation in the protocol.
///
/// Rejecting non-zero padding is strictly narrower than the spec permits and
/// costs nothing against honest peers: no correct implementation emits non-zero
/// padding, because the spec calls the region zero padding and
/// [`signature_to_wire`] zero-fills it. The residual risk is a peer that pads
/// with garbage — which would itself be a spec violation.
pub(crate) fn signature_from_wire<S: SchemeWire>(
    sig: &Signature,
) -> Result<S::Signature, CryptoError> {
    let bytes = sig.as_slice();

    let payload = bytes
        .get(..S::PAYLOAD_LEN)
        .ok_or(CryptoError::SchemeMisconfigured {
            payload_len: S::PAYLOAD_LEN,
            capacity: Signature::LEN,
        })?;

    // Reject non-zero padding — see "Why the padding must be zero" above.
    if !bytes[S::PAYLOAD_LEN..].iter().all(|b| *b == 0) {
        return Err(CryptoError::NonZeroPadding);
    }

    // Pin both SSZ offsets to their canonical values before decoding — see "Why
    // the layout is validated" above.
    let offset_path = read_u32_le(payload, 0).ok_or(CryptoError::MalformedLayout {
        field: "offset_path",
        got: 0,
        expected: S::OFFSET_PATH,
    })?;
    if offset_path != S::OFFSET_PATH {
        return Err(CryptoError::MalformedLayout {
            field: "offset_path",
            got: offset_path,
            expected: S::OFFSET_PATH,
        });
    }

    let offset_hashes = read_u32_le(payload, 32).ok_or(CryptoError::MalformedLayout {
        field: "offset_hashes",
        got: 0,
        expected: S::OFFSET_HASHES,
    })?;
    if offset_hashes != S::OFFSET_HASHES {
        return Err(CryptoError::MalformedLayout {
            field: "offset_hashes",
            got: offset_hashes,
            expected: S::OFFSET_HASHES,
        });
    }

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
        // 36 + 4 + 8 siblings x 32.
        const OFFSET_HASHES: u32 = 296;
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

    /// The canonical offsets must match what the pinned revision emits.
    ///
    /// Same class of constant as `PAYLOAD_LEN`: decoding rejects anything that
    /// disagrees, so a stale value here would reject every honest signature.
    /// Asserted against a real signature rather than trusted.
    #[test]
    fn test_canonical_offsets_match_reality() {
        let (_pk, mut sk) = test_key_pair();
        let raw = sk.sign_raw(0, &[7_u8; 32]).unwrap();
        let bytes = raw.to_bytes();

        assert_eq!(
            read_u32_le(&bytes, 0),
            Some(TestScheme::OFFSET_PATH),
            "OFFSET_PATH is stale against the pinned revision",
        );
        assert_eq!(
            read_u32_le(&bytes, 32),
            Some(TestScheme::OFFSET_HASHES),
            "OFFSET_HASHES is stale against the pinned revision",
        );
    }

    /// Regression: a re-split payload must be rejected, not panicked on.
    ///
    /// leanSig's decoder accepts any monotonic in-bounds offsets, but its
    /// verifier indexes `hashes[0..DIMENSION]` without checking the length. An
    /// attacker who rewrites the second offset field re-splits the same payload
    /// into a longer co-path and a shorter hash list; before the layout check
    /// this reached `index out of bounds: the len is 63 but the index is 63`
    /// inside leanSig — a remote crash from one observed signature.
    #[test]
    fn test_rewritten_offset_rejected_not_panic() {
        let (_pk, mut sk) = test_key_pair();
        let sig = sk.sign(0, &[7_u8; 32]).unwrap();

        let mut bytes = sig.0;
        // 9-sibling co-path instead of 8 => 63 hashes instead of 64.
        let attacked: u32 = TestScheme::OFFSET_HASHES + 32;
        bytes[32..36].copy_from_slice(&attacked.to_le_bytes());

        let Err(err) = signature_from_wire::<TestScheme>(&Signature::new(bytes)) else {
            panic!("a re-split payload must be refused");
        };
        assert!(matches!(
            err,
            CryptoError::MalformedLayout {
                field: "offset_hashes",
                ..
            }
        ));
    }

    /// Regression: the first offset field is pinned too.
    #[test]
    fn test_rewritten_offset_path_rejected() {
        let (_pk, mut sk) = test_key_pair();
        let sig = sk.sign(0, &[7_u8; 32]).unwrap();

        let mut bytes = sig.0;
        bytes[0..4].copy_from_slice(&40_u32.to_le_bytes());

        let Err(err) = signature_from_wire::<TestScheme>(&Signature::new(bytes)) else {
            panic!("a rewritten path offset must be refused");
        };
        assert!(matches!(
            err,
            CryptoError::MalformedLayout {
                field: "offset_path",
                ..
            }
        ));
    }

    /// Regression: non-zero padding is rejected, closing the malleability that
    /// would otherwise give one signature many valid encodings.
    #[test]
    fn test_non_zero_padding_rejected() {
        let (_pk, mut sk) = test_key_pair();
        let sig = sk.sign(0, &[7_u8; 32]).unwrap();

        let mut bytes = sig.0;
        bytes[Signature::LEN - 1] = 0x01;

        let Err(err) = signature_from_wire::<TestScheme>(&Signature::new(bytes)) else {
            panic!("non-zero padding must be refused");
        };
        assert!(matches!(err, CryptoError::NonZeroPadding));
    }

    /// An activation window past the scheme lifetime is an error, not a panic.
    #[test]
    fn test_generate_rejects_window_past_lifetime() {
        let mut rng = rand::rng();
        let lifetime = usize::try_from(<TestScheme as SignatureScheme>::LIFETIME)
            .expect("test scheme lifetime fits usize");
        let err = generate::<TestScheme, _>(&mut rng, 1, lifetime).unwrap_err();
        assert!(matches!(err, CryptoError::ActivationOutOfRange { .. }));
    }

    /// A window whose bounds overflow `usize` is an error, not a panic.
    #[test]
    fn test_generate_rejects_overflowing_window() {
        let mut rng = rand::rng();
        let err = generate::<TestScheme, _>(&mut rng, usize::MAX, 1).unwrap_err();
        assert!(matches!(err, CryptoError::ActivationOutOfRange { .. }));
    }
}
