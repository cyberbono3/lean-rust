# `runtime-p2p` split: shared-types inventory

Pre-flight for Phase 5 of `.claude/commit_plan.md`. No code changes in this commit.

## Subsystem layout (current)

```
crates/runtime/p2p/src/
├── lib.rs               (44 LOC)  Public surface + re-exports.
├── devnet.rs           (103 LOC)  DevnetHost::build.
├── error.rs            (170 LOC)  HostError / HostResult.
├── options.rs          (317 LOC)  HostOptions / AgentVersion.
├── service.rs          (808 LOC)  P2pService — lifecycle + swarm task.
├── wiring.rs            (10 LOC)  Compile-time Service-impl assertion.
├── host/                (~1040 LOC total)
│   ├── mod.rs           (99 LOC)  Host handle, HostCommand, peer_id().
│   ├── behaviour.rs    (328 LOC)  DevnetBehaviour aggregate + 2× request_response.
│   ├── behaviour/codec.rs        RpcRequest / RpcResponse / SszSnappyCodec.
│   ├── bootnodes.rs    (237 LOC)  Multiaddr parsing.
│   ├── keypair.rs      (335 LOC)  Identity load / persist.
│   └── transport.rs     (43 LOC)  QUIC-v1 transport assembly.
├── gossip/             (~426 LOC)
│   ├── mod.rs           (91 LOC)  Topic enum, re-exports.
│   ├── handler.rs      (258 LOC)  Inbound dispatch + GossipReceiver<T>.
│   └── publisher.rs     (77 LOC)  Host::publish_block / publish_vote.
└── rpc/                (~516 LOC)
    ├── mod.rs          (123 LOC)  RpcProvider trait, NoOpRpcProvider, RpcError.
    ├── blocks_by_root.rs (114 LOC) Inbound handler — driven by service.rs.
    ├── status.rs       (156 LOC)  Inbound handler — driven by service.rs.
    ├── client.rs        (62 LOC)  Host::send_blocks_by_root outbound wrapper.
    └── outbound.rs      (61 LOC)  OutboundTable — pending-request bookkeeping.
```

Total: ~3 436 LOC.

## Shared-types inventory

### A. Codec / wire enums (`host::behaviour::codec`)

| Type                | Owner   | Consumers                                              |
| ------------------- | ------- | ------------------------------------------------------ |
| `RpcRequest`        | `host`  | `host::behaviour` (DevnetBehaviour), `rpc::*` (re-exported via `rpc::mod`) |
| `RpcResponse`       | `host`  | `host::behaviour`, `rpc::*`                            |
| `SszSnappyCodec`    | `host`  | `host::behaviour` (the two `request_response::Behaviour<SszSnappyCodec>`) — NOT exported to rpc. |

Re-export site: `crates/runtime/p2p/src/rpc/mod.rs:29` —
`pub use crate::host::behaviour::codec::{RpcRequest, RpcResponse};`

### B. Swarm-behaviour aggregate (`host::behaviour`)

| Type                  | Owner   | Consumers                                              |
| --------------------- | ------- | ------------------------------------------------------ |
| `DevnetBehaviour`     | `host`  | `service.rs` (the swarm is `Swarm<DevnetBehaviour>`)    |
| `DevnetBehaviourEvent`| `host`  | `service.rs` (pattern-matched in swarm-poll task)       |

These aggregate gossipsub + the two request_response behaviours into a single libp2p `NetworkBehaviour`. Splitting `request_response` ownership would require either re-introducing it through a trait at composition time or keeping the aggregate behaviour together.

### C. Host handle (`host::Host` / `host::HostCommand`)

| Type                            | Owner   | Consumers                                              |
| ------------------------------- | ------- | ------------------------------------------------------ |
| `Host`                          | `host`  | `gossip::publisher`, `rpc::client`                      |
| `HostCommand`                   | `host`  | `service.rs` (channel receiver), `gossip::*`, `rpc::*` (channel senders) |
| `COMMAND_CHANNEL_CAPACITY`      | `host`  | `service.rs`                                            |

`Host` is the typed mpsc-sender wrapper. Both gossip and rpc reach the swarm by dispatching `HostCommand` variants through it.

### D. RPC provider surface (`rpc::*`)

| Type                  | Owner   | Consumers                                              |
| --------------------- | ------- | ------------------------------------------------------ |
| `RpcProvider` trait   | `rpc`   | `service.rs` (passes `&dyn RpcProvider` to handlers), `devnet::DevnetHost::build_with_provider` |
| `SharedRpcProvider`   | `rpc`   | `service.rs` (`Arc<dyn RpcProvider>` field)             |
| `NoOpRpcProvider`     | `rpc`   | `devnet::DevnetHost::build` (default wiring)            |
| `RpcError`            | `rpc`   | `Host::send_blocks_by_root` return type                 |

### E. Inbound RPC dispatch (`rpc::{blocks_by_root, status, outbound}`)

| Item                          | Owner   | Consumers                                              |
| ----------------------------- | ------- | ------------------------------------------------------ |
| `blocks_by_root::handle_*`    | `rpc`   | `service.rs` (called from swarm-event match arms)       |
| `status::handle_*`            | `rpc`   | `service.rs` (called from swarm-event match arms)       |
| `outbound::OutboundTable`     | `rpc`   | `service.rs` (tracks pending request ids → response chans) |

