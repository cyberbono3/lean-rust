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
