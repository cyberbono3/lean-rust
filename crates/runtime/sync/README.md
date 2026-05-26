# lean-sync

Peer-driven `BlocksByRoot` backfill loop (Tier 6).

On each outbound peer-connect event, performs a `Status` handshake
and—if the peer is ahead—walks backwards from the peer's head one
root at a time via `BlocksByRoot` up to [`Config::max_sync_depth`],
then imports the recovered chain in forward order through the
[`Chain`] port.

## Scope

- [`Loop`] — the orchestrator. Implements
  [`lean_core::Service`] (start / stop / status). Owns the
  watch task that drains peer-connect events plus a `TaskTracker`
  for per-peer `on_connect` workers; cancellation-via-token
  shutdown.
- [`Config`] — `max_sync_depth: NonZeroUsize`. Type-validated at
  construction.
- [`Chain`] / [`Network`] / [`PeerEventProvider`] — narrow port
  traits declared here per Decision 7 (Dependency Inversion).
  `Chain` is satisfied by [`lean_chain::Service`] via the
  in-crate [`chain_adapter`]. `Network` and `PeerEventProvider`
  have no in-crate impl; the `runtime-p2p` / `node` crates provide
  the libp2p-backed adapters in later issues.
- [`PeerId`] — opaque non-empty newtype for outbound peer
  identifiers.
- [`SyncError`] — typed failure surface (invalid depth, empty peer
  id, chain / network / subscription wrappers, lifecycle errors).

[`Loop`]: ./src/loop_.rs
[`Config`]: ./src/config.rs
[`Chain`]: ./src/ports.rs
[`Network`]: ./src/ports.rs
[`PeerEventProvider`]: ./src/ports.rs
[`chain_adapter`]: ./src/chain_adapter.rs
[`PeerId`]: ./src/peer_id.rs
[`SyncError`]: ./src/error.rs

## Behavior

Per-block import errors during walk-back are warn-logged and
dropped; an unknown parent at the deepest layer (when
`max_sync_depth` is hit before the walk meets a known block) is
the expected outcome and is resolved on a future peer-connect or
via gossip.

The crate compiles with zero `libp2p` exposure on its dependency
graph — that surface lives in `runtime-p2p`.

## Tier and dependencies

Tier 6. Depends on `lean-core`, `lean-chain`, `engine`,
`networking`, `protocol`, `types`, plus the standard async stack
(`tokio`, `tokio-util`, `async-trait`, `tracing`, `parking_lot`).

## Issue reference

Implements Issue #29. Originally lived as a `sync::` module of
`lean-chain`; extracted to its own crate when the surface
stabilized.
