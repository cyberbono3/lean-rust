# lean-core

Lifecycle spine for the runtime shell (Tier 5).

## Scope

- [`Service`] — async trait every runtime service implements
  (`start`, `stop`, `status`, `name`). All Tier-6 services
  (`chain`, `p2p`, `sync`, `duties`, `http`, `metrics`) implement it.
- [`Node`] — composition root holding up to six [`Service`] slots
  with ordered start (`chain → p2p → sync → duties → http → metrics`),
  reverse-ordered stop, and start-time unwinding on failure.
- [`NodeConfig`] — narrow process-level configuration (shutdown
  timeout, defaulted via `DEFAULT_SHUTDOWN_TIMEOUT`).
- [`NodeError`] / [`ServiceFailure`] — typed lifecycle errors with
  the offending slot label preserved.
- [`Server`] — shared HTTP shell that binds a TCP listener, serves
  an `axum::Router`, and terminates on a `CancellationToken`. Reused
  by `lean-api` for the Lean HTTP API and Prometheus metrics.
- Observability helpers: [`init_tracing`], [`FileSink`],
  [`Verbosity`], [`TracingGuard`].

[`Service`]: ./src/service.rs
[`Node`]: ./src/node.rs
[`NodeConfig`]: ./src/config.rs
[`NodeError`]: ./src/error.rs
[`ServiceFailure`]: ./src/error.rs
[`Server`]: ./src/httpsvc/
[`init_tracing`]: ./src/observability/
[`FileSink`]: ./src/observability/
[`Verbosity`]: ./src/observability/
[`TracingGuard`]: ./src/observability/

## What this crate is not

No business logic. No engine, no networking, no validator state.
Service implementations land in their own crates (`lean-chain`,
`lean-duties`, etc.); this crate carries the bare lifecycle
contract they implement.

## Tier and dependencies

Tier 5. Depends on `types`, `protocol`, `tokio`, `tokio-util`,
`async-trait`, `tracing`, `axum`. No `runtime/*` deps.

## Design notes

- **Ordered start, reverse stop.** The slot ordering matches the
  upstream Go-client composition: chain must be up before sync /
  duties can drive it; p2p must be up before sync subscribes to
  peer-connect events.
- **Start-time unwinding.** A failure in slot N triggers reverse-
  ordered stop for slots 0..N. Partial start never leaves the node
  in a half-running state.
- **`CancellationToken` shutdown budget.** `Service::stop(cancel)`
  receives a parent token; the slot has until cancellation to drain
  its background tasks gracefully, then `Drop` cancels the slot's
  own token as a fallback.
