//! `init_tracing` helper — installs a `tracing-subscriber` registry with
//! an `EnvFilter` driven by [`Verbosity`] and an optional file sink via
//! [`tracing_appender`].

use std::io;
use std::path::Path;

use thiserror::Error;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{InitError as AppenderInitError, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::{SubscriberInitExt, TryInitError};
use tracing_subscriber::{fmt, EnvFilter};

use crate::verbosity::Verbosity;

/// Default filename suffix for the rolling file sink. Combined with the
/// appender's date stamp this yields `<prefix>.<date>.log`.
const LOG_FILE_SUFFIX: &str = "log";

/// How often the file sink rolls to a new file.
///
/// Owned `Copy` mirror of [`tracing_appender::rolling::Rotation`] so the
/// public [`FileSink`] stays `Copy` and callers do not need a
/// `tracing_appender` import. Defaults to [`LogRotation::Daily`]: an
/// operator who opted into a file sink expects bounded per-file growth,
/// not a single file that grows for the whole process lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogRotation {
    /// Roll every minute (mainly for tests / very high volume).
    Minutely,
    /// Roll hourly.
    Hourly,
    /// Roll daily — the default.
    #[default]
    Daily,
    /// Never roll: one file per process lifetime. The pre-rotation
    /// behavior, retained as an explicit opt-in for operators who manage
    /// rotation externally (e.g. `logrotate`).
    Never,
}

impl LogRotation {
    /// Maps to the `tracing_appender` rotation policy.
    fn policy(self) -> Rotation {
        match self {
            Self::Minutely => Rotation::MINUTELY,
            Self::Hourly => Rotation::HOURLY,
            Self::Daily => Rotation::DAILY,
            Self::Never => Rotation::NEVER,
        }
    }
}

/// Directory + filename prefix for the optional file sink in
/// [`init_tracing`]. Either pass both (and get stderr + file) or pass
/// [`None`] (and get stderr only) — the type rules out the nonsense
/// "prefix without directory" combination.
#[derive(Debug, Clone, Copy)]
pub struct FileSink<'a> {
    /// Directory under which the log file is created. Created if it
    /// does not exist.
    pub dir: &'a Path,
    /// Basename prefix; the final file is `<prefix>.<date>.log` (the
    /// date component is added by the rolling appender per
    /// [`Self::rotation`]).
    pub prefix: &'a str,
    /// How often the file rolls. Defaults to [`LogRotation::Daily`] when
    /// built via [`FileSink::new`].
    pub rotation: LogRotation,
}

impl<'a> FileSink<'a> {
    /// Builds a file sink rolling daily (the recommended default).
    #[must_use]
    pub fn new(dir: &'a Path, prefix: &'a str) -> Self {
        Self {
            dir,
            prefix,
            rotation: LogRotation::Daily,
        }
    }

    /// Returns a copy with the rotation policy overridden.
    #[must_use]
    pub const fn with_rotation(mut self, rotation: LogRotation) -> Self {
        self.rotation = rotation;
        self
    }
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

    /// Failed to construct the rolling file appender (e.g. the directory
    /// is not writable).
    #[error("initialize rolling file appender: {0}")]
    FileAppender(#[source] AppenderInitError),
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
/// use lean_observability::{init_tracing, Verbosity};
///
/// let _guard = init_tracing(Verbosity::Info, None)?;
/// tracing::info!("ready");
/// # Ok::<(), lean_observability::TracingInitError>(())
/// ```
pub fn init_tracing(
    verbosity: Verbosity,
    file_sink: Option<FileSink<'_>>,
) -> Result<TracingGuard, TracingInitError> {
    let filter = env_filter(verbosity);

    let stderr_layer = fmt::layer().with_writer(io::stderr);

    let (file_layer, file_worker) = match file_sink {
        Some(FileSink {
            dir,
            prefix,
            rotation,
        }) => {
            std::fs::create_dir_all(dir).map_err(TracingInitError::CreateLogDir)?;
            // A rotating appender bounds per-file growth: with the
            // default daily policy a long-running node writes at most one
            // file per day (`<prefix>.<date>.log`) instead of a single
            // file that grows for the whole process lifetime.
            let appender = tracing_appender::rolling::Builder::new()
                .rotation(rotation.policy())
                .filename_prefix(prefix)
                .filename_suffix(LOG_FILE_SUFFIX)
                .build(dir)
                .map_err(TracingInitError::FileAppender)?;
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn rotation_defaults_to_daily() {
        assert_eq!(LogRotation::default(), LogRotation::Daily);
    }

    #[test]
    fn rotation_maps_to_appender_policy() {
        // The mapping must be total and distinct from NEVER for the
        // rolling variants — NEVER is the pre-rotation behavior.
        assert_eq!(LogRotation::Minutely.policy(), Rotation::MINUTELY);
        assert_eq!(LogRotation::Hourly.policy(), Rotation::HOURLY);
        assert_eq!(LogRotation::Daily.policy(), Rotation::DAILY);
        assert_eq!(LogRotation::Never.policy(), Rotation::NEVER);
    }

    #[test]
    fn file_sink_new_defaults_to_daily_rotation() {
        let dir = Path::new("/tmp/logs");
        let sink = FileSink::new(dir, "lean-rust");
        assert_eq!(sink.rotation, LogRotation::Daily);
        assert_eq!(
            sink.with_rotation(LogRotation::Never).rotation,
            LogRotation::Never
        );
    }
}
