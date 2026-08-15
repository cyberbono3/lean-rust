//! Library surface for the `lean-rust` binary.
//!
//! Everything the binary does beyond `main` and `run` lives here, so the
//! entry point at `bin/lean-rust/src/main.rs` is a genuinely thin shell:
//!
//! - [`cli`] — the `clap` parser and [`cli::parse`].
//! - [`startup`] — tracing installation, the startup-configuration log
//!   line, and warnings for accepted-but-unwired flags.
//! - [`commands`] — subcommand dispatch (`devnet-config`, keygen, peer id).
//! - [`node_config`] — CLI-to-node wiring: builds the devnet
//!   [`node::Config`] handed to the composition root (`node::new_devnet`).
//! - [`shutdown`] — SIGINT/SIGTERM handling.
//! - [`genesis`], [`keygen`], [`validator_keygen`] — genesis builders and
//!   offline key material.

#![forbid(unsafe_code)]

pub mod cli;
pub mod commands;
pub mod genesis;
pub mod keygen;
pub mod node_config;
pub mod shutdown;
pub mod startup;
pub mod validator_keygen;
