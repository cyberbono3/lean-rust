#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
ENV_FILE="$REPO_ROOT/crates/pq-devnet-0/.env"
ENV_EXAMPLE="$REPO_ROOT/crates/pq-devnet-0/.env.example"
FOLLOW_LOGS=1
AUTO_STOP=1

usage() {
  cat <<EOF
Usage: $(basename "$0") [--no-logs] [--no-stop] [-h|--help]

Runs the pq-devnet-0 Quick Start sequence:
  1. cp crates/pq-devnet-0/.env.example crates/pq-devnet-0/.env (if missing)
  2. make devnet-start
  3. make devnet-status
  4. make devnet-logs   (blocks until Ctrl+C; skip with --no-logs)
  5. make devnet-stop   (runs on exit; skip with --no-stop)

Options:
  --no-logs   Skip step 4 (do not follow container logs).
  --no-stop   Skip step 5 (leave containers running on exit).
  -h, --help  Show this help.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-logs) FOLLOW_LOGS=0 ;;
    --no-stop) AUTO_STOP=0 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "[quick-start.sh] ERROR: unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

cd "$REPO_ROOT"

stop_devnet() {
  if [[ "$AUTO_STOP" == "1" ]]; then
    echo
    echo "[quick-start.sh] INFO: stopping devnet (make devnet-stop)"
    make devnet-stop || echo "[quick-start.sh] WARN: devnet-stop returned non-zero" >&2
  else
    echo "[quick-start.sh] SKIP: --no-stop set; leaving containers running"
  fi
}
trap stop_devnet EXIT

if [[ ! -f "$ENV_FILE" ]]; then
  echo "[quick-start.sh] INFO: copying $ENV_EXAMPLE -> $ENV_FILE"
  cp "$ENV_EXAMPLE" "$ENV_FILE"
else
  echo "[quick-start.sh] SKIP: $ENV_FILE already exists"
fi

make devnet-start
make devnet-status

if [[ "$FOLLOW_LOGS" == "1" ]]; then
  echo "[quick-start.sh] INFO: following devnet logs — Ctrl+C to stop devnet"
  make devnet-logs || true
else
  echo "[quick-start.sh] SKIP: --no-logs set; not following logs"
fi
