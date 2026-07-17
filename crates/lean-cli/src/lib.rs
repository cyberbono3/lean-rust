//! Library surface for the `lean-rust` binary.
//!
//! Carries the CLI parser, genesis builders, and identity keygen helpers
//! so the binary entry-point at `bin/lean-rust/src/main.rs` stays a
//! thin shell that wires these pieces into the runtime composition root
//! (`node::new_devnet`).

#![forbid(unsafe_code)]

pub mod cli;
pub mod genesis;
pub mod keygen;
pub mod validator_keygen;
