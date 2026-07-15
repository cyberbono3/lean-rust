#!/usr/bin/env bash
# test-check-msrv-consistency.sh — verify the MSRV guard actually catches drift.
# Usage: bash scripts/tests/test-check-msrv-consistency.sh
#
# A guard that silently stops asserting is worse than no guard, so most cases
# here are FALSE-PASS regressions: a source really drifts, and the guard must
# exit non-zero. Each builds a throwaway repo skeleton and runs the real script.
#
#   TC-1  all four agree                      -> pass
#   TC-2  channel drifts                      -> fail
#   TC-3  one CI job drifts                   -> fail
#   TC-4  Dockerfile drifts                   -> fail
#   TC-5  patch-level tracker (1.87.0 vs 1.87) -> pass  (major.minor contract)
#   TC-6  second Dockerfile stage drifts      -> fail   (false-pass: head -1)
#   TC-7  a CI job uses @stable               -> fail   (false-pass: unparsed)
#   TC-8  Dockerfile drops rust-toolchain.toml COPY -> fail (false-pass: unasserted)
#   TC-9  commented-out pins are ignored      -> pass   (false-FAIL guard)
#   TC-10 unreadable source                   -> fail   (must not default to pass)
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
GUARD="${REPO_ROOT}/scripts/check-msrv-consistency.sh"

pass_count=0
fail_count=0

# Build a minimal repo skeleton; args override each source's content.
make_repo() {
    local dir="$1" cargo="$2" toolchain="$3" ci="$4" dockerfile="$5"
    mkdir -p "$dir/.github/workflows" "$dir/crates/fixtures" "$dir/scripts"
    printf '%s\n' "$cargo" > "$dir/Cargo.toml"
    printf '%s\n' "$toolchain" > "$dir/rust-toolchain.toml"
    printf '%s\n' "$ci" > "$dir/.github/workflows/ci.yml"
    printf '%s\n' "$dockerfile" > "$dir/crates/fixtures/Dockerfile"
    cp "$GUARD" "$dir/scripts/check-msrv-consistency.sh"
}

CARGO_OK='[workspace.package]
rust-version  = "1.87"

[workspace.dependencies]
ruint = "~1.12"'

TOOLCHAIN_OK='[toolchain]
channel    = "1.87.0"'

CI_OK='jobs:
  fmt:
    steps:
      - uses: dtolnay/rust-toolchain@1.87.0
  clippy:
    steps:
      - uses: dtolnay/rust-toolchain@1.87.0
  test:
    steps:
      - uses: dtolnay/rust-toolchain@1.87.0'

DOCKER_OK='FROM rust:1.87-bookworm AS builder
WORKDIR /workspace
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
RUN cargo build --release

FROM debian:bookworm-slim AS runtime
COPY --from=builder /workspace/target/release/lean-rust /usr/local/bin/lean-rust'

# run <name> <expect: pass|fail> <cargo> <toolchain> <ci> <dockerfile>
run() {
    local name="$1" expect="$2"
    local dir; dir="$(mktemp -d)"
    make_repo "$dir" "$3" "$4" "$5" "$6"
    local out rc
    out="$(cd "$dir" && bash scripts/check-msrv-consistency.sh 2>&1)"; rc=$?
    rm -rf "$dir"

    local got; [ "$rc" -eq 0 ] && got=pass || got=fail
    if [ "$got" = "$expect" ]; then
        printf '  ok    %-52s (%s, exit %d)\n' "$name" "$got" "$rc"
        pass_count=$((pass_count + 1))
    else
        printf '  FAIL  %-52s expected %s, got %s (exit %d)\n' "$name" "$expect" "$got" "$rc"
        printf '%s\n' "$out" | sed 's/^/          | /'
        fail_count=$((fail_count + 1))
    fi
}

printf 'MSRV guard — drift detection\n'

run "TC-1  all four sources agree" pass \
    "$CARGO_OK" "$TOOLCHAIN_OK" "$CI_OK" "$DOCKER_OK"

run "TC-2  channel drifts" fail \
    "$CARGO_OK" '[toolchain]
channel    = "1.85"' "$CI_OK" "$DOCKER_OK"

run "TC-3  one CI job drifts" fail \
    "$CARGO_OK" "$TOOLCHAIN_OK" 'jobs:
  fmt:
    steps:
      - uses: dtolnay/rust-toolchain@1.87.0
  clippy:
    steps:
      - uses: dtolnay/rust-toolchain@1.80.0
  test:
    steps:
      - uses: dtolnay/rust-toolchain@1.87.0' "$DOCKER_OK"

run "TC-4  Dockerfile drifts" fail \
    "$CARGO_OK" "$TOOLCHAIN_OK" "$CI_OK" 'FROM rust:1.85-bookworm AS builder
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./'

run "TC-5  patch-level tracker vs major.minor manifest" pass \
    "$CARGO_OK" "$TOOLCHAIN_OK" "$CI_OK" "$DOCKER_OK"

# --- false-pass regressions (reviewer-reproduced) ---

run "TC-6  second Dockerfile stage drifts" fail \
    "$CARGO_OK" "$TOOLCHAIN_OK" "$CI_OK" 'FROM rust:1.87-bookworm AS builder
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
RUN cargo build --release

FROM rust:1.85-bookworm AS tools
RUN cargo install something'

run "TC-7  a CI job pins @stable via toolchain input" fail \
    "$CARGO_OK" "$TOOLCHAIN_OK" 'jobs:
  fmt:
    steps:
      - uses: dtolnay/rust-toolchain@1.87.0
  clippy:
    steps:
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: 1.80.0
  test:
    steps:
      - uses: dtolnay/rust-toolchain@1.87.0' "$DOCKER_OK"

run "TC-8  Dockerfile drops the rust-toolchain.toml COPY" fail \
    "$CARGO_OK" "$TOOLCHAIN_OK" "$CI_OK" 'FROM rust:1.87-bookworm AS builder
WORKDIR /workspace
COPY Cargo.toml Cargo.lock ./
RUN cargo build --release'

# --- false-FAIL guard ---

run "TC-9  commented-out pins are ignored" pass \
    "$CARGO_OK" '[toolchain]
# channel  = "1.80"
channel    = "1.87.0"' 'jobs:
  fmt:
    steps:
      # - uses: dtolnay/rust-toolchain@1.80.0
      - uses: dtolnay/rust-toolchain@1.87.0
  clippy:
    steps:
      - uses: dtolnay/rust-toolchain@1.87.0
  test:
    steps:
      - uses: dtolnay/rust-toolchain@1.87.0' "$DOCKER_OK"

# --- unreadable source must fail, never default to pass ---

printf '  --- unreadable source ---\n'
tc10_dir="$(mktemp -d)"
make_repo "$tc10_dir" "$CARGO_OK" "$TOOLCHAIN_OK" "$CI_OK" "$DOCKER_OK"
rm "$tc10_dir/rust-toolchain.toml"
tc10_out="$(cd "$tc10_dir" && bash scripts/check-msrv-consistency.sh 2>&1)"; tc10_rc=$?
rm -rf "$tc10_dir"
if [ "$tc10_rc" -ne 0 ]; then
    printf '  ok    %-52s (fail, exit %d)\n' "TC-10 missing source file" "$tc10_rc"
    pass_count=$((pass_count + 1))
else
    printf '  FAIL  %-52s expected fail, got pass\n' "TC-10 missing source file"
    printf '%s\n' "$tc10_out" | sed 's/^/          | /'
    fail_count=$((fail_count + 1))
fi

printf '\n%d passed, %d failed\n' "$pass_count" "$fail_count"
[ "$fail_count" -eq 0 ]
