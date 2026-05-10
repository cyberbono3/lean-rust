//! Devnet0 [`Config`] struct and YAML loader.
//!
//! Mirrors the canonical chain-config shape used by the runtime. Field
//! order matches the canonical declaration order so YAML output is
//! byte-stable across releases.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use types::BasisPoint;

/// Devnet0 chain-configuration record.
///
/// All four `*_bps` fields are basis-point values: valid range `0..=10_000`.
/// Use [`Config::validate`] to enforce the invariant after deserialization.
///
/// # Example
/// ```
/// use config::{Config, ConfigError, DEVNET_CONFIG};
/// # fn main() -> Result<(), ConfigError> {
/// let cfg = Config::default();
/// assert_eq!(cfg, DEVNET_CONFIG);
/// cfg.validate()?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Slot duration in milliseconds.
    pub slot_duration_ms: u64,
    /// Slot duration in seconds (`slot_duration_ms / 1_000`).
    pub seconds_per_slot: u64,
    /// Number of slots examined for justification.
    pub justification_lookback_slots: u64,
    /// Proposer-reorg cutoff in basis points (`0..=10_000`).
    pub proposer_reorg_cutoff_bps: u64,
    /// Attestation deadline in basis points (`0..=10_000`).
    pub vote_due_bps: u64,
    /// Fast-confirm deadline in basis points (`0..=10_000`).
    pub fast_confirm_due_bps: u64,
    /// View-freeze cutoff in basis points (`0..=10_000`).
    pub view_freeze_cutoff_bps: u64,
    /// State historical-roots cap.
    pub historical_roots_limit: u64,
    /// Validator registry cap.
    pub validator_registry_limit: u64,
}

/// Canonical devnet0 [`Config`] preset.
///
/// Field values are inlined as literals — this is the single source of
/// truth for the devnet0 chain-config. Other crates read parameters via
/// `DEVNET_CONFIG.<field>` (e.g. `DEVNET_CONFIG.slot_duration_ms`).
pub const DEVNET_CONFIG: Config = Config {
    slot_duration_ms: 4_000,
    seconds_per_slot: 4,
    justification_lookback_slots: 3,
    proposer_reorg_cutoff_bps: 2_500,
    vote_due_bps: 5_000,
    fast_confirm_due_bps: 7_500,
    view_freeze_cutoff_bps: 7_500,
    historical_roots_limit: 1 << 18,
    validator_registry_limit: 1 << 12,
};

impl Default for Config {
    fn default() -> Self {
        DEVNET_CONFIG
    }
}

impl Config {
    /// Returns the four basis-point fields as `(name, value)` pairs.
    ///
    /// Single source of truth for which fields are basis points; both
    /// [`Config::validate`] and any future tooling should iterate this
    /// instead of repeating the field list.
    fn basis_point_fields(&self) -> [(&'static str, u64); 4] {
        [
            ("proposer_reorg_cutoff_bps", self.proposer_reorg_cutoff_bps),
            ("vote_due_bps", self.vote_due_bps),
            ("fast_confirm_due_bps", self.fast_confirm_due_bps),
            ("view_freeze_cutoff_bps", self.view_freeze_cutoff_bps),
        ]
    }

    /// Builds the [`ConfigError::SlotDurationMismatch`] for the current
    /// `(slot_duration_ms, seconds_per_slot)` pair.
    fn slot_duration_mismatch(&self) -> ConfigError {
        ConfigError::SlotDurationMismatch {
            slot_duration_ms: self.slot_duration_ms,
            seconds_per_slot: self.seconds_per_slot,
        }
    }

    /// Validates the four basis-point fields and the
    /// `slot_duration_ms == seconds_per_slot * 1_000` cross-field invariant.
    ///
    /// # Errors
    /// - [`ConfigError::BasisPointOutOfRange`] when any `*_bps` field
    ///   exceeds `10_000`.
    /// - [`ConfigError::SlotDurationMismatch`] when
    ///   `slot_duration_ms != seconds_per_slot * 1_000` (or the multiplication
    ///   overflows `u64`).
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (field, value) in self.basis_point_fields() {
            if BasisPoint::new(value).is_err() {
                return Err(ConfigError::BasisPointOutOfRange { field, value });
            }
        }
        let consistent = self
            .seconds_per_slot
            .checked_mul(1_000)
            .is_some_and(|derived_ms| derived_ms == self.slot_duration_ms);
        if !consistent {
            return Err(self.slot_duration_mismatch());
        }
        Ok(())
    }

