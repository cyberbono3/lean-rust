#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEVNET_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
COMPOSE_FILE="$SCRIPT_DIR/docker-compose.yml"
LOG_DIR="$DEVNET_ROOT/logs"

PATTERNS=(
  "startup configuration"
  "genesis config|genesis state|synthesized genesis|loaded genesis|engine constructed|engine block|engine attestation|engine tick|engine anchor"
  "constructed libp2p host|loaded bootnodes|bootnode|listener up"
  "connection established|connection closed|status handshake"
  "subscribed to gossipsub topic|gossipsub message|gossip .*accepted|gossip .*rejected"
  "chain .*persisted|chain .*attestation|duties block|duties attestation|block publish failed|attestation publish failed"
  "/lean/v0/head|served head|head not yet set"
)

print_matches() {
  local label="$1"
  local source="$2"
  local combined_pattern

  printf '\n== %s ==\n' "$label"
  combined_pattern="$(IFS='|'; printf '%s' "${PATTERNS[*]}")"
  if ! grep -E -i "$combined_pattern" "$source"; then
    printf '(no high-signal markers found)\n'
  fi
}

count_pattern() {
  local source="$1"
  local pattern="$2"

  grep -E -i -c "$pattern" "$source" 2>/dev/null || true
}

classify_logs() {
  local ream_logs="$1"
  local lean_logs="$2"
  local ream_warn ream_error ream_duplicate
  local lean_warn lean_error lean_rejected status_timeouts
  local classification="pass"

  ream_warn="$(count_pattern "$ream_logs" '(^|[[:space:]])WARN[[:space:]].*ream_')"
  ream_error="$(count_pattern "$ream_logs" '(^|[[:space:]])ERROR[[:space:]].*ream_')"
  ream_duplicate="$(count_pattern "$ream_logs" 'Publish (block|vote) failed.*Duplicate')"
  lean_warn="$(count_pattern "$lean_logs" '(^|[[:space:]])WARN[[:space:]]')"
  lean_error="$(count_pattern "$lean_logs" '(^|[[:space:]])ERROR[[:space:]]')"
  lean_rejected="$(count_pattern "$lean_logs" 'gossip .*rejected|rejected (block|vote|attestation)')"
  status_timeouts="$(count_pattern "$lean_logs" 'status rpc outbound timeout')"

  if [[ "$lean_warn" -gt 0 || "$lean_error" -gt 0 || "$lean_rejected" -gt 0 || "$ream_error" -gt 0 ]]; then
    classification="fail"
  elif [[ "$ream_warn" -gt 0 || "$ream_duplicate" -gt 0 || "$status_timeouts" -gt 0 ]]; then
    classification="pass-with-known-reference-noise"
  fi

  printf '\n== smoke classification ==\n'
  printf 'classification=%s\n' "$classification"
  printf 'ream_warn=%s ream_error=%s ream_duplicate_publish=%s\n' \
    "$ream_warn" \
    "$ream_error" \
    "$ream_duplicate"
  printf 'lean_warn=%s lean_error=%s lean_rejected=%s status_timeouts=%s\n' \
    "$lean_warn" \
    "$lean_error" \
    "$lean_rejected" \
    "$status_timeouts"
}

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

COMPOSE_LOGS="$TMP_DIR/compose.log"
if docker compose -f "$COMPOSE_FILE" --project-directory "$DEVNET_ROOT" logs --no-color >"$COMPOSE_LOGS"; then
  print_matches "compose logs" "$COMPOSE_LOGS"
else
  printf 'warning: docker compose logs unavailable; continuing with file logs if present\n' >&2
fi

shopt -s nullglob
FILE_LOGS=("$LOG_DIR"/*.log)
if [[ "${#FILE_LOGS[@]}" -eq 0 ]]; then
  printf '\n== file logs ==\n(no log files found under %s)\n' "$LOG_DIR"
else
  MERGED_FILE_LOGS="$TMP_DIR/file.log"
  cat "${FILE_LOGS[@]}" >"$MERGED_FILE_LOGS"
  print_matches "file logs" "$MERGED_FILE_LOGS"
fi

REAM_CONTAINER_LOGS="$TMP_DIR/ream-container.log"
LEAN_RUST_CONTAINER_LOGS="$TMP_DIR/lean-rust-container.log"
if docker logs ream-node0 >"$REAM_CONTAINER_LOGS" 2>/dev/null \
  && docker logs lean-rust-node1 >"$LEAN_RUST_CONTAINER_LOGS" 2>/dev/null; then
  classify_logs "$REAM_CONTAINER_LOGS" "$LEAN_RUST_CONTAINER_LOGS"
else
  printf '\n== smoke classification ==\n'
  printf 'classification=unknown\n'
  printf 'container logs unavailable\n'
fi
