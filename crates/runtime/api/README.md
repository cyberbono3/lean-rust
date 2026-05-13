# runtime-api

Lean HTTP API + Prometheus metrics (Tier 6).

## Status

**Scaffold only.** The crate exists for workspace wiring and to
hold the Cargo.toml dep that `node` will use; the actual handlers
land in a later issue.

## Planned scope

- Lean HTTP API endpoints (head, block-by-root, state-by-root,
  config) backed by `runtime_chain::ChainSnapshot`.
- Prometheus `/metrics` endpoint with the leanmetrics namespace.
- Hosted on the [`runtime_core::Server`] shell — same axum router
  shape, same `CancellationToken` shutdown contract.
- Implements [`runtime_core::Service`] (start / stop / status).

[`runtime_core::Server`]: ../core/src/httpsvc/
[`runtime_core::Service`]: ../core/src/service.rs

## Tier and dependencies

Tier 6. Will depend on `runtime-core` (for the HTTP shell and
`Service` trait), `runtime-chain` (for the snapshot read path),
`prometheus`, `axum`, `serde_json`.

## Issue reference

See [`lean-rust-github-issues.md`] for the deliverables checklist
(API endpoints + Prometheus exposition).

[`lean-rust-github-issues.md`]: ../../../.claude/prompts/lean-rust-github-issues.md
