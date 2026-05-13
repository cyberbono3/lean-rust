# runtime-p2p

libp2p QUIC-v1 host: gossipsub + request/response (Tier 6).

## Status

**Scaffold only.** The crate exists for workspace wiring and to
hold the Cargo.toml dep that `node` will use; the actual host
construction, gossip topic registration, message-id hook, and
req/resp handlers land in Issue #31.

## Planned scope

- libp2p QUIC-v1 host construction with the deterministic 20-byte
  gossipsub message-id (`compute_gossipsub_message_id`).
- `block` and `vote` gossip topic handlers.
- `BlocksByRoot` + `Status` request/response.
- Public `publish_block` / `publish_attestation` async API used by
  the `node`-level `runtime-duties::Publisher` adapter (Issue #37).
- Adapter `impl runtime_chain::sync::Network for Service` for the
  sync loop.

## Tier and dependencies

Tier 6. Will depend on `runtime-core`, `runtime-chain`, `networking`,
`protocol`, `libp2p` (with `quic-v1` + `gossipsub` + `request-response`
features), plus the standard async stack.

## Issue reference

Implements Issue #31. See [`lean-rust-github-issues.md`] for the
deliverables checklist.

[`lean-rust-github-issues.md`]: ../../../.claude/prompts/lean-rust-github-issues.md
