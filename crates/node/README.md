# node

Composition root: wires the concrete `runtime` services into a runnable
[`Node`] and hosts the **self-driving consensus loop** that lets a node run
consensus on its own.

The `runtime` service modules stay decoupled from each other; this crate is the
one place where the concrete `chain`, `p2p`, `sync`, and driver services are
assembled and handed to the lifecycle.

## Scope

- [`new_devnet`] / [`Config`] / [`Result`] — build a devnet [`Node`] from a node
  config: construct the store, anchor the engine at genesis, and assemble a
  flat service graph (chain, p2p, sync loop, consensus-loop driver, HTTP,
  metrics) in start order.
- [`ConsensusLoop`] (`src/consensus_loop.rs`) — the self-driving driver. One
  spawned task advances the forkchoice clock, proposes at the slot boundary,
  attests at vote-due, drains gossip, and publishes — all under the single
  engine writer.

[`new_devnet`]: ./src/devnet.rs
[`Config`]: ./src/devnet.rs
[`Result`]: ./src/devnet.rs
[`ConsensusLoop`]: ./src/consensus_loop.rs
[`Node`]: ../runtime/src/core/node.rs

## The self-driving consensus loop

`ConsensusLoop` occupies the node's duties slot. On `start` it takes the gossip
receivers from the running p2p service and spawns **exactly one** task (a
`Runner`); that task owns the whole per-interval rhythm, anchored to genesis
time. It replaces three former workaround services — a chain-owned tick loop, a
separate duty scheduler, and a gossip-ingest task — with a single driver, and
threads a **truthful `has_proposal`** into forkchoice (the previous tick loop
hard-coded `false`, which blocked post-proposal vote acceptance and stalled
finality).

```text
                   ┌───────────────────────────────────────────────┐
                   │            new_devnet (composition)            │
                   │  chain · p2p · sync::Loop · ConsensusLoop ·    │
                   │            http · metrics   ──▶  Node          │
                   └───────────────────────┬───────────────────────┘
                                           │  Node::start
                                           │  (chain → p2p → sync → driver → http → metrics)
                                           ▼
   ConsensusLoop::start  ── spawns ONE task ──▶  Runner::run(cancel)
                                           │
                       initial_sync() once │   (no-op when no peer is connected)
                                           ▼
       ┌─────────────────────  interval ticker  ──────────────────────┐
       │  interval_at(genesis_anchor + TICK_PERIOD, TICK_PERIOD)       │
       │                                                               │
       │  per tick:  slot     = tick / INTERVALS_PER_SLOT              │
       │             interval = tick % INTERVALS_PER_SLOT              │
       │                                                               │
       │   (1) drain_gossip ──▶ chain.import_block / import_attestation │
       │                                                               │
       │   (2) interval 0        ──▶ maybe_propose                     │
       │         proposer_for_slot? ─▶ produce_block ─▶ publish        │
       │         └─ sets has_proposal for this slot                    │
       │       interval VOTE_DUE   ──▶ run_attesters  (FuturesUnordered)│
       │         each local validator, concurrent on this one task:    │
       │         produce_attestation ─▶ publish                        │
       │                                                               │
       │   (3) chain.tick_interval(has_proposal)  ── advance clock     │
       └────────────────────────────┬──────────────────────────────────┘
                                    │  every engine mutation
                                    ▼  (single writer · one engine mutex)
                     runtime::chain::Service  ──▶  Engine / forkchoice store
```

Publishing and gossip both flow through the concrete `runtime::p2p::P2pService`;
peer-driven backfill runs through `runtime::sync::Loop` (its own lifecycle
service), which the driver also kicks once via `initial_sync` before the loop.

## Service wiring & lifecycle

`new_devnet` returns a [`Node`] holding six `runtime::core::Service` slots.
`Node::start` brings them up in a fixed order and tears them down in reverse:

| Slot      | Service                        | Role                                      |
| --------- | ------------------------------ | ----------------------------------------- |
| `chain`   | `runtime::chain::Service`      | Passive engine funnel (single writer).    |
| `p2p`     | `runtime::p2p::P2pService`     | libp2p QUIC host; gossip + outbound RPC.  |
| `sync`    | `runtime::sync::Loop`          | Peer-driven `BlocksByRoot` backfill.      |
| `duties`  | `ConsensusLoop`                | The self-driving driver (this crate).     |
| `http`    | `runtime::api::HttpService`    | Lean HTTP API.                            |
| `metrics` | `runtime::api::MetricsService` | Prometheus metrics + chain-state gauges.  |

The driver takes its gossip receivers in `start` (not at construction) because
they only exist after `P2pService::start`, which runs earlier in the order.

## Design invariants

- **Single writer.** Every engine mutation — import, produce, tick — goes
  through `runtime::chain::Service` under one engine mutex, and the driver
  spawns exactly one task (the concurrent attester pass uses `FuturesUnordered`,
  not per-validator spawns). No lock guard is ever held across an `.await`.
- **Truthful `has_proposal`.** Threaded through `ChainService::tick_interval`
  so forkchoice correctly accepts a proposer's own post-proposal votes.
- **Composition lives here.** Cross-service orchestration (chain + p2p + sync)
  belongs in the composition root; `runtime::chain` stays free of any p2p / sync
  dependency.

## Tests

- `self_driving_node_proposes_attests_and_advances` (`src/devnet.rs`) — a single
  node owning every validator proposes, attests, advances head ≥ 3 slots, and
  finalizes a checkpoint, deterministically under `start_paused`. No second
  node or process is required.
- `new_devnet_builds_node_that_starts_and_stops` — the full graph builds, starts,
  reports status, and stops cleanly.
