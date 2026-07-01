//! Req/resp protocol surface: `Status` handshake + `BlocksByRoot`
//! handler.
//!
//! The contract surface ([`RpcProvider`], [`RpcError`]) lives in
//! [`provider`] — the concrete type folded in from the former `p2p-rpc`
//! crate. This module owns the inbound dispatch helpers
//! ([`blocks_by_root`], [`status`], [`outbound`]) plus the outbound
//! [`client`] wrapper — those are tightly coupled to the swarm-poll
//! loop in [`crate::P2pService`].

pub(crate) mod blocks_by_root;
mod client;
pub(crate) mod outbound;
mod provider;
pub(crate) mod status;

pub use crate::host::behaviour::codec::{RpcRequest, RpcResponse};
pub use provider::{RpcError, RpcProvider};
