#!/usr/bin/env bash
#
# check-msrv-consistency.sh — assert every toolchain source declares the same MSRV.
#
# The workspace MSRV is declared in four places that cannot reference each other:
# a manifest field, a rustup channel, the CI toolchain pins, and a Docker base
# image. Nothing keeps them in sync, and they have drifted before. This guard is
# what fails when they disagree.
#
# `Cargo.toml` `[workspace.package].rust-version` is the SOURCE OF TRUTH; the
# other three are trackers. Versions are compared at major.minor precision, so a
# tracker MAY pin a patch (`1.87.0`) while the manifest declares `1.87`. Rust
# does not add lints in patch releases, so major.minor is the meaningful unit.
#
# A guard that silently stops asserting is worse than no guard, so every source
# must be positively accounted for: an unreadable file, an unparsable pin, or a
# missing declaration is a FAILURE, never a skip.
#
# Usage:   scripts/check-msrv-consistency.sh
# Exit:    0 = all sources agree, 1 = drift detected or a source could not be verified
# Tests:   scripts/tests/test-check-msrv-consistency.sh

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

CARGO_TOML="Cargo.toml"
TOOLCHAIN_TOML="rust-toolchain.toml"
CI_YML=".github/workflows/ci.yml"
DOCKERFILE="crates/fixtures/Dockerfile"

# major.minor — drops any patch component so `1.87.0` and `1.87` compare equal.
normalize() { printf '%s' "$1" | cut -d. -f1,2; }

fail() { printf 'check-msrv-consistency: %s\n' "$1" >&2; exit 1; }

# Drop whole-line comments so a commented-out pin is never mistaken for a live one.
uncommented() { grep -v '^[[:space:]]*#' "$1" || true; }

for f in "$CARGO_TOML" "$TOOLCHAIN_TOML" "$CI_YML" "$DOCKERFILE"; do
    [ -r "$f" ] || fail "missing or unreadable required file: $f"
done

# Source of truth — scoped to [workspace.package] so a rust-version key in any
# other table (a dependency, a patch section) cannot be picked up instead.
expected_raw="$(
    awk '
        /^[[:space:]]*\[/ { in_wp = ($0 ~ /^[[:space:]]*\[workspace\.package\]/) }
        in_wp && /^[[:space:]]*rust-version[[:space:]]*=/ {
            if (match($0, /"[0-9][0-9.]*"/)) {
                print substr($0, RSTART + 1, RLENGTH - 2); exit
            }
        }
    ' "$CARGO_TOML"
)"
[ -n "$expected_raw" ] || fail "could not read [workspace.package].rust-version from $CARGO_TOML"
expected="$(normalize "$expected_raw")"

status=0
report() {
    local label="$1" raw="$2"
    if [ "$(normalize "$raw")" = "$expected" ]; then
        printf '  ok    %-34s %s\n' "$label" "$raw"
    else
        printf '  DRIFT %-34s %s  (want %s.x)\n' "$label" "$raw" "$expected"
        status=1
    fi
}

printf 'MSRV consistency — source of truth: %s [workspace.package].rust-version = %s\n' \
    "$CARGO_TOML" "$expected_raw"

# --- rustup channel -----------------------------------------------------------
channel_raw="$(uncommented "$TOOLCHAIN_TOML" |
    sed -n 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"\([0-9][0-9.]*\)".*/\1/p')"
channel_raw="${channel_raw%%$'\n'*}"
[ -n "$channel_raw" ] || fail "could not read a pinned channel from $TOOLCHAIN_TOML (a floating channel such as \"stable\" is not reproducible)"
report "$TOOLCHAIN_TOML" "$channel_raw"

# --- CI toolchain pins --------------------------------------------------------
# Two shapes are in play: `dtolnay/rust-toolchain@<version>` and a floating ref
# (`@stable`) whose real version arrives via a `toolchain:` input. Every use of
# the action must yield a version, or the pin is unverifiable and we fail.
ci_body="$(uncommented "$CI_YML")"
ci_uses="$(printf '%s\n' "$ci_body" | grep -c 'dtolnay/rust-toolchain@' || true)"
[ "$ci_uses" -gt 0 ] || fail "no dtolnay/rust-toolchain usage found in $CI_YML"

ci_versions="$(
    {
        printf '%s\n' "$ci_body" | sed -n 's#.*dtolnay/rust-toolchain@\([0-9][0-9.]*\).*#\1#p'
        printf '%s\n' "$ci_body" | sed -n 's/^[[:space:]]*toolchain:[[:space:]]*["'"'"']\{0,1\}\([0-9][0-9.]*\).*/\1/p'
    } || true
)"
ci_found="$(printf '%s' "$ci_versions" | grep -c . || true)"
[ "$ci_found" -ge "$ci_uses" ] || fail "$CI_YML uses dtolnay/rust-toolchain $ci_uses time(s) but only $ci_found version(s) could be read — a floating ref without a 'toolchain:' input cannot be verified"

pin_n=0
while IFS= read -r tag; do
    [ -n "$tag" ] || continue
    pin_n=$((pin_n + 1))
    report "$CI_YML (pin $pin_n)" "$tag"
done <<< "$ci_versions"

# --- Docker builder -----------------------------------------------------------
# Every `FROM rust:` stage must match: a drifted later stage is still drift.
docker_body="$(uncommented "$DOCKERFILE")"
docker_versions="$(printf '%s\n' "$docker_body" |
    sed -n 's/^[[:space:]]*FROM[[:space:]][[:space:]]*rust:\([0-9][0-9.]*\).*/\1/p' || true)"
docker_found="$(printf '%s' "$docker_versions" | grep -c . || true)"
[ "$docker_found" -gt 0 ] || fail "no 'FROM rust:<version>' stage found in $DOCKERFILE"

stage_n=0
while IFS= read -r ver; do
    [ -n "$ver" ] || continue
    stage_n=$((stage_n + 1))
    report "$DOCKERFILE (stage $stage_n)" "$ver"
done <<< "$docker_versions"

# The builder must COPY rust-toolchain.toml, or it silently uses the base image's
# rustc and the pinned channel becomes decorative — the drift this guard exists
# to prevent, reintroduced one deleted line at a time.
if ! printf '%s\n' "$docker_body" | grep -q '^[[:space:]]*COPY[[:space:]].*rust-toolchain\.toml'; then
    printf '  DRIFT %-34s %s\n' "$DOCKERFILE" "does not COPY rust-toolchain.toml"
    printf '        the builder would fall back to its base image rustc\n'
    status=1
fi

if [ "$status" -ne 0 ]; then
    printf '\nMSRV drift: the sources above disagree with %s.\n' "$CARGO_TOML" >&2
    printf 'Every source must declare the same major.minor. Fix the DRIFT rows.\n' >&2
    exit 1
fi

printf '\nAll toolchain sources agree on %s.x\n' "$expected"
