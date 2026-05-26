#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

LEAN_GO_DIR="${LEAN_GO_DIR:-/Users/ai/go/src/github.com/cyberbono3/lean-go}"
INTEROP_DURATION_SECONDS="${INTEROP_DURATION_SECONDS:-60}"
ARTIFACT_ROOT="${ARTIFACT_ROOT:-$REPO_ROOT/target/interop/devnet0}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
ARTIFACT_DIR="${ARTIFACT_DIR:-$ARTIFACT_ROOT/$STAMP}"

BIND_ADDRESS="${BIND_ADDRESS:-127.0.0.1}"
P2P_ADDRESS="${P2P_ADDRESS:-127.0.0.1}"
GO_P2P_PORT="${GO_P2P_PORT:-9000}"
RUST_P2P_PORT="${RUST_P2P_PORT:-9001}"
GO_HTTP_PORT="${GO_HTTP_PORT:-5053}"
RUST_HTTP_PORT="${RUST_HTTP_PORT:-5052}"
GO_METRICS_PORT="${GO_METRICS_PORT:-9091}"
RUST_METRICS_PORT="${RUST_METRICS_PORT:-9090}"

GO_HEAD_PATH="${GO_HEAD_PATH:-/lean/v0/head}"
RUST_HEAD_PATH="${RUST_HEAD_PATH:-/eth/v1/head}"
GO_NODE_ID="${GO_NODE_ID:-ream_0}"
RUST_NODE_ID="${RUST_NODE_ID:-qlean_4}"
GO_INTEROP_LOG_LEVEL="${GO_INTEROP_LOG_LEVEL:-debug}"
RUST_INTEROP_LOG="${RUST_INTEROP_LOG:-debug}"
BLOCK_TOPIC="${BLOCK_TOPIC:-/leanconsensus/devnet0/block/ssz_snappy}"
VOTE_TOPIC="${VOTE_TOPIC:-/leanconsensus/devnet0/vote/ssz_snappy}"

GO_FIXTURE_DIR="${GO_FIXTURE_DIR:-$LEAN_GO_DIR/internal/testdata/devnet0}"
GO_GENESIS_CONFIG="${GO_GENESIS_CONFIG:-}"
GO_GENESIS_STATE="${GO_GENESIS_STATE:-$GO_FIXTURE_DIR/local_pq_genesis.ssz}"
GO_VALIDATORS="${GO_VALIDATORS:-$GO_FIXTURE_DIR/local_pq_validators.yaml}"
GO_GENESIS_TIME="${GO_GENESIS_TIME:-1777370673}"
GO_VALIDATOR_COUNT="${GO_VALIDATOR_COUNT:-6}"

RUST_BIN="${RUST_BIN:-$REPO_ROOT/target/release/lean-beacon}"
GO_BIN="${GO_BIN:-$ARTIFACT_DIR/lean-go-beacon}"

GO_LOG="$ARTIFACT_DIR/go.log"
RUST_LOG="$ARTIFACT_DIR/rust.log"
GO_BOOTNODES="$ARTIFACT_DIR/go-bootnodes.yaml"
RUST_BOOTNODES="$ARTIFACT_DIR/rust-bootnodes.yaml"
GO_KEY="$ARTIFACT_DIR/go-node.key"
PEER_HELPER="$ARTIFACT_DIR/peer_id_from_key.go"
GO_HEAD_JSON="$ARTIFACT_DIR/go-head.json"
RUST_HEAD_JSON="$ARTIFACT_DIR/rust-head.json"
SUMMARY="$ARTIFACT_DIR/summary.md"

GO_PID=""
RUST_PID=""
FAILURE_REASON=""
GO_PEER_ID=""

