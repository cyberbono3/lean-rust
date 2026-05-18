# local-pq Devnet0 Smoke Validation

This report records end-to-end smoke validation for the Docker
`ream <-> lean-rust` local-pq devnet.

## Latest Run

| Field | Value |
| --- | --- |
| Result | Blocked before container startup |
| Time UTC | `2026-05-18T17:15:39Z` |
| Branch | `test/run-record-e2e-devnet-smoke-validation` |
| Commit | `49ca609` plus local Issue #13 smoke-script/docs changes |
| Docker client | `Docker Engine - Community 28.5.0` |
| Blocker | Docker daemon calls hung; guarded devnet build reported the daemon was not reachable within 5 seconds. |

The smoke could not prove runtime acceptance criteria in this environment
because Docker did not return from daemon-backed commands. Both sandboxed and
escalated `docker ps` checks hung and were stopped manually before running the
devnet.

Concrete startup attempt:

```sh
DOCKER_CHECK_TIMEOUT_SECONDS=5 make devnet-start
```

Output:

```text
/Library/Developer/CommandLineTools/usr/bin/make devnet-build
crates/pq-devnet-0/scripts/core/build-lean-rust.sh
error: docker daemon is not reachable within 5s
make[1]: *** [devnet-build] Error 1
make: *** [devnet-start] Error 2
```

## Smoke Protocol

Run from a clean checkout with Docker available:

```sh
make devnet-clean
cp crates/pq-devnet-0/.env.example crates/pq-devnet-0/.env
make devnet-start
sleep 75
make devnet-status
curl --fail http://127.0.0.1:5052/lean/v0/head
curl --fail http://127.0.0.1:5053/lean/v0/head
curl --fail http://127.0.0.1:8080/metrics
curl --fail http://127.0.0.1:8081/metrics
make devnet-smoke-head-sample
docker logs ream-node0 2>&1 | grep -Ei 'peer|gossip|block|vote'
docker logs lean-rust-node1 2>&1 | grep -Ei 'peer|gossip|status|block|vote'
make devnet-clean
```

Use `make devnet-clean` for the destructive cleanup check. `make devnet-stop`
is intentionally a safe stop alias that keeps generated state.

## Evidence Checklist

| Check | Status | Evidence |
| --- | --- | --- |
| `make devnet-start` builds `lean-rust:local` if missing, generates fresh genesis, and starts both containers | Blocked | Docker daemon did not become reachable. |
| `make devnet-status` reaches both nodes after genesis warmup | Not run | Requires running containers. |
| ream and Rust load the same generated config, genesis state, validator registry, and node list | Not run | Requires generated state from `make devnet-start`. |
| Rust peer ID matches node1 identity encoded by genesis | Not run | Requires generated keys and genesis metadata. |
| Rust establishes QUIC/libp2p connectivity to ream | Not run | Requires running containers. |
| Rust subscribes to devnet0 block and vote gossip topics | Not run | Requires Rust container logs. |
| Rust imports at least one ream-produced block | Not run | Requires ream/Rust runtime logs. |
| ream imports at least one Rust-produced block | Not run | Requires ream/Rust runtime logs. |
| `head.root` and `finalized.root` agree across both nodes for 10 consecutive samples | Not run | Use `make devnet-smoke-head-sample` after warmup. |
| Metrics scrape successfully on host ports `8080` and `8081` | Not run | Requires running containers. |
| cleanup removes containers, volumes, generated files, and logs without removing Docker images | Not run | Use `make devnet-clean` after smoke. |

## Generated State Checks

After `make devnet-start`, record:

```sh
test -s crates/pq-devnet-0/config.yaml
test -s crates/pq-devnet-0/genesis/genesis.ssz
test -s crates/pq-devnet-0/genesis/validators.yaml
test -s crates/pq-devnet-0/genesis/nodes.yaml
test -s crates/pq-devnet-0/genesis/bootnodes.rust.yaml
grep -n "ream_0\|leanrust_1" crates/pq-devnet-0/genesis/validators.yaml
grep -n "/p2p/" crates/pq-devnet-0/genesis/bootnodes.rust.yaml
```

## Head Agreement Sampling

`make devnet-smoke-head-sample` prints a markdown table and exits successfully
only after it observes the configured number of consecutive matching samples.
Defaults:

| Variable | Default |
| --- | --- |
| `PQ_DEVNET_SMOKE_MATCHES` | `10` |
| `PQ_DEVNET_SMOKE_MAX_ATTEMPTS` | same as `PQ_DEVNET_SMOKE_MATCHES` |
| `PQ_DEVNET_SMOKE_INTERVAL_SECONDS` | `12` |
| `PQ_DEVNET_SMOKE_CURL_MAX_TIME_SECONDS` | `3` |

Example shorter diagnostic run:

```sh
PQ_DEVNET_SMOKE_MATCHES=2 \
PQ_DEVNET_SMOKE_MAX_ATTEMPTS=6 \
PQ_DEVNET_SMOKE_INTERVAL_SECONDS=3 \
make devnet-smoke-head-sample
```

## Follow-Up

- Re-run the smoke on a host where `docker ps` returns normally.
- Replace this blocked result with command snippets, head agreement samples,
  metrics snippets, and log markers from a successful run.
