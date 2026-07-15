#!/usr/bin/env bash
# test-check-msrv-pins.sh — verify the MSRV pin guard actually rejects a drifted lock.
# Usage: bash scripts/tests/test-check-msrv-pins.sh
#
# A guard is only worth its line count if it fails when it should. These cases
# run it against synthetic Cargo.lock fixtures rather than the real one, so the
# test asserts the guard's behaviour and not today's dependency graph.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GUARD="${REPO_ROOT}/scripts/check-msrv-pins.sh"

pass=0
fail=0

check() {
    local name="$1" want_rc="$2" got_rc="$3" out="$4"
    if [ "$got_rc" -eq "$want_rc" ]; then
        printf 'ok   — %s\n' "$name"
        pass=$((pass + 1))
    else
        printf 'FAIL — %s (want rc=%s, got rc=%s)\n%s\n' "$name" "$want_rc" "$got_rc" "$out"
        fail=$((fail + 1))
    fi
}

# Builds a throwaway repo containing only the guard and a Cargo.lock fixture.
make_fixture() {
    local dir="$1" lock="$2"
    mkdir -p "$dir/scripts"
    cp "$GUARD" "$dir/scripts/check-msrv-pins.sh"
    printf '%s' "$lock" > "$dir/Cargo.lock"
}

lock_entry() {
    printf '[[package]]\nname = "%s"\nversion = "%s"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n\n' "$1" "$2"
}

GOOD="$(lock_entry ruint 1.17.2)$(lock_entry ethereum_ssz_derive 0.10.0)"

# 1. Both pins correct → pass.
d="$(mktemp -d)"; make_fixture "$d" "$GOOD"
out="$(cd "$d" && bash scripts/check-msrv-pins.sh 2>&1)"; rc=$?
check "accepts a lock holding both pins" 0 "$rc" "$out"
rm -rf "$d"

# 2. ruint drifted forward to an MSRV-raising release → fail.
d="$(mktemp -d)"; make_fixture "$d" "$(lock_entry ruint 1.19.0)$(lock_entry ethereum_ssz_derive 0.10.0)"
out="$(cd "$d" && bash scripts/check-msrv-pins.sh 2>&1)"; rc=$?
check "rejects ruint drifted to 1.19.0" 1 "$rc" "$out"
rm -rf "$d"

# 3. ethereum_ssz_derive drifted forward, pulling darling 0.23 → fail.
d="$(mktemp -d)"; make_fixture "$d" "$(lock_entry ruint 1.17.2)$(lock_entry ethereum_ssz_derive 0.10.4)"
out="$(cd "$d" && bash scripts/check-msrv-pins.sh 2>&1)"; rc=$?
check "rejects ethereum_ssz_derive drifted to 0.10.4" 1 "$rc" "$out"
rm -rf "$d"

# 4. The legitimate second major of ethereum_ssz_derive (0.7, used by the
#    workspace ssz crate) coexists with the pinned 0.10.0 → pass. This is the
#    case a naive "appears exactly once" guard would get wrong.
d="$(mktemp -d)"; make_fixture "$d" "$(lock_entry ruint 1.17.2)$(lock_entry ethereum_ssz_derive 0.7.1)$(lock_entry ethereum_ssz_derive 0.10.0)"
out="$(cd "$d" && bash scripts/check-msrv-pins.sh 2>&1)"; rc=$?
check "accepts the 0.7 major coexisting with the pinned 0.10.0" 0 "$rc" "$out"
rm -rf "$d"

# 5. A pinned package missing entirely → fail, never silently skip.
d="$(mktemp -d)"; make_fixture "$d" "$(lock_entry ruint 1.17.2)"
out="$(cd "$d" && bash scripts/check-msrv-pins.sh 2>&1)"; rc=$?
check "rejects a lock with ethereum_ssz_derive absent" 1 "$rc" "$out"
rm -rf "$d"

# 6. Unreadable lock → fail closed.
d="$(mktemp -d)"; mkdir -p "$d/scripts"; cp "$GUARD" "$d/scripts/check-msrv-pins.sh"
out="$(cd "$d" && bash scripts/check-msrv-pins.sh 2>&1)"; rc=$?
check "rejects a missing Cargo.lock rather than skipping" 1 "$rc" "$out"
rm -rf "$d"

# 7. A prefix-colliding package name must not satisfy a pin.
d="$(mktemp -d)"; make_fixture "$d" "$(lock_entry ruint-macro 1.17.2)$(lock_entry ethereum_ssz_derive 0.10.0)"
out="$(cd "$d" && bash scripts/check-msrv-pins.sh 2>&1)"; rc=$?
check "does not let ruint-macro satisfy the ruint pin" 1 "$rc" "$out"
rm -rf "$d"

printf '\n%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
