//! Sync module — peer-driven backfill via `BlocksByRoot`.
//!
//! See [`Loop`] for the orchestrator and [`Chain`] / [`Network`] /
//! [`PeerEventProvider`] for the port traits. Per Decision 7
//! (Dependency Inversion), trait impls live in `node` (libp2p) and in
//! this crate's [`crate::Service`] adapter.

mod config;
mod error;
mod loop_;
mod ports;

pub use config::{Config, DEFAULT_MAX_SYNC_DEPTH};
pub use error::{PeerId, SyncError};
pub use loop_::Loop;
pub use ports::{Chain, Network, PeerEventProvider};
