//! Shared fixtures for the p2p (`runtime::p2p`) integration tests.
//!
//! Placed under `tests/p2p_common/mod.rs` (not `tests/p2p_common.rs`) so Cargo
//! treats it as a module sibling rather than an extra test binary; each
//! test file pulls it in with `mod p2p_common;`.

// Every token accounted for, same standard as `tests/common/mod.rs`:
// - `dead_code` is load-bearing: three integration-test binaries declare this
//   module (`p2p_gossip`, `p2p_rpc`, `p2p_two_node_interop`), each compiles all
//   of it but calls only part — `build_service` / `build_service_with` are used
//   by the first two and not the third, so the lint fires per binary. Scoped to
//   that, NOT a licence to let orphaned helpers accumulate; re-audit as it grows.
// - `unwrap_used` covers the five `.unwrap()` calls below.
// - `expect_used` was dropped — there is no `.expect(...)` in this file.
#![allow(dead_code, clippy::unwrap_used)]

use std::path::Path;
use std::sync::Arc;

use runtime::p2p::{DevnetHost, HostOptions, P2pService, RpcProvider};
use tempfile::{tempdir, TempDir};

/// Loopback QUIC-v1 listen address with an ephemeral port. Every test
/// driving a `P2pService` binds here.
pub const TEST_LISTEN_ADDR: &str = "/ip4/127.0.0.1/udp/0/quic-v1";

/// Agent-version string used in test handshakes. Mirrors the value the
/// real binary advertises but with a stable test-only tag.
pub const TEST_AGENT_VERSION: &str = "test/0.1.0";

/// Builds `HostOptions` rooted at `dir`. Pass `bootnodes` when the test
/// needs to dial a peer; `None` produces an isolated single-node setup.
pub fn options_in(dir: &Path, bootnodes: Option<&Path>) -> HostOptions {
    HostOptions::try_new(
        TEST_LISTEN_ADDR,
        TEST_AGENT_VERSION,
        &dir.join("id"),
        bootnodes,
    )
    .unwrap()
}

/// Builds a `P2pService` rooted at a fresh `TempDir`. The directory is
/// returned alongside the service so the caller can keep it alive for
/// the duration of the test (`HostOptions` references it).
pub fn build_service() -> (TempDir, P2pService) {
    let dir = tempdir().unwrap();
    let service = DevnetHost::build(options_in(dir.path(), None)).unwrap();
    (dir, service)
}

/// Like [`build_service`] but wires the given [`RpcProvider`] instead
/// of the default [`RpcProvider::NoOp`].
pub fn build_service_with(provider: Arc<RpcProvider>) -> (TempDir, P2pService) {
    let dir = tempdir().unwrap();
    let service = DevnetHost::build_with_provider(options_in(dir.path(), None), provider).unwrap();
    (dir, service)
}
