#!/usr/bin/env bash
#
# check-msrv-pins.sh — assert that Cargo.lock holds the transitive dependencies
# whose newer releases raise their MSRV above this workspace's rust-version.
#
# The problem this guards: `resolver = "2"` is not MSRV-aware. It selects the
# newest semver-compatible release of every transitive dependency regardless of
# that release's `rust-version`. Several crates leanSig pulls in have since
# raised their MSRV above this workspace's floor, so an unconstrained resolution
# picks versions the pinned toolchain cannot compile:
#
#   ruint               1.18+   rust-version above the workspace floor
#   ethereum_ssz_derive 0.10.1+ requires darling ^0.23, whose MSRV is 1.88
#
# Neither is expressible as a manifest version requirement. `ruint`'s floor is a
# real `^1.14` (alloy-primitives, via ethereum_ssz 0.10, requires it) so the
# manifest cannot also cap it without contradicting itself; `ethereum_ssz_derive`
# is not a direct dependency of this workspace at all — it arrives through
# leanSig, so this workspace has no manifest line to pin it on. Both constraints
# therefore live only in Cargo.lock, where nothing declares intent and a routine
# `cargo update` silently reverts them.
#
# That is what makes this guard necessary rather than decorative: without it the
# next `cargo update` produces a build failure naming crates nobody here depends
# on directly, with no record of why the old versions were chosen.
#
# `resolver = "3"` is MSRV-aware and looks like the durable fix, but it does not
# solve this case: it declines to backtrack to ethereum_ssz_derive 0.10.0 and
# fails with the same darling error. Re-test it when the resolver improves or
# when upstream stops requiring darling 0.23; if it ever resolves cleanly, this
# guard and the lock pins can both retire.
#
# Usage:   scripts/check-msrv-pins.sh
# Exit:    0 = every pin holds, 1 = a pin drifted or could not be verified
#
# A guard that silently stops asserting is worse than no guard, so an unreadable
# lock, an unparsable entry, or a missing package is a FAILURE, never a skip.

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

CARGO_LOCK="Cargo.lock"

# package<TAB>required-version<TAB>reason
PINS="$(
    cat <<'EOF'
ruint	1.17.2	1.18+ raises rust-version above the workspace floor
ethereum_ssz_derive	0.10.0	0.10.1+ require darling ^0.23 (MSRV 1.88)
EOF
)"

fail() { printf '[check-msrv-pins.sh] FAIL: %s\n' "$1" >&2; exit 1; }

[ -r "$CARGO_LOCK" ] || fail "missing or unreadable required file: $CARGO_LOCK"

# Collect every version recorded for a package name. A package may legitimately
# appear more than once at semver-incompatible majors — ethereum_ssz_derive does,
# at 0.7 (workspace `ssz` crate) and 0.10 (via leanSig) — so the assertion is
# that the constrained major is present at the required version, not that the
# package appears exactly once. Matching on the whole name prevents `ruint` from
# matching a hypothetical `ruint-macro`.
versions_of() {
    awk -v pkg="$1" '
        /^\[\[package\]\]/ { name = ""; version = ""; next }
        /^name = / {
            name = $3
            gsub(/"/, "", name)
            next
        }
        /^version = / {
            version = $3
            gsub(/"/, "", version)
            if (name == pkg) print version
            next
        }
    ' "$CARGO_LOCK"
}

status=0

while IFS=$'\t' read -r pkg want reason; do
    [ -n "$pkg" ] || continue

    found="$(versions_of "$pkg" || true)"

    if [ -z "$found" ]; then
        printf '[check-msrv-pins.sh] FAIL: %s absent from %s — expected %s (%s)\n' \
            "$pkg" "$CARGO_LOCK" "$want" "$reason" >&2
        status=1
        continue
    fi

    # The required version must be the only version present for the same
    # semver-compatible line. Otherwise a drift could reintroduce an MSRV-raising
    # version alongside the pinned one and still satisfy a naive "want is present"
    # check.
    major="${want%%.*}"
    rest="${want#*.}"
    minor="${rest%%.*}"
    if [ "$major" = "0" ]; then
        compat_re="^0\\.${minor}\\."
        compat_line="0.${minor}"
    else
        compat_re="^${major}\\."
        compat_line="${major}"
    fi

    compat_found="$(printf '%s\n' "$found" | grep -E "$compat_re" || true)"

    if [ -z "$compat_found" ]; then
        printf '[check-msrv-pins.sh] FAIL: %s is at [%s], expected %s — %s\n' \
            "$pkg" "$(printf '%s' "$found" | tr '\n' ' ' | sed 's/ $//')" "$want" "$reason" >&2
        printf '[check-msrv-pins.sh] FAIL: restore with: cargo update -p %s --precise %s\n' \
            "$pkg" "$want" >&2
        status=1
        continue
    fi

    compat_count="$(printf '%s\n' "$compat_found" | wc -l | tr -d ' ')"

    if [ "$compat_count" -eq 1 ] && printf '%s\n' "$compat_found" | grep -qxF "$want"; then
        printf '[check-msrv-pins.sh] PASS: %s pinned at %s\n' "$pkg" "$want"
    else
        printf '[check-msrv-pins.sh] FAIL: %s has multiple %s.* versions [%s], expected only %s — %s\n' \
            "$pkg" "$compat_line" "$(printf '%s' "$compat_found" | tr '\n' ' ' | sed 's/ $//')" "$want" "$reason" >&2
        printf '[check-msrv-pins.sh] FAIL: restore with: cargo update -p %s --precise %s\n' \
            "$pkg" "$want" >&2
        status=1
    fi
done <<< "$PINS"

if [ "$status" -ne 0 ]; then
    printf '[check-msrv-pins.sh] FAIL: MSRV-constrained pins drifted; the pinned toolchain cannot build the result.\n' >&2
    exit 1
fi

printf '[check-msrv-pins.sh] PASS: all MSRV-constrained pins hold.\n'