die() {
  FAILURE_REASON="$*"
  write_summary "fail" >/dev/null 2>&1 || true
  echo "error: $*" >&2
  echo "artifacts: $ARTIFACT_DIR" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

require_file() {
  [[ -f "$1" ]] || die "missing required file: $1"
}

assert_tcp_port_free() {
  local port="$1"
  python3 - "$BIND_ADDRESS" "$port" <<'PY' || die "TCP port $BIND_ADDRESS:$port is unavailable"
import socket
import sys

host = sys.argv[1]
port = int(sys.argv[2])
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
try:
    sock.bind((host, port))
except OSError as exc:
    raise SystemExit(f"tcp port {host}:{port} is unavailable: {exc}")
finally:
    sock.close()
PY
}

assert_udp_port_free() {
  local port="$1"
  python3 - "$P2P_ADDRESS" "$port" <<'PY' || die "UDP port $P2P_ADDRESS:$port is unavailable"
import socket
import sys

host = sys.argv[1]
port = int(sys.argv[2])
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
try:
    sock.bind((host, port))
except OSError as exc:
    raise SystemExit(f"udp port {host}:{port} is unavailable: {exc}")
finally:
    sock.close()
PY
}

write_peer_helper() {
  cat >"$PEER_HELPER" <<'GO'
package main

import (
	"encoding/hex"
	"fmt"
	"os"
	"strings"

	"github.com/libp2p/go-libp2p/core/crypto"
	"github.com/libp2p/go-libp2p/core/peer"
)

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: peer_id_from_key <hex-secp256k1-key>")
		os.Exit(2)
	}

	data, err := os.ReadFile(os.Args[1])
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	raw, err := hex.DecodeString(strings.TrimSpace(string(data)))
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	key, err := crypto.UnmarshalSecp256k1PrivateKey(raw)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	id, err := peer.IDFromPrivateKey(key)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	fmt.Println(id)
}
GO
}

is_running() {
  local pid="$1"
  local stat
  stat="$(ps -p "$pid" -o stat= 2>/dev/null || true)"
  [[ -n "$stat" && "$stat" != Z* ]]
}

