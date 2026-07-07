# Metrics

Prometheus metrics exposed by the node over HTTP.

## Overview

- **Format:** Prometheus text exposition, served over HTTP.
- **Always on:** there is **no feature flag or build flag** to enable metrics. The
  node composition root (`new_devnet`) unconditionally registers every metric and
  starts the metrics listener as one of the node's services.
- **Endpoint:** path `/metrics` on the metrics listen address.
- **Default address:** `127.0.0.1:9090`.
- **The `--metrics` CLI flag is a no-op**, accepted only for launcher
  compatibility — metrics are wired regardless.

## Accessing the endpoint

- **Scrape it directly:**
  ```
  curl -s http://127.0.0.1:9090/metrics
  ```
- **Filter to the chain-tick trigger histograms:**
  ```
  curl -s http://127.0.0.1:9090/metrics \
    | grep -E 'lean_state_transition_time_seconds|lean_fork_choice_block_processing_time_seconds'
  ```
- **Change the bind address/port:**
  - `--metrics-address <ip>` (default `127.0.0.1`)
  - `--metrics-port <port>` (default `9090`)
- **Render cache:** each scrape body is reused for **1 second** before the next
  request re-renders, so frequent scrapes cost at most one full render per second
  (invisible to typical 5–15 s Prometheus scrape intervals).
- **Dashboards:** point a Prometheus server at
  `http://<node-host>:9090/metrics`; graph from there (Grafana, etc.). No auth is
  applied — bind to a private interface or firewall the port in shared
  environments.

## Exposed metrics

### Process / baseline (gauges)

- `lean_node_up` — `1` while the node is running.
- `lean_node_start_time_seconds` — Unix start time of the process.

### Chain state (gauges, sampled per scrape)

- `lean_chain_slot` — current fork-choice slot (the clock).
- `lean_chain_justified_slot` — slot of the latest justified checkpoint.
- `lean_chain_finalized_slot` — slot of the latest finalized checkpoint.

### Chain-tick trigger histograms

- `lean_state_transition_time_seconds` — wall time of a full state transition per
  imported block, measured at the runtime boundary.
  - Trigger for the **per-block `State`-clone** performance lever.
- `lean_fork_choice_block_processing_time_seconds` — wall time of the fork-choice
  head recompute (`accept_new_votes`) on the block-import path.
  - Trigger for the **incremental fork-choice** and **prune-below-finalized**
    levers.
- **Bucket scale (both):** `0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0,
  2.0, 4.0` seconds.
- **Observation scope (both):** recorded **only when a block is fully accepted**
  on the import path (after the state transition and fork-choice recompute both
  succeed). Rejected/errored imports record **no** sample.

### Deferred (documented, not wired)

- `lean_state_transition_slots_processing_time_seconds` — HTR / process-slots
  sub-phase timing. **Not exposed:** it measures a sub-phase inside the core state
  transition, and splitting it would require timing inside `protocol`, which the
  boundary-only instrumentation rule forbids. `lean_state_transition_time_seconds`
  (whole transition) is the coarse proxy.

## When the metrics populate

- **Gauges** are sampled fresh on every scrape (they call a provider closure), so
  they always reflect the latest chain state.
- **Histograms** are registered into the per-scrape registry immediately, so the
  `_bucket` / `_sum` / `_count` series appear **right away with `_count 0`**, then
  accumulate as blocks are accepted.
- A self-driving devnet node produces and imports a block each slot, so the trigger
  histograms populate within a few slots of startup — no manual traffic needed.

## How it is wired (for maintainers)

- **Boundary-only instrumentation.** Timing wraps calls into `protocol` /
  `forkchoice` at the runtime chain-tick boundary (`transition_and_track` in the
  importer). The `protocol` and `forkchoice` transition code stays free of
  metrics / time / RNG.
- **Observation-only timing.** The `Instant` reads never influence control flow,
  the returned value, or store state — they only feed the histogram observations.
- **Handle model.** `ChainMetrics` holds `Arc`-backed histogram handles. The
  composition root builds them via `register_chain_histograms` (mirroring
  `register_chain_gauges`) and injects them into the engine with
  `Engine::with_metrics`. The metrics registry is rebuilt per scrape and
  re-registers a clone of each cumulative histogram, so counts survive across
  scrapes.
- **No-op default.** `ChainMetrics::default()` holds absent handles and observes
  nothing; it is used only by unit tests and benchmarks, never by a running node.
- **Decoupling.** Metric names and buckets are owned at the composition root, so
  the `node` crate names no `prometheus` item and stays free of that dependency.

## Using the trigger histograms

- These histograms are the **evidence gate** for the deferred-performance levers:
  a lever is implemented **only when its trigger metric shows growth**, never
  proactively.
- To open a lever, copy `docs/perf/deferred-lever-child-template.md` into a new
  perf child issue and attach the trigger-metric graph showing the regression
  before making any change.
