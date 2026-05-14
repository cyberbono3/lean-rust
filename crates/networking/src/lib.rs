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
//! - [`encode_req_resp`], [`decode_req_resp`], [`encode_gossip`],
//!   [`decode_gossip`] — generic Snappy-wrapped codecs over any
//!   [`ssz::Encode`] / [`ssz::Decode`] type.
//! - [`write_req_resp_frame`], [`read_req_resp_frame`] — length-prefixed
//!   `Read`/`Write` stream framing.
//! - [`NetworkingError`] — crate-level error enum.

#![forbid(unsafe_code)]

pub mod codecs;
pub mod config;
pub mod error;
pub mod frames;
pub mod gossipsub;
pub mod messages;
pub mod protocol_ids;
pub mod topics;

pub use codecs::{
    decode_gossip, decode_gossip_data, decode_req_resp, decode_req_resp_wire, encode_gossip,
    encode_gossip_data, encode_req_resp, encode_req_resp_wire,
};
pub use config::MAX_REQUEST_BLOCKS;
pub use error::NetworkingError;
pub use frames::{read_req_resp_frame, write_req_resp_frame};
pub use gossipsub::{
    compute_gossipsub_message_id, MESSAGE_DOMAIN_INVALID_SNAPPY, MESSAGE_DOMAIN_VALID_SNAPPY,
};
pub use messages::{BlocksByRootRequest, BlocksByRootResponse, Status};
pub use protocol_ids::{ProtocolId, BLOCKS_BY_ROOT_PROTOCOL_V1, STATUS_PROTOCOL_V1};
pub use topics::{BLOCK_TOPIC_V1, VOTE_TOPIC_V1};
