# runtime/

Tier-5 and Tier-6 crates: the runtime shell that hosts the consensus
engine, drives proposer/attester duties, exposes the network, and
serves the HTTP API + Prometheus metrics.

## Crates

| Crate              | Tier | Status      | One-liner                                            |
| ------------------ | ---- | ----------- | ---------------------------------------------------- |
| [`runtime-core`]   | 5    | implemented | `Service` trait + `Node` lifecycle composition root. |
| [`runtime-chain`]  | 6    | implemented | Single engine writer + tick driver.                  |
| [`runtime-sync`]   | 6    | implemented | Peer-driven `BlocksByRoot` backfill loop.            |
| [`runtime-duties`] | 6    | implemented | Devnet0 proposer / attester scheduler.               |
| [`runtime-p2p`]    | 6    | scaffold    | libp2p QUIC-v1 host (lands in a later issue).        |
| [`runtime-api`]    | 6    | scaffold    | Lean HTTP API + Prometheus metrics (later issue).    |

[`runtime-core`]: ./core
[`runtime-chain`]: ./chain
[`runtime-sync`]: ./sync
[`runtime-duties`]: ./duties
[`runtime-p2p`]: ./p2p
[`runtime-api`]: ./api

## Dependency graph

```
runtime-sync   ──▶ runtime-chain ──▶ runtime-core
runtime-duties ──▶ runtime-chain ──▶ runtime-core
runtime-p2p    ──▶ runtime-chain ──▶ runtime-core
runtime-api    ──▶ runtime-chain ──▶ runtime-core
```

All Tier-6 services implement [`runtime_core::Service`] (start / stop /
status); `Node` is the composition root that owns the slots and
enforces ordered startup (`chain → p2p → sync → duties → http →
metrics`) and reverse-ordered shutdown.

[`runtime_core::Service`]: ./core/src/service.rs

## Design notes

- **Single engine writer.** Only `runtime-chain::Service` holds the
  mutable handle into the forkchoice store. Sync and duties drive it
  through narrow async ports (`sync::Chain`, `duties::Chain`).
- **Dependency Inversion.** Outbound surfaces (publish, network RPCs)
  are declared as traits in the consumer crate; concrete impls live in
  the `node` composition root. See [Decision 7] in the project plan.
- **Tier ordering.** Lower tiers (`types`, `protocol`, `engine`,
  `storage`) never depend on `runtime/*`. Runtime crates depend down
  through the tiers, not sideways across them — except for the
  intra-Tier-6 deps shown above.

[Decision 7]: ../../.claude/PROJECT-KNOWLEDGE.md

## Build

```bash
cargo build  --workspace
cargo test   --workspace
cargo clippy --all-targets -- -D warnings
```
