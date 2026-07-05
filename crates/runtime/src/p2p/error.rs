//! Error type for the libp2p host service.

use std::path::PathBuf;

use libp2p::{
    identity::DecodingError,
    multiaddr::{Error as MultiaddrError, Multiaddr},
    TransportError,
};
use thiserror::Error;

/// Failures raised during host construction and lifecycle.
///
/// Construction failures (identity load, bootnode parse, options
/// validation) and lifecycle failures (bind, transport) share one enum
/// so callers above the `Service` boundary see a single typed surface.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HostError {
    /// `HostOptions::try_new` received an empty / whitespace-only
    /// listen multiaddr.
    #[error("host listen address must not be empty")]
    EmptyListenAddr,

    /// `HostOptions::try_new` received an empty / whitespace-only
    /// agent-version string.
    #[error("host agent version must not be empty")]
    EmptyAgentVersion,

    /// `HostOptions::try_new` received an empty identity path.
    #[error("host identity path must not be empty")]
    EmptyIdentityPath,

    /// `HostOptions::try_new` received an empty bootnodes path.
    /// (`None` is fine; explicit empty paths are not.)
    #[error("host bootnodes path must not be empty")]
    EmptyBootnodesPath,

    /// The supplied listen multiaddr did not parse.
    #[error("invalid listen address {input:?}: {source}")]
    InvalidListenAddr {
        /// Raw input that failed to parse.
        input: String,
        /// Underlying multiaddr decode error.
        #[source]
        source: MultiaddrError,
    },

    /// Reading the identity file from disk failed.
    #[error("identity file {path:?} unreadable: {source}")]
    IdentityIo {
        /// Resolved absolute path the host attempted to read or write.
        path: PathBuf,
        /// Underlying `io::Error`.
        #[source]
        source: std::io::Error,
    },

    /// The identity file existed but did not decode as a libp2p
    /// protobuf-encoded keypair.
    #[error("identity file {path:?} corrupt: {source}")]
    InvalidIdentity {
        /// Resolved absolute path of the corrupt file.
        path: PathBuf,
        /// Underlying libp2p keypair decode error.
        #[source]
        source: DecodingError,
    },

    /// The identity file existed but was neither protobuf nor valid raw
    /// secp256k1 hex.
    #[error("identity file {path:?} invalid raw secp256k1 hex: {reason}")]
    InvalidRawIdentityHex {
        /// Resolved absolute path of the invalid file.
        path: PathBuf,
        /// Human-readable rejection reason.
        reason: String,
    },

    /// The identity file decoded from hex but was not a 32-byte secp256k1
    /// private key.
    #[error("identity file {path:?} raw secp256k1 key has {actual} bytes, expected {expected}")]
    InvalidRawIdentityLength {
        /// Resolved absolute path of the invalid file.
        path: PathBuf,
        /// Decoded byte length.
        actual: usize,
        /// Required byte length.
        expected: usize,
    },

    /// The identity file decoded to 32 bytes but the bytes are not valid
    /// secp256k1 secret-key material.
    #[error("identity file {path:?} invalid raw secp256k1 key material: {source}")]
    InvalidRawIdentityKeyMaterial {
        /// Resolved absolute path of the invalid file.
        path: PathBuf,
        /// Underlying libp2p secp256k1 decode error.
        #[source]
        source: DecodingError,
    },

    /// Reading the bootnodes YAML file failed.
    #[error("bootnodes file {path:?} unreadable: {source}")]
    BootnodesRead {
        /// Resolved absolute path the host attempted to read.
        path: PathBuf,
        /// Underlying `io::Error`.
        #[source]
        source: std::io::Error,
    },

    /// YAML decode of the bootnodes file failed (shape mismatch, etc.).
    #[error("bootnodes file {path:?} parse error: {source}")]
    BootnodesParse {
        /// Resolved absolute path the host attempted to parse.
        path: PathBuf,
        /// Underlying `serde_yaml` decode error.
        #[source]
        source: serde_yaml::Error,
    },

    /// A single bootnode entry failed validation. Carries the offending
    /// raw string so diagnostics can pinpoint the bad line without
    /// re-reading the file.
    #[error("invalid bootnode entry {entry:?}: {reason}")]
    InvalidBootnode {
        /// Raw YAML entry that failed validation.
        entry: String,
        /// Human-readable rejection reason (parse error, missing peer
        /// id, etc.).
        reason: String,
    },

    /// `Service::start` could not bind the listen address. Wraps the
    /// underlying transport error or the bind-deadline expiry.
    #[error("bind failed for {addr}: {reason}")]
    Bind {
        /// Multiaddr the host attempted to listen on.
        addr: Multiaddr,
        /// Human-readable failure reason.
        reason: String,
    },

    /// libp2p transport setup or operation failed.
    #[error("transport: {0}")]
    Transport(#[from] TransportError<std::io::Error>),

    /// Building the gossipsub `Behaviour` rejected the configured
    /// parameters. Indicates a programming error in this crate (the
    /// config is internal); surfaced as a typed variant so callers can
    /// distinguish it from runtime bind failures.
    #[error("gossipsub init: {0}")]
    GossipsubInit(String),

    /// `Service::start` could not subscribe to a gossipsub topic.
    /// Wraps the libp2p `SubscriptionError` message — typically the
    /// max-subscribed-topics cap or a publish-only config mismatch.
    #[error("gossipsub subscribe: {0}")]
    GossipSubscribe(String),

    /// `Service::start` was called more than once on the same instance.
    #[error("host service already started")]
    AlreadyStarted,
}

/// Convenience alias for `Result<T, HostError>`. Mirrors the
/// ecosystem-standard `io::Result` / `anyhow::Result` shape so host
/// signatures stay concise.
pub type HostResult<T> = Result<T, HostError>;
