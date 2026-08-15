//! Startup sequencing for the `lean-rust` binary.
//!
//! Three steps, in this order: install tracing, emit the one-shot
//! `startup configuration` line, then warn about flags that are accepted
//! for CLI compatibility but not wired to anything. Tracing must come
//! first — everything after it logs.

use anyhow::{bail, Context, Result};
use runtime::observability::{FileSink, TracingGuard};
use tracing::{info, warn};

use crate::cli::Cli;

const DEFAULT_LOG_PREFIX: &str = "lean-rust";

/// Installs the global tracing subscriber and returns its RAII guard.
///
/// The caller MUST keep the returned guard bound for the process
/// lifetime; dropping it early stops flushing the optional rolling file
/// sink.
///
/// # Errors
///
/// Returns an error if `--log.dir.prefix` was passed without
/// `--log.dir.path`, or if the subscriber cannot be installed.
pub fn init_tracing(cli: &Cli) -> Result<TracingGuard> {
    runtime::observability::init_tracing(cli.verbosity(), file_sink(cli)?)
        .context("initialize tracing")
}

/// Emits the single `startup configuration` line summarising the resolved CLI.
pub fn log_startup_config(cli: &Cli) {
    let rust_log = std::env::var_os("RUST_LOG");
    let rust_log_present = rust_log.is_some();
    let rust_log_non_empty = rust_log.as_deref().is_some_and(|value| !value.is_empty());

    info!(
        effective_verbosity = %cli.verbosity(),
        rust_log_present,
        rust_log_non_empty,
        data_dir = ?cli.data_dir,
        genesis_config = ?cli.genesis_config,
        genesis_state = ?cli.genesis_state,
        validator_registry_path = ?cli.validator_registry_path,
        node_id = ?cli.node_id,
        private_key_path = ?cli.private_key_path,
        devnet_bootnodes = ?cli.devnet_bootnodes,
        devnet_listen_addresses = ?cli.devnet_listen_addresses,
        http_address = ?cli.http_address,
        http_port = ?cli.http_port,
        http_allow_origin = ?cli.http_allow_origin,
        metrics_enabled = cli.metrics,
        metrics_address = ?cli.metrics_address,
        metrics_port = ?cli.metrics_port,
        log_dir_path = ?cli.log_dir_path,
        log_dir_prefix = ?selected_log_prefix(cli),
        "startup configuration",
    );
}

/// Warns about flags accepted for CLI compatibility that have no effect.
pub fn warn_unwired_flags(cli: &Cli) {
    if let Some(origin) = cli.http_allow_origin.as_deref() {
        warn!(
            value = origin,
            "--http-allow-origin is accepted for CLI compatibility but NOT applied: no CORS layer is wired into the HTTP server. The HTTP API will respond with default axum headers regardless of this value.",
        );
    }
}

fn selected_log_prefix(cli: &Cli) -> Option<&str> {
    cli.log_dir_path
        .as_ref()
        .map(|_| cli.log_dir_prefix.as_deref().unwrap_or(DEFAULT_LOG_PREFIX))
}

fn file_sink(cli: &Cli) -> Result<Option<FileSink<'_>>> {
    let Some(dir) = cli.log_dir_path.as_deref() else {
        if cli.log_dir_prefix.is_some() {
            bail!("--log.dir.prefix requires --log.dir.path");
        }
        return Ok(None);
    };

    // Daily rotation by default (see `LogRotation`): an operator who
    // opted into `--log.dir.path` gets bounded per-file growth.
    Ok(Some(FileSink::new(
        dir,
        cli.log_dir_prefix.as_deref().unwrap_or(DEFAULT_LOG_PREFIX),
    )))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn file_sink_rejects_prefix_without_path() {
        let cli = Cli::try_parse_from(["lean-rust", "--log.dir.prefix", "lean"])
            .expect("parse log prefix");

        let err = file_sink(&cli).expect_err("log prefix without log path must fail");

        assert!(
            err.to_string()
                .contains("--log.dir.prefix requires --log.dir.path"),
            "got {err}"
        );
    }
}
