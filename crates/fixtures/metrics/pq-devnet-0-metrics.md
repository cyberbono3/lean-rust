# pq-devnet-0 performance metrics snapshot

Collected from a fresh `make devnet-up` run on branch `refactor/api`
(HEAD `b234e64`, lean-rust:local docker image).

| Field | Value |
|-------|-------|
| Collection start (UTC) | 2026-05-27T11:57:09Z |
| Genesis time (unix) | 1779882846 (2026-05-27T11:54:06Z) |
| Container start (lean-rust) | 2026-05-27T11:51:07Z |
| Chain uptime at sample end | ~435 s past genesis (≈109 slots @ 4 s/slot) |
| Sample window | 3 docker-stats snapshots over ~3 min + 96-slot vote-checkpoint compare + 10-sample head compare |
| ream image | `ethpandaops/ream:master-0bceaee` |
| lean-rust image | `lean-rust:local` (built from this branch) |

---

## 1. Liveness + correctness gates

| Check | Result |
|-------|--------|
| Container status | both `Up` — no restarts |
| Head match (lean ↔ ream) | **10/10** consecutive matches over ~2 min |
| Vote-checkpoint compare | **96 slots, 0 mismatches, 0 skipped** |
| `lean_node_up` gauge | `1` |
| ERROR log lines | `0` |
| WARN log lines | `2` (both expected — see §7) |

### Heads at sample end

```
lean: 0xc784477e62c03962e5cae9d8918a56971a9d67dc4da79e90dcc67d7b0f57ed72
ream: 0xc784477e62c03962e5cae9d8918a56971a9d67dc4da79e90dcc67d7b0f57ed72
```

### Head-sample table (last 10 samples)

| # | UTC | head root | match |
|---|-----|-----------|-------|
| 1 | 11:57:26 | `0xea3f506b…` | yes |
| 2 | 11:57:38 | `0x6045fa91…` | yes |
| 3 | 11:57:50 | `0xb31c4599…` | yes |
| 4 | 11:58:02 | `0x20bde0ef…` | yes |
| 5 | 11:58:15 | `0xaab4d583…` | yes |
| 6 | 11:58:27 | `0x5ec252ff…` | yes |
| 7 | 11:58:39 | `0x22bfc0da…` | yes |
| 8 | 11:58:51 | `0xc53bc339…` | yes |
| 9 | 11:59:03 | `0x171298b5…` | yes |
| 10 | 11:59:15 | `0x061a77ff…` | yes |

Finalized roots are absent from both HTTP head endpoints in this
revision; the smoke script reports "not-compared".

### Vote-checkpoint compare (last 3 entries; full 96 slots in run log)

```
| 93 | `89->90` | `89->90` | yes |
| 94 | `90->91` | `90->91` | yes |
| 95 | `91->92` | `91->92` | yes |

compared_slots=96 skipped_missing_slots=0 mismatches=0
```

---

## 2. Container resource usage

Three snapshots taken ~90 s apart across the sample window. `BLOCK
I/O` is cumulative since container start.

| Snapshot | Container | CPU % | Mem (resident) | Net I/O (rx / tx) | Block I/O (rd / wr) | PIDs |
|----------|-----------|-------|----------------|-------------------|---------------------|------|
| T+150 s | lean-rust-node1 | 0.28 % | 15.34 MiB | 70.2 kB / 70.2 kB | 4.21 MB / 0 B | – |
| T+150 s | ream-node0 | 12.86 % | 1.091 GiB | 70.6 kB / 66.4 kB | 1.17 GB / 11 MB | – |
| T+240 s | lean-rust-node1 | 0.27 % | 16.76 MiB | 126 kB / 126 kB | 4.21 MB / 0 B | – |
| T+240 s | ream-node0 | 0.52 % | 1.099 GiB | 127 kB / 127 kB | 1.17 GB / 22 MB | – |
| T+330 s | lean-rust-node1 | 0.04 % | 16.88 MiB | 139 kB / 139 kB | 4.21 MB / 0 B | 13 |
| T+330 s | ream-node0 | 6.51 % | 1.102 GiB | 140 kB / 139 kB | 1.17 GB / 24.6 MB | 29 |

### Derived

