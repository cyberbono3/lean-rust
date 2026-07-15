//! The signing surface, in wire types.

use types::Signature;

use crate::error::CryptoError;
use crate::key_state::SigningKey;
use crate::scheme::{signature_to_wire, SchemeWire};

impl<S: SchemeWire> SigningKey<S> {
    /// Signs a 32-byte message for `epoch`, returning the padded wire signature.
    ///
    /// This is the surface consumers use: it wraps the raw signer and pads the
    /// result into the wire container.
    ///
    /// # Errors
    ///
    /// - [`CryptoError::EpochReused`] when the epoch was already signed.
    /// - [`CryptoError::EpochNotPrepared`] when the prepared window does not
    ///   cover the epoch.
    /// - [`CryptoError::Signing`] when leanSig's encoder fails.
    /// - [`CryptoError::PayloadLength`] when the scheme's `PAYLOAD_LEN` is stale
    ///   against the pinned revision.
    pub fn sign(
        &mut self,
        epoch: u32,
        message: &[u8; leansig::MESSAGE_LENGTH],
    ) -> Result<Signature, CryptoError> {
        let signature = self.sign_raw(epoch, message)?;
        signature_to_wire::<S>(&signature)
    }
}
