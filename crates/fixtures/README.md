# fixtures

Crate-local Docker devnet plus compatibility fixtures for the local-pq
`ream <-> lean-rust` topology. The crate ships:

- Rust fixture helpers (`src/lib.rs`) used by other crates' contract tests.
- Integration tests under `tests/` that pin cross-client artifact shapes.
- A Docker Compose devnet under `scripts/core/` that runs one `ream` node and
  one `lean-rust` node against generated local-pq genesis state.
- A `Dockerfile` that builds the `lean-rust:local` runtime image used by the
  devnet (`make devnet-build`).

The full operator guide lives at
[`docs/local-pq-devnet0.md`](../../docs/local-pq-devnet0.md) and is the source
of truth for commands, topology, generated artifacts, and troubleshooting.
This README is a crate-level orientation.

## Quick Start
Please make sure Docker is started.

From the repo root, run the wrapper target that copies `.env`, starts the
devnet, probes both head endpoints, follows logs until you hit Ctrl+C, then
stops the containers:

```sh
make devnet-quick-start
```

Pass flags via `DEVNET_QUICK_START_ARGS`: `--no-logs` skips the blocking
`devnet-logs` step, `--no-stop` leaves the containers running on exit, and
`-h` / `--help` prints usage. Example:

```sh
make devnet-quick-start DEVNET_QUICK_START_ARGS="--no-logs --no-stop"
```

The target is a thin wrapper around the equivalent manual sequence:

```sh
cp crates/fixtures/.env.example crates/fixtures/.env
make devnet-start
make devnet-status
make devnet-logs
make devnet-stop
```

`make devnet-clean` removes generated keys, genesis artifacts, logs,
containers, and Docker volumes while preserving scaffold `.gitkeep` files.

## Layout

| Path | Purpose |
| --- | --- |
| `src/lib.rs` | Public fixture path helpers and stable peer-ID constants. |
| `tests/compatibility_contract.rs` | Cross-client validator/genesis/bootnode contract tests. |
| `tests/debug_summary.rs` | Coverage for `scripts/core/debug-summary.sh` markers. |
| `tests/vote_checkpoint_compare.rs` | Coverage for the `compare-vote-checkpoints.sh` smoke. |
| `tests/fixtures/` | Pinned 2-node validator registry, SSZ genesis, secp256k1 keys, Rust bootnodes adapter. See [`tests/fixtures/README.md`](tests/fixtures/README.md). |
| `scripts/core/` | Devnet shell scripts: `quick-start.sh`, `setup-genesis.sh`, `build-lean-rust.sh`, `status.sh`, `cleanup.sh`, `debug-summary.sh`, `smoke-head-sample.sh`, `compare-vote-checkpoints.sh`, `check-genesis-time.sh`, `check-cleanup.sh`, `devnet-paths.sh`, and `docker-compose.yml`. |
| `config/keys/` | Generated `node0.key` / `node1.key` (removed by `make devnet-clean`). |
| `genesis/` | Generated `config.yaml`, `genesis.{json,ssz}`, `nodes.yaml`, `validators.yaml`, `bootnodes.rust.yaml`, `lean-rust-devnet0.yaml`, `validator-config.yaml`, `genesis_validators.yaml` (public pubkey manifest), and `secrets/` (per-validator XMSS attestation secret keys — gitignored, never committed). |
| `logs/` | `lean-rust-<utc>.log` files written by the Rust container. |
| `Dockerfile` | Builds `lean-rust:local` from the workspace `beacon` binary. |
| `.env.example` | Default image tags and `RUST_LOG` filter. Copy to `.env`. |

## Topology

Two services on the `pq-devnet` bridge (`172.20.0.0/24`):