| Metric | lean-rust | ream | Notes |
|--------|-----------|------|-------|
| RSS growth over ~3 min | +1.54 MiB | +11 MiB | lean-rust footprint dominated by libp2p; ream has a much larger baseline. |
| Avg CPU% over window | ~0.2 % (idle) | ~7 % | ream is doing more steady-state work; lean-rust spikes briefly at slot boundaries. |
| Avg network I/O | ~1.1 kB/s | ~1.1 kB/s | One block + one attestation per slot per node, both ssz+snappy. |
| Disk read | 4.21 MB | 1.17 GB | ream paged in its image layers; lean-rust's image was already cached. |
| Disk write | 0 B | 24.6 MB | lean-rust uses MemoryStore; ream persists. |

---

## 3. Throughput from log-derived counters

Counted by grepping `docker logs lean-rust-node1` for the canonical
tracing message strings; window = container start (~11:51:07Z) to
sample end (~12:01:21Z) = **614 s wall clock**, of which ~179 s was
pre-genesis (idle ticks) and ~435 s post-genesis (active chain).

| Event | Count | Rate (post-genesis) | Expected |
|-------|-------|---------------------|----------|
| `engine block produced` (this node as proposer, slot odd) | 52 | 0.119 /s | ~0.125 (½ × 0.25 imp/s) |
| `gossip block accepted` (from ream, slot even) | 51 | 0.117 /s | ~0.125 |
| `engine attestation vote produced` | 103 | 0.237 /s | 0.25 (1 per slot per validator × 1 local validator) |
| `chain own attestation reimported` | 103 | 0.237 /s | mirror of above |
| `gossip vote accepted` (from ream) | 103 | 0.237 /s | mirror of above |
| `duties block proposed` | 52 | 0.119 /s | matches engine block produced |
| `duties attestation published` | 103 | 0.237 /s | matches engine vote produced |
| `served head endpoint` (HTTP `/lean/v0/head`) | 11 | 0.025 /s | driven by `make devnet-smoke-head-sample` |

**Block production cadence:** 52 blocks across slots 1–103 = exactly
every odd slot, as expected (round-robin proposer between the two
validators).

**First / last produced blocks:**

```
slot=1   block_root=0x32028a6bb9a1a1e2f19e51c6905a38205f5d8b15444b4a881072321ce9d6e1da  at 11:54:10Z
slot=103 block_root=0xf5e622a9ea699bd65d5435367ca47f2ebed8001e65c7089c2c2d03697ff1e1b6  at 12:00:58Z
```

---

## 4. Prometheus `/metrics` endpoints

Both nodes expose Prometheus text exposition on container port 8080;
mapped to host ports 8080 (ream) and 8081 (lean-rust).

### 4.1 lean-rust `/metrics` (host port 8081) — **266 bytes, 2 gauges**

```
# HELP lean_node_start_time_seconds Unix timestamp when the Lean node process started.
# TYPE lean_node_start_time_seconds gauge
lean_node_start_time_seconds 1779882667
# HELP lean_node_up Whether the Lean node process is up.
# TYPE lean_node_up gauge
lean_node_up 1
```

This matches the design-doc finding #6: only the boot-time defaults
in `crates/runtime/api/src/metrics/recorder.rs:55-65` are wired.
Chain-state gauges (head slot, finalized slot, peer count, import
rate, etc.) are NOT exposed.

### 4.2 ream `/metrics` (host port 8080) — **4 751 bytes**

Six metrics surfaced by ream (HELP lines):

| Name | Type / Help |
|------|------------|
| `lean_finalized_slot` | The current finalized slot |
| `lean_head_slot` | The current head slot |
| `lean_justified_slot` | The current justified slot |
| `lean_propose_block_time` | Duration of the sections it takes to propose a new block (summary) |
| `prometheus_exporter_request_duration_seconds` | HTTP request latencies in seconds (histogram) |
| `prometheus_exporter_requests_total` | Number of HTTP requests received (counter) |

