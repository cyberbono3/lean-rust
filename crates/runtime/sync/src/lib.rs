//! Sync service — peer-driven `BlocksByRoot` backfill orchestrator.
//!
//! On each outbound peer-connect event the [`Loop`] performs a
//! `Status` handshake and—if the peer is ahead—walks backwards from
//! the peer's head one root at a time via `BlocksByRoot` up to
//! [`Config::max_sync_depth`], then imports the recovered chain in
//! forward order through the [`Chain`] port.
//!
//! The chain surface is the concrete [`lean_chain::Service`], called
//! directly by the [`Loop`]. The outbound [`Network`] /
//! [`PeerEventProvider`] ports have no in-crate impl yet — the
//! `lean-p2p-host` / `node` crates provide the libp2p-backed handle in a
//! later issue; tests use in-memory fakes until then.

#![forbid(unsafe_code)]

mod config;
mod error;
mod loop_;
mod peer_id;
mod ports;

pub use config::Config;
pub use error::SyncError;
pub use loop_::Loop;
pub use peer_id::PeerId;
pub use ports::{Network, PeerEventProvider};
