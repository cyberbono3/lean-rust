//! Configuration for the duties [`Service`](super::Service).
//!
//! "Parse, don't validate" is enforced at the **type** level: every
//! field of [`Config`] is an always-valid newtype ([`ValidatorsPath`],
//! [`ValidatorGroup`], [`GenesisTimeUnix`]) whose constructor encodes
//! the invariant. The infallible [`Config::new`] then takes pre-typed
//! arguments; the convenience [`Config::try_new`] accepts loose input
//! and routes it through the newtype constructors. There is no
//! separate `validate()` step — invalid state cannot be constructed.
//!
//! The same idiom is used by [`crate::sync::Config`], whose
//! `max_sync_depth` is a [`core::num::NonZeroUsize`].

use std::fmt;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use super::error::{DutiesError, DutiesResult};

/// Repository-relative default for the devnet0 validator-assignment
/// file. Resolved against the `lean-chain` crate root when fed to
/// [`super::ValidatorAssignments::load`].
pub const DEFAULT_VALIDATORS_PATH: &str = "internal/testdata/devnet0/validators.yaml";

/// Default local validator group selector for devnet0.
pub const DEFAULT_VALIDATOR_GROUP: &str = "ream";

/// Default slot duration in milliseconds, sourced from the canonical
/// devnet0 preset (`config::DEVNET_CONFIG.slot_duration_ms` = 4000).
///
/// Modelled as a [`NonZeroU64`] so the scheduler's `elapsed /
/// slot_duration` math can never divide by zero. The `const` `match`
/// fires a compile-time panic if the preset is ever set to zero —
/// `NonZeroU64::new(0)` is `None`, which is unreachable for a valid
/// devnet config.
pub const DEFAULT_SLOT_DURATION_MS: NonZeroU64 =
    match NonZeroU64::new(config::DEVNET_CONFIG.slot_duration_ms) {
        Some(v) => v,
        None => panic!("DEVNET_CONFIG.slot_duration_ms must be non-zero"),
    };

// ============================================================================
// GenesisTimeUnix — always-valid Unix-epoch wrapper
// ============================================================================

/// Strongly typed UNIX-epoch timestamp in seconds.
///
/// Any `u64` is a valid timestamp, so the newtype has no fallible
/// constructor — it exists purely to keep "seconds since 1970"
/// structurally distinct from any other `u64`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GenesisTimeUnix(u64);

impl GenesisTimeUnix {
    /// Unix epoch (`0`). Equivalent to `GenesisTimeUnix::new(0)`,
    /// mirroring the `Slot::ZERO` / `Slot::ONE` constants in
    /// [`protocol`].
    pub const EPOCH: Self = Self(0);

    /// Builds a [`GenesisTimeUnix`] from a raw `u64`.
    #[must_use]
    pub const fn new(seconds: u64) -> Self {
        Self(seconds)
    }

    /// Returns the underlying `u64`.
    #[must_use]
    pub const fn as_secs(self) -> u64 {
        self.0
    }

    /// Returns the timestamp as a [`std::time::Duration`] since the
    /// Unix epoch. Useful for direct subtraction against
    /// `SystemTime::now()`.
    #[must_use]
    pub const fn to_duration(self) -> std::time::Duration {
        std::time::Duration::from_secs(self.0)
    }
}

impl From<u64> for GenesisTimeUnix {
    fn from(seconds: u64) -> Self {
        Self::new(seconds)
    }
}

impl From<GenesisTimeUnix> for std::time::Duration {
    fn from(ts: GenesisTimeUnix) -> Self {
        ts.to_duration()
    }
}

impl fmt::Display for GenesisTimeUnix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ============================================================================
// ValidatorsPath — non-empty path newtype
// ============================================================================

/// Filesystem path to a validator-assignment YAML file.
///
/// Guaranteed non-empty (rejects empty and whitespace-only inputs at
/// construction). Wraps [`PathBuf`] so callers cannot construct an
/// invalid path inline.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ValidatorsPath(PathBuf);