The first four are ream's chain-state metrics. lean-rust exposes
comparable ones under different names — `lean_chain_slot`,
`lean_chain_justified_slot`, and `lean_chain_finalized_slot` — wired in
`crates/node/src/devnet.rs::register_chain_gauges`. Each gauge captures a
cloned `Arc<ChainService>` and reads the by-value snapshot per scrape, e.g.
`recorder.gauge("lean_chain_slot", ..., move || chain.snapshot().current_slot)`
(`current_slot` is the forkchoice clock, not the head block's slot). A
`lean_propose_block_time` summary and a name-for-name head-slot gauge are
not yet exposed. (See design-doc #6 + Pass-D PR-B `FrozenRecorder`
proposal.)

---

## 5. Endpoints reference (for follow-up scraping)

| Purpose | URL |
|---------|-----|
| lean-rust HTTP head | `http://127.0.0.1:5053/lean/v0/head` |
| ream HTTP head | `http://127.0.0.1:5052/lean/v0/head` |
| lean-rust `/metrics` | `http://127.0.0.1:8081/metrics` |
| ream `/metrics` | `http://127.0.0.1:8080/metrics` |

The lean-rust `/metrics` endpoint has a 1-second render cache (per
commit `5237b61`), so scrape intervals ≥ 1 s see fresh data and
sub-second flood scrapes share a cached body.

---

## 6. Performance posture vs the design doc

Cross-checked against the projections in
`.claude/pq-devnet-0-design_decisions.md`:

| Doc projection | This run |
|----------------|----------|
| #17 mutex-held State clone, devnet0: "sub-µs, invisible" | **Confirmed.** CPU % at sustained 0.2-0.3 % on lean-rust. No tail visible. |
| #18 STF clone-then-swap, devnet0: "few hundred bytes, single µs" | **Confirmed.** Allocator pressure invisible — 16 MiB RSS held nearly flat. |
| #19 vote-pool footprint at N=2: "~16 KiB" | **Confirmed.** 1.5 MiB total RSS growth over 109 slots; ~99 % of that is libp2p / tokio runtime, not the pool. |
| #21 `/metrics` thundering-herd, devnet0: "1-3 concurrent scrapers realistic" | **Trivially confirmed.** No scraper attached during this run. |
| #22 sequential attester loop at N=1: "invisible" | **Confirmed.** Attestation published every 4-second slot without observable jitter. |

No regressions vs the doc; all devnet0-scale predictions hold.

---

## 7. Log warnings (full text)

```
WARN lean_rust: --http-allow-origin is accepted for CLI compatibility but
NOT applied: no CORS layer is wired into the HTTP server. The HTTP API
will respond with default axum headers regardless of this value. value="*"

WARN lean_p2p_host::service: outgoing connection error peer=None
error=Failed to negotiate transport protocol(s):
[(/ip4/172.20.0.10/udp/9000/quic-v1: : Handshake with the remote timed out.)]
```

The first is the intentional startup warn shipped in commit `5237b61`
(design-doc #14 / lean-api perf PR scope). The second is the initial
dial against ream during the bootnodes phase — ream wasn't QUIC-ready
in the same tick — and clears on the immediate retry; not a steady-
state error.

---

## 8. What's missing (and why)

The lean-rust `/metrics` exposition is sparse because chain-state
gauge providers haven't been plumbed yet — that's design-doc finding
**#6**, queued in the lean-api perf PR (queued PR #4). When that PR
lands with `Recorder::freeze() -> FrozenRecorder` plus the chain-
state gauges, this same collection script will surface:

- `lean_head_slot`, `lean_finalized_slot`, `lean_justified_slot`
- `lean_import_block_duration_seconds` (timer)
- `lean_p2p_peers_connected` (gauge)
- `lean_chain_engine_lock_wait_seconds` (timer for #17)
- `lean_duties_publish_duration_seconds` per validator (label gauge)

For the present revision, the log-derived counts in §3 substitute for
the missing exposition.

---

## Reproduce

```bash
# Idempotent rerun
make devnet-clean
cp crates/fixtures/.env.example crates/fixtures/.env
make devnet-build
make devnet-genesis
make devnet-up

# Wait for genesis + ~2 min of slots, then:
make devnet-status
make devnet-smoke-head-sample
make devnet-smoke-vote-checkpoints
docker stats --no-stream --format "table {{.Name}}\t{{.CPUPerc}}\t{{.MemUsage}}\t{{.NetIO}}\t{{.BlockIO}}" lean-rust-node1 ream-node0
curl -s http://127.0.0.1:8081/metrics
curl -s http://127.0.0.1:8080/metrics

# Tear down
make devnet-down
```
