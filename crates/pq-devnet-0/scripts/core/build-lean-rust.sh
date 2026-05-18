#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEVNET_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
REPO_ROOT="$(cd "$DEVNET_ROOT/../.." && pwd)"
ENV_FILE="$DEVNET_ROOT/.env"

if [[ -f "$ENV_FILE" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
fi

LEAN_RUST_IMAGE="${LEAN_RUST_IMAGE:-lean-rust:local}"
DOCKER_CHECK_TIMEOUT_SECONDS="${DOCKER_CHECK_TIMEOUT_SECONDS:-15}"
DOCKERFILE="$DEVNET_ROOT/Dockerfile"

run_with_timeout() {
  local timeout_seconds="$1"
  shift

  "$@" &
  local pid="$!"
  local deadline=$((SECONDS + timeout_seconds))

  while kill -0 "$pid" 2>/dev/null; do
    if ((SECONDS >= deadline)); then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
      return 124
    fi
    sleep 0.2
  done

  wait "$pid"
}

if ! command -v docker >/dev/null 2>&1; then
  echo "error: docker is required to build $LEAN_RUST_IMAGE" >&2
  exit 1
fi

if ! run_with_timeout "$DOCKER_CHECK_TIMEOUT_SECONDS" docker info >/dev/null 2>&1; then
  echo "error: docker daemon is not reachable within ${DOCKER_CHECK_TIMEOUT_SECONDS}s" >&2
  exit 1
fi

if [[ "${FORCE:-}" != "1" ]] \
  && run_with_timeout "$DOCKER_CHECK_TIMEOUT_SECONDS" docker image inspect "$LEAN_RUST_IMAGE" >/dev/null 2>&1; then
  echo "image $LEAN_RUST_IMAGE already exists; set FORCE=1 to rebuild"
  exit 0
fi

docker build \
  --file "$DOCKERFILE" \
  --tag "$LEAN_RUST_IMAGE" \
  "$REPO_ROOT"
