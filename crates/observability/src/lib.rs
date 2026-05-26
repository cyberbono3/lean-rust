//! Tracing-subscriber setup for the runtime shell.
//!
//! - [`Verbosity`] — coarse-grained level enum (`Error`..`Trace`) with a
//!   [`Verbosity::directive`] builder for the [`EnvFilter`] directive
//!   that silences known-noisy ecosystem crates.
//! - [`init_tracing`] — installs the global subscriber with the default
//!   `tracing_subscriber::fmt()` formatter; optionally adds a non-
//!   blocking file sink via [`tracing_appender`].
//! - [`TracingGuard`] — RAII guard; drop on shutdown to flush the file
//!   sink.
//!
//! No custom `FormatEvent` impl — the default human-readable output is
//! the canonical shape, parseable by every Rust log-aggregation tool.
//! `RUST_LOG` env var overrides the CLI-derived directive.
//!
//! [`EnvFilter`]: tracing_subscriber::EnvFilter
//! [`tracing_appender`]: https://docs.rs/tracing-appender

#![forbid(unsafe_code)]

mod init;
mod verbosity;

pub use init::{init_tracing, FileSink, TracingGuard, TracingInitError};
pub use verbosity::{ParseVerbosityError, Verbosity};
