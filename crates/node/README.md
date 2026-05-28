# node

Composition root wiring concrete runtime services into a `Node`.

The `runtime/*` sibling crates stay decoupled (each declares its own narrow
ports). This crate is the one place where concrete services are assembled,
those ports are adapted, and a [`lean_core::Node`] is returned ready for
lifecycle management.

## Scope

- [`new_devnet`] / [`Config`] / [`Result`] — build a devnet [`lean_core::Node`]
  from a node config: constructs the store, engine + chain service, p2p host,
  gossip-ingest, duties scheduler, HTTP + metrics services (incl. the
  chain-state gauge wiring), and assembles them in start order.
- [`PublisherAdapter`] — adapts `lean-duties`' `Publisher` port to the
  libp2p host (`publish_block` / `publish_vote`).
- `gossip_ingest` / `rpc_provider` (private) — the inbound-gossip → chain
  bridge and the `RpcProvider` impl backing `BlocksByRoot` responses.

[`new_devnet`]: ./src/devnet.rs
[`Config`]: ./src/devnet.rs
[`Result`]: ./src/devnet.rs
[`PublisherAdapter`]: ./src/publisher_adapter.rs

## Tier and dependencies

Tier 6 composition root. Depends on all the runtime service crates
(`lean-core`, `lean-chain`, `lean-sync`, `lean-duties`, `lean-api`,
`lean-p2p-host`, `p2p-rpc`, `storage`) plus `protocol` / `types`. Because the
adapter `impl` blocks live here (orphan rule), this is the only crate that
sees every service concretely.
