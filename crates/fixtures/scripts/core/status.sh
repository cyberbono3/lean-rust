#!/usr/bin/env bash
set -euo pipefail

REAM_HEAD_URL="${REAM_HEAD_URL:-http://127.0.0.1:5052/lean/v0/head}"
LEAN_RUST_HEAD_URL="${LEAN_RUST_HEAD_URL:-http://127.0.0.1:5053/lean/v0/head}"

fetch() {
  local url="$1"
  curl --silent --show-error --max-time 2 "$url"
}

normalize_status() {
  local response="$1"

  jq -r '
    (.data? // .) as $status |
    def render($name):
      if ($status | type) == "object" and $status[$name] != null then
        $status[$name] as $value |
        if ($value | type) == "object" then
          if ($value | has("root")) and ($value | has("slot")) then
            "\($name).root: \($value.root)",
            "\($name).slot: \($value.slot)"
          elif ($value | has("root")) then
            "\($name).root: \($value.root)"
          else
            "\($name): \($value | tojson)"
          end
        elif ($name == "head" or $name == "finalized") and ($value | type) == "string" then
          "\($name).root: \($value)"
        else
          "\($name): \($value)"
        end
      else
        empty
      end;
    render("head"),
    render("finalized"),
    render("latest_justified"),
    render("safe_target")
  ' <<<"$response" 2>/dev/null
}

print_status() {
  local name="$1"
  local url="$2"
  local response="$3"
  local normalized=""

  if command -v jq >/dev/null 2>&1; then
    normalized="$(normalize_status "$response" || true)"
  fi

  printf '\n--- %s ---\n' "$name"
  printf 'url: %s\n' "$url"
  if [[ -n "$normalized" ]]; then
    printf '%s\n' "$normalized"
  else
    printf '%s\n' "$response"
  fi
}

probe() {
  local name="$1"
  local url="$2"
  local response

  if ! response="$(fetch "$url" 2>/dev/null)"; then
    printf '\n--- %s ---\n' "$name"
    printf 'url: %s\n' "$url"
    printf '(unreachable)\n'
    return 0
  fi

  print_status "$name" "$url" "$response"
}

probe "ream node0" "$REAM_HEAD_URL"
probe "lean-rust node1" "$LEAN_RUST_HEAD_URL"
printf '\n'
