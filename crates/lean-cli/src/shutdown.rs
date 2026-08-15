//! Process shutdown signalling for the `lean-rust` binary.
//!
//! Signal handling is a binary-lifecycle concern, so it lives here rather
//! than in `runtime`: nothing but `main` should install process handlers,
//! and `runtime` is consumed by `node`, `fixtures`, and several test
//! suites that must not acquire that capability by accident.

use anyhow::{Context, Result};

/// Waits for the first shutdown signal — `SIGINT` or `SIGTERM`.
///
/// # Errors
///
/// Returns an error if the `SIGTERM` handler cannot be installed, or if
/// the `SIGINT` listener fails.
#[cfg(unix)]
pub async fn wait() -> Result<()> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = signal(SignalKind::terminate()).context("install SIGTERM handler")?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result.context("listen for SIGINT")?,
        _ = sigterm.recv() => {},
    }
    Ok(())
}

/// Waits for the first shutdown signal — `SIGINT`.
///
/// # Errors
///
/// Returns an error if the `SIGINT` listener fails.
#[cfg(not(unix))]
pub async fn wait() -> Result<()> {
    tokio::signal::ctrl_c().await.context("listen for SIGINT")?;
    Ok(())
}
