//! Composition root for the Lean runtime.
//!
//! The runtime sibling crates intentionally stay decoupled. This crate
//! is the place where concrete services are assembled, narrow ports are
//! adapted, and a [`runtime::core::Node`] is returned ready for lifecycle
//! management.

#![forbid(unsafe_code)]

pub mod devnet;

mod consensus_loop;

pub use devnet::{new_devnet, Config, Result, StorageKind};
