//! Frozen chain constants and the devnet0 [`Config`].
//!
//! Module-level constants are the canonical, immutable consensus values used
//! by the runtime; the [`Config`] struct (in [`devnet0`]) is the
//! `serde`-friendly mirror that loads from YAML and feeds higher-level
//! crates (`forkchoice`, `statetransition`).
//!
//! All basis-point values lie in `0..=10_000` per
//! [`types::BasisPoint`] semantics; raw values are stored as `u64` so the
//! constants compose with `const` contexts (the typed
//! [`types::BasisPoint`] constructor returns `Result` and cannot be used in
//! a const initializer under the workspace `panic = "deny"` lint).
//!
//! # Example
//! ```
//! use config::{Config, ConfigError, DEVNET_CONFIG, INTERVALS_PER_SLOT, SLOT_DURATION_MS};
//! # fn main() -> Result<(), ConfigError> {
//! assert_eq!(SLOT_DURATION_MS, 4_000);
//! assert_eq!(INTERVALS_PER_SLOT, 4);
//!
//! let cfg: Config = Config::default();
//! assert_eq!(cfg, DEVNET_CONFIG);
//! cfg.validate()?;
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

pub mod devnet0;

pub use devnet0::{Config, ConfigError, DEVNET_CONFIG};

/// Number of forkchoice intervals per slot (devnet0 = `4`).
pub const INTERVALS_PER_SLOT: u64 = 4;

/// Slot duration in milliseconds (devnet0 = `4_000`).
pub const SLOT_DURATION_MS: u64 = 4_000;

/// Slot duration in seconds (`SLOT_DURATION_MS / 1_000` = `4`).
pub const SECONDS_PER_SLOT: u64 = SLOT_DURATION_MS / 1_000;

/// Interval duration in seconds (`SECONDS_PER_SLOT / INTERVALS_PER_SLOT` = `1`).
pub const SECONDS_PER_INTERVAL: u64 = SECONDS_PER_SLOT / INTERVALS_PER_SLOT;

/// Number of slots examined for justification (devnet0 = `3`).
pub const JUSTIFICATION_LOOKBACK_SLOTS: u64 = 3;

/// Proposer-reorg cutoff in basis points (devnet0 = `2_500` == 25%).
pub const PROPOSER_REORG_CUTOFF_BPS: u64 = 2_500;

/// Attestation deadline in basis points (devnet0 = `5_000` == 50%).
pub const VOTE_DUE_BPS: u64 = 5_000;

/// Fast-confirm deadline in basis points (devnet0 = `7_500` == 75%).
pub const FAST_CONFIRM_DUE_BPS: u64 = 7_500;

/// View-freeze cutoff in basis points (devnet0 = `7_500` == 75%).
pub const VIEW_FREEZE_CUTOFF_BPS: u64 = 7_500;

/// State historical-roots cap (devnet0 = `1 << 18` = `262_144`).
pub const HISTORICAL_ROOTS_LIMIT: u64 = 1 << 18;

/// Validator registry cap (devnet0 = `1 << 12` = `4_096`).
pub const VALIDATOR_REGISTRY_LIMIT: u64 = 1 << 12;
