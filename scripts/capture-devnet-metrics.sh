#!/usr/bin/env bash
#
# capture-devnet-metrics.sh — sample + reduce the pq-devnet-0 metrics gate.
#
# Assumes a devnet is already up (`make devnet-up`). Takes N docker-stats
# snapshots of the lean-rust container, reduces them the way the fixture does
# (steady RSS = last snapshot, avg CPU = mean of all snapshots), counts
# ERROR/WARN log lines, measures the /metrics exposition, and appends one JSON
# line to the runs log for `.claude/pq-devnet-0-perf-report.md`.
#
# Usage:   scripts/capture-devnet-metrics.sh <phase-label>
# Example: scripts/capture-devnet-metrics.sh phase-0-baseline
#
# Tunables (env): LEAN_CONTAINER, REAM_CONTAINER, LEAN_METRICS_URL,
# SNAPSHOTS (default 3), INTERVAL_SECONDS (default 90), OUT (jsonl path).

set -euo pipefail

PHASE="${1:-unlabelled}"
LEAN_CONTAINER="${LEAN_CONTAINER:-lean-rust-node1}"
REAM_CONTAINER="${REAM_CONTAINER:-ream-node0}"
LEAN_METRICS_URL="${LEAN_METRICS_URL:-http://127.0.0.1:8081/metrics}"
SNAPSHOTS="${SNAPSHOTS:-3}"
INTERVAL_SECONDS="${INTERVAL_SECONDS:-90}"
OUT="${OUT:-.claude/pq-devnet-0-perf-runs.jsonl}"

log() { echo "[capture-devnet-metrics] $1: ${2}" >&2; }

command -v jq >/dev/null 2>&1 || { log FATAL "jq is required"; exit 1; }
docker inspect "$LEAN_CONTAINER" >/dev/null 2>&1 || {
  log FATAL "container '$LEAN_CONTAINER' not found — run 'make devnet-up' first"; exit 1; }

# Normalise a docker-stats memory token (e.g. "16.88MiB", "1.1GiB") to MiB.
to_mib() {
  awk '{ v=$0; sub(/[A-Za-z]+$/,"",v); u=substr($0, length(v)+1);
         if (u=="GiB") v*=1024; else if (u=="KiB") v/=1024;
         else if (u=="B") v/=1048576; printf "%.2f", v }' <<<"$1"
}

log INFO "sampling $SNAPSHOTS snapshot(s) of $LEAN_CONTAINER every ${INTERVAL_SECONDS}s"

cpu_sum=0
rss_last=0
net_last=""
for i in $(seq 1 "$SNAPSHOTS"); do
  line="$(docker stats --no-stream \
    --format '{{.CPUPerc}};{{.MemUsage}};{{.NetIO}}' "$LEAN_CONTAINER")"
  cpu="${line%%;*}"; cpu="${cpu%\%}"
  rest="${line#*;}"
  mem="${rest%%;*}"; mem_used="${mem%% *}"
  net="${rest#*;}"
  rss_mib="$(to_mib "$mem_used")"
  cpu_sum="$(awk -v a="$cpu_sum" -v b="$cpu" 'BEGIN{printf "%.4f", a+b}')"
  rss_last="$rss_mib"
  net_last="$net"
  log INFO "snapshot $i/$SNAPSHOTS cpu=${cpu}% rss=${rss_mib}MiB net=${net}"
  [ "$i" -lt "$SNAPSHOTS" ] && sleep "$INTERVAL_SECONDS"
done
cpu_mean="$(awk -v s="$cpu_sum" -v n="$SNAPSHOTS" 'BEGIN{printf "%.3f", s/n}')"

error_lines="$(docker logs "$LEAN_CONTAINER" 2>&1 | grep -cE ' ERROR ' || true)"
warn_lines="$(docker logs "$LEAN_CONTAINER" 2>&1 | grep -cE ' WARN ' || true)"

metrics_body="$(curl -s "$LEAN_METRICS_URL" || true)"
metrics_bytes="$(printf '%s' "$metrics_body" | wc -c | tr -d ' ')"
metrics_gauges="$(printf '%s\n' "$metrics_body" | grep -cE '^# TYPE .* gauge' || true)"

git_sha="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

jq -nc \
  --arg phase "$PHASE" --arg ts "$ts" --arg sha "$git_sha" \
  --argjson rss "$rss_last" --argjson cpu "$cpu_mean" \
  --arg net "$net_last" --argjson errs "$error_lines" --argjson warns "$warn_lines" \
  --argjson mbytes "$metrics_bytes" --argjson mgauges "$metrics_gauges" \
  '{phase:$phase, utc:$ts, git_sha:$sha,
    lean_rss_mib_last:$rss, lean_cpu_pct_mean:$cpu, lean_net_io:$net,
    error_lines:$errs, warn_lines:$warns,
    metrics_bytes:$mbytes, metrics_gauges:$mgauges}' \
  | tee -a "$OUT"

log PASS "appended one run to $OUT"
