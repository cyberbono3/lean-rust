//! Wire codecs + protocol IDs for the consensus networking layer.
//!
//! Tier 4: depends on `types`, `protocol`, `ssz`. No `runtime` or
//! `storage` imports.
//!
//! # Public surface
//! - [`Status`], [`BlocksByRootRequest`], [`BlocksByRootResponse`] — typed
//!   req/resp payloads with SSZ codec.
//! - [`STATUS_PROTOCOL_V1`], [`BLOCKS_BY_ROOT_PROTOCOL_V1`] — libp2p
//!   protocol-ID constants.
//! - [`MAX_REQUEST_BLOCKS`] — list-length cap.
//! - [`compute_gossipsub_message_id`] — deterministic 20-byte gossipsub
//!   message-id primitive.
//! - [`NetworkingError`] — crate-level error enum.

#![forbid(unsafe_code)]

pub mod config;
pub mod error;
pub mod gossipsub;
pub mod messages;
pub mod protocol_ids;
pub mod topics;

pub use config::MAX_REQUEST_BLOCKS;
pub use error::NetworkingError;
pub use gossipsub::{
    compute_gossipsub_message_id, MESSAGE_DOMAIN_INVALID_SNAPPY, MESSAGE_DOMAIN_VALID_SNAPPY,
};
pub use messages::{BlocksByRootRequest, BlocksByRootResponse, Status};
pub use protocol_ids::{ProtocolId, BLOCKS_BY_ROOT_PROTOCOL_V1, STATUS_PROTOCOL_V1};