| Node | Container | Image | IPv4 | Host ports | Head |
| --- | --- | --- | --- | --- | --- |
| `ream_0` | `ream-node0` | `${REAM_IMAGE:-ethpandaops/ream:master-0bceaee}` | `172.20.0.10` | UDP `9000`, HTTP `5052`, metrics `8080` | `http://127.0.0.1:5052/lean/v0/head` |
| `leanrust_1` | `lean-rust-node1` | `${LEAN_RUST_IMAGE:-lean-rust:local}` | `172.20.0.11` | UDP `9001→9000`, HTTP `5053→5052`, metrics `8081→8080` | `http://127.0.0.1:5053/lean/v0/head` |

`node1` depends on `node0`. Both mount `./config:/config:ro` and
`./genesis:/genesis:ro`; `node1` also mounts `./logs` for file logs.

## Make Targets

Defined in the root [`Makefile`](../../Makefile):

| Target | Purpose |
| --- | --- |
| `devnet-build` | Build the `lean-rust:local` image. |
| `devnet-genesis` | Generate keys, genesis files, validator registry, node metadata, and the Rust bootnode adapter. |
| `devnet-up` / `devnet-down` | Start / stop containers (keeps generated state). |
| `devnet-stop` | Safe alias for `devnet-down` (succeeds when nothing is running). |
| `devnet-start` | `devnet-build` + `devnet-genesis` + `devnet-up`. |
| `devnet-quick-start` | Wrap `quick-start.sh`: `.env` + `devnet-start` + `devnet-status` + `devnet-logs` (Ctrl+C stops). Pass flags via `DEVNET_QUICK_START_ARGS` (`--no-logs`, `--no-stop`, `--help`). |
| `devnet-status` | Probe both `/lean/v0/head` endpoints. |
| `devnet-logs` / `devnet-logs-ream` / `devnet-logs-lean` | Follow combined or per-node container logs. |
| `devnet-debug-summary` | Print high-signal log markers. |
| `devnet-smoke-head-sample` | Sample `/lean/v0/head` compatibility between nodes. |
| `devnet-smoke-vote-checkpoints` | Compare Ream/Rust vote source-target checkpoints. |
| `devnet-clean` | Remove containers, volumes, generated files, and logs. |
| `devnet-clean-check` | Verify `devnet-clean` removes generated state only. |

## Rust Public API

`src/lib.rs` exposes fixture paths and stable identifiers consumed by other
crates' contract tests:

| Item | Description |
| --- | --- |
| `REAM_0_RAW_SECP256K1_KEY_FIXTURE` / `LEANRUST_1_RAW_SECP256K1_KEY_FIXTURE` | Filenames of the raw hex secp256k1 keys under `tests/fixtures/`. |
| `RUST_BOOTNODES_2NODE_FIXTURE` | Filename of the 2-node Rust bootnodes adapter fixture. |
| `REAM_0_PEER_ID` / `LEANRUST_1_PEER_ID` | Stable libp2p peer IDs derived from the fixture keys. |
| `REAM_0_BOOTNODE_ADDR` | Dialable multiaddr prefix for `ream_0` before `/p2p/<peer-id>`. |
| `fixture_path(name)` | Resolve a fixture filename under `tests/fixtures/`. |
| `ream_0_raw_secp256k1_key_path()` / `leanrust_1_raw_secp256k1_key_path()` / `rust_bootnodes_2node_path()` | Convenience accessors for the above fixtures. |

The crate has no runtime dependencies; fixture consumers live in
`[dev-dependencies]`.

## Configuration

Defaults are documented in `.env.example` and the operator guide. The most
common override is `LEAN_RUST_RUST_LOG`, which sets `RUST_LOG` on the
`lean-rust-node1` container. When non-empty it overrides `--log-level` and
`--debug` on `lean-beacon` — use `--log-level` only for direct local runs
where `RUST_LOG` is unset.

```sh
LEAN_RUST_RUST_LOG=trace make devnet-up
```

This crate's devnet validates the repo-local local-pq flow, ream image
compatibility, generated genesis, Docker topology, and the `/lean/v0/head`
compatibility route. See
[`docs/local-pq-devnet0.md`](../../docs/local-pq-devnet0.md).
