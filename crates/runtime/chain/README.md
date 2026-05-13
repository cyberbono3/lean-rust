# runtime-chain

The single engine writer at the Tier-6 chain layer.

## Scope

- [`Service`] — wraps [`engine::Engine`] + [`storage::Store`].
  Exposes async `import_block` / `import_attestation` /
  `produce_block` / `produce_attestation`; drives the forkchoice
  tick loop on a `tokio` background task; persists every accepted
  block, post-state, and head to storage.
- [`ChainSnapshot`] — hot-read projection of engine state
  (`head_root`, `safe_target_root`, `current_slot`,
  `latest_finalized`). Refreshed after each `Accepted` import and
  each tick; consumed by `runtime-api` / `runtime-p2p` through an
  `Arc<RwLock<_>>` clone.
- [`ChainError`] — infrastructure failures (storage, engine
  invariant violations, engine forkchoice / state-transition
  errors).

[`Service`]: ./src/chain/service.rs
[`ChainSnapshot`]: ./src/chain/cache.rs
[`ChainError`]: ./src/chain/error.rs

## What lives elsewhere

This crate only contains the engine-writing layer. The downstream
Tier-6 services that drive it ship in sibling crates and host their
own adapter `impl` blocks on [`Service`] (orphan rule — each trait
is defined in its consumer crate):

- [`runtime-sync`](../sync) — peer-driven `BlocksByRoot` backfill
  loop. Calls `import_block` / `local_status` / `has_block`.
- [`runtime-duties`](../duties) — proposer / attester scheduler.
  Calls `produce_block` / `produce_attestation`.

The [`runtime-p2p`](../p2p) and [`runtime-api`](../api) crates
will consume the `ChainSnapshot` read path once they land.

## Tier and dependencies

Tier 6. Depends on `runtime-core`, `engine`, `storage`,
`networking`, `protocol`, `config`, `types`, plus the standard
async stack (`tokio`, `tokio-util`, `async-trait`, `tracing`).
