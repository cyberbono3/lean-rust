//! `init_tracing` helper — installs a `tracing-subscriber` registry with
//! an `EnvFilter` driven by [`Verbosity`] and an optional file sink via
//! [`tracing_appender`].

use std::io;
use std::path::Path;

use thiserror::Error;
use time::OffsetDateTime;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::{SubscriberInitExt, TryInitError};
use tracing_subscriber::{fmt, EnvFilter};

use crate::observability::verbosity::Verbosity;

/// Directory + filename prefix for the optional file sink in
/// [`init_tracing`]. Either pass both (and get stderr + file) or pass
/// [`None`] (and get stderr only) — the type rules out the nonsense
/// "prefix without directory" combination.
#[derive(Debug, Clone, Copy)]
pub struct FileSink<'a> {
    /// Directory under which the log file is created. Created if it
    /// does not exist.
    pub dir: &'a Path,
    /// Basename prefix; the final file is `<prefix>-<utc-stamp>.log`.
    pub prefix: &'a str,
}

/// RAII guard returned by [`init_tracing`]. Drop on shutdown so the
/// background file-writer worker flushes its buffer.
///
/// Holding `None` means no file sink was configured (stderr only); the
/// guard is still returned so the API shape stays uniform.
#[derive(Debug)]
#[must_use = "drop this guard at process shutdown to flush the file-writer worker"]
pub struct TracingGuard {
    _file_worker: Option<WorkerGuard>,
}

/// Errors raised by [`init_tracing`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TracingInitError {
    /// A global `tracing` subscriber was already installed when
    /// [`init_tracing`] was called.
    #[error("tracing subscriber already initialized")]
    AlreadyInitialized(#[source] TryInitError),

    /// Failed to create the file-sink directory.
    #[error("create log directory: {0}")]
    CreateLogDir(#[source] io::Error),
}

/// Installs a `tracing-subscriber` registry as the global subscriber.
///
/// - `verbosity` controls the [`EnvFilter`] directive (silences known-
///   noisy crates at coarse levels — see [`Verbosity::directive`]).
/// - `file_sink`, when [`Some`], adds a non-blocking file layer that
///   writes to `<dir>/<prefix>-<utc-stamp>.log`.
/// - The `RUST_LOG` env var, when set, supersedes `verbosity` (standard
///   `tracing-subscriber` precedence).
///
/// # Errors
/// - [`TracingInitError::AlreadyInitialized`] if a subscriber was already
///   installed in the current process.
/// - [`TracingInitError::CreateLogDir`] if `file_sink.dir` cannot be
///   created.
///
/// # Example
/// ```no_run
/// use lean_core::{init_tracing, Verbosity};
///
/// let _guard = init_tracing(Verbosity::Info, None)?;
/// tracing::info!("ready");
/// # Ok::<(), lean_core::TracingInitError>(())
/// ```
pub fn init_tracing(
    verbosity: Verbosity,
    file_sink: Option<FileSink<'_>>,
) -> Result<TracingGuard, TracingInitError> {
    let filter = env_filter(verbosity);

    let stderr_layer = fmt::layer().with_writer(io::stderr);

    let (file_layer, file_worker) = match file_sink {
        Some(FileSink { dir, prefix }) => {
            std::fs::create_dir_all(dir).map_err(TracingInitError::CreateLogDir)?;
            let appender = tracing_appender::rolling::never(dir, log_file_name(prefix));
            let (writer, worker) = tracing_appender::non_blocking(appender);
            (
                Some(fmt::layer().with_ansi(false).with_writer(writer)),
                Some(worker),
            )
        }
        None => (None, None),
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .try_init()
        .map_err(TracingInitError::AlreadyInitialized)?;

    Ok(TracingGuard {
        _file_worker: file_worker,
    })
}

/// Builds the [`EnvFilter`] used by [`init_tracing`].
///
/// `RUST_LOG` (the standard `tracing-subscriber` env var) wins when set
/// and non-empty; otherwise the directive derived from `verbosity` is
/// used. `parse_lossy` swallows directive-parse errors and falls back to
/// the empty filter — appropriate for a runtime entry point where a
/// malformed env var should not abort startup.
fn env_filter(verbosity: Verbosity) -> EnvFilter {
    let env = std::env::var(EnvFilter::DEFAULT_ENV).unwrap_or_default();
    let source = if env.is_empty() {
        verbosity.directive()
    } else {
        env.as_str()
    };
    EnvFilter::builder().parse_lossy(source)
}

/// Builds the timestamped log file name: `<prefix>-<utc-stamp>.log`.
///
/// The stamp uses a fixed-width compact form (`YYYYMMDDThhmmssZ`) so
/// files sort lexicographically by creation time.
fn log_file_name(prefix: &str) -> String {
    let stamp = utc_stamp();
    format!("{prefix}-{stamp}.log")
}

/// Returns the current UTC time as `YYYYMMDDThhmmssZ`.
fn utc_stamp() -> String {
    let now = OffsetDateTime::now_utc();
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn log_file_name_has_expected_shape() {
        let name = log_file_name("lean-beacon");
        // "lean-beacon-YYYYMMDDThhmmssZ.log" = 11 + 1 + 16 + 4 = 32 chars
        assert!(name.starts_with("lean-beacon-"), "got {name}");
        let extension = std::path::Path::new(&name).extension();
        assert_eq!(extension.and_then(|e| e.to_str()), Some("log"));
        assert_eq!(name.len(), "lean-beacon-".len() + 16 + ".log".len());
    }
}
