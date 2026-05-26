//! Composition root for the Lean runtime.
//!
//! The runtime sibling crates intentionally stay decoupled. This crate
//! is the place where concrete services are assembled, narrow ports are
//! adapted, and a [`lean_core::Node`] is returned ready for lifecycle
//! management.

#![forbid(unsafe_code)]

pub mod devnet;
pub mod publisher_adapter;

mod gossip_ingest;
mod rpc_provider;

pub use devnet::{new_devnet, Config, Result};
pub use publisher_adapter::PublisherAdapter;
