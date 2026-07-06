//! `init_tracing` helper — installs a `tracing-subscriber` registry with
//! an `EnvFilter` driven by [`Verbosity`] and an optional file sink via
//! [`tracing_appender`].

use std::io;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::OnceLock;

use thiserror::Error;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{InitError as AppenderInitError, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::{SubscriberInitExt, TryInitError};
use tracing_subscriber::{fmt, EnvFilter};

use crate::observability::verbosity::Verbosity;

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
///
/// # Intentional override surface
///
/// `bin/lean-rust` currently builds the sink via [`FileSink::new`], which
/// pins [`LogRotation::Daily`] — no CLI flag wires the other variants yet,
/// so only `Daily` is reachable from the shipped binary. The non-default
/// variants and [`FileSink::with_rotation`] are retained deliberately as
/// the public override surface for (a) library/embedding consumers of
/// `lean-observability` and (b) a future `--log.rotation` flag; they are
/// covered by `with_rotation`'s unit test, not dead code to be removed.
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
    ///
    /// Intentional public override surface — see [`LogRotation`]. The
    /// shipped binary always takes the [`LogRotation::Daily`] default from
    /// [`Self::new`]; this builder exists for library consumers and a
    /// future `--log.rotation` flag.
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

/// Process-wide one-shot guard: the first [`init_tracing`] caller claims
/// it and proceeds; every later caller (including threads racing the
/// first) observes the claim and returns
/// [`TracingInitError::AlreadyInitialized`] without building an appender.
/// This makes init race-safe: exactly one caller installs the subscriber,
/// and losers never create a stray file or a `WorkerGuard` whose drop
/// could tear down the winner's writer.
static INIT_CLAIMED: OnceLock<()> = OnceLock::new();

/// Errors raised by [`init_tracing`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TracingInitError {
    /// [`init_tracing`] was already called in this process (the
    /// one-shot [`INIT_CLAIMED`] slot was taken). Deterministic across
    /// concurrent callers — no dependence on subscriber-install timing.
    #[error("tracing subscriber already initialized")]
    AlreadyInitialized,

    /// Installing the global subscriber failed even though this caller
    /// won the init claim — a foreign subscriber was installed outside
    /// [`init_tracing`].
    #[error("install tracing subscriber: {0}")]
    Install(#[source] TryInitError),

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
///   writes to `<dir>/<prefix>.<date>.log`, rolling per
///   [`FileSink::rotation`].
/// - The `RUST_LOG` env var, when set and valid, supersedes `verbosity`
///   (standard `tracing-subscriber` precedence); a malformed `RUST_LOG`
///   warns once on stderr and falls back to `verbosity`.
///
/// # Errors
/// - [`TracingInitError::AlreadyInitialized`] if `init_tracing` was
///   already called in this process (deterministic across racing
///   callers — only the first wins).
/// - [`TracingInitError::CreateLogDir`] if `file_sink.dir` cannot be
///   created; [`TracingInitError::FileAppender`] if the rolling appender
///   cannot be built; [`TracingInitError::Install`] if a foreign
///   subscriber was already installed.
///
/// # Example
/// ```no_run
/// use runtime::observability::{init_tracing, Verbosity};
///
/// let _guard = init_tracing(Verbosity::Info, None)?;
/// tracing::info!("ready");
/// # Ok::<(), runtime::observability::TracingInitError>(())
/// ```
pub fn init_tracing(
    verbosity: Verbosity,
    file_sink: Option<FileSink<'_>>,
) -> Result<TracingGuard, TracingInitError> {
    // Claim the one-shot init slot before doing any work. Losing the
    // race (or a second call) returns immediately without creating an
    // appender, so only the winner ever opens a log file.
    if INIT_CLAIMED.set(()).is_err() {
        return Err(TracingInitError::AlreadyInitialized);
    }

    let filter = env_filter(verbosity);

    // Emit ANSI color escapes only when stderr is a real terminal.
    // Piped stderr (`lean-rust 2> file.log`) otherwise gets literal
    // escape bytes mixed into the log.
    let stderr_layer = fmt::layer()
        .with_ansi(stderr_ansi_enabled())
        .with_writer(io::stderr);

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
        .map_err(TracingInitError::Install)?;

    Ok(TracingGuard {
        _file_worker: file_worker,
    })
}

/// Which source the [`EnvFilter`] directive was resolved from. Returned
/// by [`build_filter`] so the caller can warn on the invalid-`RUST_LOG`
/// path without re-inspecting the env.
#[derive(Debug, PartialEq, Eq)]
enum FilterChoice {
    /// `RUST_LOG` was set and parsed cleanly.
    RustLog,
    /// `RUST_LOG` was set but failed to parse; fell back to `verbosity`.
    RustLogInvalid,
    /// `RUST_LOG` was unset/blank; used the `verbosity` directive.
    Verbosity,
}

/// Builds the [`EnvFilter`] used by [`init_tracing`].
///
/// `RUST_LOG` (the standard `tracing-subscriber` env var) wins when set
/// and parses; a malformed `RUST_LOG` emits one warning and falls back
/// to the `verbosity` directive instead of being silently swallowed by
/// `parse_lossy`. A runtime entry point should surface an operator typo,
/// not run with an unintended filter and no signal.
fn env_filter(verbosity: Verbosity) -> EnvFilter {
    let env = std::env::var(EnvFilter::DEFAULT_ENV).unwrap_or_default();
    let (filter, choice) = build_filter(&env, verbosity);
    if choice == FilterChoice::RustLogInvalid {
        // tracing is not installed yet — warn via stderr directly.
        eprintln!(
            "WARN lean-observability: RUST_LOG={env:?} is not a valid filter \
             directive; falling back to verbosity {verbosity}"
        );
    }
    filter
}

/// Resolves the directive source and parses it. Pure over its inputs (no
/// env read, no stderr) so the `RUST_LOG`-invalid fallback is unit
/// testable; [`env_filter`] reads the env and emits the warning.
fn build_filter(env: &str, verbosity: Verbosity) -> (EnvFilter, FilterChoice) {
    if !env.trim().is_empty() {
        match EnvFilter::builder().parse(env) {
            Ok(filter) => return (filter, FilterChoice::RustLog),
            Err(_) => {
                return (
                    EnvFilter::builder().parse_lossy(verbosity.directive()),
                    FilterChoice::RustLogInvalid,
                );
            }
        }
    }
    (
        EnvFilter::builder().parse_lossy(verbosity.directive()),
        FilterChoice::Verbosity,
    )
}

/// Whether the stderr `fmt` layer should emit ANSI color escapes:
/// only when stderr is connected to a terminal.
fn stderr_ansi_enabled() -> bool {
    ansi_enabled(io::stderr().is_terminal())
}

/// Pure ANSI-gating policy: color escapes are emitted only when the sink
/// is a terminal. Split out from [`stderr_ansi_enabled`] so the decision
/// is unit-testable without depending on the ambient stderr state (fd 2
/// is a TTY under an interactive `cargo test`, a pipe under CI).
fn ansi_enabled(is_terminal: bool) -> bool {
    is_terminal
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn build_filter_uses_verbosity_when_env_blank() {
        let (_f, choice) = build_filter("", Verbosity::Info);
        assert_eq!(choice, FilterChoice::Verbosity);
        let (_f, choice) = build_filter("   ", Verbosity::Info);
        assert_eq!(choice, FilterChoice::Verbosity);
    }

    #[test]
    fn build_filter_accepts_valid_rust_log() {
        let (_f, choice) = build_filter("debug", Verbosity::Info);
        assert_eq!(choice, FilterChoice::RustLog);
    }

    #[test]
    fn build_filter_falls_back_on_invalid_rust_log() {
        // `app=notalevel` has a valid target but an invalid level, so
        // `EnvFilter::parse` rejects it; build_filter must report the
        // fallback (env_filter turns this into a warn) rather than
        // silently dropping the filter.
        let (_f, choice) = build_filter("app=notalevel", Verbosity::Info);
        assert_eq!(choice, FilterChoice::RustLogInvalid);
    }

    #[test]
    fn ansi_gated_on_terminal_state() {
        // ANSI escapes must be emitted only for a terminal sink. Asserting
        // both branches of the pure policy keeps this deterministic: the
        // previous form called `stderr_ansi_enabled()`, whose result is
        // decided by fd 2 (a TTY under interactive `cargo test`, a pipe
        // under CI) rather than by the test.
        assert!(!ansi_enabled(false));
        assert!(ansi_enabled(true));
    }

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
