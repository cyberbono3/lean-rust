#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEVNET_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GENESIS_CONFIG_FILE="$DEVNET_ROOT/genesis/config.yaml"
MIN_LEAD_SECS="${PQ_DEVNET_MIN_GENESIS_LEAD_SECS:-10}"

die() {
  echo "error: $*" >&2
  exit 1
}

if [[ ! "$MIN_LEAD_SECS" =~ ^[0-9]+$ ]]; then
  die "PQ_DEVNET_MIN_GENESIS_LEAD_SECS must be a non-negative integer, got $MIN_LEAD_SECS"
fi

[[ -s "$GENESIS_CONFIG_FILE" ]] || die "missing generated genesis config: $GENESIS_CONFIG_FILE"

GENESIS_TIME="$(
  awk -F: '
    /^GENESIS_TIME:/ {
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", $2);
      print $2;
      exit;
    }
  ' "$GENESIS_CONFIG_FILE"
)"

[[ "$GENESIS_TIME" =~ ^[0-9]+$ ]] \
  || die "could not read numeric GENESIS_TIME from $GENESIS_CONFIG_FILE"

NOW="$(date +%s)"
REMAINING=$((GENESIS_TIME - NOW))

if ((REMAINING < MIN_LEAD_SECS)); then
  die "generated genesis time is too close or stale: now=$NOW genesis_time=$GENESIS_TIME remaining=${REMAINING}s minimum=${MIN_LEAD_SECS}s. Rerun 'make devnet-genesis' or set a larger GENESIS_OFFSET_SECS."
fi

echo "Genesis time check passed: ${REMAINING}s remaining before genesis."
