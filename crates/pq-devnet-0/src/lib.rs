//! pq-devnet-0 compatibility fixtures.
//!
//! This crate intentionally owns cross-client local-pq fixture contracts
//! rather than production runtime code. Runtime crates consume these contracts
//! through tests before the Docker devnet scripts are moved into this repo.

#![forbid(unsafe_code)]
