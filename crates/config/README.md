# config

Frozen chain constants + devnet0 YAML config (Tier 1).

All tunable consensus parameters live on the [`Config`] struct and are read
through the canonical [`DEVNET_CONFIG`] preset — e.g.
`DEVNET_CONFIG.slot_duration_ms`. Basis-point values are stored as raw `u64`
(in `0..=10_000`) so the constants compose in `const` contexts.

## Scope

- [`Config`] — the devnet0 chain-configuration record (slot timing,
  basis-point cutoffs, registry/historical-roots limits) with a
  cross-field `validate()`.
- [`DEVNET_CONFIG`] — the canonical preset; the single source of truth
  other crates read parameters from.
- [`ConfigError`] — validation / YAML-load error type.
- [`INTERVALS_PER_SLOT`](./src/lib.rs) / [`SECONDS_PER_INTERVAL`](./src/lib.rs)
  — fixed forkchoice topology constants (not part of the tunable
  chain-config shape).

[`Config`]: ./src/devnet0.rs
[`DEVNET_CONFIG`]: ./src/devnet0.rs
[`ConfigError`]: ./src/devnet0.rs

## Tier and dependencies

Tier 1. Depends on `types` (for `BasisPoint`) only. No consensus or runtime
imports.
