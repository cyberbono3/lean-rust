//! Sync service — peer-driven `BlocksByRoot` backfill orchestrator.
//!
//! On each outbound peer-connect event the [`Loop`] performs a
//! `Status` handshake and—if the peer is ahead—walks backwards from
//! the peer's head one root at a time via `BlocksByRoot` up to
//! [`Config::max_sync_depth`], then imports the recovered chain in
//! forward order through the concrete [`crate::chain::Service`].
//!
//! Both surfaces are concrete: the chain is [`crate::chain::Service`] and
//! the outbound RPC + connect events come from [`crate::p2p::P2pService`]
//! (the former `Network` / `PeerEventProvider` port traits collapsed to
//! this one handle). `P2pService` speaks base-58 `String` peer ids and
//! maps its `RpcError` into [`SyncError`], so this module never compiles
//! against `libp2p`.

mod config;
mod error;
mod loop_;
mod peer_id;

pub use config::Config;
pub use error::SyncError;
pub use loop_::Loop;
pub use peer_id::PeerId;
