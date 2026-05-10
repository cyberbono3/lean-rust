//! Frozen chain constants and the devnet0 [`Config`].
//!
//! All tunable consensus parameters live on the [`Config`] struct (see
//! [`devnet0`]) and are accessed through the canonical [`DEVNET_CONFIG`]
//! preset — e.g. `DEVNET_CONFIG.slot_duration_ms`. The two values below
//! ([`INTERVALS_PER_SLOT`], [`SECONDS_PER_INTERVAL`]) are the only
//! module-level constants because they are not part of the chain-config
//! shape: they're fixed forkchoice topology, not user-tunable knobs.
//!
//! All basis-point values lie in `0..=10_000` per
//! [`types::BasisPoint`] semantics; raw values are stored as `u64` so the
//! constants compose with `const` contexts (the typed
//! [`types::BasisPoint`] constructor returns `Result` and cannot be used
//! in a const initializer under the workspace `panic = "deny"` lint).
//!
//! # Example
//! ```
//! use config::{Config, ConfigError, DEVNET_CONFIG, INTERVALS_PER_SLOT};
//! # fn main() -> Result<(), ConfigError> {
//! assert_eq!(INTERVALS_PER_SLOT, 4);
//! assert_eq!(DEVNET_CONFIG.slot_duration_ms, 4_000);
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
///
/// Not part of [`Config`]: this is fixed forkchoice topology, not a
/// chain-config knob.
pub const INTERVALS_PER_SLOT: u64 = 4;

/// Interval duration in seconds (`DEVNET_CONFIG.seconds_per_slot /
/// INTERVALS_PER_SLOT` = `1`).
///
/// Not part of [`Config`]: derived from [`DEVNET_CONFIG`] and pinned to a
/// scalar so callers don't recompute it.
pub const SECONDS_PER_INTERVAL: u64 = DEVNET_CONFIG.seconds_per_slot / INTERVALS_PER_SLOT;