impl ValidatorsPath {
    /// Builds a [`ValidatorsPath`] from any path-like input.
    ///
    /// # Errors
    /// [`DutiesError::EmptyValidatorsPath`] when the input is empty or
    /// whitespace-only.
    #[must_use = "constructing a ValidatorsPath without using it is almost always a bug"]
    pub fn new(path: impl Into<PathBuf>) -> DutiesResult<Self> {
        let path = path.into();
        if path_is_blank(&path) {
            return Err(DutiesError::EmptyValidatorsPath);
        }
        Ok(Self(path))
    }

    /// Returns the path as a [`&Path`].
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Returns the path as a [`&PathBuf`]. Useful when callers need to
    /// pass `&PathBuf` to APIs that accept it by reference.
    #[must_use]
    pub const fn as_path_buf(&self) -> &PathBuf {
        &self.0
    }

    /// Consumes the wrapper and returns the underlying [`PathBuf`].
    #[must_use]
    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

impl AsRef<Path> for ValidatorsPath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl FromStr for ValidatorsPath {
    type Err = DutiesError;

    /// Parses a [`ValidatorsPath`] from a string slice, enabling
    /// `"path".parse::<ValidatorsPath>()`. Delegates to [`Self::new`].
    fn from_str(s: &str) -> DutiesResult<Self> {
        Self::new(s)
    }
}

impl fmt::Display for ValidatorsPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

// ============================================================================
// ValidatorGroup — non-empty, trimmed group-name newtype
// ============================================================================

/// Local validator group selector — the YAML key whose validators this
/// node schedules.
///
/// Guaranteed non-empty and trimmed; the inner string never holds
/// leading / trailing whitespace.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ValidatorGroup(String);

impl ValidatorGroup {
    /// Builds a [`ValidatorGroup`] from any string-like input,
    /// trimming whitespace.
    ///
    /// # Errors
    /// [`DutiesError::EmptyValidatorGroup`] when the input trims to
    /// the empty string.
    #[must_use = "constructing a ValidatorGroup without using it is almost always a bug"]
    pub fn new(raw: impl Into<String>) -> DutiesResult<Self> {
        let raw = raw.into();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DutiesError::EmptyValidatorGroup);
        }
        // Avoid an allocation when the input was already canonical.
        Ok(Self(if trimmed.len() == raw.len() {
            raw
        } else {
            trimmed.to_owned()
        }))
    }

    /// Returns the group name as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the wrapper and returns the underlying [`String`].
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for ValidatorGroup {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for ValidatorGroup {
    type Err = DutiesError;

    /// Parses a [`ValidatorGroup`] from a string slice, enabling
    /// `"ream".parse::<ValidatorGroup>()`. Delegates to [`Self::new`].
    fn from_str(s: &str) -> DutiesResult<Self> {
        Self::new(s)
    }
}

impl fmt::Display for ValidatorGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ============================================================================
// Config
// ============================================================================

/// Narrow devnet0 duties service configuration.
///
/// Fields are private and accessed through `validators_path()`,
/// `validator_group()`, and `genesis_time_unix()`. All field types are
/// always-valid newtypes, so the struct can never hold invalid data.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
#[must_use]
pub struct Config {
    validators_path: ValidatorsPath,
    validator_group: ValidatorGroup,
    genesis_time_unix: GenesisTimeUnix,
    /// Slot duration in milliseconds. [`NonZeroU64`] so the scheduler
    /// cannot divide by zero. Defaults to [`DEFAULT_SLOT_DURATION_MS`];
    /// override through [`Config::with_slot_duration_ms`].
    slot_duration_ms: NonZeroU64,
}

impl Config {
    /// Builds a configuration from pre-typed inputs. Infallible: every
    /// argument is an always-valid newtype. `slot_duration_ms` is seeded
    /// to [`DEFAULT_SLOT_DURATION_MS`]; override via
    /// [`Self::with_slot_duration_ms`].
    #[must_use = "building a Config without using it discards the construction"]
    pub const fn new(
        validators_path: ValidatorsPath,
        validator_group: ValidatorGroup,
        genesis_time_unix: GenesisTimeUnix,
    ) -> Self {
        Self {
            validators_path,
            validator_group,
            genesis_time_unix,
            slot_duration_ms: DEFAULT_SLOT_DURATION_MS,
        }
    }

    /// Convenience: builds a [`Config`] from loose inputs, routing
    /// them through the newtype constructors.
    ///
    /// # Errors
    /// Forwards every variant raised by [`ValidatorsPath::new`] /
    /// [`ValidatorGroup::new`].
    #[must_use = "building a Config without using it discards the construction"]
    pub fn try_new(
        validators_path: impl Into<PathBuf>,
        validator_group: impl Into<String>,
        genesis_time_unix: GenesisTimeUnix,
    ) -> DutiesResult<Self> {
        Ok(Self::new(
            ValidatorsPath::new(validators_path)?,
            ValidatorGroup::new(validator_group)?,
            genesis_time_unix,
        ))
    }

    /// Returns the validator-assignment file path.
    #[must_use]
    pub fn validators_path(&self) -> &Path {
        self.validators_path.as_path()
    }

    /// Returns the local validator group name.
    #[must_use]
    pub fn validator_group(&self) -> &str {
        self.validator_group.as_str()
    }

    /// Returns the chain-genesis timestamp.
    #[must_use]
    pub const fn genesis_time_unix(&self) -> GenesisTimeUnix {
        self.genesis_time_unix
    }

    /// Returns the configured slot duration in milliseconds.
    #[must_use]
    pub const fn slot_duration_ms(&self) -> NonZeroU64 {
        self.slot_duration_ms
    }

    /// Validates the cross-field invariants that cannot be encoded in
    /// the field newtypes alone — currently that genesis time has been
    /// set away from the Unix epoch. Called by
    /// [`super::Service::start`] before the scheduler spawns.
    ///
    /// `slot_duration_ms` needs no check here: it is a [`NonZeroU64`],
    /// so the zero case is unrepresentable.
    ///
    /// # Errors
    /// [`DutiesError::GenesisTimeUnset`] when `genesis_time_unix` is
    /// still [`GenesisTimeUnix::EPOCH`].
    pub fn ensure_runnable(&self) -> DutiesResult<()> {
        if self.genesis_time_unix == GenesisTimeUnix::EPOCH {
            return Err(DutiesError::GenesisTimeUnset);
        }
        Ok(())
    }

    /// Returns a copy with `validators_path` overridden, routing the
    /// input through [`ValidatorsPath::new`] so the invariant survives.
    ///
    /// # Errors
    /// As [`ValidatorsPath::new`].
    #[must_use = "builder returns a new Config — discarding it drops your override"]
    pub fn with_validators_path(mut self, path: impl Into<PathBuf>) -> DutiesResult<Self> {
        self.validators_path = ValidatorsPath::new(path)?;
        Ok(self)
    }

    /// Returns a copy with `validator_group` overridden, routing the
    /// input through [`ValidatorGroup::new`].
    ///
    /// # Errors
    /// As [`ValidatorGroup::new`].
    #[must_use = "builder returns a new Config — discarding it drops your override"]
    pub fn with_validator_group(mut self, group: impl Into<String>) -> DutiesResult<Self> {
        self.validator_group = ValidatorGroup::new(group)?;
        Ok(self)
    }

    /// Returns a copy with `genesis_time_unix` overridden. Infallible:
    /// every [`GenesisTimeUnix`] is structurally valid.
    #[must_use = "builder returns a new Config — discarding it drops your override"]
    pub const fn with_genesis_time_unix(mut self, genesis_time_unix: GenesisTimeUnix) -> Self {
        self.genesis_time_unix = genesis_time_unix;
        self
    }

    /// Returns a copy with `slot_duration_ms` overridden, rejecting a
    /// zero value at the loose-input boundary (the stored field is a
    /// [`NonZeroU64`]).
    ///
    /// Accepts any non-zero value — in particular the spec's devnet0
    /// value of `4000`. Only `0` is rejected.
    ///
    /// # Errors
    /// [`DutiesError::ZeroSlotDuration`] when `slot_duration_ms` is `0`.
    #[must_use = "builder returns a new Config — discarding it drops your override"]
    pub fn with_slot_duration_ms(mut self, slot_duration_ms: u64) -> DutiesResult<Self> {
        self.slot_duration_ms =
            NonZeroU64::new(slot_duration_ms).ok_or(DutiesError::ZeroSlotDuration)?;
        Ok(self)
    }
}

impl Default for Config {
    /// Always-valid defaults seeded from [`DEFAULT_VALIDATORS_PATH`]
    /// and [`DEFAULT_VALIDATOR_GROUP`].
    ///
    /// Routes through the public [`ValidatorsPath::new`] and
    /// [`ValidatorGroup::new`] constructors so that any future edit
    /// that introduces an invalid default (an empty const string)
    /// fires the `unreachable!()` arm at test time instead of silently
    /// shipping an "invalid" [`Config`] that would fail [`Self::try_new`].
    fn default() -> Self {
        Self::new(
            default_validators_path(),
            default_validator_group(),
            GenesisTimeUnix::EPOCH,
        )
    }
}

fn default_validators_path() -> ValidatorsPath {
    ValidatorsPath::new(DEFAULT_VALIDATORS_PATH)
        .unwrap_or_else(|_| unreachable!("DEFAULT_VALIDATORS_PATH must be non-empty"))
}

fn default_validator_group() -> ValidatorGroup {
    ValidatorGroup::new(DEFAULT_VALIDATOR_GROUP)
        .unwrap_or_else(|_| unreachable!("DEFAULT_VALIDATOR_GROUP must be non-empty"))
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "duties config(group={}, validators={}, genesis_unix={})",
            self.validator_group, self.validators_path, self.genesis_time_unix,
        )
    }
}

