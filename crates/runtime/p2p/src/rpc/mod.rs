//! Req/resp protocol surface: `Status` handshake + `BlocksByRoot`
//! handler.
//!
//! The contract surface ([`RpcProvider`], [`NoOpRpcProvider`],
//! [`RpcError`], [`SharedRpcProvider`]) lives in the sibling `p2p-rpc`
//! crate so the application can implement the trait without pulling in
//! libp2p. This module re-exports those types for backwards-compatible
//! consumption and owns the inbound dispatch helpers
//! ([`blocks_by_root`], [`status`], [`outbound`]) plus the outbound
//! [`client`] wrapper — those are tightly coupled to the swarm-poll
//! loop in [`crate::P2pService`] and stay in the host crate.

pub(crate) mod blocks_by_root;
mod client;
pub(crate) mod outbound;
pub(crate) mod status;

pub use crate::host::behaviour::codec::{RpcRequest, RpcResponse};
pub use p2p_rpc::{NoOpRpcProvider, RpcError, RpcProvider};

pub(crate) use p2p_rpc::SharedRpcProvider;
