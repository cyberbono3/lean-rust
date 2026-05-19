#!/usr/bin/env bash
set -euo pipefail

REAM_LOG_FILE="${REAM_LOG_FILE:-}"
LEAN_RUST_LOG_FILE="${LEAN_RUST_LOG_FILE:-}"
REAM_CONTAINER="${REAM_CONTAINER:-ream-node0}"
LEAN_RUST_CONTAINER="${LEAN_RUST_CONTAINER:-lean-rust-node1}"
REAM_VOTE_LINE_PATTERN="${REAM_VOTE_LINE_PATTERN:-Processing vote by Validator 1}"
LEAN_RUST_VOTE_LINE_PATTERN="${LEAN_RUST_VOTE_LINE_PATTERN:-engine attestation vote produced}"
MODE="${PQ_DEVNET_VOTE_CHECKPOINT_MODE:-fail}"
MIN_SLOT="${PQ_DEVNET_VOTE_CHECKPOINT_MIN_SLOT:-0}"
MAX_SLOT="${PQ_DEVNET_VOTE_CHECKPOINT_MAX_SLOT:-}"
REQUIRE_ALL_SLOTS="${PQ_DEVNET_VOTE_CHECKPOINT_REQUIRE_ALL_SLOTS:-0}"

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

require_command() {
  local name="$1"

  command -v "$name" >/dev/null 2>&1 || die "$name is required"
}

require_non_negative_integer() {
  local name="$1"
  local value="$2"

  case "$value" in
    '' | *[!0-9]*) die "$name must be a non-negative integer, got $value" ;;
  esac
}

read_log_source() {
  local file="$1"
  local container="$2"

  if [[ -n "$file" ]]; then
    cat "$file"
  else
    docker logs "$container" 2>&1
  fi
}

parse_vote_log() {
  local pattern="$1"
  local output="$2"

  VOTE_LINE_PATTERN="$pattern" perl -ne '
    BEGIN { $pattern = $ENV{"VOTE_LINE_PATTERN"}; }
    s/\e\[[0-9;]*m//g;
    next unless /$pattern/i;
    my ($slot) = /(?:^|[^\w])slot\s*[=:]\s*([0-9]+)/;
    my ($source) = /source_slot\s*[=:]\s*([0-9]+)/;
    my ($target) = /target_slot\s*[=:]\s*([0-9]+)/;
    next unless defined $slot && defined $source && defined $target;
    print "$slot\t$source->$target\n";
  ' | sort -n -k1,1 | awk -F '\t' '!seen[$1]++' >"$output"
}

lookup_vote() {
  local file="$1"
  local slot="$2"

  awk -F '\t' -v slot="$slot" '$1 == slot { print $2; exit }' "$file"
}

require_command awk
require_command sort
require_command perl
if [[ -z "$REAM_LOG_FILE" ]] || [[ -z "$LEAN_RUST_LOG_FILE" ]]; then
  require_command docker
fi

require_non_negative_integer PQ_DEVNET_VOTE_CHECKPOINT_MIN_SLOT "$MIN_SLOT"
if [[ -n "$MAX_SLOT" ]]; then
  require_non_negative_integer PQ_DEVNET_VOTE_CHECKPOINT_MAX_SLOT "$MAX_SLOT"
fi
case "$REQUIRE_ALL_SLOTS" in
  0 | 1) ;;
  *) die "PQ_DEVNET_VOTE_CHECKPOINT_REQUIRE_ALL_SLOTS must be 0 or 1, got $REQUIRE_ALL_SLOTS" ;;
esac

case "$MODE" in
  fail | report) ;;
  *) die "PQ_DEVNET_VOTE_CHECKPOINT_MODE must be fail or report, got $MODE" ;;
esac

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

REAM_PARSED="$TMP_DIR/ream.tsv"
LEAN_RUST_PARSED="$TMP_DIR/lean-rust.tsv"
SLOTS="$TMP_DIR/slots.txt"

read_log_source "$REAM_LOG_FILE" "$REAM_CONTAINER" \
  | parse_vote_log "$REAM_VOTE_LINE_PATTERN" "$REAM_PARSED"
read_log_source "$LEAN_RUST_LOG_FILE" "$LEAN_RUST_CONTAINER" \
  | parse_vote_log "$LEAN_RUST_VOTE_LINE_PATTERN" "$LEAN_RUST_PARSED"

[[ -s "$REAM_PARSED" ]] || die "no Ream vote checkpoint records found"
[[ -s "$LEAN_RUST_PARSED" ]] || die "no lean-rust vote checkpoint records found"

cut -f1 "$REAM_PARSED" "$LEAN_RUST_PARSED" | sort -n -u >"$SLOTS"

printf '| slot | Ream source->target | lean-rust source->target | match |\n'
printf '| --- | --- | --- | --- |\n'

COMPARED=0
SKIPPED_MISSING=0
MISMATCHES=0
FIRST_MISMATCH=""

while IFS= read -r slot; do
  [[ -n "$slot" ]] || continue
  if [[ "$slot" -lt "$MIN_SLOT" ]]; then
    continue
  fi
  if [[ -n "$MAX_SLOT" ]] && [[ "$slot" -gt "$MAX_SLOT" ]]; then
    continue
  fi

  REAM_VOTE="$(lookup_vote "$REAM_PARSED" "$slot")"
  LEAN_RUST_VOTE="$(lookup_vote "$LEAN_RUST_PARSED" "$slot")"

  MATCH="no"
  if [[ -z "$REAM_VOTE" || -z "$LEAN_RUST_VOTE" ]]; then
    if [[ "$REQUIRE_ALL_SLOTS" == "1" ]]; then
      MATCH="missing"
      MISMATCHES=$((MISMATCHES + 1))
    else
      MATCH="skipped"
      SKIPPED_MISSING=$((SKIPPED_MISSING + 1))
    fi
  elif [[ "$REAM_VOTE" == "$LEAN_RUST_VOTE" ]]; then
    MATCH="yes"
    COMPARED=$((COMPARED + 1))
  else
    COMPARED=$((COMPARED + 1))
    MISMATCHES=$((MISMATCHES + 1))
  fi

  if [[ "$MATCH" != "yes" && "$MATCH" != "skipped" && -z "$FIRST_MISMATCH" ]]; then
    FIRST_MISMATCH="$slot"
  fi

  printf '| %s | `%s` | `%s` | %s |\n' \
    "$slot" \
    "${REAM_VOTE:-missing}" \
    "${LEAN_RUST_VOTE:-missing}" \
    "$MATCH"
done <"$SLOTS"

if [[ "$COMPARED" -eq 0 ]]; then
  die "no overlapping vote checkpoint slots found"
fi

printf '\ncompared_slots=%s skipped_missing_slots=%s mismatches=%s' \
  "$COMPARED" \
  "$SKIPPED_MISSING" \
  "$MISMATCHES"
if [[ -n "$FIRST_MISMATCH" ]]; then
  printf ' first_mismatch_slot=%s' "$FIRST_MISMATCH"
fi
printf '\n'

if [[ "$MISMATCHES" -gt 0 && "$MODE" == "fail" ]]; then
  exit 1
fi
