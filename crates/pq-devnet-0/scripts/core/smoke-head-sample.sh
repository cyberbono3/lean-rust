#!/usr/bin/env bash
set -euo pipefail

REAM_HEAD_URL="${REAM_HEAD_URL:-http://127.0.0.1:5052/lean/v0/head}"
LEAN_RUST_HEAD_URL="${LEAN_RUST_HEAD_URL:-http://127.0.0.1:5053/lean/v0/head}"
TARGET_MATCHES="${PQ_DEVNET_SMOKE_MATCHES:-10}"
MAX_ATTEMPTS="${PQ_DEVNET_SMOKE_MAX_ATTEMPTS:-$TARGET_MATCHES}"
INTERVAL_SECONDS="${PQ_DEVNET_SMOKE_INTERVAL_SECONDS:-12}"
CURL_MAX_TIME_SECONDS="${PQ_DEVNET_SMOKE_CURL_MAX_TIME_SECONDS:-3}"

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

require_non_negative_integer() {
  local name="$1"
  local value="$2"

  case "$value" in
    '' | *[!0-9]*) die "$name must be a non-negative integer, got $value" ;;
  esac
}

require_positive_integer() {
  local name="$1"
  local value="$2"

  require_non_negative_integer "$name" "$value"
  [[ "$value" -gt 0 ]] || die "$name must be greater than zero"
}

require_command() {
  local name="$1"

  command -v "$name" >/dev/null 2>&1 || die "$name is required"
}

fetch_status() {
  local url="$1"

  curl --fail --silent --show-error --max-time "$CURL_MAX_TIME_SECONDS" "$url"
}

status_fields() {
  jq -r '
    (.data? // .) as $status |
    def checkpoint($name):
      if ($status[$name]? | type) == "object" then $status[$name] else {} end;
    [
      (checkpoint("head").slot // ""),
      (checkpoint("head").root // ""),
      (checkpoint("finalized").slot // ""),
      (checkpoint("finalized").root // "")
    ] | @tsv
  '
}

read_fields() {
  local response="$1"
  local __slot_var="$2"
  local __head_root_var="$3"
  local __finalized_slot_var="$4"
  local __finalized_root_var="$5"
  local fields slot head_root finalized_slot finalized_root

  fields="$(printf '%s' "$response" | status_fields)"
  IFS=$'\t' read -r slot head_root finalized_slot finalized_root <<<"$fields"

  printf -v "$__slot_var" '%s' "$slot"
  printf -v "$__head_root_var" '%s' "$head_root"
  printf -v "$__finalized_slot_var" '%s' "$finalized_slot"
  printf -v "$__finalized_root_var" '%s' "$finalized_root"
}

is_match() {
  [[ -n "$REAM_HEAD_SLOT" ]] \
    && [[ -n "$REAM_HEAD_ROOT" ]] \
    && [[ -n "$REAM_FINALIZED_ROOT" ]] \
    && [[ "$REAM_HEAD_SLOT" == "$LEAN_RUST_HEAD_SLOT" ]] \
    && [[ "$REAM_FINALIZED_SLOT" == "$LEAN_RUST_FINALIZED_SLOT" ]] \
    && [[ "$REAM_HEAD_ROOT" == "$LEAN_RUST_HEAD_ROOT" ]] \
    && [[ "$REAM_FINALIZED_ROOT" == "$LEAN_RUST_FINALIZED_ROOT" ]]
}

require_command curl
require_command jq
require_positive_integer PQ_DEVNET_SMOKE_MATCHES "$TARGET_MATCHES"
require_positive_integer PQ_DEVNET_SMOKE_MAX_ATTEMPTS "$MAX_ATTEMPTS"
require_non_negative_integer PQ_DEVNET_SMOKE_INTERVAL_SECONDS "$INTERVAL_SECONDS"
require_positive_integer PQ_DEVNET_SMOKE_CURL_MAX_TIME_SECONDS "$CURL_MAX_TIME_SECONDS"

printf '| sample | time_utc | ream_head_slot | rust_head_slot | head.root | finalized.root | match | consecutive |\n'
printf '| --- | --- | --- | --- | --- | --- | --- | --- |\n'

CONSECUTIVE_MATCHES=0

for ((ATTEMPT = 1; ATTEMPT <= MAX_ATTEMPTS; ATTEMPT++)); do
  TIMESTAMP="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
  REAM_RESPONSE="$(fetch_status "$REAM_HEAD_URL")"
  LEAN_RUST_RESPONSE="$(fetch_status "$LEAN_RUST_HEAD_URL")"

  read_fields "$REAM_RESPONSE" \
    REAM_HEAD_SLOT \
    REAM_HEAD_ROOT \
    REAM_FINALIZED_SLOT \
    REAM_FINALIZED_ROOT
  read_fields "$LEAN_RUST_RESPONSE" \
    LEAN_RUST_HEAD_SLOT \
    LEAN_RUST_HEAD_ROOT \
    LEAN_RUST_FINALIZED_SLOT \
    LEAN_RUST_FINALIZED_ROOT

  MATCH="no"
  if is_match; then
    MATCH="yes"
    CONSECUTIVE_MATCHES=$((CONSECUTIVE_MATCHES + 1))
  else
    CONSECUTIVE_MATCHES=0
  fi

  printf '| %s | %s | %s | %s | `%s` | `%s` | %s | %s |\n' \
    "$ATTEMPT" \
    "$TIMESTAMP" \
    "$REAM_HEAD_SLOT" \
    "$LEAN_RUST_HEAD_SLOT" \
    "$REAM_HEAD_ROOT" \
    "$REAM_FINALIZED_ROOT" \
    "$MATCH" \
    "$CONSECUTIVE_MATCHES"

  if [[ "$CONSECUTIVE_MATCHES" -ge "$TARGET_MATCHES" ]]; then
    printf '\nobserved %s consecutive matching head/finalized samples\n' "$CONSECUTIVE_MATCHES"
    exit 0
  fi

  if [[ "$ATTEMPT" -lt "$MAX_ATTEMPTS" ]] && [[ "$INTERVAL_SECONDS" -gt 0 ]]; then
    sleep "$INTERVAL_SECONDS"
  fi
done

die "did not observe $TARGET_MATCHES consecutive matching samples in $MAX_ATTEMPTS attempts"
