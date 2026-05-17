# Devnet0 Interop Verification

Devnet0 interop verification is automated by:

```sh
make interop-devnet0
```

The target runs `scripts/devnet0-interop.sh`, which starts one `lean-go`
node and one `lean-rust` node on loopback, connects Rust to the generated
Go bootnode, waits for the verification window, and compares both head
endpoints.

## Prerequisites

- `cargo`
- `go`
- `curl`
- `python3`
- a local `lean-go` checkout

By default the script expects:

```sh
LEAN_GO_DIR=/Users/ai/go/src/github.com/cyberbono3/lean-go
```

Override it when needed:

```sh
make interop-devnet0 LEAN_GO_DIR=/path/to/lean-go
```

## What The Script Does

1. Builds `target/release/lean-beacon`.
2. Builds the Go beacon binary into the interop artifact directory.
3. Generates a deterministic Go node key for this run.
4. Writes a local-pq config matching the checked-in Go genesis fixture unless
   `GO_GENESIS_CONFIG` is provided.
5. Derives the Go peer ID from that key.
6. Writes a Rust bootnodes file pointing at the Go node.
7. Starts Go on QUIC `9000`, HTTP `5053`, metrics `9091`.
8. Starts Rust on QUIC `9001`, HTTP `5052`, metrics `9090`.
9. Polls both head endpoints:
   - Go: `/lean/v0/head`
   - Rust: `/eth/v1/head`
10. Waits `INTEROP_DURATION_SECONDS` seconds.
11. Compares final `head` and `finalized` checkpoints.
12. Fails if processes exit early, head endpoints are unreadable, checkpoints
    differ, Rust misses the devnet0 gossip topics, Rust publishes without mesh
    peers, or panic-style log markers are found.

## Artifacts

Each run writes artifacts under:

```text
target/interop/devnet0/<timestamp>/
```

Expected files:

```text
go.log
rust.log
go-node.key
go-local-pq-config.yaml
go-bootnodes.yaml
rust-bootnodes.yaml
go-head.json
rust-head.json
summary.md
```

## Useful Overrides

```sh
INTEROP_DURATION_SECONDS=120 make interop-devnet0
```

```sh
GO_P2P_PORT=9100 RUST_P2P_PORT=9101 \
GO_HTTP_PORT=5153 RUST_HTTP_PORT=5152 \
GO_METRICS_PORT=9191 RUST_METRICS_PORT=9190 \
make interop-devnet0
```

## Pass Criteria

The target exits `0` when:

- both clients start successfully;
- Rust dials the Go bootnode over QUIC-v1;
- Rust logs a successful status handshake;
- Rust subscribes to the devnet0 block and vote gossip topics;
- Rust publish attempts do not report `InsufficientPeers`;
- both head endpoints return JSON;
- normalized `head` and `finalized` checkpoints match;
- no panic, unwrap, or backtrace markers appear in logs;
- both child processes are cleaned up.
