//! The crate's error type.
//!
//! leanSig reports failure three different ways: a `SigningError` for encoding
//! exhaustion, an SSZ decode error for malformed bytes, and a bare `false` from
//! `verify`. This enum is where those become one typed surface, so callers never
//! see upstream's shapes.

use thiserror::Error;

/// Errors returned by the crypto adapter.
#[derive(Debug, Error)]
pub enum CryptoError {
    /// The caller attempted to sign an epoch at or before the last-signed one.
    ///
    /// Signing twice for one epoch with different messages discloses the
    /// one-time-signature key, so this is refused rather than reported as a
    /// warning.
    #[error("epoch {epoch} was already signed (last signed epoch: {last_signed}); reusing a one-time key discloses it")]
    EpochReused {
        /// Epoch the caller asked to sign.
        epoch: u32,
        /// Highest epoch this key has already signed.
        last_signed: u32,
    },

    /// The requested epoch lies outside the key's prepared interval.
    #[error("epoch {epoch} is outside the prepared interval {start}..{end}; call prepare() first")]
    EpochNotPrepared {
        /// Epoch the caller asked to sign.
        epoch: u32,
        /// Inclusive start of the prepared interval.
        start: u64,
        /// Exclusive end of the prepared interval.
        end: u64,
    },

    /// The requested epoch lies outside the key's activation interval.
    #[error("epoch {epoch} is outside the activation interval {start}..{end}")]
    EpochNotActive {
        /// Epoch the caller asked to sign.
        epoch: u32,
        /// Inclusive start of the activation interval.
        start: u64,
        /// Exclusive end of the activation interval.
        end: u64,
    },

    /// The signature did not verify against the public key, epoch, and message.
    ///
    /// leanSig's `verify` returns a bare `bool`; this variant is what that
    /// `false` becomes. It carries no detail on purpose — a verifier that
    /// explains *why* a signature failed hands an attacker a probe.
    #[error("signature verification failed")]
    InvalidSignature,

    /// leanSig's signing routine failed.
    #[error("leanSig signing failed")]
    Signing(#[source] leansig::signature::SigningError),

    /// A signature's payload length disagrees with the scheme's constant.
    ///
    /// Not a per-signature condition: the payload length is fixed per scheme, so
    /// this means the scheme's `PAYLOAD_LEN` is stale against the pinned leanSig
    /// revision. Padding a wrong-length payload into the right-sized container
    /// would produce a signature no downstream length check could reject, so
    /// this fails loudly instead.
    #[error("signature payload length {got} does not match the scheme's expected {expected}; PAYLOAD_LEN is stale against the pinned revision")]
    PayloadLength {
        /// Length leanSig actually produced.
        got: usize,
        /// Length the scheme's `PAYLOAD_LEN` declares.
        expected: usize,
    },

    /// A signature payload does not fit the wire container.
    ///
    /// The container is padded, so the payload is normally smaller than the
    /// container. This variant is the guard on that assumption.
    #[error("signature payload of {got} bytes exceeds the {capacity}-byte wire container")]
    PayloadTooLong {
        /// Payload length.
        got: usize,
        /// Capacity the wire container provides.
        capacity: usize,
    },

    /// The scheme's declared payload length exceeds the wire container.
    ///
    /// Distinct from [`CryptoError::PayloadTooLong`] on purpose: nothing is wrong
    /// with the signature being decoded — the scheme constant itself does not fit,
    /// which is a build-time misconfiguration. Reporting it as a too-long payload
    /// would send whoever reads it looking at the wrong thing.
    #[error("scheme declares a {payload_len}-byte payload, which exceeds the {capacity}-byte wire container")]
    SchemeMisconfigured {
        /// The scheme's declared payload length.
        payload_len: usize,
        /// Capacity the wire container provides.
        capacity: usize,
    },

    /// The wire container's padding region is not zero.
    ///
    /// The padding carries no information, so accepting arbitrary bytes there
    /// would make one logical signature into many distinct containers that all
    /// verify — malleability that defeats duplicate suppression on gossip. No
    /// correct implementation emits non-zero padding.
    #[error("signature container padding is not zero")]
    NonZeroPadding,

    /// A signature payload's SSZ offset fields are not the scheme's canonical
    /// values.
    ///
    /// leanSig's decoder accepts any monotonic in-bounds offsets, while its
    /// verifier assumes the canonical field lengths and indexes on them. A
    /// re-split payload therefore panics rather than failing to verify, so the
    /// offsets are pinned here before anything is decoded.
    #[error("signature layout field {field} is {got}, expected {expected}")]
    MalformedLayout {
        /// Which offset field was wrong.
        field: &'static str,
        /// Value found on the wire.
        got: u32,
        /// The scheme's canonical value.
        expected: u32,
    },

    /// The requested key activation window does not fit the scheme's lifetime.
    #[error("activation window {activation_epoch}..+{num_active_epochs} does not fit the scheme lifetime {lifetime}")]
    ActivationOutOfRange {
        /// Requested first active epoch.
        activation_epoch: usize,
        /// Requested number of active epochs.
        num_active_epochs: usize,
        /// The scheme's total lifetime.
        lifetime: u64,
    },

    /// A public key's encoded length does not match the wire newtype's width.
    #[error("public key length {got} does not match the wire width {expected}")]
    PublicKeyLength {
        /// Length leanSig actually produced.
        got: usize,
        /// Width the wire public key declares.
        expected: usize,
    },

    /// A signature could not be decoded from its wire bytes.
    ///
    /// The upstream decode error is rendered to a string rather than carried as
    /// a `#[source]`: it does not implement `std::error::Error`, so it cannot
    /// chain.
    #[error("signature decode failed: {detail}")]
    SignatureDecode {
        /// Upstream decoder detail.
        detail: String,
    },

    /// A public key could not be decoded from its wire bytes.
    #[error("public key decode failed: {detail}")]
    PublicKeyDecode {
        /// Upstream decoder detail.
        detail: String,
    },
}
