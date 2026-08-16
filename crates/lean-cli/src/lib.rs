// The crate docs ARE the README. The module inventory used to be written out
// in both, which meant every module added here had to be described twice —
// `README.md` is now the single source.
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

pub mod cli;
pub mod commands;
pub mod genesis;
pub mod keygen;
pub mod node_config;
pub mod shutdown;
pub mod startup;
pub mod validator_keygen;
