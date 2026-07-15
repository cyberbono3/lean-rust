# runtime/

Tier-5 and Tier-6 runtime shell: hosts the consensus engine, drives
proposer/attester duties, exposes the network, and serves the HTTP API +
Prometheus metrics. Consolidated from the former seven per-crate shells into a
single crate — each is now an internal module.

## Modules

| Module            | Tier | Status      | One-liner                                            |
| ----------------- | ---- | ----------- | ---------------------------------------------------- |
| [`core`]          | 5    | implemented | `Service` trait + `Node` lifecycle composition root. |
| [`chain`]         | 6    | implemented | Single engine writer + tick driver.                  |
| [`sync`]          | 6    | implemented | Peer-driven `BlocksByRoot` backfill loop.            |
| [`duties`]        | 6    | implemented | Devnet0 proposer / attester scheduler.               |
| [`p2p`]           | 6    | scaffold    | libp2p QUIC-v1 host.                                  |
| [`api`]           | 6    | scaffold    | Lean HTTP API + Prometheus metrics.                  |
| [`observability`] | 6    | implemented | Tracing / log-verbosity init.                        |

[`core`]: ./src/core/mod.rs
[`chain`]: ./src/chain/mod.rs
[`sync`]: ./src/sync/mod.rs
[`duties`]: ./src/duties/mod.rs
[`p2p`]: ./src/p2p/mod.rs
[`api`]: ./src/api/mod.rs
[`observability`]: ./src/observability/mod.rs

## Dependency graph

```
sync   ──▶ chain ──▶ core
duties ──▶ chain ──▶ core
p2p    ──▶ chain ──▶ core
api    ──▶ chain ──▶ core
```

All Tier-6 services implement [`core::Service`] (start / stop / status); `Node`
is the composition root that owns the slots and enforces ordered startup
(`chain → p2p → sync → duties → http → metrics`) and reverse-ordered shutdown.

[`core::Service`]: ./src/core/service.rs

## Design notes

- **Single engine writer.** Only `chain::Service` holds the mutable handle into
  the forkchoice store. Sync and duties drive it through narrow async ports
  (`sync::Chain`, `duties::Chain`).
- **Dependency Inversion.** Outbound surfaces (publish, network RPCs) are
  declared as traits in the consumer module; concrete impls live in the `node`
  composition root. See [Decision 7] in the project plan.
- **Module isolation (review convention).** `p2p`, `chain`, and `api` are
  sibling modules that must not reach into each other's internals. This was
  formerly a Cargo crate boundary; after consolidation it is a review
  convention, not a compiler barrier — keep truly-internal items scoped with
  `pub(in crate::<module>)` / `pub(super)` rather than `pub(crate)`.
- **Audit boundary preserved.** The eight sync-core crates (`types`, `ssz`,
  `config`, `crypto`, `protocol`, `forkchoice`, `storage`, `networking`) stay
  separate so `cargo tree` keeps consensus logic free of `tokio`/`libp2p`/`axum`. `libp2p`
  is confined to `p2p`; `axum`/`prometheus` to `api`.

[Decision 7]: ../../.claude/PROJECT-KNOWLEDGE.md

## Build

```bash
cargo build  --workspace
cargo test   --workspace
cargo clippy --all-targets -- -D warnings
```