These functions receive `request_response::Event` variants directly from the swarm poll loop. They are not pure helpers — they mutate `OutboundTable` and dispatch `HostCommand`.

### F. External types (workspace deps, not split-relevant)

- `lean_wire::Status` — Status handshake payload (already external).
- `protocol::{SignedBlock, SignedVote}` — gossip + rpc payloads.
- `libp2p::{gossipsub, request_response, swarm::SwarmEvent, PeerId, Multiaddr, Swarm}` — third-party.

## Coupling matrix

|             | host | gossip | rpc |
| ----------- | ---- | ------ | --- |
| **host**    | —    | —      | exports `RpcRequest`/`RpcResponse`/`SszSnappyCodec` (D pull from `rpc::mod`). Indirectly via service.rs (E). |
| **gossip**  | uses `Host` handle (C), `HostCommand` (C). | — | none |
| **rpc**     | uses `Host` handle (C), `HostCommand` (C), `RpcRequest`/`RpcResponse` (A). | none | — |
| **service** | drives `Swarm<DevnetBehaviour>` (B). | drives `gossip::handler::*`, `Topic`. | drives `rpc::blocks_by_root::*`, `rpc::status::*`, `OutboundTable`. |

## Split decision

**Decision: re-export shared types from `p2p-host`; `p2p-rpc` depends on `p2p-host`. No third `p2p-common` crate.**

Reasoning:
- Group A (codec / wire enums) is ~80 LOC. Well under the 200-LOC threshold the plan called out for keeping types in `p2p-host`.
- Group B (DevnetBehaviour) cannot leave `p2p-host` — it owns the libp2p `NetworkBehaviour` impl that the swarm is parameterised over.
- Group C (Host handle + HostCommand) is intrinsic to `p2p-host` — `HostCommand` is the message protocol the swarm-poll task in `service.rs` receives.
- Group E (inbound RPC dispatch) is the hard one. Three options:
  1. **Keep inbound handlers in `p2p-host`.** `service.rs` calls them directly. `p2p-rpc` shrinks to: `RpcProvider` trait, `NoOpRpcProvider`, `RpcError`, and the outbound `Host::send_blocks_by_root` ergonomic wrapper (`client.rs`). Net ~200 LOC for `p2p-rpc`; the bulk of `rpc/` (handlers + OutboundTable) actually belongs in host because it is intertwined with the swarm-event loop.
  2. **Move `service.rs` to `p2p-rpc`.** Inverts the dependency: `p2p-host` becomes a thin wiring crate, `p2p-rpc` owns the swarm task. Awkward — gossip handlers would still live in host.
  3. **Inject a `RpcInboundHandler` trait at composition time.** Cleanest dependency graph but adds dyn-dispatch on every inbound RPC event and a trait the `node` crate must wire.

Recommendation: **option 1** for commit 5.2. The `rpc/` directory is misleading — the inbound handlers are de facto host concern. Move only the boundary types (RpcProvider trait + RpcError + outbound client wrapper) to the new crate.

### Files moving to `p2p-rpc` in commit 5.2

- `crates/runtime/p2p/src/rpc/mod.rs` → `crates/runtime/p2p-rpc/src/lib.rs` (RpcProvider, NoOpRpcProvider, RpcError, SharedRpcProvider).
- `crates/runtime/p2p/src/rpc/client.rs` → `crates/runtime/p2p-rpc/src/client.rs` (outbound `send_blocks_by_root`).

### Files staying in `p2p-host` (recommended deviation from the plan)

- `crates/runtime/p2p/src/rpc/{blocks_by_root,status,outbound}.rs` — move under `crates/runtime/p2p/src/host/rpc_inbound/` (or keep in place under `src/rpc/inbound/`). These cannot leave because `service.rs` calls them with `request_response::Event` and a `&mut Swarm<DevnetBehaviour>` reference.

The plan listed all 5 files moving to `p2p-rpc`. After pre-flight, the realistic split is 2-of-5: the boundary surface, not the dispatch loop. Commit 5.2 should follow this revised plan and note the deviation.

## Cycle check

After the split:
- `p2p-rpc` → `p2p-host` (consumes `Host`, `HostCommand`, `RpcRequest`, `RpcResponse`).
- `p2p-host` → `p2p-rpc`: only if `service.rs` references `RpcProvider`. Resolution: keep `RpcProvider` *and* `SharedRpcProvider` in `p2p-rpc`; `p2p-host::service` accepts `Arc<dyn p2p_rpc::RpcProvider>`. This means `p2p-host` depends on `p2p-rpc` for the trait. Cycle: yes, BOTH directions.

To avoid the cycle, choose ONE of:
- **A.** Move `RpcProvider` trait to `p2p-host` (re-shapes the boundary; `p2p-rpc` shrinks further).
- **B.** Move outbound `client.rs` (the `Host::send_blocks_by_root` wrapper) into `p2p-host` so `p2p-rpc` doesn't need `Host`. Then `p2p-rpc` is `RpcProvider` trait + `RpcError` + `NoOpRpcProvider` only — ~120 LOC, no `p2p-host` dep.

Option B is cleaner and matches the spirit of the plan: `p2p-rpc` is the *contract surface* node implements, not the wire driver. Commit 5.2 will use option B.
