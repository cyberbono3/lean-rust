# p2p-rpc

Contract surface for the p2p req/resp layer (Tier 6 boundary).

The boundary between the composition root (`node`) and the libp2p driver in
`lean-p2p-host`. It carries **no libp2p dependency** — only the trait the
application implements, a no-op default for tests, and the failure surface
returned by outbound requests. This keeps the host crate from depending on
`storage` directly (it accepts an `Arc<dyn RpcProvider>` at construction).

## Scope

- [`RpcProvider`] — trait the composing binary (`node`) implements to
  supply the local `Status` and look up blocks by tree root.
- [`NoOpRpcProvider`] — default no-op provider wired by `DevnetHost::build`;
  used by lifecycle tests.
- [`RpcError`] — failure surface returned by outbound requests.

[`RpcProvider`]: ./src/lib.rs
[`NoOpRpcProvider`]: ./src/lib.rs
[`RpcError`]: ./src/lib.rs

## Tier and dependencies

Tier 6 boundary crate. Depends on `lean-wire` (for the req/resp payload
types) and `protocol`/`types`. No `libp2p`, no `storage` — that is the whole
point: it inverts the dependency so the host and the composition root meet
through this trait.
