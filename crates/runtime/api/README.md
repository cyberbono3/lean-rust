# lean-api

Lean HTTP API + Prometheus metrics (Tier 6).

## Status

Provides the runtime HTTP service and Prometheus metrics service used
by composition crates.

Current head endpoints:

- `GET /eth/v1/head`
- `GET /lean/v0/head`
- `GET /lean/v0/head/full`

`GET /lean/v0/head` is the Ream-compatible endpoint and returns
`{"head":"0x..."}`. The `/eth/v1/head` and `/lean/v0/head/full`
routes return lean-rust's richer diagnostic JSON body with `head` and
`finalized` checkpoints.

## Planned scope

- Lean HTTP API endpoints (head, block-by-root, state-by-root,
  config) backed by `lean_chain::ChainSnapshot`.
- Prometheus `/metrics` endpoint with the leanmetrics namespace.
- Hosted on the [`lean_core::Server`] shell — same axum router
  shape, same `CancellationToken` shutdown contract.
- Implements [`lean_core::Service`] (start / stop / status).

[`lean_core::Server`]: ../core/src/httpsvc/
[`lean_core::Service`]: ../core/src/service.rs

## Tier and dependencies

Tier 6. Will depend on `lean-core` (for the HTTP shell and
`Service` trait), `lean-chain` (for the snapshot read path),
`prometheus`, `axum`, `serde_json`.

## Issue reference

See [`lean-rust-github-issues.md`] for the deliverables checklist
(API endpoints + Prometheus exposition).

[`lean-rust-github-issues.md`]: ../../../.claude/prompts/lean-rust-github-issues.md