    /// Loads a [`Config`] from a YAML string and validates it.
    ///
    /// # Errors
    /// - [`ConfigError::Yaml`] when the input is not valid YAML or has
    ///   missing / unknown fields.
    /// - Any error returned by [`Config::validate`].
    pub fn from_yaml(s: &str) -> Result<Self, ConfigError> {
        let cfg: Self = serde_yaml::from_str(s)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Serializes the [`Config`] to a YAML string.
    ///
    /// # Errors
    /// Returns [`ConfigError::Yaml`] if the underlying serializer fails. In
    /// practice this cannot happen for [`Config`]'s shape (all fields are
    /// `u64`), but the `Result` is preserved so the API stays stable if a
    /// non-trivial field is added later.
    pub fn to_yaml(&self) -> Result<String, ConfigError> {
        Ok(serde_yaml::to_string(self)?)
    }
}

/// Errors raised when loading or validating a [`Config`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The YAML input was malformed or had unknown / missing fields.
    #[error("invalid config YAML: {source}")]
    Yaml {
        /// Underlying serialization error.
        #[from]
        source: serde_yaml::Error,
    },

    /// A basis-point field was outside the `0..=10_000` range.
    #[error("config field `{field}` = {value} exceeds the basis-point maximum of 10_000")]
    BasisPointOutOfRange {
        /// Name of the offending field.
        field: &'static str,
        /// The out-of-range value.
        value: u64,
    },

