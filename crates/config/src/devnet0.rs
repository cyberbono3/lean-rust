//! Devnet0 [`Config`] struct and YAML loader.
//!
//! Mirrors the canonical chain-config shape used by the runtime. Field
//! order matches the canonical declaration order so YAML output is
//! byte-stable across releases.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use types::BasisPoint;

use crate::{
    FAST_CONFIRM_DUE_BPS, HISTORICAL_ROOTS_LIMIT, JUSTIFICATION_LOOKBACK_SLOTS,
    PROPOSER_REORG_CUTOFF_BPS, SECONDS_PER_SLOT, SLOT_DURATION_MS, VALIDATOR_REGISTRY_LIMIT,
    VIEW_FREEZE_CUTOFF_BPS, VOTE_DUE_BPS,
};

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
pub const DEVNET_CONFIG: Config = Config {
    slot_duration_ms: SLOT_DURATION_MS,
    seconds_per_slot: SECONDS_PER_SLOT,
    justification_lookback_slots: JUSTIFICATION_LOOKBACK_SLOTS,
    proposer_reorg_cutoff_bps: PROPOSER_REORG_CUTOFF_BPS,
    vote_due_bps: VOTE_DUE_BPS,
    fast_confirm_due_bps: FAST_CONFIRM_DUE_BPS,
    view_freeze_cutoff_bps: VIEW_FREEZE_CUTOFF_BPS,
    historical_roots_limit: HISTORICAL_ROOTS_LIMIT,
    validator_registry_limit: VALIDATOR_REGISTRY_LIMIT,
};

impl Default for Config {
    fn default() -> Self {
        DEVNET_CONFIG
    }
}

impl Config {
    /// Validates the four basis-point fields and the
    /// `slot_duration_ms == seconds_per_slot * 1_000` cross-field invariant.
    ///
    /// # Errors
    /// - [`ConfigError::BasisPointOutOfRange`] when any `*_bps` field
    ///   exceeds `10_000`.
    /// - [`ConfigError::SlotDurationMismatch`] when
    ///   `slot_duration_ms != seconds_per_slot * 1_000`.
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (field, value) in [
            ("proposer_reorg_cutoff_bps", self.proposer_reorg_cutoff_bps),
            ("vote_due_bps", self.vote_due_bps),
            ("fast_confirm_due_bps", self.fast_confirm_due_bps),
            ("view_freeze_cutoff_bps", self.view_freeze_cutoff_bps),
        ] {
            if BasisPoint::new(value).is_err() {
                return Err(ConfigError::BasisPointOutOfRange { field, value });
            }
        }
        let derived_ms =
            self.seconds_per_slot
                .checked_mul(1_000)
                .ok_or(ConfigError::SlotDurationMismatch {
                    slot_duration_ms: self.slot_duration_ms,
                    seconds_per_slot: self.seconds_per_slot,
                })?;
        if derived_ms != self.slot_duration_ms {
            return Err(ConfigError::SlotDurationMismatch {
                slot_duration_ms: self.slot_duration_ms,
                seconds_per_slot: self.seconds_per_slot,
            });
        }
        Ok(())
    }

    /// Loads a [`Config`] from a YAML string and validates it.
    ///
    /// # Errors
    /// - [`ConfigError::Yaml`] when the input is not valid YAML or has
    ///   missing/extra fields.
    /// - Any error returned by [`Config::validate`].
    pub fn from_yaml(s: &str) -> Result<Self, ConfigError> {
        let cfg: Self = serde_yaml::from_str(s)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Serializes the [`Config`] to a YAML string.
    ///
    /// # Errors
    /// Returns [`ConfigError::Yaml`] if the underlying serializer fails
    /// (in practice, never for this struct shape).
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
    use crate::{
        FAST_CONFIRM_DUE_BPS, HISTORICAL_ROOTS_LIMIT, INTERVALS_PER_SLOT,
        JUSTIFICATION_LOOKBACK_SLOTS, PROPOSER_REORG_CUTOFF_BPS, SECONDS_PER_INTERVAL,
        SECONDS_PER_SLOT, SLOT_DURATION_MS, VALIDATOR_REGISTRY_LIMIT, VIEW_FREEZE_CUTOFF_BPS,
        VOTE_DUE_BPS,
    };

    // ---------------------------------------------------------------------
    // Constant values match the canonical reference exactly.
    // ---------------------------------------------------------------------

    #[test]
    fn constants_match_canonical_values() {
        assert_eq!(INTERVALS_PER_SLOT, 4);
        assert_eq!(SLOT_DURATION_MS, 4_000);
        assert_eq!(SECONDS_PER_SLOT, 4);
        assert_eq!(SECONDS_PER_INTERVAL, 1);
        assert_eq!(JUSTIFICATION_LOOKBACK_SLOTS, 3);
        assert_eq!(PROPOSER_REORG_CUTOFF_BPS, 2_500);
        assert_eq!(VOTE_DUE_BPS, 5_000);
        assert_eq!(FAST_CONFIRM_DUE_BPS, 7_500);
        assert_eq!(VIEW_FREEZE_CUTOFF_BPS, 7_500);
        assert_eq!(HISTORICAL_ROOTS_LIMIT, 262_144);
        assert_eq!(VALIDATOR_REGISTRY_LIMIT, 4_096);
    }

    #[test]
    fn devnet_config_matches_constants() {
        assert_eq!(DEVNET_CONFIG.slot_duration_ms, SLOT_DURATION_MS);
        assert_eq!(DEVNET_CONFIG.seconds_per_slot, SECONDS_PER_SLOT);
        assert_eq!(
            DEVNET_CONFIG.justification_lookback_slots,
            JUSTIFICATION_LOOKBACK_SLOTS
        );
        assert_eq!(
            DEVNET_CONFIG.proposer_reorg_cutoff_bps,
            PROPOSER_REORG_CUTOFF_BPS
        );
        assert_eq!(DEVNET_CONFIG.vote_due_bps, VOTE_DUE_BPS);
        assert_eq!(DEVNET_CONFIG.fast_confirm_due_bps, FAST_CONFIRM_DUE_BPS);
        assert_eq!(DEVNET_CONFIG.view_freeze_cutoff_bps, VIEW_FREEZE_CUTOFF_BPS);
        assert_eq!(DEVNET_CONFIG.historical_roots_limit, HISTORICAL_ROOTS_LIMIT);
        assert_eq!(
            DEVNET_CONFIG.validator_registry_limit,
            VALIDATOR_REGISTRY_LIMIT
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
