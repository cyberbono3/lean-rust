//! Sync service — peer-driven `BlocksByRoot` backfill orchestrator.
//!
//! On each outbound peer-connect event the [`Loop`] performs a
//! `Status` handshake and—if the peer is ahead—walks backwards from
//! the peer's head one root at a time via `BlocksByRoot` up to
//! [`Config::max_sync_depth`], then imports the recovered chain in
//! forward order through the [`Chain`] port.
//!
//! Per Decision 7 (Dependency Inversion), trait impls live elsewhere:
//!
//! - [`Chain`] is satisfied by [`runtime_chain::Service`] via the
//!   adapter `impl` in [`chain_adapter`]. Tests in this crate use
//!   in-memory fakes.
//! - [`Network`] / [`PeerEventProvider`] have no in-crate impl. The
//!   `runtime-p2p` / `node` crates provide the libp2p-backed
//!   adapters in later issues.
//!
//! The crate compiles with zero `libp2p` exposure on its dependency
//! graph.

#![forbid(unsafe_code)]

mod chain_adapter;
mod config;
mod error;
mod loop_;
mod peer_id;
mod ports;

pub use config::Config;
pub use error::SyncError;
pub use loop_::Loop;
pub use peer_id::PeerId;
pub use ports::{Chain, Network, PeerEventProvider};