    /// `slot_duration_ms` was not equal to `seconds_per_slot * 1_000`.
    #[error(
        "slot_duration_ms ({slot_duration_ms}) does not equal seconds_per_slot ({seconds_per_slot}) × 1_000"
    )]
    SlotDurationMismatch {
        /// `slot_duration_ms` field value.
        slot_duration_ms: u64,
        /// `seconds_per_slot` field value.
        seconds_per_slot: u64,
    },
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{INTERVALS_PER_SLOT, SECONDS_PER_INTERVAL};

    // ---------------------------------------------------------------------
    // Canonical values — single source-of-truth assertion
    // ---------------------------------------------------------------------

    #[test]
    fn devnet_config_matches_canonical_values() {
        assert_eq!(DEVNET_CONFIG.slot_duration_ms, 4_000);
        assert_eq!(DEVNET_CONFIG.seconds_per_slot, 4);
        assert_eq!(DEVNET_CONFIG.justification_lookback_slots, 3);
        assert_eq!(DEVNET_CONFIG.proposer_reorg_cutoff_bps, 2_500);
        assert_eq!(DEVNET_CONFIG.vote_due_bps, 5_000);
        assert_eq!(DEVNET_CONFIG.fast_confirm_due_bps, 7_500);
        assert_eq!(DEVNET_CONFIG.view_freeze_cutoff_bps, 7_500);
        assert_eq!(DEVNET_CONFIG.historical_roots_limit, 262_144);
        assert_eq!(DEVNET_CONFIG.validator_registry_limit, 4_096);
    }

    #[test]
    fn module_constants_match_canonical_values() {
        // Two values that are not on `Config` (forkchoice topology, not knobs).
        assert_eq!(INTERVALS_PER_SLOT, 4);
        assert_eq!(SECONDS_PER_INTERVAL, 1);
        // Derived invariant: SECONDS_PER_INTERVAL = seconds_per_slot / INTERVALS_PER_SLOT.
        assert_eq!(
            SECONDS_PER_INTERVAL,
            DEVNET_CONFIG.seconds_per_slot / INTERVALS_PER_SLOT
        );
    }

    #[test]
    fn default_equals_devnet_config() {
        assert_eq!(Config::default(), DEVNET_CONFIG);
    }

    #[test]
    fn devnet_config_validates_clean() {
        DEVNET_CONFIG.validate().unwrap();
    }

    // ---------------------------------------------------------------------
    // YAML round-trip
    // ---------------------------------------------------------------------

    #[test]
    fn round_trips_default_via_yaml() {
        let cfg = Config::default();
        let yaml = cfg.to_yaml().unwrap();
        let parsed = Config::from_yaml(&yaml).unwrap();
        assert_eq!(parsed, cfg);
    }

    #[test]
    fn yaml_output_is_stable_field_order() {
        let yaml = Config::default().to_yaml().unwrap();
        // First non-whitespace line must be slot_duration_ms (declaration order).
        let first_field = yaml.lines().find(|l| !l.trim().is_empty()).unwrap().trim();
        assert!(
            first_field.starts_with("slot_duration_ms:"),
            "first field was {first_field:?}"
        );
    }

    // ---------------------------------------------------------------------
    // Validation — basis-point range
    // ---------------------------------------------------------------------

    #[test]
    fn rejects_basis_point_above_max() {
        let mut bad = DEVNET_CONFIG;
        bad.vote_due_bps = 10_001;
        let err = bad.validate().unwrap_err();
        match err {
            ConfigError::BasisPointOutOfRange { field, value } => {
                assert_eq!(field, "vote_due_bps");
                assert_eq!(value, 10_001);
            }
            other => panic!("unexpected ConfigError variant: {other:?}"),
        }
    }

    #[test]
    fn rejects_basis_point_proposer_reorg_above_max() {
        let mut bad = DEVNET_CONFIG;
        bad.proposer_reorg_cutoff_bps = 50_000;
        let err = bad.validate().unwrap_err();
        match err {
            ConfigError::BasisPointOutOfRange { field, value } => {
                assert_eq!(field, "proposer_reorg_cutoff_bps");
                assert_eq!(value, 50_000);
            }
            other => panic!("unexpected ConfigError variant: {other:?}"),
        }
    }

    #[test]
    fn accepts_basis_point_at_max_boundary() {
        let mut cfg = DEVNET_CONFIG;
        cfg.vote_due_bps = 10_000;
        cfg.fast_confirm_due_bps = 10_000;
        cfg.view_freeze_cutoff_bps = 10_000;
        cfg.proposer_reorg_cutoff_bps = 10_000;
        cfg.validate().unwrap();
    }

    // ---------------------------------------------------------------------
    // Validation — slot-duration cross-field invariant
    // ---------------------------------------------------------------------

    #[test]
    fn rejects_slot_duration_mismatch() {
        let mut bad = DEVNET_CONFIG;
        bad.seconds_per_slot = 5; // 5 * 1000 = 5000 != slot_duration_ms (4000)
        let err = bad.validate().unwrap_err();
        match err {
            ConfigError::SlotDurationMismatch {
                slot_duration_ms,
                seconds_per_slot,
            } => {
                assert_eq!(slot_duration_ms, 4_000);
                assert_eq!(seconds_per_slot, 5);
            }
            other => panic!("unexpected ConfigError variant: {other:?}"),
        }
    }

    #[test]
    fn rejects_slot_duration_overflow() {
        let mut bad = DEVNET_CONFIG;
        bad.seconds_per_slot = u64::MAX;
        let err = bad.validate().unwrap_err();
        assert!(matches!(err, ConfigError::SlotDurationMismatch { .. }));
    }

    // ---------------------------------------------------------------------
    // YAML deserializer surface
    // ---------------------------------------------------------------------

    #[test]
    fn from_yaml_rejects_unknown_field() {
        let cfg_yaml = Config::default().to_yaml().unwrap();
        let with_extra = format!("{cfg_yaml}rogue_field: 42\n");
        let err = Config::from_yaml(&with_extra).unwrap_err();
        assert!(matches!(err, ConfigError::Yaml { .. }));
    }

    #[test]
    fn from_yaml_rejects_missing_field() {
        let yaml = "slot_duration_ms: 4000\n";
        let err = Config::from_yaml(yaml).unwrap_err();
        assert!(matches!(err, ConfigError::Yaml { .. }));
    }

    #[test]
    fn from_yaml_propagates_validation_error() {
        let cfg_yaml = Config::default().to_yaml().unwrap();
        let bumped = cfg_yaml.replace("vote_due_bps: 5000", "vote_due_bps: 99999");
        let err = Config::from_yaml(&bumped).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::BasisPointOutOfRange {
                field: "vote_due_bps",
                value: 99_999
            }
        ));
    }
}
