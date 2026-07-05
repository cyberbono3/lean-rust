//! Consolidated runtime shell.
//!
//! Each module below was previously a standalone crate; they were merged into
//! one crate to remove per-crate manifest and re-export boilerplate while
//! keeping the sync-core crates (`types`, `ssz`, `config`, `protocol`,
//! `forkchoice`, `storage`, `networking`) separate — that split is the
//! audited boundary guaranteeing consensus logic never pulls `tokio`/`libp2p`.
#![forbid(unsafe_code)]

pub mod api;
pub mod chain;
pub mod core;
pub mod duties;
pub mod observability;
pub mod p2p;
pub mod sync;