stop_process() {
  local pid="$1"
  [[ -n "$pid" ]] || return 0
  is_running "$pid" || return 0

  kill -TERM "$pid" 2>/dev/null || return 0
  for _ in {1..20}; do
    is_running "$pid" || {
      wait "$pid" 2>/dev/null || true
      return 0
    }
    sleep 0.25
  done
  kill -KILL "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

cleanup() {
  local status=$?
  trap - EXIT INT TERM
  stop_process "$RUST_PID"
  stop_process "$GO_PID"
  exit "$status"
}

check_processes() {
  if [[ -n "$GO_PID" ]] && ! is_running "$GO_PID"; then
    wait "$GO_PID" 2>/dev/null || true
    die "lean-go exited before verification completed; see $GO_LOG"
  fi
  if [[ -n "$RUST_PID" ]] && ! is_running "$RUST_PID"; then
    wait "$RUST_PID" 2>/dev/null || true
    die "lean-rust exited before verification completed; see $RUST_LOG"
  fi
}

fetch_head() {
  local url="$1"
  local out="$2"
  local timeout_seconds="$3"
  local deadline
  deadline=$(( $(date +%s) + timeout_seconds ))

  while [[ "$(date +%s)" -lt "$deadline" ]]; do
    check_processes
    if curl --fail --silent --show-error --max-time 2 "$url" -o "$out.tmp"; then
      mv "$out.tmp" "$out"
      return 0
    fi
    sleep 1
  done
  die "timed out waiting for $url"
}

compare_heads() {
  python3 - "$GO_HEAD_JSON" "$RUST_HEAD_JSON" <<'PY'
import json
import sys


def checkpoint(value):
    return {
        "root": str(value["root"]).lower(),
        "slot": int(value["slot"]),
    }


def normalized(path):
    with open(path, "r", encoding="utf-8") as handle:
        data = json.load(handle)
    return {
        "head": checkpoint(data["head"]),
        "finalized": checkpoint(data["finalized"]),
    }


go = normalized(sys.argv[1])
rust = normalized(sys.argv[2])
if go != rust:
    print("head mismatch", file=sys.stderr)
    print(f"go:   {json.dumps(go, sort_keys=True)}", file=sys.stderr)
    print(f"rust: {json.dumps(rust, sort_keys=True)}", file=sys.stderr)
    raise SystemExit(1)
PY
}

scan_logs() {
  if grep -Eiq 'panic|panicked|backtrace|unwrap' "$GO_LOG" "$RUST_LOG"; then
    die "panic-style marker found in logs"
  fi
  if grep -Eiq 'StatusExchangeFailed|status rpc .* failure|status handshake mismatch' "$GO_LOG" "$RUST_LOG"; then
    die "status-handshake failure marker found in logs"
  fi
  if ! grep -Eq 'status handshake ok' "$RUST_LOG"; then
    die "Rust log did not record a successful status handshake"
  fi
  for topic in "$BLOCK_TOPIC" "$VOTE_TOPIC"; do
    if ! grep -Fq "topic=$topic" "$RUST_LOG"; then
      die "Rust log did not record subscription to $topic"
    fi
  done
  if grep -Eiq 'InsufficientPeers' "$RUST_LOG"; then
    die "Rust gossipsub publish had insufficient mesh peers"
  fi
}

write_summary() {
  local status="$1"
  [[ -d "$ARTIFACT_DIR" ]] || return 0
  cat >"$SUMMARY" <<EOF
# Devnet0 Interop Verification

- status: $status
- artifact_dir: \`$ARTIFACT_DIR\`
- lean_go_dir: \`$LEAN_GO_DIR\`
- go_peer_id: \`${GO_PEER_ID:-unknown}\`
- go_head_url: \`http://$BIND_ADDRESS:$GO_HTTP_PORT$GO_HEAD_PATH\`
- rust_head_url: \`http://$BIND_ADDRESS:$RUST_HTTP_PORT$RUST_HEAD_PATH\`
- duration_seconds: \`$INTEROP_DURATION_SECONDS\`
- failure: \`${FAILURE_REASON:-none}\`

## Artifacts

- \`go.log\`
- \`rust.log\`
- \`go-local-pq-config.yaml\`
- \`go-bootnodes.yaml\`
- \`rust-bootnodes.yaml\`
- \`go-head.json\`
- \`rust-head.json\`
EOF
}

main() {
  mkdir -p "$ARTIFACT_DIR"
  trap cleanup EXIT
  trap 'die "interrupted"' INT TERM

  require_command cargo
  require_command go
  require_command curl
  require_command python3
  require_command ps

  [[ -d "$LEAN_GO_DIR" ]] || die "LEAN_GO_DIR does not exist: $LEAN_GO_DIR"
  require_file "$LEAN_GO_DIR/go.mod"
  require_file "$GO_GENESIS_STATE"
  require_file "$GO_VALIDATORS"
  if [[ -z "$GO_GENESIS_CONFIG" ]]; then
    GO_GENESIS_CONFIG="$ARTIFACT_DIR/go-local-pq-config.yaml"
    cat >"$GO_GENESIS_CONFIG" <<EOF
GENESIS_TIME: $GO_GENESIS_TIME
VALIDATOR_COUNT: $GO_VALIDATOR_COUNT
EOF
  fi
  require_file "$GO_GENESIS_CONFIG"

  assert_tcp_port_free "$GO_HTTP_PORT"
  assert_tcp_port_free "$RUST_HTTP_PORT"
  assert_tcp_port_free "$GO_METRICS_PORT"
  assert_tcp_port_free "$RUST_METRICS_PORT"
  assert_udp_port_free "$GO_P2P_PORT"
  assert_udp_port_free "$RUST_P2P_PORT"

  echo "building lean-rust beacon"
  (cd "$REPO_ROOT" && cargo build -p beacon --release)

  echo "building lean-go beacon"
  (cd "$LEAN_GO_DIR" && go build -o "$GO_BIN" ./cmd/lean-beacon)

  echo "generating lean-go private key"
  "$GO_BIN" generate_private_key --output-path "$GO_KEY"
  write_peer_helper
  GO_PEER_ID="$(cd "$LEAN_GO_DIR" && go run "$PEER_HELPER" "$GO_KEY")"
  [[ -n "$GO_PEER_ID" ]] || die "failed to derive lean-go peer id"

  : >"$GO_BOOTNODES"
  printf -- "- /ip4/%s/udp/%s/quic-v1/p2p/%s\n" \
    "$P2P_ADDRESS" "$GO_P2P_PORT" "$GO_PEER_ID" >"$RUST_BOOTNODES"

  echo "starting lean-go"
  (
    cd "$LEAN_GO_DIR"
    "$GO_BIN" \
      --devnet-listen-addresses "/ip4/$P2P_ADDRESS/udp/$GO_P2P_PORT/quic-v1" \
      --devnet-bootnodes "$GO_BOOTNODES" \
      --data-dir "$ARTIFACT_DIR/go-data" \
      --network "$GO_GENESIS_CONFIG" \
      --genesis "$GO_GENESIS_STATE" \
      --validator-registry-path "$GO_VALIDATORS" \
      --node-id "$GO_NODE_ID" \
      --private-key-path "$GO_KEY" \
      --http-address "$BIND_ADDRESS" \
      --http-port "$GO_HTTP_PORT" \
      --metrics \
      --metrics-address "$BIND_ADDRESS" \
      --metrics-port "$GO_METRICS_PORT" \
      --verbosity "$GO_INTEROP_LOG_LEVEL" \
      --log.dir.path "$ARTIFACT_DIR/go-logdir" \
      --log.dir.prefix lean-go
  ) >"$GO_LOG" 2>&1 &
  GO_PID=$!
  echo "$GO_PID" >"$ARTIFACT_DIR/go.pid"

  fetch_head "http://$BIND_ADDRESS:$GO_HTTP_PORT$GO_HEAD_PATH" "$GO_HEAD_JSON" 45

  echo "starting lean-rust"
  mkdir -p "$ARTIFACT_DIR/rust-cwd"
  (
    cd "$ARTIFACT_DIR/rust-cwd"
    RUST_LOG="$RUST_INTEROP_LOG" \
    "$RUST_BIN" \
      --devnet-listen-addresses "/ip4/$P2P_ADDRESS/udp/$RUST_P2P_PORT/quic-v1" \
      --devnet-bootnodes "$RUST_BOOTNODES" \
      --genesis-state "$GO_GENESIS_STATE" \
      --validator-registry-path "$GO_VALIDATORS" \
      --node-id "$RUST_NODE_ID" \
      --http-address "$BIND_ADDRESS" \
      --http-port "$RUST_HTTP_PORT" \
      --metrics-address "$BIND_ADDRESS" \
      --metrics-port "$RUST_METRICS_PORT" \
      --log-level debug \
      --log.dir.path "$ARTIFACT_DIR/rust-logdir" \
      --log.dir.prefix lean-rust
  ) >"$RUST_LOG" 2>&1 &
  RUST_PID=$!
  echo "$RUST_PID" >"$ARTIFACT_DIR/rust.pid"

  fetch_head "http://$BIND_ADDRESS:$RUST_HTTP_PORT$RUST_HEAD_PATH" "$RUST_HEAD_JSON" 45

  echo "running interop window for ${INTEROP_DURATION_SECONDS}s"
  deadline=$(( $(date +%s) + INTEROP_DURATION_SECONDS ))
  while [[ "$(date +%s)" -lt "$deadline" ]]; do
    check_processes
    sleep 1
  done

  fetch_head "http://$BIND_ADDRESS:$GO_HTTP_PORT$GO_HEAD_PATH" "$GO_HEAD_JSON" 10
  fetch_head "http://$BIND_ADDRESS:$RUST_HTTP_PORT$RUST_HEAD_PATH" "$RUST_HEAD_JSON" 10
  compare_heads || die "Go and Rust heads differ"
  scan_logs
  write_summary "pass"

  echo "interop verification passed"
  echo "artifacts: $ARTIFACT_DIR"
}

main "$@"
