#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEVNET_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ENV_FILE="$DEVNET_ROOT/.env"
CONTAINER_DEVNET_ROOT="/data"

if [[ -f "$ENV_FILE" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
fi

REAM_IMAGE="${REAM_IMAGE:-ethpandaops/ream:master-0bceaee}"
LEAN_RUST_IMAGE="${LEAN_RUST_IMAGE:-lean-rust:local}"
GENESIS_GEN_IMAGE="${GENESIS_GEN_IMAGE:-ethpandaops/eth-beacon-genesis:pk910-leanchain}"
GENESIS_OFFSET_SECS="${GENESIS_OFFSET_SECS:-60}"

REAM_NODE_ID="ream_0"
LEAN_RUST_NODE_ID="leanrust_1"
REAM_IP="172.20.0.10"
LEAN_RUST_IP="172.20.0.11"
P2P_PORT="9000"

KEYS_DIR="$DEVNET_ROOT/config/keys"
GENESIS_DIR="$DEVNET_ROOT/genesis"
CONFIG_FILE="$DEVNET_ROOT/config.yaml"
VALIDATOR_CONFIG="$GENESIS_DIR/validator-config.yaml"
RUST_BOOTNODES="$GENESIS_DIR/bootnodes.rust.yaml"
NODE0_KEY_FILE="$KEYS_DIR/node0.key"
NODE1_KEY_FILE="$KEYS_DIR/node1.key"
GENESIS_STATE_FILE="$GENESIS_DIR/genesis.ssz"
GENESIS_JSON_FILE="$GENESIS_DIR/genesis.json"
GENESIS_CONFIG_FILE="$GENESIS_DIR/config.yaml"
NODES_FILE="$GENESIS_DIR/nodes.yaml"
VALIDATORS_FILE="$GENESIS_DIR/validators.yaml"

die() {
  echo "error: $*" >&2
  exit 1
}

require_file() {
  local path="$1"
  [[ -s "$path" ]] || die "expected non-empty file: $path"
}

assert_contains() {
  local needle="$1"
  local path="$2"
  grep -Fq -- "$needle" "$path" || die "expected $path to contain $needle"
}

yaml_entry_count() {
  local path="$1"
  grep -c '^- ' "$path" || true
}

key_contents() {
  local path="$1"
  tr -d '[:space:]' <"$path"
}

container_path() {
  local host_path="$1"
  case "$host_path" in
    "$DEVNET_ROOT"/*) echo "$CONTAINER_DEVNET_ROOT/${host_path#$DEVNET_ROOT/}" ;;
    *) die "path is outside devnet root: $host_path" ;;
  esac
}

docker_run_devnet() {
  docker run --rm -v "$DEVNET_ROOT:$CONTAINER_DEVNET_ROOT" "$@"
}

docker_run_devnet_readonly() {
  docker run --rm -v "$DEVNET_ROOT:$CONTAINER_DEVNET_ROOT:ro" "$@"
}

print_genesis_time() {
  local genesis_time="$1"
  local formatted
  if formatted="$(date -r "$genesis_time" '+%Y-%m-%d %H:%M:%S' 2>/dev/null)"; then
    echo "$formatted"
  elif formatted="$(date -d "@$genesis_time" '+%Y-%m-%d %H:%M:%S' 2>/dev/null)"; then
    echo "$formatted"
  else
    echo "$genesis_time"
  fi
}

derive_peer_id() {
  local key_path="$1"
  local peer_id

  peer_id="$(
    docker_run_devnet_readonly \
      "$LEAN_RUST_IMAGE" \
      peer-id --private-key-path "$(container_path "$key_path")"
  )"
  peer_id="$(printf '%s' "$peer_id" | tr -d '[:space:]')"
  [[ -n "$peer_id" ]] || die "failed to derive peer id from $key_path"

  echo "$peer_id"
}

ensure_peer_id_helper() {
  docker run --rm "$LEAN_RUST_IMAGE" peer-id --help >/dev/null 2>&1 \
    || die "$LEAN_RUST_IMAGE does not support 'peer-id'; rerun with FORCE=1 to rebuild it"
}

if [[ ! "$GENESIS_OFFSET_SECS" =~ ^[0-9]+$ ]]; then
  die "GENESIS_OFFSET_SECS must be a non-negative integer, got $GENESIS_OFFSET_SECS"
fi

echo "REAM_IMAGE=$REAM_IMAGE"
echo "LEAN_RUST_IMAGE=$LEAN_RUST_IMAGE"
echo "GENESIS_GEN_IMAGE=$GENESIS_GEN_IMAGE"
echo "GENESIS_OFFSET_SECS=$GENESIS_OFFSET_SECS"

"$SCRIPT_DIR/build-lean-rust.sh"
ensure_peer_id_helper

mkdir -p "$KEYS_DIR" "$GENESIS_DIR"

KEY_FILES=("$NODE0_KEY_FILE" "$NODE1_KEY_FILE")
for node in "${!KEY_FILES[@]}"; do
  key_file="${KEY_FILES[$node]}"
  echo "Generating private key for node${node}..."
  docker_run_devnet \
    "$REAM_IMAGE" \
    generate_private_key --output-path "$(container_path "$key_file")"
  require_file "$key_file"
done

NODE0_KEY="$(key_contents "$NODE0_KEY_FILE")"
NODE1_KEY="$(key_contents "$NODE1_KEY_FILE")"

cat >"$VALIDATOR_CONFIG" <<EOF
shuffle: roundrobin
validators:
  - name: "$REAM_NODE_ID"
    privkey: "$NODE0_KEY"
    enrFields:
      ip: "$REAM_IP"
      quic: $P2P_PORT
      seq: 1
    count: 1

  - name: "$LEAN_RUST_NODE_ID"
    privkey: "$NODE1_KEY"
    enrFields:
      ip: "$LEAN_RUST_IP"
      quic: $P2P_PORT
      seq: 1
    count: 1
EOF

GENESIS_TIME=$(($(date +%s) + GENESIS_OFFSET_SECS))
echo "Genesis time: $(print_genesis_time "$GENESIS_TIME") (now + ${GENESIS_OFFSET_SECS}s)"

cat >"$CONFIG_FILE" <<EOF
GENESIS_TIME: $GENESIS_TIME
VALIDATOR_COUNT: 0
EOF

echo "Running eth-beacon-genesis (leanchain)..."
docker_run_devnet \
  "$GENESIS_GEN_IMAGE" \
  leanchain \
  --config "$(container_path "$CONFIG_FILE")" \
  --mass-validators "$(container_path "$VALIDATOR_CONFIG")" \
  --state-output "$(container_path "$GENESIS_STATE_FILE")" \
  --json-output "$(container_path "$GENESIS_JSON_FILE")" \
  --nodes-output "$(container_path "$NODES_FILE")" \
  --validators-output "$(container_path "$VALIDATORS_FILE")" \
  --config-output "$(container_path "$GENESIS_CONFIG_FILE")"

REAM_PEER_ID="$(derive_peer_id "$NODE0_KEY_FILE")"
cat >"$RUST_BOOTNODES" <<EOF
- /ip4/$REAM_IP/udp/$P2P_PORT/quic-v1/p2p/$REAM_PEER_ID
EOF

require_file "$GENESIS_STATE_FILE"
require_file "$NODES_FILE"
require_file "$VALIDATORS_FILE"
require_file "$RUST_BOOTNODES"

ENR_COUNT="$(yaml_entry_count "$NODES_FILE")"
[[ "$ENR_COUNT" -eq 2 ]] || die "expected 2 ENRs in $NODES_FILE, found $ENR_COUNT"

BOOTNODE_COUNT="$(yaml_entry_count "$RUST_BOOTNODES")"
[[ "$BOOTNODE_COUNT" -eq 1 ]] || die "expected 1 Rust bootnode in $RUST_BOOTNODES, found $BOOTNODE_COUNT"

assert_contains "$REAM_NODE_ID" "$VALIDATORS_FILE"
assert_contains "$LEAN_RUST_NODE_ID" "$VALIDATORS_FILE"
assert_contains "/p2p/" "$RUST_BOOTNODES"

echo
echo "Genesis ready."
ls -la "$GENESIS_DIR"
