# local-pq Devnet0

This guide covers the Docker-based local-pq devnet that runs one `ream` node
and one `lean-rust` node from this repo. The devnet lives under
`crates/fixtures` and uses generated local-pq keys, genesis state, validator
registry, and bootnode metadata.

## Prerequisites

- Docker with Compose support.
- A working Rust toolchain for building `lean-rust:local`.
- Optional local overrides in `crates/fixtures/.env`.

Create the local environment file before the first run:

```sh
cp crates/fixtures/.env.example crates/fixtures/.env
```

The default images are:

| Variable | Default |
| --- | --- |
| `REAM_IMAGE` | `ethpandaops/ream:master-0bceaee` |
| `LEAN_RUST_IMAGE` | `lean-rust:local` |
| `GENESIS_GEN_IMAGE` | `ethpandaops/eth-beacon-genesis:pk910-leanchain` |
| `GENESIS_OFFSET_SECS` | `60` |
| `LEAN_RUST_RUST_LOG` | `info,lean_beacon=debug,node=debug,engine=debug,lean_core=debug,runtime_p2p=debug,lean_chain=debug,lean_duties=debug,lean_api=debug,networking=debug,libp2p_swarm=info,discv5=info` |

## Quick Start

```sh
cp crates/fixtures/.env.example crates/fixtures/.env
make devnet-start
make devnet-status
make devnet-logs
make devnet-stop
```

## Commands

| Command | Purpose |
| --- | --- |
| `make devnet-build` | Build the `LEAN_RUST_IMAGE` Docker image, default `lean-rust:local`. |
| `make devnet-genesis` | Generate local-pq keys, genesis files, validator registry, node metadata, and the Rust bootnode adapter. |
| `make devnet-up` | Start the `ream` and `lean-rust` containers with Docker Compose. |
| `make devnet-start` | Run build, genesis generation, and compose startup in order. |
| `make devnet-status` | Probe both `/lean/v0/head` compatibility endpoints. |
| `make devnet-down` | Stop containers and remove Compose orphans while keeping generated state. |
| `make devnet-stop` | Safe stop alias for `devnet-down`; succeeds when nothing is running. |
| `make devnet-clean` | Stop Compose, remove volumes and orphans, and delete generated keys, genesis files, adapter files, and logs. |
| `make devnet-clean-check` | Create generated-state sentinels and verify `make devnet-clean` removes generated files while preserving scaffold files. |
| `make devnet-logs` | Follow both devnet containers. |
| `make devnet-logs-ream` | Follow only the ream node logs. |
| `make devnet-logs-lean` | Follow only the lean-rust node logs. |
| `make devnet-debug-summary` | Print high-signal log markers from Compose and Rust file logs. |
| `make devnet-smoke-head-sample` | Sample ream/Rust `/lean/v0/head` compatibility. |

## Topology

Docker Compose file:
`crates/fixtures/scripts/core/docker-compose.yml`

| Node | Service | Container | Image | Role | Container IP | Host ports | Head URL |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `ream_0` | `node0` | `ream-node0` | `${REAM_IMAGE:-ethpandaops/ream:master-0bceaee}` | ream validator/node, genesis bootnode source | `172.20.0.10` | UDP `9000`, HTTP `5052`, metrics `8080` | `http://127.0.0.1:5052/lean/v0/head` |
| `leanrust_1` | `node1` | `lean-rust-node1` | `${LEAN_RUST_IMAGE:-lean-rust:local}` | Rust node using generated local-pq state | `172.20.0.11` | UDP `9001` to container `9000`, HTTP `5053` to container `5052`, metrics `8081` to container `8080` | `http://127.0.0.1:5053/lean/v0/head` |

Both services run on the `pq-devnet` bridge network with subnet
`172.20.0.0/24`. The ream node reads `/genesis/nodes.yaml` as its bootnodes
file. The Rust node reads `/genesis/bootnodes.rust.yaml`.