// ============================================================================
// helpers
// ============================================================================

fn path_is_blank(path: &Path) -> bool {
    if path.as_os_str().is_empty() {
        return true;
    }
    // Preserve the whitespace-only rejection from the previous
    // implementation. Non-UTF-8 paths cannot be whitespace-only under
    // Unicode rules (they hold non-printable bytes that aren't
    // whitespace), so a missing UTF-8 view is fine.
    match path.to_str() {
        Some(s) => s.trim().is_empty(),
        None => false,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // -- GenesisTimeUnix ----------------------------------------------------

    #[test]
    fn genesis_time_unix_epoch_is_zero() {
        assert_eq!(GenesisTimeUnix::EPOCH.as_secs(), 0);
        assert_eq!(GenesisTimeUnix::EPOCH, GenesisTimeUnix::new(0));
    }

    #[test]
    fn genesis_time_unix_from_u64() {
        let g: GenesisTimeUnix = 7_u64.into();
        assert_eq!(g.as_secs(), 7);
    }

    #[test]
    fn genesis_time_unix_into_duration() {
        let g = GenesisTimeUnix::new(42);
        let d: std::time::Duration = g.into();
        assert_eq!(d, std::time::Duration::from_secs(42));
    }

    // -- ValidatorsPath -----------------------------------------------------

    #[test]
    fn validators_path_accepts_well_formed_input() {
        let p = ValidatorsPath::new("fixtures/validators.yaml").unwrap();
        assert_eq!(p.as_path(), Path::new("fixtures/validators.yaml"));
    }

    #[test]
    fn validators_path_rejects_empty() {
        let err = ValidatorsPath::new("").unwrap_err();
        assert!(matches!(err, DutiesError::EmptyValidatorsPath));
    }

    #[test]
    fn validators_path_rejects_whitespace() {
        let err = ValidatorsPath::new("   ").unwrap_err();
        assert!(matches!(err, DutiesError::EmptyValidatorsPath));
    }

    #[test]
    fn validators_path_parse_from_str() {
        let p: ValidatorsPath = "fixtures/validators.yaml".parse().unwrap();
        assert_eq!(p.as_path(), Path::new("fixtures/validators.yaml"));
    }

    #[test]
    fn validators_path_parse_rejects_empty() {
        let err = "".parse::<ValidatorsPath>().unwrap_err();
        assert!(matches!(err, DutiesError::EmptyValidatorsPath));
    }

    #[test]
    fn validators_path_is_hashable() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(ValidatorsPath::new("a").unwrap());
        set.insert(ValidatorsPath::new("a").unwrap());
        set.insert(ValidatorsPath::new("b").unwrap());
        assert_eq!(set.len(), 2);
    }

    // -- ValidatorGroup -----------------------------------------------------

    #[test]
    fn validator_group_accepts_well_formed_input() {
        let g = ValidatorGroup::new("ream").unwrap();
        assert_eq!(g.as_str(), "ream");
    }

    #[test]
    fn validator_group_trims_whitespace() {
        let g = ValidatorGroup::new("  ream  ").unwrap();
        assert_eq!(g.as_str(), "ream");
    }

    #[test]
    fn validator_group_rejects_empty() {
        let err = ValidatorGroup::new("").unwrap_err();
        assert!(matches!(err, DutiesError::EmptyValidatorGroup));
    }

    #[test]
    fn validator_group_rejects_whitespace_only() {
        let err = ValidatorGroup::new("   ").unwrap_err();
        assert!(matches!(err, DutiesError::EmptyValidatorGroup));
    }

    #[test]
    fn validator_group_parse_from_str() {
        let g: ValidatorGroup = "ream".parse().unwrap();
        assert_eq!(g.as_str(), "ream");
    }

    #[test]
    fn validator_group_parse_trims_whitespace() {
        let g: ValidatorGroup = "  ream  ".parse().unwrap();
        assert_eq!(g.as_str(), "ream");
    }

    #[test]
    fn validator_group_parse_rejects_empty() {
        let err = "   ".parse::<ValidatorGroup>().unwrap_err();
        assert!(matches!(err, DutiesError::EmptyValidatorGroup));
    }

    // -- Config -------------------------------------------------------------

    #[test]
    fn default_is_always_valid() {
        let cfg = Config::default();
        assert_eq!(cfg.validators_path(), Path::new(DEFAULT_VALIDATORS_PATH));
        assert_eq!(cfg.validator_group(), DEFAULT_VALIDATOR_GROUP);
        assert_eq!(cfg.genesis_time_unix(), GenesisTimeUnix::EPOCH);
    }

    #[test]
    fn default_matches_try_new_with_constants() {
        // Regression guard: `Default` routes through `Config::new` via
        // `default_validators_path()` / `default_validator_group()`, so
        // it cannot drift away from the public constructor. If anyone
        // ever changes a `DEFAULT_*` constant to something the
        // newtype constructor rejects, the `unreachable!()` arm fires
        // here at test time instead of shipping silently.
        let from_default = Config::default();
        let from_try_new = Config::try_new(
            DEFAULT_VALIDATORS_PATH,
            DEFAULT_VALIDATOR_GROUP,
            GenesisTimeUnix::EPOCH,
        )
        .unwrap();
        assert_eq!(from_default, from_try_new);
    }

    #[test]
    fn try_new_accepts_loose_inputs() {
        let cfg = Config::try_new(
            "fixtures/validators.yaml",
            "ream",
            GenesisTimeUnix::new(1_700_000_000),
        )
        .unwrap();
        assert_eq!(cfg.validators_path(), Path::new("fixtures/validators.yaml"));
        assert_eq!(cfg.validator_group(), "ream");
        assert_eq!(cfg.genesis_time_unix().as_secs(), 1_700_000_000);
    }

    #[test]
    fn try_new_trims_group_whitespace() {
        let cfg = Config::try_new("p", "  ream  ", GenesisTimeUnix::EPOCH).unwrap();
        assert_eq!(cfg.validator_group(), "ream");
    }

    #[test]
    fn try_new_rejects_empty_path() {
        let err = Config::try_new("", "ream", GenesisTimeUnix::EPOCH).unwrap_err();
        assert!(matches!(err, DutiesError::EmptyValidatorsPath));
    }

    #[test]
    fn try_new_rejects_empty_group() {
        let err = Config::try_new("p", "", GenesisTimeUnix::EPOCH).unwrap_err();
        assert!(matches!(err, DutiesError::EmptyValidatorGroup));
    }

    #[test]
    fn new_is_infallible_with_typed_inputs() {
        let cfg = Config::new(
            ValidatorsPath::new("p").unwrap(),
            ValidatorGroup::new("ream").unwrap(),
            GenesisTimeUnix::new(99),
        );
        assert_eq!(cfg.validator_group(), "ream");
        assert_eq!(cfg.genesis_time_unix().as_secs(), 99);
    }

    #[test]
    fn with_validators_path_re_checks_invariant() {
        let err = Config::default().with_validators_path("").unwrap_err();
        assert!(matches!(err, DutiesError::EmptyValidatorsPath));
    }

    #[test]
    fn with_validator_group_re_checks_invariant() {
        let err = Config::default().with_validator_group("  ").unwrap_err();
        assert!(matches!(err, DutiesError::EmptyValidatorGroup));
    }

    #[test]
    fn with_genesis_time_unix_is_infallible() {
        let cfg = Config::default().with_genesis_time_unix(GenesisTimeUnix::new(42));
        assert_eq!(cfg.genesis_time_unix().as_secs(), 42);
    }

    // -- slot_duration_ms ---------------------------------------------------

    #[test]
    fn default_slot_duration_is_devnet_value() {
        assert_eq!(Config::default().slot_duration_ms().get(), 4_000);
        assert_eq!(DEFAULT_SLOT_DURATION_MS.get(), 4_000);
    }

    #[test]
    fn with_slot_duration_ms_accepts_spec_value() {
        // Spec guardrail: 4000 ms must be accepted, not rejected.
        let cfg = Config::default().with_slot_duration_ms(4_000).unwrap();
        assert_eq!(cfg.slot_duration_ms().get(), 4_000);
    }

    #[test]
    fn with_slot_duration_ms_accepts_arbitrary_non_zero() {
        let cfg = Config::default().with_slot_duration_ms(1).unwrap();
        assert_eq!(cfg.slot_duration_ms().get(), 1);
    }

    #[test]
    fn with_slot_duration_ms_rejects_zero() {
        let err = Config::default().with_slot_duration_ms(0).unwrap_err();
        assert!(matches!(err, DutiesError::ZeroSlotDuration), "got {err:?}");
    }

    // -- ensure_runnable (genesis guard) ------------------------------------

    #[test]
    fn ensure_runnable_rejects_epoch_genesis() {
        // `Config::default()` seeds `GenesisTimeUnix::EPOCH`.
        let err = Config::default().ensure_runnable().unwrap_err();
        assert!(matches!(err, DutiesError::GenesisTimeUnset), "got {err:?}");
    }

    #[test]
    fn ensure_runnable_accepts_real_genesis() {
        let cfg = Config::default().with_genesis_time_unix(GenesisTimeUnix::new(1_700_000_000));
        cfg.ensure_runnable().unwrap();
    }

    #[test]
    fn display_contains_all_fields() {
        let cfg = Config::default().with_genesis_time_unix(GenesisTimeUnix::new(99));
        let s = format!("{cfg}");
        assert!(s.contains("ream"), "got {s}");
        assert!(s.contains("validators.yaml"), "got {s}");
        assert!(s.contains("99"), "got {s}");
    }
}
