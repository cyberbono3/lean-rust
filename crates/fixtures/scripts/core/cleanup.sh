#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEVNET_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
COMPOSE_FILE="$DEVNET_ROOT/scripts/core/docker-compose.yml"
SKIP_DOCKER_CLEANUP="${PQ_DEVNET_SKIP_DOCKER_CLEANUP:-0}"
COMPOSE_ARGS=(
  -f "$COMPOSE_FILE"
  --project-directory "$DEVNET_ROOT"
)

source "$SCRIPT_DIR/devnet-paths.sh"

warn() {
  printf 'warning: %s\n' "$*" >&2
}

cleanup_compose() {
  if [[ "$SKIP_DOCKER_CLEANUP" == "1" ]]; then
    warn "PQ_DEVNET_SKIP_DOCKER_CLEANUP=1; skipping container and volume cleanup"
    return
  fi

  if ! command -v docker >/dev/null 2>&1; then
    warn "docker not found; skipping container and volume cleanup"
    return
  fi

  docker compose "${COMPOSE_ARGS[@]}" down -v --remove-orphans \
    || warn "docker compose cleanup failed; continuing with file cleanup"
}

remove_generated_files() {
  rm -f "${PQ_DEVNET_GENERATED_PATHS[@]}"
}

cleanup_compose
remove_generated_files

echo "pq-devnet-0 generated state removed."
