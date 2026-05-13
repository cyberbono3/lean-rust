# runtime-duties

Narrow devnet0 validator-duty scheduler (Tier 6).

Loads validator assignments from YAML, schedules proposers at slot
boundaries and attesters at the `vote_due_bps` deadline. Production
goes through a [`Chain`] port satisfied by `runtime-chain`; publish
goes through a [`Publisher`] port whose impl lives in `node`.

## Scope

- [`Service`] — proposer / attester scheduler. Implements
  [`runtime_core::Service`] (start / stop / status). Owns one
  worker task driven by `tokio::time`.
- [`Config`] — `validators_path`, `validator_group`,
  `genesis_time_unix`. Always-valid by construction:
  [`ValidatorsPath`] and [`ValidatorGroup`] newtypes guarantee
  non-empty inputs; [`GenesisTimeUnix`] is a typed wrapper.
- [`Chain`] / [`Publisher`] — narrow port traits declared here per
  Decision 7 (Dependency Inversion). `Chain` is satisfied by
  [`runtime_chain::Service`] via [`chain_adapter`]; `Publisher`
  has no in-crate impl (the `node` crate provides the libp2p
  adapter in Issue #37).
- [`ValidatorAssignments`] — YAML loader for the canonical devnet0
  shape (`group_name: [indices...]`). Validates non-empty groups,
  unique indices, and 0..N contiguity.
- [`DutiesError`] / [`DutiesResult`] — error type + alias.

[`Service`]: ./src/service.rs
[`Config`]: ./src/config.rs
[`ValidatorsPath`]: ./src/config.rs
[`ValidatorGroup`]: ./src/config.rs
[`GenesisTimeUnix`]: ./src/config.rs
[`Chain`]: ./src/ports.rs
[`Publisher`]: ./src/ports.rs
[`chain_adapter`]: ./src/chain_adapter.rs
[`ValidatorAssignments`]: ./src/validators.rs
[`DutiesError`]: ./src/error.rs
[`DutiesResult`]: ./src/error.rs

## Out of scope

Mirrors lean-go `runtime/duties/`: aggregator duties, direct
forkchoice mutation, post-MVP metrics hooks. The scheduler is the
narrow devnet0 surface — nothing more.

## Tier and dependencies

Tier 6. Depends on `runtime-core`, `runtime-chain`, `protocol`,
`config`, plus the async / serde stack (`tokio`, `tokio-util`,
`async-trait`, `serde`, `serde_yaml`, `tracing`).

**No `runtime-p2p` import.** Enforced by the cargo-metadata gate
from Issue #30:

```bash
cargo metadata --format-version=1 \
  | jq -r '.packages[] | select(.name == "runtime-duties") | .dependencies[].name' \
  | grep -v '^runtime-p2p$'
```

## Why a separate crate (vs. a module of `runtime-chain`)

- Mirrors lean-go layout (`runtime/duties/` is its own package).
- The `Publisher` port has its concrete impl in `node`, not in
  this crate — the crate boundary makes the dependency-inversion
  story explicit.
- Cargo enforces the no-`runtime-p2p` invariant at the dependency
  graph level, not just by convention.

## Issue reference

Implements Issue #30. The previous in-`runtime-chain` module was
extracted to its own crate when the surface stabilized.
