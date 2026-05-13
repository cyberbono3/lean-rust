//! Opaque [`PeerId`] newtype for the sync module.

use super::error::SyncError;

/// Opaque peer identifier — guaranteed non-empty.
///
/// Held as a `String` so this crate never compiles against the libp2p
/// crate. Adapters in `node` construct values via base-58 encodings of
/// the underlying transport's peer id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeerId(String);

impl PeerId {
    /// Wraps a raw identifier (typically a base-58-encoded libp2p peer id).
    ///
    /// # Errors
    /// [`SyncError::EmptyPeerId`] when `raw` is empty.
    pub fn new(raw: impl Into<String>) -> Result<Self, SyncError> {
        let raw = raw.into();
        if raw.is_empty() {
            return Err(SyncError::EmptyPeerId);
        }
        Ok(Self(raw))
    }

    /// Returns the underlying string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the [`PeerId`] and returns the inner `String`. Useful for
    /// adapters that need to feed the identifier into APIs that take
    /// ownership without an extra clone.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for PeerId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for PeerId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}