## Generated Artifacts

| Path | Created by | Used by | Removed by `make devnet-clean` |
| --- | --- | --- | --- |
| `crates/fixtures/config.yaml` | `make devnet-genesis` | genesis generator input | yes |
| `crates/fixtures/config/keys/node0.key` | `make devnet-genesis` | ream node identity and validator config | yes |
| `crates/fixtures/config/keys/node1.key` | `make devnet-genesis` | lean-rust node identity and validator config | yes |
| `crates/fixtures/genesis/config.yaml` | genesis generator | both nodes as network config | yes |
| `crates/fixtures/genesis/genesis.json` | genesis generator | inspection/debugging | yes |
| `crates/fixtures/genesis/genesis.ssz` | genesis generator | lean-rust genesis state | yes |
| `crates/fixtures/genesis/nodes.yaml` | genesis generator | ream bootnodes/local-pq node metadata | yes |
| `crates/fixtures/genesis/validators.yaml` | genesis generator | both nodes as validator registry | yes |
| `crates/fixtures/genesis/validator-config.yaml` | `make devnet-genesis` | genesis generator mass validator input | yes |
| `crates/fixtures/genesis/bootnodes.rust.yaml` | `make devnet-genesis` | lean-rust `--bootnodes` input | yes |
| `crates/fixtures/logs/*.log` | lean-rust container logging | debugging | yes |

Scaffold files such as `config/keys/.gitkeep`, `genesis/.gitkeep`,
`logs/.gitkeep`, and `scripts/core/.gitkeep` are preserved by
`make devnet-clean`.

## Bootnode Compatibility

Rust currently uses the generated
`crates/fixtures/genesis/bootnodes.rust.yaml` adapter. The setup script
derives the ream node peer ID from `config/keys/node0.key` with the Rust image
and writes a multiaddr bootnode entry:

```text
/ip4/172.20.0.10/udp/9000/quic-v1/p2p/<peer-id>
```

This adapter is generated from the same local-pq node identity material used by
the ream/local-pq genesis flow. Do not treat it as proof that Rust directly
accepts every ENR shape emitted in `genesis/nodes.yaml`; the adapter is the
compatibility boundary for this devnet.

## Status

`make devnet-status` probes:

```text
ream node0:      http://127.0.0.1:5052/lean/v0/head
lean-rust node1: http://127.0.0.1:5053/lean/v0/head
```

The status script normalizes common checkpoint fields when `jq` is available.
If a node is still starting, the probe prints `(unreachable)` and exits
successfully so it can be reused while waiting for the devnet to settle.

## Logs

The Rust node receives this default filter through `RUST_LOG`:

```text
info,lean_beacon=debug,node=debug,engine=debug,lean_core=debug,runtime_p2p=debug,lean_chain=debug,lean_duties=debug,lean_api=debug,networking=debug,libp2p_swarm=info,discv5=info
```

`RUST_LOG` is the highest-precedence tracing input for `lean-beacon`. When
`LEAN_RUST_RUST_LOG` is non-empty, it overrides `--log-level` and `--debug`.
Use `--log-level` only for direct local runs where `RUST_LOG` is unset.

Raise all Rust logs temporarily:

```sh
LEAN_RUST_RUST_LOG=trace make devnet-up
```

Raise p2p/networking diagnostics:

```sh
LEAN_RUST_RUST_LOG=lean_beacon=debug,runtime_p2p=trace,networking=trace make devnet-up
```

Raise chain/duties diagnostics:

```sh
LEAN_RUST_RUST_LOG=lean_beacon=debug,engine=debug,lean_chain=debug,lean_duties=debug,node=debug make devnet-up
```

Raise API/storage diagnostics:

```sh
LEAN_RUST_RUST_LOG=lean_api=trace,storage=debug make devnet-up
```

Follow both containers:

```sh
make devnet-logs
```

Compare Ream and Rust logs in separate terminals:

```sh
make devnet-logs-ream
make devnet-logs-lean
```

Print a compact marker summary:

```sh
make devnet-debug-summary
```

The Rust container also writes file logs under:

```text
crates/fixtures/logs/lean-rust-<utc>.log
```

Logs include configured identity paths and peer IDs, but not raw private key
bytes.

## Cleanup

Stop containers while preserving generated state:

```sh
make devnet-stop
```

Remove containers, volumes, generated files, and logs:

```sh
make devnet-clean
```

If behavior changes around generated state, run:

```sh
make devnet-clean-check
```

The check refuses to run when existing generated state is present, creates only
sentinel generated files, runs `make devnet-clean` with Docker cleanup skipped,
and verifies scaffold files remain.

## Troubleshooting

### Genesis Decode Failure

Symptoms:

- lean-rust exits during startup after reading `/genesis/genesis.ssz`.
- `make devnet-status` reaches ream but not lean-rust.

Checks:

```sh
test -s crates/fixtures/genesis/genesis.ssz
make devnet-debug-summary
```

Regenerate state with:

```sh
make devnet-clean
make devnet-genesis
```

### secp256k1 Identity Mismatch

Symptoms:

- Rust dials a peer ID that ream does not own.
- Handshake or peer identity logs disagree with the generated bootnode entry.

Checks:

```sh
cat crates/fixtures/config/keys/node0.key
cat crates/fixtures/genesis/bootnodes.rust.yaml
```

Regenerate keys and bootnodes together. Do not edit individual generated files
by hand:

```sh
make devnet-clean
make devnet-genesis
```

### ENR Parse Or Bootnode Adapter Failure

Symptoms:

- `make devnet-genesis` fails before Compose starts.
- `bootnodes.rust.yaml` is empty or missing `/p2p/`.
- Rust reports invalid bootnode input.

Checks:

```sh
test -s crates/fixtures/genesis/nodes.yaml
test -s crates/fixtures/genesis/bootnodes.rust.yaml
grep -n "/p2p/" crates/fixtures/genesis/bootnodes.rust.yaml
```

Rebuild the Rust image if the `peer-id` helper is missing or stale:

```sh
FORCE=1 make devnet-build
make devnet-genesis
```

### Peers Connected But No Gossip

Symptoms:

- Containers are running and peer connections appear in logs.
- Blocks or votes do not propagate.
- Rust logs publish failures or insufficient mesh peers.

Checks:

```sh
make devnet-debug-summary
```

Confirm both nodes use the same generated network config and validator
registry:

```sh
test -s crates/fixtures/genesis/config.yaml
test -s crates/fixtures/genesis/validators.yaml
```

### Rust Proposal Not Imported By ream

Symptoms:

- lean-rust logs proposal production or publication.
- ream does not advance to the expected head.

Checks:

```sh
make devnet-status
docker compose -f crates/fixtures/scripts/core/docker-compose.yml \
  --project-directory crates/fixtures logs node0
```

Verify the validator registry contains both local-pq node IDs:

```sh
grep -n "ream_0\\|leanrust_1" crates/fixtures/genesis/validators.yaml
```

### Head Divergence

Symptoms:

- Both `/lean/v0/head` endpoints respond.
- `head`, `finalized`, `latest_justified`, or `safe_target` differ.

Checks:

```sh
make devnet-status
curl -s http://127.0.0.1:5052/lean/v0/head
curl -s http://127.0.0.1:5053/lean/v0/head
```

Check for recent startup churn first. If divergence persists, inspect both
logs and regenerate state from a clean devnet:

```sh
make devnet-clean
make devnet-start
```

### Stale Docker Image Or Generated State

Symptoms:

- CLI flags documented in this repo are rejected by the container.
- Genesis files do not match the current scripts.

Checks and recovery:

```sh
FORCE=1 make devnet-build
make devnet-clean
make devnet-start
```
