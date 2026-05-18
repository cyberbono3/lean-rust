#!/usr/bin/env bash
set -euo pipefail
shopt -s nullglob

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEVNET_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
REPO_ROOT="$(cd "$DEVNET_ROOT/../.." && pwd)"

source "$SCRIPT_DIR/devnet-paths.sh"

SENTINEL_PATHS=(
  "$DEVNET_ROOT/config.yaml"
  "$DEVNET_ROOT/genesis/config.yaml"
  "$DEVNET_ROOT/genesis/genesis.json"
  "$DEVNET_ROOT/genesis/genesis.ssz"
  "$DEVNET_ROOT/genesis/nodes.yaml"
  "$DEVNET_ROOT/genesis/validators.yaml"
  "$DEVNET_ROOT/genesis/validator-config.yaml"
  "$DEVNET_ROOT/genesis/bootnodes.rust.yaml"
  "$DEVNET_ROOT/config/keys/cleanup-check.key"
  "$DEVNET_ROOT/logs/cleanup-check.log"
)

print_relative_paths() {
  local path

  for path in "$@"; do
    printf '  %s\n' "${path#$REPO_ROOT/}"
  done
}

ensure_no_generated_state() {
  local existing_state=()
  local path

  for path in "${PQ_DEVNET_GENERATED_PATHS[@]}"; do
    if [[ -e "$path" ]]; then
      existing_state+=("$path")
    fi
  done

  if ((${#existing_state[@]} == 0)); then
    return
  fi

  printf 'error: refusing cleanup check because generated state already exists:\n' >&2
  print_relative_paths "${existing_state[@]}" >&2
  exit 1
}

create_sentinels() {
  local path

  mkdir -p "$DEVNET_ROOT/config/keys" "$DEVNET_ROOT/genesis" "$DEVNET_ROOT/logs"

  for path in "${SENTINEL_PATHS[@]}"; do
    printf 'cleanup-check\n' >"$path"
  done
}

cleanup_sentinels() {
  rm -f "${SENTINEL_PATHS[@]}"
}

run_devnet_clean() {
  (
    cd "$REPO_ROOT"
    PQ_DEVNET_SKIP_DOCKER_CLEANUP=1 "${MAKE:-make}" devnet-clean
  )
}

assert_sentinels_removed() {
  local missing_cleanup=()
  local path

  for path in "${SENTINEL_PATHS[@]}"; do
    if [[ -e "$path" ]]; then
      missing_cleanup+=("$path")
    fi
  done

  if ((${#missing_cleanup[@]} == 0)); then
    return
  fi

  printf 'error: devnet-clean left generated state behind:\n' >&2
  print_relative_paths "${missing_cleanup[@]}" >&2
  exit 1
}

assert_scaffold_preserved() {
  local missing_scaffold=()
  local path

  for path in "${PQ_DEVNET_SCAFFOLD_PATHS[@]}"; do
    if [[ ! -f "$path" ]]; then
      missing_scaffold+=("$path")
    fi
  done

  if ((${#missing_scaffold[@]} == 0)); then
    return
  fi

  printf 'error: devnet-clean removed scaffold files:\n' >&2
  print_relative_paths "${missing_scaffold[@]}" >&2
  exit 1
}

ensure_no_generated_state
create_sentinels
trap cleanup_sentinels EXIT

run_devnet_clean
assert_sentinels_removed
assert_scaffold_preserved

trap - EXIT
echo "devnet-clean scenario passed."
