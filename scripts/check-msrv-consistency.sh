#!/usr/bin/env bash
#
# check-msrv-consistency.sh — assert every toolchain source declares the same MSRV.
#
# The workspace MSRV is declared in four places that cannot reference each other:
# a manifest field, a rustup channel, three CI toolchain tags, and a Docker base
# image. Nothing keeps them in sync, and they have drifted before. This guard is
# what fails when they disagree.
#
# `Cargo.toml` `[workspace.package].rust-version` is the SOURCE OF TRUTH; the other
# three are trackers. Versions are compared at major.minor precision, so a tracker
# MAY pin a patch (`1.87.0`) while the manifest declares `1.87`.
#
# Usage:   scripts/check-msrv-consistency.sh
# Exit:    0 = all sources agree, 1 = drift detected (prints the table)
#
# Run by CI on every push/PR, and locally before committing a toolchain change.

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

CARGO_TOML="Cargo.toml"
TOOLCHAIN_TOML="rust-toolchain.toml"
CI_YML=".github/workflows/ci.yml"
DOCKERFILE="crates/fixtures/Dockerfile"

# major.minor — drops any patch component so `1.87.0` and `1.87` compare equal.
normalize() { printf '%s' "$1" | cut -d. -f1,2; }

fail() { printf 'check-msrv-consistency: %s\n' "$1" >&2; exit 1; }

for f in "$CARGO_TOML" "$TOOLCHAIN_TOML" "$CI_YML" "$DOCKERFILE"; do
    [ -f "$f" ] || fail "missing required file: $f"
done

# Source of truth.
expected_raw="$(sed -n 's/^rust-version[[:space:]]*=[[:space:]]*"\([0-9.]*\)".*/\1/p' "$CARGO_TOML" | head -1)"
[ -n "$expected_raw" ] || fail "could not read [workspace.package].rust-version from $CARGO_TOML"
expected="$(normalize "$expected_raw")"

status=0
report() {
    local label="$1" raw="$2" got="$3"
    if [ "$got" = "$expected" ]; then
        printf '  ok    %-28s %s\n' "$label" "$raw"
    else
        printf '  DRIFT %-28s %s  (want %s.x)\n' "$label" "${raw:-<unreadable>}" "$expected"
        status=1
    fi
}

printf 'MSRV consistency — source of truth: %s rust-version = %s\n' "$CARGO_TOML" "$expected_raw"

# rustup channel.
channel_raw="$(sed -n 's/^channel[[:space:]]*=[[:space:]]*"\([0-9.]*\)".*/\1/p' "$TOOLCHAIN_TOML" | head -1)"
report "$TOOLCHAIN_TOML" "$channel_raw" "$(normalize "${channel_raw:-0}")"

# CI toolchain tags — every dtolnay/rust-toolchain@<ver> pin must match.
ci_found=0
while IFS= read -r tag; do
    [ -n "$tag" ] || continue
    ci_found=$((ci_found + 1))
    report "$CI_YML (job $ci_found)" "$tag" "$(normalize "$tag")"
done < <(sed -n 's#.*dtolnay/rust-toolchain@\([0-9][0-9.]*\).*#\1#p' "$CI_YML")
[ "$ci_found" -gt 0 ] || fail "no dtolnay/rust-toolchain@<version> pins found in $CI_YML"

# Docker builder base image.
docker_raw="$(sed -n 's/^FROM rust:\([0-9.]*\)-.*/\1/p' "$DOCKERFILE" | head -1)"
report "$DOCKERFILE" "$docker_raw" "$(normalize "${docker_raw:-0}")"

if [ "$status" -ne 0 ]; then
    printf '\nMSRV drift: the sources above disagree with %s.\n' "$CARGO_TOML" >&2
    printf 'Every source must declare the same major.minor. Fix the DRIFT rows.\n' >&2
    exit 1
fi

printf '\nAll toolchain sources agree on %s.x\n' "$expected"
