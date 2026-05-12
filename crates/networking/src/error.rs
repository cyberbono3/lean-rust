//! Crate-level error type for the networking layer.

use std::io;

use thiserror::Error;

/// Errors raised by the networking codec + framing surface.
///
/// `PartialEq` is intentionally not derived: [`snap::Error`] and
/// [`std::io::Error`] are not comparable. Use [`matches!`] for variant
/// assertions in tests.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NetworkingError {
    /// A bounded list constructor received more elements than allowed.
    ///
    /// `kind` identifies the rejected list (e.g. `"blocks_by_root request"`).
    #[error("{kind} length {len} exceeds max {max}")]
    ListTooLarge {
        /// Short label for the list being constructed.
        kind: &'static str,
        /// Length of the rejected input.
        len: usize,
        /// Inclusive upper bound.
        max: usize,
    },

    /// SSZ codec failure forwarded from the `ssz` crate.
    #[error(transparent)]
    Ssz(#[from] ssz::SszError),

    /// I/O failure while reading or writing a req/resp frame.
    #[error("req/resp frame io: {0}")]
    Io(#[from] io::Error),

    /// Snappy compression or decompression failure.
    #[error("snappy: {0}")]
    Snappy(#[source] snap::Error),

    /// Frame violated the req/resp framing invariants (bad magic, bad chunk
    /// type, checksum mismatch, premature end-of-stream, etc.).
    #[error("malformed req/resp frame: {reason}")]
    MalformedFrame {
        /// Static description of the structural violation.
        reason: &'static str,
    },

    /// Declared uncompressed length exceeded the caller-supplied cap.
    #[error("frame uncompressed length {length} exceeds limit {max}")]
    FrameTooLarge {
        /// Declared uncompressed length.
        length: u64,
        /// Caller-supplied cap.
        max: u64,
    },

    /// Length-prefix uvarint did not fit in `u64`.
    #[error("req/resp frame length prefix overflows u64")]
    UvarintOverflow,
}
