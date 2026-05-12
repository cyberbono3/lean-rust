//! [`Verbosity`] enum + `EnvFilter` directive builder.
//!
//! Maps a coarse-grained verbosity level (typically driven by a
//! `--verbosity` CLI flag) to a `tracing-subscriber` `EnvFilter`
//! directive string that silences known-noisy ecosystem crates.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// Coarse-grained verbosity level for the runtime shell.
///
/// Maps 1..=5 / `error`..=`trace` to a [`tracing_subscriber::EnvFilter`]
/// directive via [`Verbosity::directive`]. The directive silences known-
/// noisy ecosystem crates (`libp2p_swarm`, `discv5`) at coarse levels so
/// `Info` doesn't drown in transport chatter.
///
/// Variants are ordered from least to most verbose; comparison operators
/// follow that order (`Verbosity::Info < Verbosity::Debug`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Verbosity {
    /// Only error-level events.
    Error,
    /// Warnings and errors.
    Warn,
    /// Info, warnings, and errors (default).
    Info,
    /// Debug and above; drops the per-target silencers.
    Debug,
    /// All events; drops the per-target silencers.
    Trace,
}

impl Verbosity {
    /// Returns the [`tracing_subscriber::EnvFilter`] directive for this
    /// verbosity.
    ///
    /// Coarse levels (`Error`/`Warn`/`Info`) silence known-noisy crates
    /// to keep the default output readable. `Debug` and `Trace` drop the
    /// silencers — when you ask for everything, you get everything.
    #[must_use]
    pub const fn directive(self) -> &'static str {
        match self {
            Self::Error => "error,libp2p_swarm=warn,discv5=error",
            Self::Warn => "warn,libp2p_swarm=warn,discv5=error",
            Self::Info => "info,libp2p_swarm=warn,discv5=error",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }

    /// Returns the canonical lowercase name (`"error"`, …, `"trace"`).
    /// Used by both [`fmt::Display`] and the [`tracing::Level`] bridge.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

impl Default for Verbosity {
    fn default() -> Self {
        Self::Info
    }
}

impl fmt::Display for Verbosity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<Verbosity> for tracing::Level {
    fn from(v: Verbosity) -> Self {
        match v {
            Verbosity::Error => Self::ERROR,
            Verbosity::Warn => Self::WARN,
            Verbosity::Info => Self::INFO,
            Verbosity::Debug => Self::DEBUG,
            Verbosity::Trace => Self::TRACE,
        }
    }
}

/// Error returned when [`Verbosity::from_str`] cannot parse its input.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("invalid verbosity '{0}': expected one of 1..=5 or error|warn|info|debug|trace")]
pub struct ParseVerbosityError(pub String);

impl FromStr for Verbosity {
    type Err = ParseVerbosityError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "1" | "error" => Ok(Self::Error),
            "2" | "warn" => Ok(Self::Warn),
            "3" | "info" => Ok(Self::Info),
            "4" | "debug" => Ok(Self::Debug),
            "5" | "trace" => Ok(Self::Trace),
            _ => Err(ParseVerbosityError(s.to_owned())),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use tracing_subscriber::EnvFilter;

    #[test]
    fn directive_parses_for_every_variant() {
        for v in [
            Verbosity::Error,
            Verbosity::Warn,
            Verbosity::Info,
            Verbosity::Debug,
            Verbosity::Trace,
        ] {
            EnvFilter::builder()
                .parse(v.directive())
                .unwrap_or_else(|e| panic!("{v:?}: {e}"));
        }
    }

    #[test]
    fn from_str_accepts_numeric_and_named() {
        let cases = [
            ("1", Verbosity::Error),
            ("error", Verbosity::Error),
            ("ERROR", Verbosity::Error),
            ("2", Verbosity::Warn),
            ("warn", Verbosity::Warn),
            ("3", Verbosity::Info),
            ("info", Verbosity::Info),
            ("4", Verbosity::Debug),
            ("debug", Verbosity::Debug),
            ("5", Verbosity::Trace),
            ("trace", Verbosity::Trace),
        ];
        for (input, expected) in cases {
            assert_eq!(
                Verbosity::from_str(input).unwrap(),
                expected,
                "input {input}"
            );
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        let err = Verbosity::from_str("foo").unwrap_err();
        assert_eq!(err, ParseVerbosityError("foo".to_owned()));
    }

    #[test]
    fn default_is_info() {
        assert_eq!(Verbosity::default(), Verbosity::Info);
    }

    #[test]
    fn display_round_trips_through_from_str() {
        for v in [
            Verbosity::Error,
            Verbosity::Warn,
            Verbosity::Info,
            Verbosity::Debug,
            Verbosity::Trace,
        ] {
            assert_eq!(Verbosity::from_str(&v.to_string()).unwrap(), v);
        }
    }

    #[test]
    fn ordering_runs_least_to_most_verbose() {
        assert!(Verbosity::Error < Verbosity::Warn);
        assert!(Verbosity::Warn < Verbosity::Info);
        assert!(Verbosity::Info < Verbosity::Debug);
        assert!(Verbosity::Debug < Verbosity::Trace);
    }

    #[test]
    fn into_tracing_level_maps_each_variant() {
        assert_eq!(
            tracing::Level::from(Verbosity::Error),
            tracing::Level::ERROR
        );
        assert_eq!(tracing::Level::from(Verbosity::Warn), tracing::Level::WARN);
        assert_eq!(tracing::Level::from(Verbosity::Info), tracing::Level::INFO);
        assert_eq!(
            tracing::Level::from(Verbosity::Debug),
            tracing::Level::DEBUG
        );
        assert_eq!(
            tracing::Level::from(Verbosity::Trace),
            tracing::Level::TRACE
        );
    }
}
