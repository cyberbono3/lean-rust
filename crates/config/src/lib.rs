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

/// SSZ `List`/`Bitlist` cap `N` — the single source for every
/// `List<Signature, N>` and `Bitlist<N>` bound across the workspace.
///
/// Pinned to the `validator_registry_limit` field of [`DEVNET_CONFIG`]
/// (`4_096` on devnet0).
/// The one and only `usize` derivation of the cap: downstream consts
/// (`protocol`'s `MAX_ATTESTATIONS`, `VALIDATOR_REGISTRY_LIMIT`) alias this by
/// name rather than re-casting.
///
/// The `u64` -> `usize` cast is exact on every supported target (the value is
/// `4_096`, well within `usize::MAX`); the `#[allow]` silences the generic-cast
/// lint, not a real truncation.
#[allow(clippy::cast_possible_truncation)]
pub const VALIDATOR_REGISTRY_LIMIT: usize = DEVNET_CONFIG.validator_registry_limit as usize;

#[cfg(test)]
mod tests {
    use super::{DEVNET_CONFIG, VALIDATOR_REGISTRY_LIMIT};

    #[test]
    fn validator_registry_limit_usize_const_matches_field() {
        // Truncation safety of the const's `u64` -> `usize` cast rests on the
        // compile-time fact that the value (4096) fits the narrowest supported
        // `usize`, NOT on this test. The `as u64` round-trip below is only
        // load-bearing on sub-64-bit `usize` targets (which this std crate never
        // ships to); the real value guard is the `== 4_096` literal assertion.
        assert_eq!(
            VALIDATOR_REGISTRY_LIMIT as u64,
            DEVNET_CONFIG.validator_registry_limit
        );
        assert_eq!(VALIDATOR_REGISTRY_LIMIT, 4_096);
    }
}
